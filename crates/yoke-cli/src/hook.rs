use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{ensure, Context, Result};
use serde_json::Value;
use yoke_core::config::{self, Mode};
use yoke_core::engine::{evaluate_with_context, EvaluationContext};
use yoke_core::event::Agent;
use yoke_core::redact::redact;
use yoke_core::store::{DecisionRecord, Store};
use yoke_core::{memory, plan, stop_guard, wire, AgentEvent, EventKind};

use crate::commands;

const INPUT_CAP: u64 = 1024 * 1024;
const UPDATE_INTERVAL: i64 = 86400;
pub(crate) const UPDATE_ATTEMPT: &str = "update.last_attempt";
// Retention runs at most hourly and briefly, so SessionStart never waits on a large backlog.
const RETENTION_ATTEMPT: &str = "retention.last_prune";
const RETENTION_INTERVAL: i64 = 3600;
const RETENTION_BUDGET: std::time::Duration = std::time::Duration::from_millis(250);
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
        let reason = format!("yoke error: hook evaluation exceeded {} ms; for an explicit bypass, restart your coding agent from your own terminal with YOKE_OFF=1 in its environment", limit.as_millis());
        eprintln!("{}", redact(&reason));
        if let Ok(home) = commands::home() {
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
        return denial;
    };
    let mut row = context.row;
    row.reason = Some(reason.into());
    row.payload = String::new();
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
        denial
    })
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
}

/// Hook errors never escape into ordinary CLI stdout.
pub fn run(event: &str, agent: Agent) -> i32 {
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
    };
    if let Some(kind) = kind.filter(|kind| kind.is_blocking()) {
        watchdog(agent, kind);
    }
    let result = evaluate(event, agent, &mut state);
    // Holding this until the process exits keeps a late watchdog from answering a second time.
    let mut answered = ANSWERED.lock().unwrap_or_else(PoisonError::into_inner);
    *answered = true;
    match result {
        Ok(output) => match output
            .map(|output| writeln!(std::io::stdout().lock(), "{output}"))
            .transpose()
        {
            Ok(_) => 0,
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

fn evaluate(event: &str, agent: Agent, state: &mut HookState) -> Result<Option<Value>> {
    let home = commands::home()?;
    state.home = Some(home.clone());
    state.store = Some(match commands::open_store(&home) {
        Ok(store) => store,
        Err(error) => {
            let store = Store::open_read_only(home.join(".yoke/state.db")).with_context(|| {
                format!("writable state unavailable ({error:#}); opening read-only state")
            })?;
            eprintln!(
                "yoke: {}; retrying trusted policy using read-only state; audit and memory writes may be unavailable; check free disk space and filesystem permissions in your own terminal",
                redact(&format!("writable state unavailable: {error:#}"))
            );
            store
        }
    });
    let store = state.store.as_ref().expect("opened above");
    let process_cwd = std::env::current_dir().context("resolve hook working directory")?;
    state.row.project = process_cwd.display().to_string();
    // Resolve mode before reading malformed input so enforce-mode errors still deny.
    let mut loaded = config::load_trusted(store, &process_cwd, &home)?;
    state.mode = Some(loaded.config.mode);
    publish(state.mode);
    state.row.mode = loaded.config.mode.as_str().into();
    state.cap = loaded.config.log.payload_cap;
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(INPUT_CAP + 1)
        .read_to_end(&mut bytes)
        .context("read hook payload")?;
    state.row.payload = String::from_utf8_lossy(&bytes).into_owned();
    ensure!(
        bytes.len() <= INPUT_CAP as usize,
        "hook payload exceeds 1 MiB"
    );
    let raw: Value = serde_json::from_slice(&bytes).context("parse hook JSON object")?;
    let ev: AgentEvent = serde_json::from_value(raw.clone()).context("parse hook event fields")?;
    state.row.session_id = ev.session_id.clone();
    state.row.tool = ev.tool_name.clone();
    if ev.cwd.is_some() {
        state.mode = None;
        publish(state.mode);
        state.row.mode = "unknown".into();
    }
    let cwd = match ev.cwd.as_deref() {
        Some(path) => {
            let path = PathBuf::from(path);
            ensure!(path.is_absolute(), "hook cwd must be absolute");
            path.canonicalize().context("resolve payload cwd")?
        }
        None => process_cwd.clone(),
    };
    // Agents send the directory the hook already runs in; its policy was loaded above.
    if ev.cwd.is_some() && cwd != process_cwd {
        loaded = config::load_trusted(store, &cwd, &home)?;
    }
    state.mode = Some(loaded.config.mode);
    publish(state.mode);
    state.row.mode = loaded.config.mode.as_str().into();
    state.row.project = loaded.project_root.display().to_string();
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
    if !ev.session_id.is_empty()
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
    let outcome = evaluate_with_context(
        &loaded.config,
        kind,
        &ev,
        &EvaluationContext {
            cwd: cwd.clone(),
            project: loaded.project_root.clone(),
            home: home.clone(),
        },
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
    let atomic_stop = kind == EventKind::Stop && !ev.session_id.is_empty();
    if !ev.session_id.is_empty() && !atomic_stop {
        if let Err(error) = memory::record_hook_facts(store, &state.row, &raw) {
            warn_advisory(&home, "memory facts were not saved", &error);
        }
    }
    if loaded.config.mode == Mode::Enforce
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
    if kind == EventKind::SessionStart {
        let (now, days) = (state.row.ts, loaded.config.log.retention_days);
        let pruned = store
            .claim_interval(RETENTION_ATTEMPT, now, RETENTION_INTERVAL)
            .and_then(|claimed| {
                claimed
                    .then(|| store.prune_history(now, days, RETENTION_BUDGET))
                    .transpose()
            });
        if let Err(error) = pruned {
            warn_advisory(&home, "retention cleanup failed", &error);
        }
        for status in loaded
            .snapshots
            .iter()
            .filter(|snapshot| snapshot.stale || !snapshot.trusted)
        {
            eprintln!(
                "yoke: config snapshot {} is {}; inspect yoke config",
                status.file.display(),
                if status.trusted {
                    "stale (using trusted bytes)"
                } else {
                    "untrusted (ignored)"
                }
            );
        }
        if let Err(error) = schedule_update(store, &home, state.row.ts) {
            warn_advisory(&home, "update check scheduling failed", &error);
        }
        if loaded.config.memory.digest && !ev.session_id.is_empty() {
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
    if state.persisted
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

fn fail(state: &mut HookState, agent: Agent, kind: Option<EventKind>, detail: &str) -> i32 {
    let unavailable = if state.mode.is_none() {
        "trusted policy unavailable; unread gates cannot be bypassed automatically; check free disk space and filesystem permissions in your own terminal; "
    } else {
        ""
    };
    let reason = redact(&format!("yoke error: {detail}; {unavailable}for an explicit bypass, restart your coding agent from your own terminal with YOKE_OFF=1 in its environment; setting it inside a blocked tool command cannot bypass that tool's pre-hook"));
    eprintln!("{reason}");
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
    if !matches!(persisted, Some(Ok(()))) {
        let suffix = match persisted {
            Some(Err(error)) => format!("; decision persistence failed: {error:#}"),
            _ => String::new(),
        };
        if let Some(home) = &state.home {
            if let Err(error) = append_error(home, &format!("{reason}{suffix}")) {
                eprintln!(
                    "yoke: error.log fallback failed: {}",
                    redact(&error.to_string())
                );
            }
        } else {
            eprintln!("yoke: error.log unavailable because home directory could not be resolved");
        }
    }
    // An unavailable store cannot establish observe mode. Do not silently bypass enforcement.
    if state.mode != Some(Mode::Observe) {
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
        "{operation}: {error:#}; trusted gate decision preserved; check free disk space and filesystem permissions in your own terminal"
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
    eprintln!("yoke: {reason}");
    if let Err(error) = append_error(home, &reason) {
        eprintln!(
            "yoke: error.log fallback failed: {}",
            redact(&error.to_string())
        );
    }
}

/// The only writer of error.log entries: every line starts with a Unix timestamp.
pub(crate) fn append_error(home: &Path, reason: &str) -> Result<()> {
    std::fs::create_dir_all(home.join(".yoke"))?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(home.join(".yoke/error.log"))?;
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
    eprintln!("{}", redact(message));
}

fn schedule_update(store: &Store, home: &Path, timestamp: i64) -> Result<()> {
    if !store.claim_interval(UPDATE_ATTEMPT, timestamp, UPDATE_INTERVAL)? {
        return Ok(());
    }
    let errors = OpenOptions::new()
        .create(true)
        .append(true)
        .open(home.join(".yoke/error.log"))?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["update", "--check"])
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
    command
        .spawn()
        .context("start bounded daily update check")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
