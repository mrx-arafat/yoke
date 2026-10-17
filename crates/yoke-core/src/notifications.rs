//! Actionable notifications with human acknowledgement and searchable recovery help.
//!
//! Rows are derived on read by `sync` from decisions, plans, check runs and trust snapshots,
//! so hooks never pay for them. Repeats share one row per dedupe key; conditions that stop
//! holding are resolved, and unread ones are acknowledged by `yoke` on the human's behalf.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::config::LoadedConfig;
use crate::redact::redact;
use crate::store::Store;

const DECISION_CURSOR: &str = "notifications.decision_id";
const CHECK_CURSOR: &str = "notifications.check_run_id";
const DETAIL_CAP: usize = 600;
const EVENT_KINDS: &str = "'hook_error','observe_would_deny','check_failed'";
/// Row ids per write transaction, so a hook never waits long for the write lock.
const BATCH_IDS: i64 = 20_000;
/// The window of one hook, which has no time budget: 2,000 ids cost a few milliseconds, while a
/// 20,000-id window made the session-start hook that took it wait over 100 ms.
const HOOK_BATCH_IDS: i64 = 2_000;
/// Work per call; a large backlog after an upgrade drains over several calls.
const BUDGET: Duration = Duration::from_millis(250);
/// Windows per table per call, so a call is bounded even on a machine fast enough to beat the
/// budget; each window is a separate transaction that other writers can get between.
const MAX_WINDOWS: usize = 25;
/// Pause before taking the write lock for a window. SQLite's default busy handler
/// (`sqlite3_busy_timeout`, used by hooks via `Store::open`) sleeps at most 100 ms between
/// retries, so a waiting hook retries, and wins the free lock, during any longer pause instead
/// of losing every race to back-to-back windows.
const PAUSE: Duration = Duration::from_millis(110);

#[derive(Clone, Debug, Serialize)]
pub struct Notification {
    pub id: i64,
    pub kind: String,
    /// Empty for the global configuration.
    pub project: String,
    /// Plan slug, configuration file, or hook event the notification is about.
    pub subject: String,
    pub plan_id: Option<i64>,
    pub title: String,
    pub detail: String,
    pub count: i64,
    pub first_seen: i64,
    pub last_seen: i64,
    pub state: String,
    pub acknowledged_at: Option<i64>,
    /// Who acknowledged it: `human`, or `yoke` when the condition resolved while unread.
    pub actor: Option<String>,
    pub resolved_at: Option<i64>,
    pub help: &'static str,
}

/// Recovery guidance shared by the CLI, the console and `docs/troubleshooting.md`.
#[derive(Debug, Serialize)]
pub struct HelpTopic {
    pub id: &'static str,
    pub title: &'static str,
    pub summary: &'static str,
    pub steps: &'static [&'static str],
    /// Exact commands; placeholders such as `<slug>` are written in angle brackets.
    pub commands: &'static [&'static str],
    /// Full URL of the section of `docs/troubleshooting.md` with the same guidance; an installed
    /// machine has no `docs/` directory, so a repository path would lead nowhere.
    pub doc: &'static str,
}

macro_rules! troubleshooting {
    ($anchor:literal) => {
        concat!(
            "https://github.com/mrx-arafat/yoke/blob/main/docs/troubleshooting.md",
            $anchor
        )
    };
}

pub const HELP: &[HelpTopic] = &[
    HelpTopic {
        id: "observe-mode",
        title: "Enforcement is inactive (observe mode)",
        summary: "Observe mode is the default. Yoke still runs gates and records would_deny decisions, but it denies, rewrites and blocks nothing, does not block Stop, and does not inject plan or memory context.",
        steps: &[
            "Review what would have been denied.",
            "Set top-level mode = \"enforce\" in yoke.toml, or in ~/.yoke/config.toml for every project.",
            "Trust the changed file from your own terminal, then confirm that yoke config shows mode enforce with its source file.",
        ],
        commands: &[
            "yoke log --outcome would_deny --limit 20",
            "yoke trust",
            "yoke trust --global",
            "yoke config",
        ],
        doc: troubleshooting!("#failures-are-logged-but-actions-continue"),
    },
    HelpTopic {
        id: "stale-trust",
        title: "Configuration changed after it was trusted",
        summary: "Yoke keeps enforcing the exact bytes a human trusted. An edited, deleted or unreadable file on disk has no effect until new bytes are trusted.",
        steps: &[
            "Compare trusted and disk content on the console Trust & config page, or look for stale = true in yoke config.",
            "If the edit is intended, trust it from your own terminal and type trust to confirm; use --global for ~/.yoke/config.toml.",
            "If it is not intended, restore the file. A deleted file cannot be trusted; the trusted snapshot keeps applying meanwhile.",
        ],
        commands: &["yoke config", "yoke trust", "yoke trust --global"],
        doc: troubleshooting!("#configuration-changed-after-it-was-trusted"),
    },
    HelpTopic {
        id: "untrusted-config",
        title: "Configuration is not trusted",
        summary: "A yoke.toml, ~/.yoke/config.toml or .yoke/knowledge.md without a trusted snapshot contributes no settings, gates or knowledge. yoke init creates a starter file but does not trust it.",
        steps: &[
            "Review the file, then trust it from your own terminal; use --global for ~/.yoke/config.toml.",
            "Project knowledge in .yoke/knowledge.md is trusted together with the project yoke.toml, which must exist.",
            "Agents cannot trust configuration: trust needs an interactive terminal outside an agent tool process.",
        ],
        commands: &["yoke trust", "yoke trust --global", "yoke config"],
        doc: troubleshooting!("#configuration-is-not-trusted"),
    },
    HelpTopic {
        id: "plan-recovery",
        title: "A plan is waiting, stalled or stuck checking",
        summary: "A plan waits after the agent runs yoke plan ask, stalls after stop.max_blocks Stop attempts without progress (default 5), and stays checking if its check process was killed. Only a human can continue it.",
        steps: &[
            "Inspect the plan, answer the question or fix the cause, then resume it from your own terminal.",
            "The console plan page also offers Resume work for waiting and stalled plans.",
            "Resuming a checking plan records the interrupted check as invalid evidence, and is refused while a live check process still owns the plan.",
            "To stop instead, cancel it with a reason; to continue in a new session, reassign it.",
        ],
        commands: &[
            "yoke plan status <slug>",
            "yoke plan resume <slug>",
            "yoke plan cancel <slug> --reason <why>",
            "yoke plan reassign <slug> --session <session-id>",
        ],
        doc: troubleshooting!("#a-plan-cannot-complete"),
    },
    HelpTopic {
        id: "check-failed",
        title: "A plan check failed or was interrupted",
        summary: "Approved checks and required review run on the actual tree. A failed, timed out, errored, voided or interrupted result is stored as evidence, and the plan cannot complete.",
        steps: &[
            "Read the stored output in the plan status or on the console plan page.",
            "Fix the cause and rerun the checks; changes to the tree during a check void its evidence, so rerun on a stable tree.",
            "For review failures, confirm the reviewer CLI (claude or codex) is installed and authenticated.",
            "An interrupted check can leave the plan checking; a human then resumes it.",
        ],
        commands: &[
            "yoke plan status <slug>",
            "yoke plan check <slug> --agent <claude|codex>",
            "yoke plan resume <slug>",
        ],
        doc: troubleshooting!("#a-plan-cannot-complete"),
    },
    HelpTopic {
        id: "hooks-disabled",
        title: "Hooks are disabled or missing",
        summary: "Yoke protects nothing when its hooks do not run. Claude Code skips them when the highest-precedence settings file that sets disableAllHooks sets it to true (project .claude/settings.local.json, then project .claude/settings.json, then settings.json in CLAUDE_CONFIG_DIR or ~/.claude), or when managed settings set disableAllHooks or allowManagedHooksOnly. Codex skips them when [features] hooks or codex_hooks is false in config.toml in CODEX_HOME or ~/.codex.",
        steps: &[
            "Check which agents have Yoke hooks installed and what stops them.",
            "Remove disableAllHooks from the Claude settings file that sets it, or set it to false there.",
            "allowManagedHooksOnly has effect only in managed settings (managed-settings.json or managed-settings.d in /Library/Application Support/ClaudeCode on macOS, /etc/claude-code elsewhere), which an administrator controls. Policy from MDM or the claude.ai console is not visible to Yoke.",
            "Remove hooks = false or codex_hooks = false from [features] in the Codex config.toml.",
            "Preview, then reinstall missing hooks; limit it with --agent claude or --agent codex.",
            "In Codex, trust new or changed hooks with /hooks.",
        ],
        commands: &["yoke health", "yoke install --dry-run", "yoke install"],
        doc: troubleshooting!("#hooks-are-disabled-or-missing"),
    },
    HelpTopic {
        id: "hook-error",
        title: "A hook error blocked work",
        summary: "When a Yoke hook fails in enforce mode, or when the mode cannot be read, Yoke fails closed and denies blocking events instead of allowing them. The redacted reason is stored as an error decision, or appended to ~/.yoke/error.log when the database cannot be written.",
        steps: &[
            "Read the recent errors.",
            "Check free disk space and permissions for ~/.yoke. Never delete state.db or its WAL files.",
            "Fix the cause, then retry the action.",
            "For an explicit temporary bypass, a human runs yoke pause in their own terminal. It prints a private token to start the agent with, for example YOKE_OFF=<token> claude, and yoke pause --for 30m skips every hook for a limited time. Nothing set inside a blocked tool command can bypass that hook.",
        ],
        commands: &["yoke log --outcome error --limit 10", "yoke health"],
        doc: troubleshooting!("#a-hook-error-blocked-work"),
    },
];

/// Topics whose text or commands contain every whitespace-separated term, ignoring case.
pub fn search_help(query: &str) -> Vec<&'static HelpTopic> {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    HELP.iter()
        .filter(|topic| {
            let text = format!(
                "{} {} {} {} {}",
                topic.id,
                topic.title,
                topic.summary,
                topic.steps.join(" "),
                topic.commands.join(" ")
            )
            .to_lowercase();
            terms.iter().all(|term| text.contains(term.as_str()))
        })
        .collect()
}

pub fn help_for(kind: &str) -> &'static HelpTopic {
    let id = match kind {
        "hook_error" => "hook-error",
        "observe_would_deny" => "observe-mode",
        "plan_waiting" | "plan_stalled" => "plan-recovery",
        "check_failed" => "check-failed",
        "config_stale" => "stale-trust",
        _ => "untrusted-config",
    };
    HELP.iter()
        .find(|topic| topic.id == id)
        .expect("every kind maps to a topic")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Exactly these notifications, typically the ones a human just saw.
    Ids(Vec<i64>),
    /// Every unread notification in scope.
    All,
}

/// Materialize new notifications for all projects, plus configuration conditions for the
/// loaded project and the global config. Writes nothing when nothing changed.
/// History is read in short transactions within a time budget; returns true when older
/// decisions or check runs are still waiting for a later call.
pub fn sync(store: &Store, loaded: &LoadedConfig, now: i64) -> Result<bool> {
    sync_bounded(store, loaded, now, BATCH_IDS, BUDGET, PAUSE)
}

/// `sync` for a hook: one window per table and no pause, so the hook stays in milliseconds and
/// never sleeps. Older history drains over later hooks and `yoke notifications` runs.
pub fn sync_for_hook(store: &Store, loaded: &LoadedConfig, now: i64) -> Result<bool> {
    sync_bounded(
        store,
        loaded,
        now,
        HOOK_BATCH_IDS,
        Duration::ZERO,
        Duration::ZERO,
    )
}

fn sync_bounded(
    store: &Store,
    loaded: &LoadedConfig,
    now: i64,
    batch: i64,
    budget: Duration,
    pause: Duration,
) -> Result<bool> {
    let start = Instant::now();
    let pace = Pace { budget, pause };
    let mut pending = drain(
        store,
        DECISION_CURSOR,
        "decisions",
        batch,
        start,
        pace,
        decisions,
    )?;
    pending |= drain(
        store,
        CHECK_CURSOR,
        "check_runs",
        batch,
        start,
        pace,
        |tx, from, to| checks(tx, from, to, now),
    )?;
    store.transaction(|tx| {
        plans(tx, now)?;
        configs(tx, loaded, now)?;
        let cutoff = now.saturating_sub(i64::from(loaded.config.log.retention_days) * 86_400);
        let project = scope(&loaded.project_root)?;
        tx.execute(
            &format!("DELETE FROM notifications WHERE project IN (?1,?2,'') AND ((kind IN ({EVENT_KINDS}) AND last_seen < ?3) OR resolved_at < ?3)"),
            params![project.identity, project.legacy, cutoff],
        )?;
        Ok(())
    })?;
    Ok(pending)
}

/// How a call spaces its windows: stop after `budget`, and wait `pause` before each one.
#[derive(Clone, Copy)]
struct Pace {
    budget: Duration,
    pause: Duration,
}

/// Apply `batch` ids at a time, one transaction each after the pace's pause, until caught up,
/// over budget, or at `MAX_WINDOWS`.
/// At least one batch runs per call, so every call makes progress.
fn drain(
    store: &Store,
    cursor: &str,
    table: &str,
    batch: i64,
    start: Instant,
    pace: Pace,
    mut apply: impl FnMut(&Transaction<'_>, i64, i64) -> Result<()>,
) -> Result<bool> {
    for _ in 0..MAX_WINDOWS {
        // Peek without the write lock so an idle call never pauses.
        let (from, max) = window(store.connection(), cursor, table)?;
        if from == max {
            return Ok(false);
        }
        std::thread::sleep(pace.pause);
        let more = store.transaction(|tx| {
            let (from, max) = window(tx, cursor, table)?;
            if from == max {
                return Ok(false);
            }
            let to = max.min(from.saturating_add(batch));
            apply(tx, from, to)?;
            advance(tx, cursor, from, to)?;
            Ok(to < max)
        })?;
        if !more {
            return Ok(false);
        }
        if start.elapsed() >= pace.budget {
            return Ok(true);
        }
    }
    Ok(true)
}

/// Unread first, then newest; scoped to the project and the global configuration.
pub fn list(
    store: &Store,
    project: &Path,
    unread_only: bool,
    limit: usize,
) -> Result<Vec<Notification>> {
    let project = scope(project)?;
    let mut statement = store.connection().prepare(
        "SELECT id,kind,project,subject,plan_id,title,detail,count,first_seen,last_seen,state,acknowledged_at,actor,resolved_at
         FROM notifications WHERE project IN (?1,?2,'') AND (?3=0 OR state='unread')
         ORDER BY state='unread' DESC,last_seen DESC,id DESC LIMIT ?4",
    )?;
    let rows = statement.query_map(
        params![
            project.identity,
            project.legacy,
            unread_only,
            limit.min(1000) as i64
        ],
        |row| {
            let kind: String = row.get(1)?;
            Ok(Notification {
                id: row.get(0)?,
                help: help_for(&kind).id,
                kind,
                project: if row.get::<_, String>(2)?.is_empty() {
                    String::new()
                } else {
                    project.display.clone()
                },
                subject: row.get(3)?,
                plan_id: row.get(4)?,
                title: row.get(5)?,
                detail: row.get(6)?,
                count: row.get(7)?,
                first_seen: row.get(8)?,
                last_seen: row.get(9)?,
                state: row.get(10)?,
                acknowledged_at: row.get(11)?,
                actor: row.get(12)?,
                resolved_at: row.get(13)?,
            })
        },
    )?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn unread_count(store: &Store, project: &Path) -> Result<i64> {
    let project = scope(project)?;
    Ok(store.connection().query_row(
        "SELECT count(*) FROM notifications WHERE project IN (?1,?2,'') AND state='unread'",
        params![project.identity, project.legacy],
        |row| row.get(0),
    )?)
}

/// Unread count for a status line, which knows a session or a project path but has no trusted
/// configuration to resolve a root from: the global ones plus the session's recorded project, or
/// the given path when the session is unknown. Reads only.
pub fn unread_for_status(
    store: &Store,
    session: Option<&str>,
    project: Option<&str>,
) -> Result<i64> {
    let recorded: Option<String> = match session {
        Some(id) => store
            .connection()
            .query_row(
                "SELECT project FROM sessions WHERE id=?1",
                [redact(id)],
                |row| row.get(0),
            )
            .optional()?,
        None => None,
    };
    match recorded.as_deref().or(project) {
        Some(project) => unread_count(store, Path::new(project)),
        None => Ok(store.connection().query_row(
            "SELECT count(*) FROM notifications WHERE project='' AND state='unread'",
            [],
            |row| row.get(0),
        )?),
    }
}

/// Human acknowledgement; returns how many unread notifications changed. Every listed id
/// must exist in scope, or nothing changes.
pub fn acknowledge(store: &Store, project: &Path, target: &Target, now: i64) -> Result<usize> {
    let project = scope(project)?;
    store.transaction(|tx| match target {
        Target::Ids(ids) => {
            ensure!(
                !ids.is_empty() && ids.len() <= 1000,
                "acknowledge between 1 and 1000 notifications"
            );
            let mut changed = 0;
            for id in ids {
                let exists = tx
                    .query_row(
                        "SELECT 1 FROM notifications WHERE project IN (?1,?2,'') AND id=?3",
                        params![project.identity, project.legacy, id],
                        |_| Ok(()),
                    )
                    .optional()?;
                if exists.is_none() {
                    bail!("notification {id} does not exist in this project");
                }
                changed += tx.execute(
                    "UPDATE notifications SET state='acknowledged',acknowledged_at=?2,actor='human'
                     WHERE id=?1 AND state='unread'",
                    params![id, now],
                )?;
            }
            Ok(changed)
        }
        Target::All => Ok(tx.execute(
            "UPDATE notifications SET state='acknowledged',acknowledged_at=?3,actor='human'
             WHERE project IN (?1,?2,'') AND state='unread'",
            params![project.identity, project.legacy, now],
        )?),
    })
}

struct ProjectScope {
    identity: String,
    legacy: Option<String>,
    display: String,
}

fn scope(project: &Path) -> Result<ProjectScope> {
    ensure!(project.is_absolute(), "project path must be absolute");
    let raw = project.to_str().context("project path must be UTF-8")?;
    let display = redact(raw);
    Ok(ProjectScope {
        identity: project_identity(raw),
        // A redacted legacy path cannot establish which original project owned it.
        legacy: (display == raw && !raw.contains("[REDACTED]")).then(|| raw.to_owned()),
        display,
    })
}

fn project_identity(project: &str) -> String {
    if project.is_empty() {
        String::new()
    } else {
        format!("sha256:{:x}", Sha256::digest(project.as_bytes()))
    }
}

/// How a repeated dedupe key changes an existing row.
#[derive(Clone, Copy)]
enum Repeat {
    /// A new failure needs attention again, even after acknowledgement or resolution.
    Reopen,
    /// A summary only grows; acknowledging it once is enough.
    Accumulate,
    /// A condition reopens only after it had been resolved.
    Condition,
}

struct Occurrence<'a> {
    key: String,
    kind: &'a str,
    project: &'a str,
    subject: &'a str,
    plan_id: Option<i64>,
    title: String,
    detail: String,
    count: i64,
    first_seen: i64,
    last_seen: i64,
}

fn record(tx: &Transaction<'_>, row: &Occurrence<'_>, repeat: Repeat) -> Result<()> {
    let project = project_identity(row.project);
    // These keys identify an exact source (raw config path, plan id, or unambiguous decision
    // path). Upgrade its legacy scope without changing acknowledgement or occurrence counts.
    tx.execute(
        "UPDATE notifications SET project=?1 WHERE key=?2 AND project<>?1",
        params![project, row.key],
    )?;
    let reopen = "state='unread',acknowledged_at=NULL,actor=NULL,resolved_at=NULL";
    let update = match repeat {
        Repeat::Reopen => format!("count=count+excluded.count,first_seen=min(first_seen,excluded.first_seen),last_seen=max(last_seen,excluded.last_seen),title=excluded.title,detail=excluded.detail,{reopen}"),
        Repeat::Accumulate => "count=count+excluded.count,last_seen=max(last_seen,excluded.last_seen),title=excluded.title,detail=excluded.detail".into(),
        Repeat::Condition => format!("count=count+1,last_seen=excluded.last_seen,title=excluded.title,detail=excluded.detail,{reopen} WHERE notifications.resolved_at IS NOT NULL"),
    };
    tx.execute(
        &format!("INSERT INTO notifications(key,kind,project,subject,plan_id,title,detail,count,first_seen,last_seen)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(key) DO UPDATE SET {update}"),
        params![
            row.key,
            row.kind,
            project,
            redact(row.subject),
            row.plan_id,
            cap(&row.title),
            cap(&row.detail),
            row.count,
            row.first_seen,
            row.last_seen
        ],
    )?;
    Ok(())
}

/// Mark a condition as no longer holding; an unread one is acknowledged by Yoke.
fn resolve(tx: &Transaction<'_>, id: i64, now: i64) -> Result<()> {
    tx.execute(
        "UPDATE notifications SET resolved_at=?2,state='acknowledged',acknowledged_at=coalesce(acknowledged_at,?2),actor=coalesce(actor,'yoke')
         WHERE id=?1 AND resolved_at IS NULL",
        params![id, now],
    )?;
    Ok(())
}

fn cap(text: &str) -> String {
    let text = redact(text);
    match text.char_indices().nth(DETAIL_CAP) {
        Some((at, _)) => format!("{}...", &text[..at]),
        None => text,
    }
}

fn digest(parts: &[&str]) -> String {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update(part.as_bytes());
        hash.update([0]);
    }
    format!("{:x}", hash.finalize())[..32].into()
}

/// Rows after the saved cursor up to the current maximum id. Pruning can let SQLite reuse
/// ids below the cursor; a maximum below the cursor means the table restarted.
fn window(tx: &Connection, key: &str, table: &str) -> Result<(i64, i64)> {
    let saved = tx
        .query_row("SELECT value FROM kv WHERE key=?1", [key], |row| {
            row.get::<_, String>(0)
        })
        .optional()?
        .map(|value| value.parse::<i64>())
        .transpose()
        .context("invalid notification cursor")?
        .unwrap_or(0);
    let max: i64 = tx.query_row(
        &format!("SELECT coalesce(max(id),0) FROM {table}"),
        [],
        |row| row.get(0),
    )?;
    Ok((if max < saved { 0 } else { saved }, max))
}

fn advance(tx: &Transaction<'_>, key: &str, from: i64, to: i64) -> Result<()> {
    if to != from {
        tx.execute(
            "INSERT INTO kv(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, to.to_string()],
        )?;
    }
    Ok(())
}

// `+outcome` keeps SQLite on the rowid range instead of scanning every row with that outcome.
const HOOK_ERRORS_SQL: &str = "SELECT project,event,mode,coalesce(reason,''),count(*),min(ts),max(ts) FROM decisions
             WHERE id>?1 AND id<=?2 AND +outcome='error' GROUP BY project,event,mode,coalesce(reason,'')";
// One pass over the id range; the newest row of each project joins back for the bare columns.
// A correlated min() per project would rescan the whole window once for every project.
const WOULD_DENY_SQL: &str =
    "SELECT g.project,g.n,d.id,d.ts,d.event,coalesce(d.tool,''),coalesce(d.reason,''),g.first
             FROM (SELECT project,count(*) AS n,max(id) AS last_id,min(ts) AS first FROM decisions
                   WHERE id>?1 AND id<=?2 AND +outcome='would_deny' GROUP BY project) g
             JOIN decisions d ON d.id=g.last_id";

fn decisions(tx: &Transaction<'_>, from: i64, to: i64) -> Result<()> {
    let errors = {
        let mut statement = tx.prepare(HOOK_ERRORS_SQL)?;
        let rows = statement.query_map([from, to], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (project, event, mode, reason, count, first, last) in errors {
        // The decisions store already redacted this identity; its owner cannot be recovered.
        if project.contains("[REDACTED]") {
            continue;
        }
        record(
            tx,
            &Occurrence {
                key: format!("hook-error:{}", digest(&[&project, &event, &mode, &reason])),
                kind: "hook_error",
                project: &project,
                subject: &event,
                plan_id: None,
                title: format!("Yoke hook failed on {event} in {mode} mode"),
                detail: reason,
                count,
                first_seen: first,
                last_seen: last,
            },
            Repeat::Reopen,
        )?;
    }
    let observed = {
        let mut statement = tx.prepare(WOULD_DENY_SQL)?;
        let rows = statement.query_map([from, to], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (project, count, last, event, tool, reason, first) in observed {
        if project.contains("[REDACTED]") {
            continue;
        }
        let action = if tool.is_empty() {
            event
        } else {
            format!("{event} {tool}")
        };
        record(
            tx,
            &Occurrence {
                key: format!("observe-would-deny:{}", digest(&[&project])),
                kind: "observe_would_deny",
                project: &project,
                subject: "would_deny",
                plan_id: None,
                title: "Enforcement is off: Yoke would have denied actions".into(),
                detail: format!("Observe mode recorded these decisions and blocked nothing. Latest: {action}: {reason}"),
                count,
                first_seen: first,
                last_seen: last,
            },
            Repeat::Accumulate,
        )?;
    }
    Ok(())
}

fn checks(tx: &Transaction<'_>, from: i64, to: i64, now: i64) -> Result<()> {
    let runs = {
        let mut statement = tx.prepare(
            "SELECT c.plan_id,c.ts,c.name,c.result,c.blocking,c.output_tail,p.slug,p.project
             FROM check_runs c JOIN plans p ON p.id=c.plan_id WHERE c.id>?1 AND c.id<=?2 ORDER BY c.id",
        )?;
        let rows = statement.query_map([from, to], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, bool>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (plan_id, ts, name, result, blocking, output, slug, project) in runs {
        let key = format!("check:{plan_id}:{}", digest(&[&name]));
        if result == "pass" {
            let open: Option<i64> = tx
                .query_row(
                    "SELECT id FROM notifications WHERE key=?1 AND resolved_at IS NULL",
                    [&key],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(id) = open {
                resolve(tx, id, now)?;
            }
            continue;
        }
        let outcome = match result.as_str() {
            "fail" => "failed",
            "timeout" => "timed out",
            "error" => "could not run",
            "interrupted" => "was interrupted",
            "void" => "was voided",
            other => other,
        };
        // Redact before slicing: a tail can otherwise lose the key that identifies a secret.
        let output = redact(&output);
        let mut start = output.len().saturating_sub(300);
        while !output.is_char_boundary(start) {
            start += 1;
        }
        record(
            tx,
            &Occurrence {
                key,
                kind: "check_failed",
                project: &project,
                subject: &slug,
                plan_id: Some(plan_id),
                title: format!("Check {name} {outcome} on plan {slug}"),
                detail: format!(
                    "{} check. Output tail: {}",
                    if blocking { "Blocking" } else { "Advisory" },
                    output[start..].trim()
                ),
                count: 1,
                first_seen: ts,
                last_seen: ts,
            },
            Repeat::Reopen,
        )?;
    }
    Ok(())
}

fn plans(tx: &Transaction<'_>, now: i64) -> Result<()> {
    let current = {
        let mut statement = tx.prepare(
            "SELECT p.id,p.slug,p.project,p.state,coalesce(t.id,0),coalesce(t.ts,p.updated_at),coalesce(t.note,'')
             FROM plans p LEFT JOIN transitions t ON t.id=(SELECT max(id) FROM transitions WHERE plan_id=p.id AND to_state=p.state)
             WHERE p.state IN ('waiting','stalled')",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut keys = BTreeSet::new();
    for (id, slug, project, state, transition, ts, note) in current {
        let key = format!("plan:{id}:{state}:{transition}");
        let note = if note.trim().is_empty() {
            String::new()
        } else {
            format!("Recorded note: {} ", note.trim())
        };
        let (kind, title, detail) = if state == "waiting" {
            (
                "plan_waiting",
                format!("Plan {slug} is waiting for you"),
                format!("{note}Answer the agent, then run yoke plan resume {slug} in your own terminal."),
            )
        } else {
            (
                "plan_stalled",
                format!("Plan {slug} stalled at the stop limit"),
                format!("{note}Find out why the agent made no progress, then run yoke plan resume {slug} in your own terminal."),
            )
        };
        record(
            tx,
            &Occurrence {
                key: key.clone(),
                kind,
                project: &project,
                subject: &slug,
                plan_id: Some(id),
                title,
                detail,
                count: 1,
                first_seen: ts,
                last_seen: ts,
            },
            Repeat::Condition,
        )?;
        keys.insert(key);
    }
    let open = {
        let mut statement = tx.prepare(
            "SELECT n.id,n.key,n.kind,coalesce(p.state,'') FROM notifications n LEFT JOIN plans p ON p.id=n.plan_id
             WHERE n.kind IN ('plan_waiting','plan_stalled','check_failed') AND n.resolved_at IS NULL",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (id, key, kind, plan_state) in open {
        let finished = matches!(plan_state.as_str(), "done" | "cancelled");
        if (kind == "check_failed" && finished) || (kind != "check_failed" && !keys.contains(&key))
        {
            resolve(tx, id, now)?;
        }
    }
    Ok(())
}

fn configs(tx: &Transaction<'_>, loaded: &LoadedConfig, now: i64) -> Result<()> {
    let project = scope(&loaded.project_root)?;
    let project_root = loaded
        .project_root
        .to_str()
        .context("project path must be UTF-8")?;
    let mut files = BTreeSet::new();
    let mut keys = BTreeSet::new();
    for status in &loaded.snapshots {
        let global = status.scope == "global";
        let file = status.file.to_str().context("config path must be UTF-8")?;
        let root = if global {
            status
                .file
                .parent()
                .and_then(Path::parent)
                .and_then(Path::to_str)
                .context("global config path has no home")?
        } else {
            project_root
        };
        let project = if global { "" } else { project_root };
        let trust = if global {
            "yoke trust --global"
        } else {
            "yoke trust"
        };
        files.insert(redact(file));
        let occurrence = if status.trusted && status.stale {
            let sha: String = tx.query_row(
                "SELECT sha256 FROM trust WHERE scope=?1 AND root=?2 AND file=?3",
                params![status.scope, root, file],
                |row| row.get(0),
            )?;
            Some((
                format!("config-stale:{}", digest(&[file, &sha])),
                "config_stale",
                match &status.disk_error {
                    Some(_) => format!("Trusted file is missing or unreadable: {file}"),
                    None => format!("Trusted file changed on disk: {file}"),
                },
                format!(
                    "Yoke keeps using the exact bytes you trusted; the file on disk has no effect until a human runs {trust}.{}",
                    status
                        .disk_error
                        .as_deref()
                        .map(|error| format!(" Disk: {error}"))
                        .unwrap_or_default()
                ),
            ))
        } else if !status.trusted && std::fs::symlink_metadata(&status.file).is_ok() {
            Some((
                format!("config-untrusted:{}", digest(&[file])),
                "config_untrusted",
                format!("File is not trusted: {file}"),
                format!("It contributes no settings, gates or knowledge until a human reviews it and runs {trust}."),
            ))
        } else {
            None
        };
        if let Some((key, kind, title, detail)) = occurrence {
            record(
                tx,
                &Occurrence {
                    key: key.clone(),
                    kind,
                    project,
                    subject: file,
                    plan_id: None,
                    title,
                    detail,
                    count: 1,
                    first_seen: now,
                    last_seen: now,
                },
                Repeat::Condition,
            )?;
            keys.insert(key);
        }
    }
    let open = {
        let mut statement = tx.prepare(
            "SELECT id,key,subject FROM notifications WHERE project IN (?1,?2,'') AND kind IN ('config_stale','config_untrusted') AND resolved_at IS NULL",
        )?;
        let rows = statement.query_map(params![project.identity, project.legacy], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (id, key, subject) in open {
        if files.contains(&subject) && !keys.contains(&key) {
            resolve(tx, id, now)?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "notifications/tests.rs"]
mod tests;
