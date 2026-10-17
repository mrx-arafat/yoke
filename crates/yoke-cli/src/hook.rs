use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{ensure, Context, Result};
use serde_json::Value;
use yoke_core::config::{self, Mode};
use yoke_core::engine::{evaluate_for_agent_with_context, EvaluationContext};
use yoke_core::event::Agent;
use yoke_core::redact::redact;
use yoke_core::store::{DecisionRecord, Store};
use yoke_core::{memory, notifications, plan, stop_guard, trust, wire, AgentEvent, EventKind};

use crate::commands;

mod mode_cache;
mod provenance;

/// A line for the person watching stderr. Unlike `eprintln!` it never panics: a stderr pipe the
/// agent has already closed would otherwise end the hook with exit 101 before it printed its
/// denial, and an agent treats a crashed hook as no answer at all.
macro_rules! say {
    ($($arg:tt)*) => {{
        let _ = writeln!(std::io::stderr().lock(), $($arg)*);
    }};
}

const INPUT_CAP: u64 = 1024 * 1024;
/// How much of an oversized payload is read and dropped, so that an agent still writing it is
/// not cut off by a broken pipe and the hook still ends in bounded time.
const INPUT_DRAIN_CAP: u64 = 64 * 1024 * 1024;
/// error.log is moved to error.log.1 once it passes this size, so a failure that repeats on
/// every tool call cannot fill the disk.
const ERROR_LOG_CAP: u64 = 1024 * 1024;
const UPDATE_INTERVAL: i64 = 86400;
pub(crate) const UPDATE_ATTEMPT: &str = "update.last_attempt";
// A yoke.toml that nobody trusted, nearer to an event than the trusted policy, is reported
// when it is first seen, at most once in this interval, instead of at the next session start.
const IGNORED_CONFIG_NOTICE: &str = "notifications.ignored_config";
const IGNORED_CONFIG_INTERVAL: i64 = 300;
/// Marks a process Yoke started in the background with its stderr pointed at error.log.
pub(crate) const BACKGROUND: &str = "YOKE_BACKGROUND";
/// Claude Code lets a call through when a hook outlives its timeout (600 s by default, 30 s for
/// UserPromptSubmit) and Codex defaults to 600 s, so a blocking hook answers itself first.
/// These defaults grow to fit the event's trusted check gates, up to `config::hook_budget`.
const DEADLINE: Duration = Duration::from_secs(120);
const PROMPT_DEADLINE: Duration = Duration::from_secs(25);
/// Set once a decision has been answered, so the watchdog and the normal path never both print.
static ANSWERED: Mutex<bool> = Mutex::new(false);
/// The mode the watchdog answers in: 0 not yet known (treated as enforce), 1 observe, 2 enforce.
static MODE: AtomicU8 = AtomicU8::new(0);
/// The watchdog's current deadline in milliseconds since the hook started.
static DEADLINE_MS: AtomicU64 = AtomicU64::new(u64::MAX);
/// What the watchdog needs to record its own decision, published once the policy is loaded.
static OVERRUN: Mutex<Option<Overrun>> = Mutex::new(None);
/// The payload said an earlier Stop hook already kept the agent working (`stop_hook_active`).
static STOP_ACTIVE: AtomicBool = AtomicBool::new(false);

struct Overrun {
    home: PathBuf,
    project: PathBuf,
    row: DecisionRecord,
    max_blocks: u32,
    cap: usize,
}

fn publish(mode: Option<Mode>) {
    let value = match mode {
        None => 0,
        Some(Mode::Observe) => 1,
        Some(Mode::Enforce) => 2,
    };
    MODE.store(value, Ordering::SeqCst);
}

/// A blocking event's deadline: its default, raised to fit the trusted check gates on it plus
/// Yoke's margin, never above the ceiling under the agent's own hook timeout.
/// `YOKE_HOOK_DEADLINE_MS` may only shorten it.
fn deadline(kind: EventKind, gate_seconds: u64, shorter_ms: Option<u64>) -> Duration {
    let base = if kind == EventKind::UserPromptSubmit {
        PROMPT_DEADLINE
    } else {
        DEADLINE
    };
    let limit = match config::hook_budget(kind) {
        Some((ceiling, margin)) => base
            .max(Duration::from_secs(gate_seconds.saturating_add(margin)))
            .min(Duration::from_secs(ceiling)),
        None => base,
    };
    shorter_ms.map_or(limit, |millis| limit.min(Duration::from_millis(millis)))
}

fn shorter_deadline() -> Option<u64> {
    std::env::var("YOKE_HOOK_DEADLINE_MS")
        .ok()
        .and_then(|value| value.parse().ok())
}

fn set_deadline(limit: Duration) {
    DEADLINE_MS.store(
        u64::try_from(limit.as_millis()).unwrap_or(u64::MAX),
        Ordering::SeqCst,
    );
}

/// If evaluation has not answered by the deadline, stop the gates still running, record the
/// overrun, deny in enforce (or unknown) mode, print nothing in observe, and exit.
fn watchdog(agent: Agent, kind: EventKind) {
    set_deadline(deadline(kind, 0, shorter_deadline()));
    let start = std::time::Instant::now();
    std::thread::spawn(move || {
        let limit = loop {
            let limit = Duration::from_millis(DEADLINE_MS.load(Ordering::SeqCst));
            let elapsed = start.elapsed();
            if elapsed >= limit {
                break limit;
            }
            std::thread::sleep((limit - elapsed).min(Duration::from_millis(100)));
        };
        let mut answered = ANSWERED.lock().unwrap_or_else(PoisonError::into_inner);
        if *answered {
            return;
        }
        *answered = true;
        let reason = format!("yoke error: hook evaluation exceeded {} ms; for an explicit bypass, a human runs yoke pause in their own terminal", limit.as_millis());
        say!("{}", redact(&reason));
        if let Ok(home) = crate::bypass::hook_home() {
            let _ = append_error(&home, &reason);
        }
        let enforce = MODE.load(Ordering::SeqCst) != 1;
        if let Some(denial) = record_overrun(kind, &reason, limit, enforce) {
            if let Some(output) = wire::deny(agent, kind, &denial) {
                let _ = writeln!(std::io::stdout().lock(), "{output}");
            }
        }
        let _ = std::io::stdout().flush();
        // Gates still running would outlive the hook; killing them only now keeps the main
        // thread from recording a second decision for a gate it saw fail.
        yoke_core::engine::kill_running_groups();
        std::process::exit(0);
    });
}

/// Store the watchdog's decision; in enforce mode a Stop also counts toward the stall cap and
/// is allowed once its plan stalls. Returns the denial reason, if any; enforce mode denies when
/// nothing could be recorded. The payload is not stored, because redacting it may be what was
/// slow.
fn record_overrun(kind: EventKind, reason: &str, limit: Duration, enforce: bool) -> Option<String> {
    let denial = enforce.then(|| reason.to_owned());
    let Some(context) = OVERRUN
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
    else {
        return once_per_stop(kind, denial);
    };
    let mut row = context.row;
    row.reason = Some(reason.into());
    row.payload = provenance::payload(&mut serde_json::json!({}));
    row.gates = serde_json::json!([{"name": "watchdog", "result": "timeout", "ms": limit.as_millis(), "exit_code": null}]);
    let result = commands::open_store(&context.home).and_then(|store| {
        if enforce && kind == EventKind::Stop && !row.session_id.is_empty() {
            stop_guard::record_overrun(
                &store,
                &context.project,
                context.max_blocks,
                &mut row,
                context.cap,
            )
            .map(|decision| {
                decision
                    .block
                    .then(|| row.reason.clone().unwrap_or_default())
            })
        } else {
            row.outcome = "error".into();
            store
                .record_decision(&row, context.cap)
                .map(|()| denial.clone())
        }
    });
    result.unwrap_or_else(|error| {
        warn_logged(
            &context.home,
            &format!("watchdog decision was not saved: {error:#}"),
        );
        once_per_stop(kind, denial)
    })
}

/// A Stop that blocks keeps the agent working and sends it back to Stop, so a block that rests
/// on Yoke's own failure (unreadable state, no stall count to cap it) would repeat for as long
/// as the failure lasts. The agent sets `stop_hook_active` on that second Stop: it is told once
/// and then allowed. Gate failures are not affected; the stall cap bounds them.
fn once_per_stop(kind: EventKind, denial: Option<String>) -> Option<String> {
    if denial.is_some() && kind == EventKind::Stop && STOP_ACTIVE.load(Ordering::SeqCst) {
        say!("yoke: not blocking this Stop again; the agent was already kept working once by a Yoke failure");
        return None;
    }
    denial
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

struct HookState {
    home: Option<PathBuf>,
    store: Option<Store>,
    mode: Option<Mode>,
    cap: usize,
    persisted: bool,
    row: DecisionRecord,
    /// Whether stdin has been consumed, so that a failure before that can still read it.
    input_read: bool,
    /// The first policy load finished, so the mode is known from state, not from the record.
    policy_loaded: bool,
    /// The project key and mode to write to the mode record after a successful run.
    remember: Option<(String, Mode)>,
}

/// Agents end a hook that outlives its timeout, or that the user interrupts, with a signal. The
/// gates run in their own process groups, so without this they would outlive the hook and keep
/// running (a test suite, for example) after the agent has moved on. SIGKILL cannot be caught,
/// so it still leaves them behind.
fn stop_gates_on_termination() {
    extern "C" fn terminate(signal: libc::c_int) {
        // Only async-signal-safe work: atomics, kill(2) and _exit(2).
        yoke_core::engine::kill_running_groups();
        // SAFETY: _exit is async-signal-safe and ends the process without running destructors.
        unsafe { libc::_exit(128 + signal) };
    }
    for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: the handler above is async-signal-safe. A handler, unlike a blocked signal
        // mask, is reset by exec, so the gates start with their normal signal behavior.
        unsafe {
            libc::signal(
                signal,
                terminate as extern "C" fn(libc::c_int) as libc::sighandler_t,
            )
        };
    }
}

/// Hook errors never escape into ordinary CLI stdout.
pub fn run(event: &str, agent: Agent) -> i32 {
    stop_gates_on_termination();
    // Redaction compiles its patterns on first use, about a millisecond of CPU that otherwise
    // sits on the critical path at the decision write. A helper thread compiles them now,
    // while this thread opens the state database and loads the policy; the first redaction
    // on this thread then finds them ready (or waits for the helper to finish them). The
    // patterns and their output are unchanged. A thread that cannot start costs nothing.
    let _ = std::thread::Builder::new().spawn(yoke_core::redact::warm);
    let kind = EventKind::parse(event);
    let mut state = HookState {
        home: None,
        store: None,
        mode: None,
        cap: 4096,
        persisted: false,
        row: DecisionRecord {
            ts: now(),
            session_id: String::new(),
            agent: agent.as_str().into(),
            project: String::new(),
            event: event.into(),
            tool: None,
            mode: "unknown".into(),
            outcome: "error".into(),
            reason: None,
            gates: serde_json::json!([]),
            payload: String::new(),
        },
        input_read: false,
        policy_loaded: false,
        remember: None,
    };
    if let Some(kind) = kind.filter(|kind| kind.is_blocking()) {
        watchdog(agent, kind);
    }
    let result = contain(|| evaluate(event, agent, &mut state));
    // Holding this until the process exits keeps a late watchdog from answering a second time.
    let mut answered = ANSWERED.lock().unwrap_or_else(PoisonError::into_inner);
    *answered = true;
    match result {
        Ok(output) => match output
            .map(|output| writeln!(std::io::stdout().lock(), "{output}"))
            .transpose()
        {
            Ok(_) => {
                // After the answer is out; a hook that read its policy knows the mode for sure.
                if let (Some(home), Some((key, mode))) = (&state.home, &state.remember) {
                    mode_cache::remember(home, key, *mode, now());
                }
                0
            }
            Err(error) => fail(
                &mut state,
                agent,
                kind,
                &format!("writing hook output: {error}"),
            ),
        },
        Err(error) => fail(&mut state, agent, kind, &format!("{error:#}")),
    }
}

/// The payload from stdin: at most `INPUT_CAP + 1` bytes, so a larger one is seen as too large.
/// The rest of an oversized payload is read and dropped, because an agent that is still writing
/// would otherwise get a broken pipe instead of Yoke's answer. Stdin is consumed only once.
fn read_input(consumed: &mut bool) -> Result<Vec<u8>> {
    *consumed = true;
    let mut stdin = std::io::stdin().lock();
    let mut bytes = Vec::new();
    (&mut stdin)
        .take(INPUT_CAP + 1)
        .read_to_end(&mut bytes)
        .context("read hook payload")?;
    if bytes.len() as u64 > INPUT_CAP {
        let _ = std::io::copy(
            &mut (&mut stdin).take(INPUT_DRAIN_CAP),
            &mut std::io::sink(),
        );
    }
    Ok(bytes)
}

/// Turn a panic into an error. A hook that panics exits 101 without printing anything, which an
/// agent reads as "no answer" and lets the action through; as an error it is denied in enforce
/// mode and recorded like any other failure.
fn contain<T>(work: impl FnOnce() -> Result<T>) -> Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).unwrap_or_else(|payload| {
        let message = payload
            .downcast_ref::<&str>()
            .map(|text| (*text).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".into());
        Err(anyhow::anyhow!("internal error: {message}"))
    })
}

/// Where a session starts, as far as one event can tell: the project directory Claude Code
/// names in `CLAUDE_PROJECT_DIR` when it is a usable absolute directory, else `fallback` (the
/// directory the event reports). Only the first event of a session uses this; after that the
/// stored anchor decides.
fn start_directory(agent: Agent, fallback: &Path) -> PathBuf {
    let named = (agent == Agent::Claude)
        .then(|| std::env::var_os("CLAUDE_PROJECT_DIR"))
        .flatten()
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .and_then(|path| path.canonicalize().ok())
        .filter(|path| path.is_dir());
    named.unwrap_or_else(|| fallback.to_path_buf())
}

fn evaluate(event: &str, agent: Agent, state: &mut HookState) -> Result<Option<Value>> {
    // A HOME that settings files inject must not move Yoke's state, trust and logs elsewhere.
    let home = crate::bypass::hook_home()?;
    state.home = Some(home.clone());
    // An old empty state file means earlier trust approvals are gone, and Yoke starts over with
    // its defaults; a fresh one is another hook creating the database right now.
    let was_empty = std::fs::metadata(home.join(".yoke/state.db")).is_ok_and(|file| {
        file.len() == 0
            && file
                .modified()
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age > Duration::from_secs(10))
    });
    state.store = Some(match commands::open_store(&home) {
        Ok(store) => store,
        Err(error) => {
            let store = Store::open_read_only(home.join(".yoke/state.db")).with_context(|| {
                format!("writable state unavailable ({error:#}); opening read-only state")
            })?;
            let detail = format!("{error:#}");
            say!(
                "yoke: {}; retrying trusted policy using read-only state; audit and memory writes may be unavailable; {}",
                redact(&format!("writable state unavailable: {detail}")),
                state_hint(&detail)
            );
            store
        }
    });
    if was_empty {
        warn_logged(
            &home,
            "state.db was empty (0 bytes), so earlier trust approvals, history and memory are not available; if you had approved a policy, review it and run `yoke trust` again in your own terminal (docs/troubleshooting.md)",
        );
    }
    let store = state.store.as_ref().expect("opened above");
    let process_cwd = std::env::current_dir().context("resolve hook working directory")?;
    state.row.project = process_cwd.display().to_string();
    // Resolve mode before reading malformed input so enforce-mode errors still deny. The
    // session's start directory is not known before the payload is read, so this uses the
    // project directory the agent names, else the directory the hook runs in.
    let hint = start_directory(agent, &process_cwd);
    let mut loaded = config::load_trusted(store, &hint, &home)?;
    state.policy_loaded = true;
    state.remember = mode_cache::project_key(&hint).map(|key| (key, loaded.config.mode));
    state.mode = Some(loaded.config.mode);
    publish(state.mode);
    state.row.mode = loaded.config.mode.as_str().into();
    state.cap = loaded.config.log.payload_cap;
    let bytes = read_input(&mut state.input_read)?;
    state.row.payload = String::from_utf8_lossy(&bytes).into_owned();
    ensure!(
        bytes.len() <= INPUT_CAP as usize,
        "hook payload exceeds 1 MiB, more than Yoke can check; ask the agent to use smaller tool inputs, for example a large file written in several smaller edits"
    );
    // Some producers put a byte order mark in front of the JSON; it carries no meaning here.
    let mut raw: Value =
        serde_json::from_slice(strip_bom(&bytes)).context("parse hook JSON object")?;
    let ev: AgentEvent = serde_json::from_value(raw.clone()).context("parse hook event fields")?;
    state.row.payload = provenance::payload(&mut raw);
    state.row.session_id = ev.session_id.clone();
    state.row.tool = ev.tool_name.clone();
    STOP_ACTIVE.store(stop_hook_active(&raw), Ordering::SeqCst);
    let cwd = match ev.cwd.as_deref() {
        Some(path) => {
            let path = PathBuf::from(path);
            ensure!(path.is_absolute(), "hook cwd must be absolute");
            match path.canonicalize() {
                Ok(path) => path,
                // The reported directory can vanish under a session (renamed, or a removed
                // worktree). The policy this hook started under still governs the event, so
                // the agent is not denied every tool call because of a missing directory.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => process_cwd.clone(),
                Err(error) => return Err(error).context("resolve payload cwd"),
            }
        }
        None => process_cwd.clone(),
    };
    // The project is fixed where the session started, not where this event says it is: a
    // directory the agent can change must not decide whether the trusted policy applies. The
    // payload cwd only picks a trusted nested policy at or below that start directory.
    // Until the policy of a different anchor or directory loads, the mode is unknown.
    let mut anchor = trust::session_anchor(
        store,
        agent.as_str(),
        &ev.session_id,
        &start_directory(agent, &cwd),
    )?;
    if !anchor.dir.is_dir() {
        // The project directory was renamed or removed under the session. Gates cannot run
        // there, so the event is governed by the directory it reports, as before anchoring.
        anchor = trust::session_anchor(store, agent.as_str(), "", &cwd)?;
    }
    if anchor.dir != hint || cwd != hint || loaded.project_root != anchor.root {
        state.mode = None;
        publish(state.mode);
        state.row.mode = "unknown".into();
    }
    let trust::EventPolicy {
        loaded: policy,
        policy_root,
        ignored,
    } = trust::load_for_event(store, &anchor, &cwd, &home)?;
    loaded = policy;
    state.mode = Some(loaded.config.mode);
    publish(state.mode);
    state.row.mode = loaded.config.mode.as_str().into();
    let project_available = loaded.project_root.to_str().is_some();
    state.row.project = loaded.project_root.to_str().unwrap_or("").to_owned();
    if !project_available {
        say!("yoke: project scope unavailable because the project path is not UTF-8; rename the project directory using UTF-8 names to enable project policy, plans and memory; trusted parent and global policy still apply");
    }
    state.cap = loaded.config.log.payload_cap;
    let kind = EventKind::parse(event).with_context(|| format!("unknown hook event {event}"))?;
    state.row.event = kind.as_str().into();
    ensure!(
        agent.supports(kind),
        "{} does not document the {event} event",
        agent.as_str()
    );
    if let Some(name) = &ev.event_name {
        ensure!(
            EventKind::parse(name) == Some(kind),
            "payload event does not match requested event"
        );
    }
    if kind.is_blocking() {
        set_deadline(deadline(
            kind,
            loaded.config.gate_seconds(kind),
            shorter_deadline(),
        ));
        *OVERRUN.lock().unwrap_or_else(PoisonError::into_inner) = Some(Overrun {
            home: home.clone(),
            project: loaded.project_root.clone(),
            row: state.row.clone(),
            max_blocks: loaded.config.stop.max_blocks,
            cap: state.cap,
        });
    }
    // Accounting never decides or errors an event: a bad or oversized transcript warns and
    // the trusted gates and stop guard still run.
    if project_available
        && !ev.session_id.is_empty()
        && matches!(
            kind,
            EventKind::Stop | EventKind::SessionEnd | EventKind::PreCompact
        )
    {
        if let Some(path) = ev.transcript_path.as_deref() {
            if let Err(error) = yoke_core::usage::ingest(
                store,
                agent.as_str(),
                &ev.session_id,
                &state.row.project,
                Path::new(path),
            ) {
                warn_usage_ingestion(&home, &error);
            }
        }
    }
    let outcome = evaluate_for_agent_with_context(
        &loaded.config,
        kind,
        &ev,
        &EvaluationContext {
            cwd: cwd.clone(),
            project: loaded.project_root.clone(),
            policy_root,
            home: home.clone(),
        },
        agent,
    );
    state.row.gates = serde_json::to_value(&outcome.gates)?;
    if outcome.error {
        anyhow::bail!("{}", outcome.reasons.join("; "));
    }
    state.row.reason = (!outcome.reasons.is_empty()).then(|| outcome.reasons.join("; "));
    state.row.outcome = if outcome.blocked() {
        if loaded.config.mode == Mode::Observe {
            "would_deny"
        } else {
            "deny"
        }
    } else if outcome.updated_input.is_some() && loaded.config.mode == Mode::Enforce {
        "rewrite"
    } else {
        "allow"
    }
    .into();
    let mut output = if loaded.config.mode == Mode::Observe {
        None
    } else if outcome.blocked() {
        wire::deny(
            agent,
            kind,
            state.row.reason.as_deref().unwrap_or("gate denied"),
        )
    } else if let Some(input) = outcome.updated_input.as_ref() {
        Some(wire::rewrite(agent, ev.tool_name.as_deref(), input)?)
    } else {
        None
    };
    let atomic_stop = project_available && kind == EventKind::Stop && !ev.session_id.is_empty();
    if project_available && !ev.session_id.is_empty() && !atomic_stop {
        if let Err(error) = memory::record_hook_facts(store, &state.row, &raw) {
            warn_advisory(&home, "memory facts were not saved", &error);
        }
    }
    if project_available
        && loaded.config.mode == Mode::Enforce
        && !outcome.blocked()
        && matches!(kind, EventKind::SessionStart | EventKind::UserPromptSubmit)
    {
        let mut context = String::new();
        if !ev.session_id.is_empty() {
            // Plan context is advice; a lookup failure (such as an older read-only schema)
            // must not deny the prompt.
            match plan::active(store, &loaded.project_root, &ev.session_id) {
                Ok(Some(active)) => context.push_str(&format!("Yoke plan {} is {}. Inspect with yoke plan status {}. Waiting or stalled plans require a human to use yoke plan resume {}.\n", active.slug, active.state, active.slug, active.slug)),
                Ok(None) => {}
                Err(error) => warn_advisory(&home, "plan context unavailable", &error),
            }
        }
        if kind == EventKind::SessionStart {
            context.push_str(&memory::context(
                store,
                &state.row.project,
                "",
                loaded.knowledge.as_deref().unwrap_or(""),
                loaded.config.memory.inject_tokens,
            )?);
        }
        output = wire::context(agent, kind, &context);
    }
    if project_available
        && kind != EventKind::SessionStart
        && !ignored.is_empty()
        && store
            .claim_interval(IGNORED_CONFIG_NOTICE, state.row.ts, IGNORED_CONFIG_INTERVAL)
            .unwrap_or(false)
    {
        if let Err(error) = notifications::sync_for_hook(store, &loaded, state.row.ts) {
            warn_advisory(&home, "notification sync failed", &error);
        }
    }
    if kind == EventKind::SessionStart {
        let (now, days) = (state.row.ts, loaded.config.log.retention_days);
        // Retention is queued here and run by the background worker that `tick` starts below,
        // so a session never waits for deletes.
        if let Err(error) = crate::retention::schedule(store, now, days) {
            warn_advisory(&home, "retention scheduling failed", &error);
        }
        for status in loaded
            .snapshots
            .iter()
            .filter(|snapshot| snapshot.stale || (!snapshot.trusted && snapshot.exists))
        {
            say!(
                "yoke: config snapshot {} is {}; inspect yoke config",
                status.file.display(),
                if status.trusted {
                    "stale (using trusted bytes)"
                } else {
                    "untrusted (ignored)"
                }
            );
        }
        if let Err(error) = schedule_update(store, &home, state.row.ts, loaded.config.update.check)
        {
            warn_advisory(&home, "update check scheduling failed", &error);
        }
        // The statusline only counts materialized rows, so this is what lets it flag a new
        // untrusted or stale configuration; the bounded call never sleeps.
        if project_available {
            if let Err(error) = notifications::sync_for_hook(store, &loaded, now) {
                warn_advisory(&home, "notification sync failed", &error);
            }
        }
        if project_available && loaded.config.memory.digest && !ev.session_id.is_empty() {
            // Facts of sessions whose end hook never ran are digested now; `tick` starts the worker.
            if let Err(error) = crate::memory::schedule_orphan_digests(
                store,
                &loaded.project_root,
                &ev.session_id,
                now,
            ) {
                warn_advisory(&home, "orphan digest recovery failed", &error);
            }
        }
        if let Err(error) = crate::jobs::tick(store, &home, now, days) {
            warn_advisory(&home, "background job recovery failed", &error);
        }
    }
    if atomic_stop {
        match stop_guard::record_stop(
            store,
            &loaded.project_root,
            loaded.config.stop.max_blocks,
            &mut state.row,
            state.cap,
            &raw,
        ) {
            Ok(_decision) => {
                state.persisted = true;
            }
            Err(error) => {
                // Only audit persistence may fail open. A gate denial already decides the Stop;
                // otherwise an unreadable plan state or a required stop-guard write fails closed.
                if state.row.outcome == "allow" {
                    match stop_guard::allows_without_write(
                        store,
                        &loaded.project_root,
                        &ev.session_id,
                    ) {
                        Ok(true) => {}
                        Ok(false) => return Err(error),
                        Err(lookup) => {
                            return Err(error.context(format!(
                                "stop guard state is also unreadable: {lookup:#}"
                            )))
                        }
                    }
                }
                warn_advisory(&home, "stop audit was not saved", &error);
                if let Err(fallback) = memory::record_hook_facts(store, &state.row, &raw) {
                    warn_advisory(&home, "stop memory facts were not saved", &fallback);
                }
                match store.record_decision(&state.row, state.cap) {
                    Ok(()) => state.persisted = true,
                    Err(fallback) => {
                        warn_advisory(&home, "stop audit decision was not saved", &fallback);
                    }
                }
            }
        }
        output = if loaded.config.mode == Mode::Observe {
            None
        } else if state.row.outcome == "deny" {
            wire::deny(
                agent,
                kind,
                state.row.reason.as_deref().unwrap_or("stop denied"),
            )
        } else {
            None
        };
    } else {
        match store.record_decision(&state.row, state.cap) {
            Ok(()) => state.persisted = true,
            Err(error) => warn_advisory(&home, "audit decision was not saved", &error),
        }
    }
    if project_available
        && state.persisted
        && loaded.config.memory.digest
        && !ev.session_id.is_empty()
        && ((agent == Agent::Claude && kind == EventKind::SessionEnd)
            || (agent == Agent::Codex
                && kind == EventKind::Stop
                && !matches!(state.row.outcome.as_str(), "deny" | "would_deny" | "error")))
    {
        if let Err(error) = crate::memory::schedule_digest(
            store,
            &loaded.project_root,
            &home,
            &ev.session_id,
            ev.transcript_path.as_deref(),
            state.row.ts,
            agent == Agent::Codex,
        ) {
            warn_advisory(&home, "memory digest scheduling failed", &error);
        }
    }
    Ok(output)
}

/// What to tell a user whose state database a hook could not open or write. A database from a
/// newer Yoke is not a disk problem, and the way out is different: use that Yoke or remove
/// the hooks.
pub(crate) fn newer_state_hint(detail: &str) -> Option<&'static str> {
    detail.contains("is newer than supported").then_some(
        "this yoke is older than the state database; run the yoke that wrote it, or remove the hooks with `yoke uninstall --agent claude,codex` in your own terminal (docs/upgrade-notes.md)",
    )
}

/// The next step for a state problem, chosen by what the error says. Every hook that cannot read
/// its policy denies, so the message has to say how to get unblocked, not only that it failed.
pub(crate) fn state_hint(detail: &str) -> &'static str {
    if let Some(hint) = newer_state_hint(detail) {
        return hint;
    }
    let has = |needles: &[&str]| needles.iter().any(|needle| detail.contains(needle));
    if has(&["HOME is not set", "HOME must be an absolute path"]) {
        "set HOME to your account's absolute home directory in the environment the agent starts with"
    } else if has(&["is not a regular file"]) {
        "~/.yoke/state.db is not a regular file: in your own terminal move it aside (do not delete anything you need), then restart the agent and run `yoke trust` again"
    } else if has(&["not a database", "disk image is malformed"]) {
        "~/.yoke/state.db is damaged: in your own terminal move it and its -wal and -shm files aside (do not delete them), restart the agent, then run `yoke trust` again to approve your policy (docs/troubleshooting.md)"
    } else if has(&["trusted snapshot hash mismatch"]) {
        "the trusted record named above is damaged: review that file and run `yoke trust` (`yoke trust --global` for the global file) in your own terminal to replace it"
    } else if has(&["database is locked", "database is busy"]) {
        "another process holds ~/.yoke/state.db locked: find it with `lsof ~/.yoke/state.db`, or wait for it to finish and retry"
    } else if has(&["disk is full", "No space left"]) {
        "free disk space in your own terminal"
    } else if has(&[
        "Permission denied",
        "Read-only file system",
        "I/O error",
        "unable to open database",
        "database or disk is full",
    ]) {
        "check free disk space and filesystem permissions in your own terminal"
    } else {
        "run `yoke config` in your own terminal and resolve the policy diagnostic above before retrying"
    }
}

fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes)
}

fn stop_hook_active(raw: &Value) -> bool {
    raw.get("stop_hook_active").and_then(Value::as_bool) == Some(true)
}

/// A failure before the payload was read still needs to know whether this Stop is a repeat.
fn learn_stop_flag(state: &mut HookState) {
    if state.input_read {
        return;
    }
    if let Ok(bytes) = read_input(&mut state.input_read) {
        if let Ok(raw) = serde_json::from_slice::<Value>(strip_bom(&bytes)) {
            STOP_ACTIVE.store(stop_hook_active(&raw), Ordering::SeqCst);
        }
    }
}

/// What the record of the last known mode says, when a failure left the mode unknown because the
/// state could not be read. A failure after the first policy load is not a state problem.
fn recall_mode(state: &HookState, agent: Agent) -> Option<mode_cache::Recall> {
    if state.mode.is_some() || state.policy_loaded {
        return None;
    }
    let home = state.home.as_ref()?;
    let cwd = std::env::current_dir().ok()?;
    let key = mode_cache::project_key(&start_directory(agent, &cwd))?;
    Some(mode_cache::recall(home, &key))
}

fn ago(seconds: i64) -> String {
    match seconds.max(0) {
        s if s < 3600 => format!("{} min", s / 60),
        s if s < 2 * 86_400 => format!("{} h", s / 3600),
        s => format!("{} days", s / 86_400),
    }
}

fn fail(state: &mut HookState, agent: Agent, kind: Option<EventKind>, detail: &str) -> i32 {
    let recalled = recall_mode(state, agent);
    // The last known mode is observe: that promised to block nothing, so nothing is blocked.
    let observed_before = match recalled {
        Some(mode_cache::Recall::Observe(stamp)) => Some(stamp),
        _ => None,
    };
    if observed_before.is_some() {
        state.mode = Some(Mode::Observe);
        state.row.mode = "observe".into();
    }
    let record_note = match &recalled {
        Some(mode_cache::Recall::Enforce) => {
            "the last known mode for this project is enforce; ".to_owned()
        }
        Some(mode_cache::Recall::Unusable(why)) => {
            format!("the record of the last known mode cannot be used ({why}); ")
        }
        _ => String::new(),
    };
    let unavailable = if state.mode.is_none() {
        format!(
            "trusted policy unavailable; unread gates cannot be bypassed automatically; {record_note}{}; ",
            state_hint(detail)
        )
    } else {
        String::new()
    };
    let reason = if let Some(stamp) = observed_before {
        redact(&format!(
            "yoke error: {detail}; Yoke cannot read its state, so this call was allowed on the last known mode for this project, observe (recorded {} ago), which blocks nothing; {}",
            ago(now() - stamp),
            state_hint(detail)
        ))
    } else if state.mode == Some(Mode::Observe) {
        redact(&format!(
            "yoke error: {detail}; observe mode, so nothing was blocked"
        ))
    } else {
        redact(&format!("yoke error: {detail}; {unavailable}for an explicit bypass, a human runs yoke pause in their own terminal and follows what it prints; nothing set inside a blocked tool command can bypass that tool's pre-hook"))
    };
    say!("{reason}");
    state.row.outcome = "error".into();
    state.row.reason = Some(reason.clone());
    if state.mode.is_none() {
        state.row.mode = "enforce".into();
        state.row.reason = Some(format!(
            "{reason}; mode unavailable, conservatively enforcing"
        ));
    }
    // A failed stdout write happens after persistence. Keep one decision and
    // report the delivery error through the redacted fallback instead.
    let persisted = if state.persisted {
        None
    } else {
        state
            .store
            .as_ref()
            .map(|store| store.record_decision(&state.row, state.cap))
    };
    if observed_before.is_some() || !matches!(persisted, Some(Ok(()))) {
        let suffix = match persisted {
            Some(Err(error)) => format!("; decision persistence failed: {error:#}"),
            _ => String::new(),
        };
        if let Some(home) = &state.home {
            if let Err(error) = append_error(home, &format!("{reason}{suffix}")) {
                say!(
                    "yoke: error.log fallback failed: {}",
                    redact(&error.to_string())
                );
            }
        } else {
            say!("yoke: error.log unavailable because home directory could not be resolved");
        }
    }
    // An unavailable store cannot establish observe mode. Do not silently bypass enforcement.
    if state.mode != Some(Mode::Observe) {
        if kind == Some(EventKind::Stop) {
            learn_stop_flag(state);
            if once_per_stop(EventKind::Stop, Some(reason.clone())).is_none() {
                return 0;
            }
        }
        if let Some(output) = kind.and_then(|kind| wire::deny(agent, kind, &reason)) {
            if writeln!(std::io::stdout().lock(), "{output}").is_err() {
                return 2;
            }
        } else if kind.is_none() {
            return 2;
        }
    }
    0
}

fn warn_advisory(home: &Path, operation: &str, error: &anyhow::Error) {
    warn_logged(
        home,
        &format!(
            "{operation}: {error:#}; trusted gate decision preserved; {}",
            state_hint(&format!("{error:#}"))
        ),
    );
}

fn warn_usage_ingestion(home: &Path, error: &anyhow::Error) {
    warn_logged(
        home,
        &format!(
            "usage ingestion failed: {error:#}; trusted gate decision preserved; usage for this session may be incomplete"
        ),
    );
}

fn warn_logged(home: &Path, message: &str) {
    let reason = redact(message);
    say!("yoke: {reason}");
    if let Err(error) = append_error(home, &reason) {
        say!(
            "yoke: error.log fallback failed: {}",
            redact(&error.to_string())
        );
    }
}

/// The only writer of error.log entries: every line starts with a Unix timestamp.
pub(crate) fn append_error(home: &Path, reason: &str) -> Result<()> {
    yoke_core::private::create_dir_all(&home.join(".yoke"))?;
    let path = home.join(".yoke/error.log");
    if std::fs::metadata(&path).is_ok_and(|metadata| metadata.len() > ERROR_LOG_CAP) {
        // One older generation is kept; a failure that repeats on every tool call stays bounded.
        let _ = std::fs::rename(&path, home.join(".yoke/error.log.1"));
    }
    let mut file = yoke_core::private::open_append(&path)?;
    let stamp = now();
    let entry: String = redact(reason)
        .lines()
        .map(|line| format!("{stamp} {line}\n"))
        .collect();
    file.write_all(entry.as_bytes())?;
    Ok(())
}

/// Print a diagnostic, or, in a background child whose stderr is error.log, append it in
/// error.log's timestamped format; stderr stays the fallback if that append fails.
pub(crate) fn diagnostic(message: &str) {
    if std::env::var_os(BACKGROUND).is_some()
        && commands::home().is_ok_and(|home| append_error(&home, message).is_ok())
    {
        return;
    }
    say!("{}", redact(message));
}

fn schedule_update(store: &Store, home: &Path, timestamp: i64, enabled: bool) -> Result<()> {
    if !enabled || !store.claim_interval(UPDATE_ATTEMPT, timestamp, UPDATE_INTERVAL)? {
        return Ok(());
    }
    update_command(home)?
        .spawn()
        .context("start bounded daily update check")?;
    Ok(())
}

/// Refresh the same fallback record hooks use immediately after a human approves policy.
pub(crate) fn refresh_mode_record(store: &Store, home: &Path, cwd: &Path) -> Result<()> {
    let loaded = config::load_trusted(store, cwd, home)?;
    if let Some(key) = mode_cache::project_key(&loaded.project_root) {
        mode_cache::remember(home, &key, loaded.config.mode, now());
    }
    Ok(())
}

fn update_command(home: &Path) -> Result<Command> {
    let errors = yoke_core::private::open_append(&home.join(".yoke/error.log"))?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["update", "--check"])
        .current_dir(home)
        .env("HOME", home)
        .env("YOKE_OFF", "1")
        .env(BACKGROUND, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(errors);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_mode_refresh_updates_the_fallback_before_another_hook() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let project = temp.path().join("project");
        std::fs::create_dir_all(home.join(".yoke")).unwrap();
        std::fs::create_dir(&project).unwrap();
        let store = Store::open(":memory:").unwrap();
        let key = mode_cache::project_key(&project).unwrap();
        mode_cache::remember(&home, &key, Mode::Observe, 1);
        std::fs::write(project.join("yoke.toml"), "mode='enforce'").unwrap();
        trust::prepare_project(&store, &project)
            .unwrap()
            .commit(&store)
            .unwrap();
        assert_eq!(
            mode_cache::recall(&home, &key),
            mode_cache::Recall::Observe(1)
        );
        refresh_mode_record(&store, &home, &project).unwrap();
        assert_eq!(mode_cache::recall(&home, &key), mode_cache::Recall::Enforce);
    }

    #[test]
    fn disabled_update_check_creates_no_job_or_interval_and_child_uses_chosen_home() {
        let home = tempfile::tempdir().unwrap();
        let store = Store::open(":memory:").unwrap();
        schedule_update(&store, home.path(), 123, false).unwrap();
        assert_eq!(store.get_kv(UPDATE_ATTEMPT).unwrap(), None);
        assert!(!home.path().join(".yoke").exists());
        std::fs::create_dir(home.path().join(".yoke")).unwrap();
        let command = update_command(home.path()).unwrap();
        assert_eq!(command.get_current_dir(), Some(home.path()));
        assert!(command
            .get_envs()
            .any(|(key, value)| key == "HOME" && value == Some(home.path().as_os_str())));
    }

    #[test]
    fn policy_errors_do_not_suggest_storage_repairs() {
        assert!(!state_hint("project path must be UTF-8").contains("disk"));
        assert!(!state_hint("unknown field in trusted config").contains("permissions"));
        assert!(state_hint("Permission denied (os error 13)").contains("permissions"));
        assert!(state_hint("database is locked").contains("lsof"));
    }

    #[test]
    fn deadline_fits_the_gates_own_timeouts_up_to_the_agent_ceiling() {
        let seconds = |kind, gates| deadline(kind, gates, None).as_secs();
        assert_eq!(seconds(EventKind::Stop, 0), 120);
        // A 300 s test-suite gate is never cut while it is within its own timeout.
        assert_eq!(seconds(EventKind::Stop, 300), 310);
        assert_eq!(seconds(EventKind::PreToolUse, 10_000), 540);
        assert_eq!(seconds(EventKind::UserPromptSubmit, 20), 25);
        assert_eq!(seconds(EventKind::UserPromptSubmit, 60), 25);
        assert_eq!(
            deadline(EventKind::Stop, 300, Some(500)),
            Duration::from_millis(500)
        );
        assert_eq!(
            deadline(EventKind::Stop, 300, Some(1_000_000)).as_secs(),
            310,
            "the override can only shorten"
        );
    }

    #[test]
    fn a_panic_becomes_an_error_instead_of_ending_the_hook_without_an_answer() {
        let outcome: Result<()> = contain(|| panic!("boom {}", 7));
        let message = format!("{:#}", outcome.unwrap_err());
        assert!(
            message.contains("internal error") && message.contains("boom 7"),
            "{message}"
        );
        assert_eq!(contain(|| Ok(3)).unwrap(), 3);
    }

    #[test]
    fn error_log_moves_aside_once_it_passes_its_cap() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join(".yoke");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("error.log"),
            vec![b'x'; ERROR_LOG_CAP as usize + 1],
        )
        .unwrap();
        append_error(home.path(), "fresh failure").unwrap();
        let older = std::fs::metadata(dir.join("error.log.1")).unwrap();
        assert_eq!(older.len(), ERROR_LOG_CAP + 1);
        let log = std::fs::read_to_string(dir.join("error.log")).unwrap();
        assert_eq!(log.lines().count(), 1);
        assert!(log.ends_with(" fresh failure\n"), "{log}");
    }
}
