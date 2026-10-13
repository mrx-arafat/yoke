use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

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
    match evaluate(event, agent, &mut state) {
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
    state.store = Some(commands::open_store(&home)?);
    let store = state.store.as_ref().expect("opened above");
    let cwd = std::env::current_dir().context("resolve hook working directory")?;
    state.row.project = cwd.display().to_string();
    // Resolve mode before reading malformed input so enforce-mode errors still deny.
    let mut loaded = config::load_trusted(store, &cwd, &home)?;
    state.mode = Some(loaded.config.mode);
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
        state.row.mode = "unknown".into();
    }
    let cwd = match ev.cwd.as_deref() {
        Some(path) => {
            let path = PathBuf::from(path);
            ensure!(path.is_absolute(), "hook cwd must be absolute");
            path.canonicalize().context("resolve payload cwd")?
        }
        None => cwd,
    };
    if ev.cwd.is_some() {
        loaded = config::load_trusted(store, &cwd, &home)?;
    }
    state.mode = Some(loaded.config.mode);
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
    // Validate/import fallible transcript data before the stop guard can advance state.
    if !ev.session_id.is_empty()
        && matches!(
            kind,
            EventKind::Stop | EventKind::SessionEnd | EventKind::PreCompact
        )
    {
        if let Some(path) = ev.transcript_path.as_deref() {
            yoke_core::usage::ingest(
                store,
                agent.as_str(),
                &ev.session_id,
                &state.row.project,
                Path::new(path),
            )?;
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
        memory::record_hook_facts(store, &state.row, &raw)?;
    }
    if loaded.config.mode == Mode::Enforce
        && !outcome.blocked()
        && matches!(kind, EventKind::SessionStart | EventKind::UserPromptSubmit)
    {
        let mut context = String::new();
        if !ev.session_id.is_empty() {
            if let Some(active) = plan::active(store, &loaded.project_root, &ev.session_id)? {
                context.push_str(&format!("Yoke plan {} is {}. Inspect with yoke plan status {}. Waiting or stalled plans require a human to use yoke plan resume {}.\n", active.slug, active.state, active.slug, active.slug));
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
        store.prune_decisions(state.row.ts, loaded.config.log.retention_days)?;
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
        schedule_update(store, &home, state.row.ts)?;
    }
    if atomic_stop {
        stop_guard::record_stop(
            store,
            &loaded.project_root,
            loaded.config.stop.max_blocks,
            &mut state.row,
            state.cap,
            &raw,
        )?;
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
        store.record_decision(&state.row, state.cap)?;
    }
    state.persisted = true;
    if loaded.config.memory.digest
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
            // Background launch failure cannot change an already persisted hook decision.
            let reason = redact(&format!("memory digest scheduling failed: {error:#}"));
            eprintln!("yoke: {reason}");
            if let Err(error) = append_error(&home, &reason) {
                eprintln!(
                    "yoke: error.log fallback failed: {}",
                    redact(&error.to_string())
                );
            }
        }
    }
    Ok(output)
}

fn fail(state: &mut HookState, agent: Agent, kind: Option<EventKind>, detail: &str) -> i32 {
    let reason = redact(&format!("yoke error: {detail}; set YOKE_OFF=1 to bypass"));
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

fn append_error(home: &Path, reason: &str) -> Result<()> {
    std::fs::create_dir_all(home.join(".yoke"))?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(home.join(".yoke/error.log"))?;
    writeln!(file, "{} {}", now(), redact(reason))?;
    Ok(())
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
