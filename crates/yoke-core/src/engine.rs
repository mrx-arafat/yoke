use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use regex::Regex;
use serde::Serialize;
use serde_json::Value;

use crate::config::{Config, Gate, GateAction};
use crate::event::{AgentEvent, EventKind};

#[path = "review/process.rs"]
mod process;
pub use process::{execute_bounded, kill_running_groups, ProcessOutput};

/// Paths supplied by the hook integration, independent of the process cwd.
#[derive(Debug, Clone)]
pub struct EvaluationContext {
    /// The directory the event reports. The agent chooses it, so it is data (`YOKE_CWD`),
    /// never where a trusted command is resolved.
    pub cwd: PathBuf,
    /// The project the session belongs to (`YOKE_PROJECT`).
    pub project: PathBuf,
    /// The directory the trusted `yoke.toml` was read from. Gate commands run here, so a
    /// relative command means the same file whichever directory the agent works in.
    pub policy_root: PathBuf,
    pub home: PathBuf,
}

/// One matched gate's observable result, suitable for decision logging.
#[derive(Debug, Clone, Serialize)]
pub struct GateResult {
    pub name: String,
    pub result: String,
    pub ms: u64,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Default)]
pub struct Outcome {
    /// False when a gate vetoed a blocking event.
    pub allow: bool,
    /// Names of the gates that ran, in order.
    pub ran: Vec<String>,
    /// Failure reasons from gates that exited non-zero.
    pub reasons: Vec<String>,
    pub updated_input: Option<Value>,
    pub gates: Vec<GateResult>,
    /// An invalid gate, spawn failure, or execution I/O error occurred.
    pub error: bool,
}

impl Outcome {
    pub fn blocked(&self) -> bool {
        !self.allow
    }
}

fn matches(gate: &Gate, ev: &AgentEvent, haystack: &str) -> Result<bool, regex::Error> {
    if let Some(tool) = &gate.match_tool {
        let regex = Regex::new(&format!(r"\A(?:{tool})\z"))?;
        if !ev
            .tool_name
            .as_deref()
            .is_some_and(|name| regex.is_match(name))
        {
            return Ok(false);
        }
    }
    if let Some(needle) = &gate.match_contains {
        if !haystack.contains(needle.as_str()) {
            return Ok(false);
        }
    }
    if let Some(pattern) = &gate.match_pattern {
        if !Regex::new(pattern)?.is_match(haystack) {
            return Ok(false);
        }
    }
    Ok(true)
}

const OUTPUT_CAP: usize = 4096;

struct CheckResult {
    reason: Option<String>,
    exit_code: Option<i32>,
    timed_out: bool,
}

/// A NUL byte cannot be put in an environment variable and would stop the gate from starting.
fn env_text(value: &str) -> String {
    value.replace('\0', "")
}

fn run_gate(
    gate: &Gate,
    kind: EventKind,
    ev: &AgentEvent,
    ctx: &EvaluationContext,
) -> io::Result<CheckResult> {
    let mut command = Command::new(gate.shell.as_deref().unwrap_or("sh"));
    command
        .arg("-c")
        .arg(&gate.run)
        .current_dir(&ctx.policy_root)
        .env("YOKE_EVENT", format!("{kind:?}"))
        .env("YOKE_GATE", env_text(&gate.name))
        .env(
            "YOKE_TOOL_NAME",
            env_text(ev.tool_name.as_deref().unwrap_or_default()),
        )
        .env("YOKE_TOOL_INPUT", ev.tool_input.to_string())
        .env("YOKE_SESSION", env_text(&ev.session_id))
        .env("YOKE_CWD", &ctx.cwd)
        .env("YOKE_PROJECT", &ctx.project);
    let output = execute_bounded(
        &mut command,
        &[],
        Duration::from_secs(gate.timeout),
        OUTPUT_CAP,
    )
    .map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            // The same errno means a missing shell or a missing working directory.
            let what = if ctx.policy_root.is_dir() {
                format!("shell '{}' was not found", gate.shell.as_deref().unwrap_or("sh"))
            } else {
                format!(
                    "working directory {} does not exist",
                    ctx.policy_root.display()
                )
            };
            io::Error::new(error.kind(), what)
        } else if error.kind() == io::ErrorKind::ArgumentListTooLong {
            // The tool input travels in the environment, and the system caps its size (about
            // 128 KiB per variable on Linux, 1 MiB for the whole environment on macOS).
            io::Error::other(format!(
                "the tool input is too large ({} KiB) to pass to a check gate; ask the agent to use smaller tool inputs",
                ev.tool_input.to_string().len() / 1024
            ))
        } else {
            error
        }
    })?;
    let reason = if output.timed_out {
        Some(format!(
            "gate '{}' timed out after {}s",
            gate.name, gate.timeout
        ))
    } else if output.success {
        None
    } else {
        Some(gate.message.clone().unwrap_or_else(|| {
            let text = String::from_utf8_lossy(&output.stderr);
            let mut offset = text.len().saturating_sub(OUTPUT_CAP);
            while !text.is_char_boundary(offset) {
                offset += 1;
            }
            let text = text[offset..].trim();
            if text.is_empty() {
                format!(
                    "gate '{}' failed (exit {})",
                    gate.name,
                    output.exit_code.unwrap_or(-1)
                )
            } else {
                text.to_owned()
            }
        }))
    };
    Ok(CheckResult {
        reason,
        exit_code: output.exit_code,
        timed_out: output.timed_out,
    })
}

/// Evaluate using process paths; hook callers should supply explicit context.
pub fn evaluate(cfg: &Config, kind: EventKind, ev: &AgentEvent) -> Outcome {
    match (std::env::current_dir(), crate::config::home()) {
        (Ok(cwd), Ok(home)) => evaluate_with_context(
            cfg,
            kind,
            ev,
            &EvaluationContext {
                project: cwd.clone(),
                policy_root: cwd.clone(),
                cwd,
                home,
            },
        ),
        _ => Outcome {
            allow: !kind.is_blocking(),
            reasons: vec!["cannot resolve evaluation paths".into()],
            error: true,
            ..Default::default()
        },
    }
}

/// Run the guard, all checks/denies, then at most the first matching rewrite.
/// Mode handling belongs to the caller so observe mode records the same decision.
pub fn evaluate_with_context(
    cfg: &Config,
    kind: EventKind,
    ev: &AgentEvent,
    ctx: &EvaluationContext,
) -> Outcome {
    let mut out = Outcome {
        allow: true,
        ..Default::default()
    };
    let guard_start = Instant::now();
    if let Some(reason) = crate::guard::evaluate(kind, ev, &ctx.home, &ctx.cwd) {
        out.record(
            "self-protection",
            "deny",
            guard_start,
            None,
            Some(reason),
            kind,
        );
        return out;
    }
    let haystack = ev.haystack();
    let mut rewrites = Vec::new();
    for gate in cfg.gates.iter().filter(|g| g.on == kind) {
        let start = Instant::now();
        match matches(gate, ev, &haystack) {
            Ok(false) => continue,
            Err(e) => {
                out.record(
                    &gate.name,
                    "error",
                    start,
                    None,
                    Some(format!("invalid gate '{}': {e}", gate.name)),
                    kind,
                );
                continue;
            }
            Ok(true) => {}
        }
        match gate.action {
            GateAction::Rewrite => {
                if gate.require_command.as_deref().is_none_or(|command| {
                    command_available(
                        command,
                        &ctx.policy_root,
                        std::env::var_os("PATH").as_deref(),
                    )
                }) {
                    rewrites.push(gate);
                }
            }
            GateAction::Deny => out.record(
                &gate.name,
                "deny",
                start,
                None,
                Some(
                    gate.message
                        .clone()
                        .unwrap_or_else(|| format!("gate '{}' denied", gate.name)),
                ),
                kind,
            ),
            GateAction::Check => match run_gate(gate, kind, ev, ctx) {
                Ok(check) => {
                    let result = if check.timed_out {
                        "timeout"
                    } else if check.reason.is_some() {
                        "fail"
                    } else {
                        "pass"
                    };
                    out.record(
                        &gate.name,
                        result,
                        start,
                        check.exit_code,
                        check.reason,
                        kind,
                    );
                }
                Err(e) => out.record(
                    &gate.name,
                    "error",
                    start,
                    None,
                    Some(format!("gate '{}' could not run: {e}", gate.name)),
                    kind,
                ),
            },
        }
    }
    if out.reasons.is_empty() && kind == EventKind::PreToolUse {
        if let Some(gate) = rewrites.first() {
            let start = Instant::now();
            match rewrite(gate, &ev.tool_input) {
                Ok(input) => {
                    let changed = input != ev.tool_input;
                    out.record(
                        &gate.name,
                        if changed { "rewrite" } else { "pass" },
                        start,
                        None,
                        None,
                        kind,
                    );
                    if changed {
                        let proposed = AgentEvent {
                            tool_name: ev.tool_name.clone(),
                            tool_input: input,
                            ..Default::default()
                        };
                        let guard_start = Instant::now();
                        if let Some(reason) =
                            crate::guard::evaluate(kind, &proposed, &ctx.home, &ctx.cwd)
                        {
                            out.record(
                                "self-protection",
                                "deny",
                                guard_start,
                                None,
                                Some(reason),
                                kind,
                            );
                        } else {
                            out.updated_input = Some(proposed.tool_input);
                        }
                    }
                }
                Err(reason) => out.record(&gate.name, "error", start, None, Some(reason), kind),
            }
        }
    }
    out
}

fn command_available(command: &str, cwd: &std::path::Path, path: Option<&std::ffi::OsStr>) -> bool {
    path.is_some_and(|path| {
        std::env::split_paths(path).any(|directory| {
            let candidate = cwd.join(directory).join(command);
            candidate.metadata().is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
    })
}

impl Outcome {
    fn record(
        &mut self,
        name: &str,
        result: &str,
        start: Instant,
        exit_code: Option<i32>,
        reason: Option<String>,
        kind: EventKind,
    ) {
        self.ran.push(name.into());
        self.gates.push(GateResult {
            name: name.into(),
            result: result.into(),
            ms: start.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
            exit_code,
        });
        self.error |= result == "error";
        if let Some(reason) = reason {
            self.reasons.push(reason);
            self.allow &= !kind.is_blocking();
        }
    }
}

fn rewrite(gate: &Gate, input: &Value) -> Result<Value, String> {
    let pattern = gate
        .rewrite_pattern
        .as_deref()
        .ok_or("missing rewrite_pattern")?;
    let replacement = gate.rewrite_to.as_deref().ok_or("missing rewrite_to")?;
    let regex = Regex::new(pattern).map_err(|e| e.to_string())?;
    let original = input
        .get(&gate.rewrite_field)
        .and_then(Value::as_str)
        .ok_or_else(|| {
            format!(
                "gate '{}': rewrite field '{}' must be a string",
                gate.name, gate.rewrite_field
            )
        })?;
    let mut updated = input.clone();
    updated[&gate.rewrite_field] =
        Value::String(regex.replace_all(original, replacement).into_owned());
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluate_without_an_absolute_home_fails_closed() {
        // Re-execute so HOME changes only in a child; a relative HOME never reaches the
        // platform's fallback to the real home, so the old behaviour is safe to observe.
        if std::env::var_os("YOKE_TEST_RELATIVE_HOME").is_some() {
            let outcome = evaluate(&Config::default(), EventKind::PreToolUse, &bash_event("ls"));
            assert!(outcome.error && !outcome.allow, "{outcome:?}");
            return;
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "engine::tests::evaluate_without_an_absolute_home_fails_closed",
            ])
            .env("HOME", "relative/home")
            .env("YOKE_TEST_RELATIVE_HOME", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    /// True while the process exists. A killed orphan that nothing has reaped is a zombie on
    /// hosts whose PID 1 does not reap children (containers), and a zombie is not running.
    fn process_exists(pid: i32) -> bool {
        // SAFETY: signal zero only probes whether the process exists.
        if unsafe { libc::kill(pid, 0) } == -1 {
            return io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH);
        }
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => !stat
                .rsplit_once(')')
                .is_some_and(|(_, rest)| rest.trim_start().starts_with(['Z', 'X'])),
            Err(_) => true,
        }
    }

    fn bash_event(command: &str) -> AgentEvent {
        AgentEvent {
            tool_name: Some("Bash".into()),
            tool_input: serde_json::json!({ "command": command }),
            ..Default::default()
        }
    }

    fn guard() -> Config {
        Config::parse(
            r#"
            [[gate]]
            name = "no-force-push"
            on = "pre_tool_use"
            match_tool = "Bash"
            match_contains = "push --force"
            run = "exit 1"
            message = "force push blocked"
            "#,
        )
        .unwrap()
    }

    #[test]
    fn failing_gate_vetoes_matching_pre_tool_use() {
        let out = evaluate(
            &guard(),
            EventKind::PreToolUse,
            &bash_event("git push --force"),
        );
        assert!(out.blocked());
        assert_eq!(out.reasons, vec!["force push blocked"]);
    }

    #[test]
    fn non_matching_event_is_allowed_without_running_gate() {
        let out = evaluate(&guard(), EventKind::PreToolUse, &bash_event("git push"));
        assert!(!out.blocked());
        assert!(out.ran.is_empty());
    }

    #[test]
    fn tool_regex_matches_alternatives_but_not_substrings() {
        let cfg = Config::parse(
            r#"
            [[gate]]
            name = "tools"
            on = "pre_tool_use"
            match_tool = "Bash|Edit"
            run = "exit 1"
        "#,
        )
        .unwrap();
        assert!(evaluate(&cfg, EventKind::PreToolUse, &bash_event("ls")).blocked());
        let mut event = bash_event("ls");
        event.tool_name = Some("BashExtra".into());
        assert!(evaluate(&cfg, EventKind::PreToolUse, &event).ran.is_empty());
    }

    fn context(root: &std::path::Path) -> EvaluationContext {
        EvaluationContext {
            cwd: root.into(),
            project: root.into(),
            policy_root: root.into(),
            home: root.into(),
        }
    }

    #[test]
    fn deny_after_rewrite_wins_and_checks_still_run() {
        let root = tempfile::tempdir().unwrap();
        let cfg = Config::parse(
            r#"
            [[gate]]
            name = "rewrite-first"
            on = "pre_tool_use"
            action = "rewrite"
            rewrite_pattern = "danger"
            rewrite_to = "safe"
            [[gate]]
            name = "deny-later"
            on = "pre_tool_use"
            action = "deny"
            match_pattern = "danger"
            [[gate]]
            name = "check-last"
            on = "pre_tool_use"
            run = "printf checked > checked"
        "#,
        )
        .unwrap();
        let out = evaluate_with_context(
            &cfg,
            EventKind::PreToolUse,
            &bash_event("danger"),
            &context(root.path()),
        );
        assert!(out.blocked());
        assert_eq!(out.updated_input, None);
        assert_eq!(out.ran, ["deny-later", "check-last"]);
        assert_eq!(
            std::fs::read_to_string(root.path().join("checked")).unwrap(),
            "checked"
        );
    }

    #[test]
    fn first_rewrite_preserves_other_input_and_failed_checks_suppress_it() {
        let cfg = Config::parse(
            r#"
            [[gate]]
            name = "first"
            on = "pre_tool_use"
            action = "rewrite"
            rewrite_pattern = '^(.*)$'
            rewrite_to = 'wrapper $1'
            [[gate]]
            name = "second"
            on = "pre_tool_use"
            action = "rewrite"
            rewrite_pattern = '^(.*)$'
            rewrite_to = 'wrong $1'
        "#,
        )
        .unwrap();
        let mut event = bash_event("ls");
        event.tool_input["description"] = Value::String("list".into());
        let out = evaluate(&cfg, EventKind::PreToolUse, &event);
        assert_eq!(
            out.updated_input,
            Some(serde_json::json!({"command":"wrapper ls","description":"list"}))
        );
        assert_eq!(out.ran, ["first"]);
        let mut cfg = cfg;
        cfg.gates[0].rewrite_pattern = Some("not-present".into());
        let out = evaluate(&cfg, EventKind::PreToolUse, &event);
        assert_eq!(out.updated_input, None);
        assert_eq!(out.ran, ["first"]);
        cfg.gates[0].rewrite_pattern = Some("^(.*)$".into());
        let mut invalid_event = bash_event("ls");
        invalid_event.tool_input["command"] = Value::Null;
        let out = evaluate(&cfg, EventKind::PreToolUse, &invalid_event);
        assert!(out.blocked());
        assert!(out.error);
        cfg.merge(Config::parse("[[gate]]\nname='fail'\non='pre_tool_use'\nrun='exit 9'").unwrap());
        let out = evaluate(&cfg, EventKind::PreToolUse, &event);
        assert!(out.blocked());
        assert_eq!(out.updated_input, None);
        assert_eq!(out.gates[0].exit_code, Some(9));
    }

    #[test]
    fn rewrite_cannot_introduce_a_protected_operation() {
        let root = tempfile::tempdir().unwrap();
        let cfg = Config::parse(
            r#"
            [[gate]]
            name = "rewrite"
            on = "pre_tool_use"
            action = "rewrite"
            rewrite_pattern = '^safe$'
            rewrite_to = 'yoke trust'
        "#,
        )
        .unwrap();
        let out = evaluate_with_context(
            &cfg,
            EventKind::PreToolUse,
            &bash_event("safe"),
            &context(root.path()),
        );
        assert!(out.blocked());
        assert_eq!(out.updated_input, None);
        assert_eq!(out.gates.last().unwrap().name, "self-protection");
    }

    #[test]
    fn passes_environment_and_bounds_noisy_output() {
        let root = tempfile::tempdir().unwrap();
        let cfg = Config::parse(r#"
            [[gate]]
            name = "environment"
            on = "pre_tool_use"
            run = '''test "$YOKE_EVENT" = PreToolUse && test "$YOKE_GATE" = environment && test "$YOKE_SESSION" = session && test "$YOKE_TOOL_NAME" = Bash && test "$YOKE_TOOL_INPUT" = '{"command":"ls"}' && test "$YOKE_CWD" = "$YOKE_PROJECT" && test "$PWD" = "$YOKE_CWD"'''
            [[gate]]
            name = "noisy"
            on = "pre_tool_use"
            run = "i=0; while [ $i -lt 10000 ]; do printf 1234567890; printf abcdefghij >&2; i=$((i+1)); done; printf TAIL >&2; exit 2"
        "#).unwrap();
        let mut event = bash_event("ls");
        event.session_id = "session".into();
        // Shells canonicalize PWD on macOS, so use the canonical temp path.
        let ctx = context(&root.path().canonicalize().unwrap());
        let out = evaluate_with_context(&cfg, EventKind::PreToolUse, &event, &ctx);
        assert_eq!(out.gates[0].result, "pass");
        assert_eq!(out.gates[1].exit_code, Some(2));
        assert_eq!(out.reasons.len(), 1);
        assert!(out.reasons[0].len() <= OUTPUT_CAP);
        assert!(out.reasons[0].ends_with("TAIL"));
    }

    #[test]
    fn timeout_kills_descendant_and_returns_promptly() {
        let root = tempfile::tempdir().unwrap();
        let cfg = Config::parse(
            r#"
            [[gate]]
            name = "timeout"
            on = "pre_tool_use"
            timeout = 1
            run = "sleep 30 & child=$!; printf '%s' $child > child.pid; wait $child"
        "#,
        )
        .unwrap();
        let start = Instant::now();
        let out = evaluate_with_context(
            &cfg,
            EventKind::PreToolUse,
            &bash_event("ls"),
            &context(root.path()),
        );
        // The gate leaves a 30 s sleeper behind; returning in 1 s plus kill time, not 30 s, is
        // what this pins, so the bound only has to sit well under the sleeper.
        assert!(start.elapsed() < Duration::from_secs(15));
        assert!(out.blocked());
        assert_eq!(out.gates[0].result, "timeout");
        assert!(out.gates[0].ms >= 1000);
        let pid: i32 = std::fs::read_to_string(root.path().join("child.pid"))
            .unwrap()
            .parse()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while process_exists(pid) {
            assert!(
                Instant::now() < deadline,
                "descendant {pid} survived timeout"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn guard_runs_before_user_gate_and_spawn_errors_fail_closed() {
        let root = tempfile::tempdir().unwrap();
        let cfg = Config::parse("[[gate]]\nname='marker'\non='pre_tool_use'\nrun='touch executed'")
            .unwrap();
        let out = evaluate_with_context(
            &cfg,
            EventKind::PreToolUse,
            &bash_event("yoke trust"),
            &context(root.path()),
        );
        assert_eq!(out.ran, ["self-protection"]);
        assert!(!root.path().join("executed").exists());
        let mut cfg = cfg;
        cfg.gates[0].shell = Some(root.path().join("missing-shell").to_string_lossy().into());
        let out = evaluate_with_context(
            &cfg,
            EventKind::PreToolUse,
            &bash_event("ls"),
            &context(root.path()),
        );
        assert!(out.blocked());
        assert!(out.error);
        assert_eq!(out.gates[0].result, "error");
        // The reason names what is missing, not just an errno.
        assert!(
            out.reasons[0].contains("missing-shell"),
            "{:?}",
            out.reasons
        );
        let mut gone = cfg.clone();
        gone.gates[0].shell = None;
        let mut context = context(root.path());
        context.policy_root = root.path().join("deleted");
        let out = evaluate_with_context(&gone, EventKind::PreToolUse, &bash_event("ls"), &context);
        assert!(
            out.reasons[0].contains("working directory"),
            "{:?}",
            out.reasons
        );
    }

    #[test]
    fn a_gate_resolves_relative_commands_in_the_policy_root_not_the_event_directory() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().canonicalize().unwrap();
        let sub = project.join("sub");
        std::fs::create_dir(&sub).unwrap();
        // The project's own check fails; a passing look-alike sits where the agent works.
        std::fs::write(project.join("check.sh"), "exit 1").unwrap();
        std::fs::write(sub.join("check.sh"), "exit 0").unwrap();
        let cfg =
            Config::parse("[[gate]]\nname = 'lint'\non = 'pre_tool_use'\nrun = 'sh check.sh'\n")
                .unwrap();
        let mut ctx = context(&project);
        ctx.cwd = sub.clone();
        let out = evaluate_with_context(&cfg, EventKind::PreToolUse, &bash_event("ls"), &ctx);
        assert!(out.blocked(), "{:?}", out.reasons);
    }

    #[test]
    fn required_wrapper_is_checked_at_invocation_time_and_missing_gates_are_skipped() {
        let root = tempfile::tempdir().unwrap();
        let wrapper = root.path().join("fixture-wrapper");
        let path = Some(root.path().as_os_str());
        assert!(!command_available("fixture-wrapper", root.path(), path));
        std::fs::write(&wrapper, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(!command_available("fixture-wrapper", root.path(), path));
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(command_available("fixture-wrapper", root.path(), path));
        std::fs::remove_file(&wrapper).unwrap();
        assert!(!command_available("fixture-wrapper", root.path(), path));
        let cfg = Config::parse(
            r#"
            [[gate]]
            name = "missing-wrapper"
            on = "pre_tool_use"
            action = "rewrite"
            require_command = "yoke-nonexistent-wrapper-fixture"
            rewrite_pattern = '^'
            rewrite_to = 'missing '
            [[gate]]
            name = "available"
            on = "pre_tool_use"
            action = "rewrite"
            rewrite_pattern = '^'
            rewrite_to = 'wrapper '
        "#,
        )
        .unwrap();
        let out = evaluate(&cfg, EventKind::PreToolUse, &bash_event("ls"));
        assert_eq!(out.ran, ["available"]);
        assert_eq!(out.updated_input.unwrap()["command"], "wrapper ls");
    }

    #[test]
    fn failing_gate_on_observing_event_does_not_block() {
        let cfg = Config::parse(
            r#"
            [[gate]]
            name = "lint"
            on = "post_tool_use"
            run = "exit 3"
            "#,
        )
        .unwrap();
        let out = evaluate(&cfg, EventKind::PostToolUse, &bash_event("ls"));
        assert!(!out.blocked());
        assert_eq!(out.reasons, vec!["gate 'lint' failed (exit 3)"]);
    }
}
