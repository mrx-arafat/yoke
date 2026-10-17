//! `yoke watch`: a read-only live view of what Yoke records for this project. It opens the
//! state database read-only, never creates or migrates it, and keeps running while the state is
//! missing, locked or unreadable. Without a terminal it prints one snapshot and exits.

mod git;
mod render;
mod term;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, VecDeque};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde_json::Value;
use yoke_core::engine::execute_bounded;
use yoke_core::quota::{self, Window};
use yoke_core::store::{DecisionFilter, DecisionRow, MissingSidecars, SessionSummary, Store};
use yoke_core::{config, jobs, notifications, plan, timefmt};

use render::{Frame, Tone};

pub(crate) const AGENTS: [&str; 2] = ["claude", "codex"];
/// Decisions read per page while counting.
const PAGE: usize = 500;
/// Decisions one refresh reads at most, so a huge database stays fast; counts past it show `+`.
const ROW_CAP: usize = 20_000;
/// Newest events kept for the list.
const EVENTS_KEPT: usize = 200;
/// A session that started longer ago than this is counted from this far back only.
const SESSION_LOOKBACK: i64 = 7 * 86_400;
const DATA_EVERY: Duration = Duration::from_secs(2);
/// Even with no write noticed, re-read this often, so a missed change cannot freeze the view.
const DATA_AT_LEAST: Duration = Duration::from_secs(15);
const GIT_EVERY: Duration = Duration::from_secs(5);
const HEALTH_EVERY: Duration = Duration::from_secs(30);
/// Tool input fields that say what a tool call acted on, most specific first.
const TOOL_TARGETS: [&str; 5] = ["command", "cmd", "file_path", "path", "notebook_path"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    pub allowed: u64,
    pub would_deny: u64,
    pub denied: u64,
    pub errors: u64,
}

impl Counts {
    /// A rewritten call went ahead, so it counts as allowed.
    fn add(&mut self, outcome: &str) {
        match outcome {
            "allow" | "rewrite" => self.allowed += 1,
            "would_deny" => self.would_deny += 1,
            "deny" => self.denied += 1,
            "error" => self.errors += 1,
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Filter {
    All,
    Denied,
    Errors,
}

impl Filter {
    fn next(self) -> Self {
        match self {
            Self::All => Self::Denied,
            Self::Denied => Self::Errors,
            Self::Errors => Self::All,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Trust {
    Trusted {
        gates: usize,
        stale: bool,
    },
    /// A policy file exists but no human trusted it, so its gates are ignored.
    Untrusted,
    NoPolicy,
    Unknown(String),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StateNote {
    Ready,
    Missing,
    /// Readable, but at an older schema than some blocks need.
    Old,
    Problem(String),
}

#[derive(Clone, Debug)]
pub(crate) struct SessionView {
    pub id: String,
    pub open: bool,
    pub counts: Counts,
    /// The row cap stopped counting before the session's start: counts are lower bounds.
    pub partial: bool,
    /// The session started before the look-back, so it is counted from this time on.
    pub since: Option<i64>,
}

#[derive(Clone, Debug)]
pub(crate) struct AgentRow {
    pub name: &'static str,
    pub today: Counts,
    pub today_partial: bool,
    pub session: Option<SessionView>,
    pub last_event: Option<i64>,
}

#[derive(Clone, Debug)]
pub(crate) struct Event {
    pub ts: i64,
    pub agent: String,
    pub event: String,
    pub tool: Option<String>,
    pub outcome: String,
    pub gate: String,
    pub detail: String,
}

#[derive(Clone, Debug)]
pub(crate) struct QuotaRow {
    pub agent: &'static str,
    pub windows: Vec<Window>,
    /// Why there are no windows to show.
    pub note: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Attention {
    pub unread: i64,
    /// Title and the command that inspects it.
    pub items: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub(crate) struct HookRow {
    pub agent: &'static str,
    /// `None` until `yoke health` answered.
    pub installed: Option<bool>,
    pub last_event: Option<i64>,
}

/// A block read from the state: `None` when there is no state, an error text when unreadable.
pub(crate) type Section<T> = Option<std::result::Result<T, String>>;

/// Everything one frame shows, gathered before any rendering.
#[derive(Clone, Debug)]
pub(crate) struct Snapshot {
    pub now: i64,
    pub offset: i64,
    pub version: &'static str,
    pub project_name: String,
    pub branch: Option<String>,
    pub changed: Option<usize>,
    pub mode: Option<String>,
    pub trust: Trust,
    pub state: StateNote,
    pub agents: Vec<AgentRow>,
    pub quota: Vec<QuotaRow>,
    pub filter: Filter,
    pub events: Vec<Event>,
    pub plans: Section<Vec<(String, String)>>,
    pub attention: Section<Attention>,
    /// Pending jobs by state, for example `("queued", 2)`.
    pub jobs: Section<Vec<(String, usize)>>,
    pub hooks: Vec<HookRow>,
    /// Median and largest gate time over the listed events, in milliseconds.
    pub gate_ms: Option<(u64, u64)>,
    /// What `yoke health` says to do next, with its tone.
    pub next: Option<(Tone, String)>,
}

/// Running per-agent and per-session counts of one project's decisions since `floor`, advanced
/// with only the rows written since the previous refresh.
#[derive(Default)]
struct Tally {
    project: String,
    floor: i64,
    newest_id: Option<i64>,
    today_start: i64,
    today: BTreeMap<String, Counts>,
    sessions: BTreeMap<String, Counts>,
    last_event: BTreeMap<String, i64>,
    /// Time of the oldest decision counted when the row cap stopped a pass before `floor`:
    /// counts that start before it are lower bounds.
    capped_at: Option<i64>,
    events: VecDeque<DecisionRow>,
}

impl Tally {
    fn new(project: String, floor: i64, today_start: i64) -> Self {
        Self {
            project,
            floor,
            today_start,
            ..Self::default()
        }
    }

    /// Whether counts from `since` on miss rows because the row cap stopped the first pass.
    fn partial_since(&self, since: i64) -> bool {
        self.capped_at.is_some_and(|at| at > since)
    }

    /// Read decisions newer than the last pass, newest first, down to `floor` or the row cap.
    fn advance(&mut self, store: &Store) -> Result<()> {
        let filter = DecisionFilter {
            project: Some(self.project.clone()),
            limit: PAGE,
            ..DecisionFilter::default()
        };
        let (mut before, mut read, mut newest) = (None, 0, self.newest_id);
        // Pages run newest first, so the first row of a pass is its newest.
        let mut fresh = Vec::new();
        'pages: loop {
            let page = store.decision_page(&filter, before)?;
            for row in &page {
                if self.newest_id.is_some_and(|id| row.id <= id) || row.record.ts < self.floor {
                    break 'pages;
                }
                newest = newest.max(Some(row.id));
                let record = &row.record;
                if record.ts >= self.today_start {
                    self.today
                        .entry(record.agent.clone())
                        .or_default()
                        .add(&record.outcome);
                }
                self.sessions
                    .entry(record.session_id.clone())
                    .or_default()
                    .add(&record.outcome);
                let last = self.last_event.entry(record.agent.clone()).or_default();
                *last = (*last).max(record.ts);
                if fresh.len() < EVENTS_KEPT {
                    fresh.push(row.clone());
                }
            }
            read += page.len();
            if page.len() < PAGE {
                break;
            }
            if read >= ROW_CAP {
                self.capped_at = page.last().map(|row| row.record.ts);
                break;
            }
            before = page.last().map(|row| row.id);
        }
        self.newest_id = newest;
        for row in fresh.into_iter().rev() {
            self.events.push_front(row);
        }
        self.events.truncate(EVENTS_KEPT);
        Ok(())
    }
}

/// Facts that take a subprocess, refreshed on a background thread so the view never waits.
#[derive(Clone, Default, PartialEq)]
struct Slow {
    changed: Option<usize>,
    next: Option<(Tone, String)>,
    installed: BTreeMap<String, bool>,
}

impl Slow {
    fn apply(&self, snapshot: &mut Snapshot) {
        snapshot.changed = self.changed;
        snapshot.next.clone_from(&self.next);
        for hook in &mut snapshot.hooks {
            hook.installed = self.installed.get(hook.agent).copied();
        }
    }
}

/// `yoke health --json` from this same executable, reduced to its first step and install state.
fn health() -> (Option<(Tone, String)>, BTreeMap<String, bool>) {
    let Ok(executable) = std::env::current_exe() else {
        return (None, BTreeMap::new());
    };
    let output = execute_bounded(
        Command::new(executable).args(["health", "--json"]),
        b"",
        Duration::from_secs(10),
        1024 * 1024,
    );
    let report = output
        .ok()
        .filter(|output| !output.timed_out && !output.stdout_truncated)
        .and_then(|output| serde_json::from_slice::<Value>(&output.stdout).ok());
    let Some(report) = report else {
        return (
            Some((
                Tone::Dim,
                "yoke health did not answer; run it to check setup.".into(),
            )),
            BTreeMap::new(),
        );
    };
    let first = |key: &str| report[key][0].as_str().map(str::to_owned);
    let next = if let Some(problem) = first("problems") {
        Some((Tone::Bad, problem))
    } else if let Some(step) = first("guidance") {
        Some((Tone::Warn, step))
    } else {
        Some((
            Tone::Good,
            "Everything yoke health checks is in place.".into(),
        ))
    };
    let installed = report["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|agent| {
            Some((
                agent["agent"].as_str()?.to_owned(),
                agent["installed"].as_bool()?,
            ))
        })
        .collect();
    (next, installed)
}

fn spawn_slow(root: PathBuf, slow: Arc<Mutex<Slow>>, stop: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let (mut git_at, mut health_at) = (None::<Instant>, None::<Instant>);
        while !stop.load(Ordering::SeqCst) {
            if git_at.is_none_or(|at| at.elapsed() >= GIT_EVERY) {
                let changed = git::changed_files(&root, Duration::from_secs(2));
                if let Ok(mut slow) = slow.lock() {
                    slow.changed = changed;
                }
                git_at = Some(Instant::now());
            }
            if health_at.is_none_or(|at| at.elapsed() >= HEALTH_EVERY) {
                let (next, installed) = health();
                if let Ok(mut slow) = slow.lock() {
                    slow.next = next;
                    slow.installed = installed;
                }
                health_at = Some(Instant::now());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    });
}

/// The state and what was read from it, kept between refreshes.
struct Source {
    home: PathBuf,
    cwd: PathBuf,
    /// The project the last refresh resolved; the working directory until then.
    root: PathBuf,
    store: Option<Store>,
    /// Answers for trusted policy while there is no state: an empty database, never a file.
    memory: Store,
    state: StateNote,
    revision: Option<i64>,
    read_at: Option<Instant>,
    tally: Option<Tally>,
    filter: Filter,
    filtered: Vec<DecisionRow>,
    cached: Option<Snapshot>,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

fn short(error: &anyhow::Error) -> String {
    let text = format!("{error:#}");
    yoke_core::redact::redact(text.lines().next().unwrap_or_default())
}

impl Source {
    fn new(home: PathBuf, cwd: PathBuf) -> Result<Self> {
        Ok(Self {
            home,
            root: cwd.clone(),
            cwd,
            store: None,
            memory: Store::open(":memory:")?,
            state: StateNote::Missing,
            revision: None,
            read_at: None,
            tally: None,
            filter: Filter::All,
            filtered: Vec::new(),
            cached: None,
        })
    }

    /// Open the state read-only when it appears; report why when it cannot be read.
    fn connect(&mut self) {
        if self.store.is_some() {
            return;
        }
        let path = self.home.join(".yoke/state.db");
        if !path.exists() {
            self.state = StateNote::Missing;
            return;
        }
        self.state = match Store::open_read_only(&path) {
            Ok(store) => match store.schema_version() {
                Ok((0, _)) => StateNote::Problem(format!(
                    "The state database {} is empty or incomplete; start your agent to initialise it.",
                    path.display()
                )),
                Ok((schema, supported)) => {
                    self.store = Some(store);
                    if schema < supported {
                        StateNote::Old
                    } else {
                        StateNote::Ready
                    }
                }
                Err(error) => StateNote::Problem(format!("Cannot read state: {}", short(&error))),
            },
            Err(error) if error.downcast_ref::<MissingSidecars>().is_some() => {
                StateNote::Problem(short(&error))
            }
            Err(error) => StateNote::Problem(format!(
                "Cannot read state: {}; retrying.",
                short(&error)
            )),
        };
    }

    /// Whether the database changed since the last read, or enough time passed to read anyway.
    fn changed(&mut self) -> bool {
        let revision = self.store.as_ref().and_then(|s| s.revision().ok());
        let stale = self.read_at.is_none_or(|at| at.elapsed() >= DATA_AT_LEAST);
        let changed = revision.is_none() || revision != self.revision || stale;
        self.revision = revision;
        changed
    }

    fn collect(&mut self, now: i64, offset: i64, slow: &Slow) -> Snapshot {
        self.connect();
        let store = self.store.as_ref().unwrap_or(&self.memory);
        let loaded = config::load_trusted(store, &self.cwd, &self.home);
        let root = loaded
            .as_ref()
            .map_or_else(|_| self.cwd.clone(), |l| l.project_root.clone());
        self.root = root.clone();
        let trust = match &loaded {
            Err(error) => Trust::Unknown(short(error)),
            Ok(loaded) if loaded.snapshots.iter().any(|s| s.trusted) => Trust::Trusted {
                gates: loaded.config.gates.len(),
                stale: loaded
                    .snapshots
                    .iter()
                    .any(|s| s.stale || (s.exists && !s.trusted)),
            },
            Ok(loaded) if loaded.snapshots.iter().any(|s| s.exists) => Trust::Untrusted,
            Ok(_) => Trust::NoPolicy,
        };
        let mut snapshot = Snapshot {
            now,
            offset,
            version: env!("CARGO_PKG_VERSION"),
            project_name: root.file_name().map_or_else(
                || root.display().to_string(),
                |n| n.to_string_lossy().into(),
            ),
            branch: git::branch(&root),
            changed: None,
            mode: loaded.as_ref().ok().map(|l| l.config.mode.as_str().into()),
            trust,
            state: self.state.clone(),
            agents: Vec::new(),
            quota: quotas(&self.home, self.store.as_ref()),
            filter: self.filter,
            events: Vec::new(),
            plans: None,
            attention: None,
            jobs: None,
            hooks: Vec::new(),
            gate_ms: None,
            next: None,
        };
        let project = root.display().to_string();
        if self.store.is_some() {
            if self.changed() || self.cached.is_none() {
                if let Err(error) = self.read(&root, &project, now, offset) {
                    self.state = StateNote::Problem(format!(
                        "Cannot read state: {}; showing the last reading.",
                        short(&error)
                    ));
                    // A replaced or damaged file is reopened on the next refresh.
                    self.store = None;
                } else {
                    self.read_at = Some(Instant::now());
                }
            }
            snapshot.state = self.state.clone();
        }
        if let Some(cached) = &self.cached {
            snapshot.agents = cached.agents.clone();
            snapshot.events = cached.events.clone();
            snapshot.plans = cached.plans.clone();
            snapshot.attention = cached.attention.clone();
            snapshot.jobs = cached.jobs.clone();
            snapshot.gate_ms = cached.gate_ms;
            snapshot.hooks = cached.hooks.clone();
        } else {
            // Nothing read yet: no agent rows rather than zero counts that would look measured.
            snapshot.hooks = AGENTS
                .iter()
                .map(|agent| HookRow {
                    agent,
                    installed: None,
                    last_event: None,
                })
                .collect();
        }
        slow.apply(&mut snapshot);
        snapshot
    }

    /// Re-read every block from the state into `cached`.
    fn read(&mut self, root: &Path, project: &str, now: i64, offset: i64) -> Result<()> {
        let store = self.store.as_ref().expect("connected before reading");
        let today_start = timefmt::day_start(now, offset);
        let sessions = store.sessions(Some(project), 50)?;
        let current: Vec<Option<&SessionSummary>> = AGENTS
            .iter()
            .map(|agent| {
                let mine = || sessions.iter().filter(move |s| s.agent == *agent);
                mine()
                    .find(|s| s.ended_at.is_none())
                    .or_else(|| mine().next())
            })
            .collect();
        let floor = current
            .iter()
            .flatten()
            .map(|s| s.started_at)
            .fold(today_start, i64::min)
            .max(now - SESSION_LOOKBACK);
        // A new day, project or earlier session start needs a fresh count.
        if self
            .tally
            .as_ref()
            .is_none_or(|t| t.project != project || t.today_start != today_start || floor < t.floor)
        {
            self.tally = Some(Tally::new(project.into(), floor, today_start));
        }
        let tally = self.tally.as_mut().expect("tally exists");
        tally.advance(store)?;
        let agents = AGENTS
            .iter()
            .zip(&current)
            .map(|(name, session)| AgentRow {
                name,
                today: tally.today.get(*name).copied().unwrap_or_default(),
                today_partial: tally.partial_since(today_start),
                session: session.map(|s| SessionView {
                    id: s.id.clone(),
                    open: s.ended_at.is_none(),
                    counts: tally.sessions.get(&s.id).copied().unwrap_or_default(),
                    partial: tally.partial_since(s.started_at.max(tally.floor)),
                    since: (s.started_at < tally.floor).then_some(tally.floor),
                }),
                last_event: tally.last_event.get(*name).copied().or_else(|| {
                    sessions
                        .iter()
                        .filter(|s| s.agent == *name)
                        .filter_map(|s| s.last_event_at)
                        .max()
                }),
            })
            .collect();
        let rows: Vec<&DecisionRow> = if self.filter == Filter::All {
            tally.events.iter().collect()
        } else {
            self.filtered = filtered(store, project, self.filter)?;
            self.filtered.iter().collect()
        };
        let events: Vec<Event> = rows.iter().map(|row| event(row, project)).collect();
        let mut times: Vec<u64> = rows
            .iter()
            .filter_map(|row| {
                let gates = row.record.gates.as_array()?;
                (!gates.is_empty()).then(|| gates.iter().filter_map(|g| g["ms"].as_u64()).sum())
            })
            .collect();
        times.sort_unstable();
        let gate_ms = (!times.is_empty()).then(|| (times[times.len() / 2], times[times.len() - 1]));
        let old = self.state == StateNote::Old;
        let plans = plan::list(store, root).map(|plans| {
            plans
                .into_iter()
                .filter(|p| !matches!(p.state.as_str(), "done" | "cancelled"))
                .map(|p| (p.slug, p.state))
                .collect()
        });
        let attention = (|| {
            Ok(Attention {
                unread: notifications::unread_count(store, root)?,
                items: notifications::list(store, root, true, 3)?
                    .iter()
                    .map(|n| (n.title.clone(), crate::notifications::next_command(n)))
                    .collect(),
            })
        })();
        let jobs = jobs::list(store, false, 1000).map(|jobs| {
            let mut states: BTreeMap<String, usize> = BTreeMap::new();
            for job in jobs {
                *states.entry(job.state).or_default() += 1;
            }
            ["running", "queued", "failed", "dead"]
                .iter()
                .filter_map(|state| states.get(*state).map(|n| ((*state).to_owned(), *n)))
                .collect()
        });
        let (plans, attention, jobs) = (
            section(plans, old),
            section(attention, old),
            section(jobs, old),
        );
        let hooks = AGENTS
            .iter()
            .map(|agent| HookRow {
                agent,
                installed: None,
                last_event: store.last_decision(agent).ok().flatten().map(|d| d.0),
            })
            .collect();
        self.cached = Some(Snapshot {
            agents,
            events,
            plans,
            attention,
            jobs,
            hooks,
            gate_ms,
            ..empty(now, offset)
        });
        Ok(())
    }
}

/// A block's reading, or why it failed; at an older schema the cause is almost always a table or
/// column only the newer schema has.
fn section<T>(result: Result<T>, old: bool) -> Section<T> {
    Some(result.map_err(|error| {
        if old {
            "needs the newer state schema; the next hook event upgrades it".into()
        } else {
            short(&error)
        }
    }))
}

/// A blank snapshot that `Source::read` fills in; only its read-from-state fields are kept.
fn empty(now: i64, offset: i64) -> Snapshot {
    Snapshot {
        now,
        offset,
        version: env!("CARGO_PKG_VERSION"),
        project_name: String::new(),
        branch: None,
        changed: None,
        mode: None,
        trust: Trust::NoPolicy,
        state: StateNote::Ready,
        agents: Vec::new(),
        quota: Vec::new(),
        filter: Filter::All,
        events: Vec::new(),
        plans: None,
        attention: None,
        jobs: None,
        hooks: Vec::new(),
        gate_ms: None,
        next: None,
    }
}

/// The newest decisions of the project with the filter's outcomes, merged newest first.
fn filtered(store: &Store, project: &str, filter: Filter) -> Result<Vec<DecisionRow>> {
    let outcomes: &[&str] = match filter {
        Filter::All => &[],
        Filter::Denied => &["deny", "would_deny"],
        Filter::Errors => &["error"],
    };
    let mut rows = Vec::new();
    for outcome in outcomes {
        rows.extend(store.decision_page(
            &DecisionFilter {
                project: Some(project.into()),
                outcome: Some((*outcome).into()),
                limit: EVENTS_KEPT,
                ..DecisionFilter::default()
            },
            None,
        )?);
    }
    rows.sort_by_key(|row| std::cmp::Reverse(row.id));
    rows.truncate(EVENTS_KEPT);
    Ok(rows)
}

/// One list row: gates that did more than pass, then what the tool acted on, relative to the
/// project when it is inside it, and the reason.
fn event(row: &DecisionRow, root: &str) -> Event {
    let record = &row.record;
    let gate = record
        .gates
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|gate| {
            let (name, result) = (gate["name"].as_str()?, gate["result"].as_str()?);
            (!matches!(result, "pass" | "skip")).then(|| name.to_owned())
        })
        .collect::<Vec<_>>()
        .join("+");
    let target = serde_json::from_str::<Value>(&record.payload)
        .ok()
        .and_then(|payload| {
            TOOL_TARGETS
                .iter()
                .find_map(|key| payload["tool_input"][*key].as_str().map(str::to_owned))
        })
        .map(|target| match target.strip_prefix(root) {
            Some(inside) if inside.starts_with('/') && !root.is_empty() => {
                inside.trim_start_matches('/').to_owned()
            }
            _ => target,
        });
    let target = match (target, record.reason.as_deref()) {
        (Some(target), Some(reason)) if !reason.is_empty() => format!("{target} - {reason}"),
        (Some(target), _) => target,
        (None, reason) => reason.unwrap_or_default().to_owned(),
    };
    Event {
        ts: record.ts,
        agent: record.agent.clone(),
        event: record.event.clone(),
        tool: record.tool.clone(),
        outcome: record.outcome.clone(),
        gate,
        detail: target.split_whitespace().collect::<Vec<_>>().join(" "),
    }
}

/// Quota as `yoke usage` reads it: Claude's from the statusline's file, Codex's from the state.
fn quotas(home: &Path, store: Option<&Store>) -> Vec<QuotaRow> {
    let readings = [
        (
            "claude",
            quota::read_file(&quota::claude_file(home), "claude"),
            "not recorded yet; the Yoke statusline records it once Claude Code reports rate limits",
        ),
        (
            "codex",
            store.map_or(Ok(None), |store| quota::load(store, "codex")),
            "not recorded yet; read from Codex session records when a session stops or ends",
        ),
    ];
    readings
        .into_iter()
        .map(|(agent, reading, missing)| match reading {
            Ok(Some(reading)) if !reading.windows.is_empty() => QuotaRow {
                agent,
                note: None,
                windows: reading.windows,
            },
            Ok(_) => QuotaRow {
                agent,
                windows: Vec::new(),
                note: Some(missing.into()),
            },
            Err(error) => QuotaRow {
                agent,
                windows: Vec::new(),
                note: Some(format!("unreadable: {}", short(&error))),
            },
        })
        .collect()
}

fn frame_for_snapshot(color: bool, ascii: bool) -> Frame {
    let width = std::env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|width| (20..=1000).contains(width))
        .unwrap_or(100);
    Frame {
        width,
        height: None,
        color,
        ascii,
    }
}

/// `NO_COLOR` set to anything but empty turns color off, as does a dumb terminal.
fn color_allowed() -> bool {
    std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty())
        && std::env::var_os("TERM").is_none_or(|term| term != "dumb")
}

fn ascii_only() -> bool {
    std::env::var_os("YOKE_ASCII").is_some_and(|value| value == "1")
}

pub fn run() -> Result<()> {
    let mut source = Source::new(crate::commands::home()?, std::env::current_dir()?)?;
    let (ascii, color) = (ascii_only(), color_allowed());
    let now = now_secs();
    let offset = timefmt::utc_offset_secs(now);
    // The first reading resolves the project root that git and the worker look at.
    source.collect(now, offset, &Slow::default());
    if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
        let (next, installed) = health();
        let slow = Slow {
            changed: git::changed_files(&source.root, Duration::from_secs(2)),
            next,
            installed,
        };
        let snapshot = source.collect(now, offset, &slow);
        let frame = frame_for_snapshot(color && std::io::stdout().is_terminal(), ascii);
        for line in render::render(&snapshot, &frame, false) {
            println!("{line}");
        }
        return Ok(());
    }
    live(source, color, ascii)
}

fn live(mut source: Source, color: bool, ascii: bool) -> Result<()> {
    let slow = Arc::new(Mutex::new(Slow::default()));
    let stop = Arc::new(AtomicBool::new(false));
    spawn_slow(source.root.clone(), Arc::clone(&slow), Arc::clone(&stop));
    let raw = term::Raw::enter()?;
    let result = (|| -> Result<()> {
        let mut snapshot = None::<Snapshot>;
        let mut read_at = None::<Instant>;
        // Size and second of the last paint; a new reading always repaints.
        let mut painted = None::<((usize, usize), i64)>;
        let mut applied = Slow::default();
        loop {
            if term::stop_requested() {
                return Ok(());
            }
            let now = now_secs();
            // The worker's latest facts, shown as soon as they arrive rather than at the next read.
            let latest = slow
                .lock()
                .map_err(|_| anyhow::anyhow!("the watch worker stopped"))?
                .clone();
            if snapshot.is_none() || read_at.is_none_or(|at| at.elapsed() >= DATA_EVERY) {
                snapshot = Some(source.collect(now, timefmt::utc_offset_secs(now), &latest));
                read_at = Some(Instant::now());
                painted = None;
            }
            let size = term::size().unwrap_or((80, 24));
            if let Some(snapshot) = snapshot.as_mut() {
                if latest != applied {
                    latest.apply(snapshot);
                    applied = latest;
                    painted = None;
                }
                if painted != Some((size, now)) {
                    snapshot.now = now;
                    // The last column stays empty: a full line would wrap on some terminals.
                    let frame = Frame {
                        width: size.0.saturating_sub(1).max(1),
                        height: Some(size.1),
                        color,
                        ascii,
                    };
                    term::paint(&render::render(snapshot, &frame, true))?;
                    painted = Some((size, now));
                }
            }
            for key in raw.keys(Duration::from_millis(250)) {
                match key {
                    b'q' | b'Q' | 3 | 4 => return Ok(()),
                    b'f' | b'F' => {
                        source.filter = source.filter.next();
                        source.cached = None;
                        read_at = None;
                    }
                    b'r' | b'R' => {
                        source.cached = None;
                        read_at = None;
                    }
                    _ => {}
                }
            }
        }
    })();
    stop.store(true, Ordering::SeqCst);
    drop(raw);
    result
}
