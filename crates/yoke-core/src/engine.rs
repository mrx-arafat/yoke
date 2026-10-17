use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use regex::Regex;
use serde::Serialize;
use serde_json::Value;

use crate::config::{Config, Gate, GateAction};
use crate::event::{Agent, AgentEvent, EventKind};

#[path = "review/process.rs"]
mod process;
pub use process::{
    execute_bounded, execute_bounded_with_optional_stdin, kill_running_groups, ProcessOutput,
};

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
const TOOL_INPUT_CAP: usize = 32 * 1024;

fn gate_input(input: &Value) -> (String, bool) {
    let mut text = input.to_string();
    if text.len() <= TOOL_INPUT_CAP {
        return (text, false);
    }
    const MARKER: &str = "\n[YOKE TOOL INPUT TRUNCATED]";
    let mut end = TOOL_INPUT_CAP - MARKER.len();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push_str(MARKER);
    (text, true)
}

fn gate_environment_key(key: &std::ffi::OsStr) -> bool {
    matches!(
        key.to_str(),
        Some(
            "PATH"
                | "TMPDIR"
                | "TMP"
                | "TEMP"
                | "LANG"
                | "LC_ALL"
                | "LC_CTYPE"
                | "LC_MESSAGES"
                | "TZ"
                | "USER"
                | "LOGNAME"
                | "SHELL"
                | "TERM"
                | "COLORTERM"
                | "NO_COLOR"
                | "CI"
                | "XDG_CONFIG_HOME"
                | "XDG_CACHE_HOME"
                | "XDG_DATA_HOME"
                | "XDG_STATE_HOME"
                | "XDG_RUNTIME_DIR"
                | "CARGO_HOME"
                | "RUSTUP_HOME"
                | "RUSTUP_TOOLCHAIN"
                | "GOPATH"
                | "GOROOT"
                | "GOCACHE"
                | "GOMODCACHE"
                | "GOFLAGS"
                | "NVM_DIR"
                | "NODE_OPTIONS"
                | "VOLTA_HOME"
                | "PNPM_HOME"
                | "BUN_INSTALL"
                | "PYENV_ROOT"
                | "VIRTUAL_ENV"
                | "CONDA_PREFIX"
                | "UV_CACHE_DIR"
                | "JAVA_HOME"
                | "GRADLE_USER_HOME"
                | "MAVEN_OPTS"
                | "ANDROID_HOME"
                | "DOTNET_ROOT"
                | "SSL_CERT_FILE"
                | "SSL_CERT_DIR"
                | "NODE_EXTRA_CA_CERTS"
                | "REQUESTS_CA_BUNDLE"
                | "CURL_CA_BUNDLE"
                | "NO_PROXY"
                | "no_proxy"
                | "PKG_CONFIG_PATH"
        )
    )
}

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
    pass_env: &[String],
) -> io::Result<CheckResult> {
    let (input, truncated) = gate_input(&ev.tool_input);
    let shell = gate.shell.as_deref().unwrap_or("/bin/sh");
    let shell = if std::path::Path::new(shell).is_absolute() {
        PathBuf::from(shell)
    } else {
        crate::process::executable(shell)
            .map_err(|error| io::Error::new(io::ErrorKind::NotFound, error.to_string()))?
    };
    let mut command = Command::new(shell);
    let inherited: Vec<_> = std::env::vars_os().collect();
    let allowed = |key: &std::ffi::OsStr| {
        gate_environment_key(key)
            || pass_env
                .iter()
                .chain(&gate.pass_env)
                .any(|name| key == std::ffi::OsStr::new(name))
    };
    command
        .env_clear()
        .envs(
            inherited
                .iter()
                .filter(|(key, _)| allowed(key))
                .map(|(key, value)| (key, value)),
        )
        .env("HOME", &ctx.home)
        .arg("-c")
        .arg(&gate.run)
        .current_dir(&ctx.policy_root)
        .env("YOKE_EVENT", format!("{kind:?}"))
        .env("YOKE_GATE", env_text(&gate.name))
        .env(
            "YOKE_TOOL_NAME",
            env_text(ev.tool_name.as_deref().unwrap_or_default()),
        )
        .env("YOKE_TOOL_INPUT", input)
        .env(
            "YOKE_TOOL_INPUT_TRUNCATED",
            if truncated { "1" } else { "0" },
        )
        .env("YOKE_SESSION", env_text(&ev.session_id))
        .env("YOKE_CWD", &ctx.cwd)
        .env("YOKE_PROJECT", &ctx.project);
    // SAFETY: this child-only callback calls only async-signal-safe libc functions.
    // Existing stricter hard limits are preserved; no process-wide limit is changed.
    unsafe {
        command.pre_exec(move || {
            let limit = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            if libc::setrlimit(libc::RLIMIT_CORE, &limit) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
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
    let mut reason = if output.timed_out {
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
    if let Some(reason) = &mut reason {
        let mut withheld: Vec<_> = inherited
            .iter()
            .filter_map(|(key, _)| {
                let name = key.to_str()?;
                (!allowed(key) && crate::config::environment_name(name) && name != "HOME")
                    .then_some(name)
            })
            .collect();
        withheld.sort_unstable();
        if !withheld.is_empty() {
            let count = withheld.len();
            withheld.truncate(12);
            let diagnostic = format!("\nWithheld environment names: {}{}; pass only required names with [gates].pass_env or this [[gate]].pass_env, then run yoke trust in your own terminal.", withheld.join(", "), if count > 12 { " (first 12)" } else { "" });
            let mut start = reason
                .len()
                .saturating_sub(OUTPUT_CAP.saturating_sub(diagnostic.len()));
            while !reason.is_char_boundary(start) {
                start += 1;
            }
            reason.drain(..start);
            reason.push_str(&diagnostic);
        }
    }
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
    evaluate_for_agent_with_context(cfg, kind, ev, ctx, Agent::Claude)
}

/// Evaluate gates using the vendor's rewrite contract and explicit trusted paths.
pub fn evaluate_for_agent_with_context(
    cfg: &Config,
    kind: EventKind,
    ev: &AgentEvent,
    ctx: &EvaluationContext,
    agent: Agent,
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
        if agent == Agent::Codex
            && gate.action == GateAction::Rewrite
            && (gate
                .match_tool
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
                || gate
                    .match_contains
                    .as_deref()
                    .is_none_or(|value| value.trim().is_empty()))
        {
            out.record(&gate.name, "error", start, None, Some(format!("Codex rewrite gate '{}' requires nonempty match_tool and match_contains; add both matchers to yoke.toml and approve the updated policy with yoke trust", gate.name)), kind);
            continue;
        }
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
            GateAction::Check => {
                match run_gate(gate, kind, ev, ctx, &cfg.gate_environment.pass_env) {
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
                }
            }
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
    fn check_input_is_bounded_without_losing_full_match() {
        let root = tempfile::tempdir().unwrap();
        let cfg = Config::parse("[[gate]]\nname='bounded'\non='pre_tool_use'\nmatch_contains='TAIL'\nrun='test ${#YOKE_TOOL_INPUT} -lt 65536 && test \"$YOKE_TOOL_INPUT_TRUNCATED\" = 1 && case \"$YOKE_TOOL_INPUT\" in *\"[YOKE TOOL INPUT TRUNCATED]\"*) exit 0;; *) exit 1;; esac'").unwrap();
        let out = evaluate_with_context(
            &cfg,
            EventKind::PreToolUse,
            &bash_event(&format!("{}TAIL", "界".repeat(100_000))),
            &context(root.path()),
        );
        assert!(out.allow, "{:?}", out.reasons);
        assert_eq!(out.ran, ["bounded"]);
    }

    #[test]
    fn check_environment_and_resource_limits_are_restricted() {
        let root = tempfile::tempdir().unwrap();
        let limits = "printf '%s\\n' \"$(ulimit -f)\" \"$(ulimit -t)\" \"$(ulimit -n)\"";
        let parent = Command::new("/bin/sh")
            .args(["-c", limits])
            .output()
            .unwrap();
        assert!(parent.status.success());
        let mut cfg =
            Config::parse("[[gate]]\nname='restricted'\non='pre_tool_use'\nrun='true'").unwrap();
        cfg.gates[0].run = format!("test \"$(ulimit -c)\" = 0 && {limits} > limits.txt");
        let out = evaluate_with_context(
            &cfg,
            EventKind::PreToolUse,
            &bash_event("ls"),
            &context(root.path()),
        );
        assert!(out.allow, "{:?}", out.reasons);
        assert_eq!(
            std::fs::read(root.path().join("limits.txt")).unwrap(),
            parent.stdout,
            "file size, CPU and descriptor limits must stay inherited"
        );
    }

    #[test]
    fn gate_environment_keeps_toolchains_and_passes_only_trusted_names() {
        const CHILD: &str = "YOKE_TEST_GATE_ENV";
        if std::env::var_os(CHILD).is_none() {
            let out = Command::new(std::env::current_exe().unwrap())
                .env_clear().env("HOME", std::env::var_os("HOME").unwrap())
                .env("PATH", std::env::var_os("PATH").unwrap())
                .args(["--exact", "engine::tests::gate_environment_keeps_toolchains_and_passes_only_trusted_names", "--quiet"])
                .env(CHILD, "1").env("CARGO_HOME", "/gate-cargo")
                .env("VIRTUAL_ENV", "/gate-python")
                .env("AWS_SECRET_ACCESS_KEY", "cx3-test-secret")
                .env("GITHUB_TOKEN", "cx3-test-token")
                .output().unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stdout)
            );
            return;
        }
        let root = tempfile::tempdir().unwrap();
        for (settings, command) in [
            ("", "test \"$CARGO_HOME\" = /gate-cargo && test \"$VIRTUAL_ENV\" = /gate-python && test -z \"${AWS_SECRET_ACCESS_KEY+x}\" && test -z \"${GITHUB_TOKEN+x}\""),
            ("[gates]\npass_env=['AWS_SECRET_ACCESS_KEY']\n", "test \"$AWS_SECRET_ACCESS_KEY\" = cx3-test-secret && test -z \"${GITHUB_TOKEN+x}\""),
        ] {
            let mut cfg = Config::parse(&format!("{settings}[[gate]]\nname='env'\non='pre_tool_use'\nrun='true'")).unwrap();
            cfg.gates[0].run = command.into();
            let out = evaluate_with_context(&cfg, EventKind::PreToolUse, &bash_event("ls"), &context(root.path()));
            assert!(out.allow, "{:?}", out.reasons);
        }
        let cfg = Config::parse("[[gate]]\nname='env'\non='pre_tool_use'\npass_env=['GITHUB_TOKEN']\nrun='test \"$GITHUB_TOKEN\" = cx3-test-token && test -z \"${AWS_SECRET_ACCESS_KEY+x}\"'").unwrap();
        assert!(
            evaluate_with_context(
                &cfg,
                EventKind::PreToolUse,
                &bash_event("ls"),
                &context(root.path())
            )
            .allow
        );
        let cfg = Config::parse("[[gate]]\nname='env'\non='pre_tool_use'\nrun='exit 3'").unwrap();
        let out = evaluate_with_context(
            &cfg,
            EventKind::PreToolUse,
            &bash_event("ls"),
            &context(root.path()),
        );
        let reason = out.reasons.join(" ");
        assert!(
            reason.contains("AWS_SECRET_ACCESS_KEY")
                && reason.contains("GITHUB_TOKEN")
                && reason.contains("pass_env"),
            "{reason}"
        );
        assert!(!reason.contains("cx3-test-secret") && !reason.contains("cx3-test-token"));
    }

    #[test]
    fn codex_rewrite_requires_both_matchers_without_breaking_claude() {
        let root = tempfile::tempdir().unwrap();
        let mut cfg = Config::parse("[[gate]]\nname='rewrite'\non='pre_tool_use'\naction='rewrite'\nrewrite_pattern='safe'\nrewrite_to='safer'").unwrap();
        let event = bash_event("safe");
        let ctx = context(root.path());
        let out = evaluate_for_agent_with_context(
            &cfg,
            EventKind::PreToolUse,
            &event,
            &ctx,
            Agent::Codex,
        );
        assert!(out.blocked());
        assert!(out.error);
        assert!(out.reasons[0].contains("match_tool and match_contains"));
        assert!(
            evaluate_with_context(&cfg, EventKind::PreToolUse, &event, &ctx)
                .updated_input
                .is_some()
        );
        cfg.gates[0].match_tool = Some("Bash".into());
        cfg.gates[0].match_contains = Some("safe".into());
        assert!(evaluate_for_agent_with_context(
            &cfg,
            EventKind::PreToolUse,
            &event,
            &ctx,
            Agent::Codex
        )
        .updated_input
        .is_some());
        cfg.gates[0].match_contains = Some(" ".into());
        assert!(evaluate_for_agent_with_context(
            &cfg,
            EventKind::PreToolUse,
            &event,
            &ctx,
            Agent::Codex
        )
        .blocked());
    }

    #[test]
    fn failing_gate_vetoes_matching_pre_tool_use() {
        let out = evaluate(
            &guard(),
            EventKind::PreToolUse,
            &bash_event("git push --force"),
        );
        assert!(out.blocked());
        assert_eq!(out.reasons.len(), 1);
        assert_eq!(out.reasons[0].lines().next(), Some("force push blocked"));
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
        assert!(out.reasons[0].contains("TAIL"));
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
            run = '''sh -c 'sleep 30 & child=$!; printf "%s" "$child" > child.pid; wait "$child"' '''
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
        assert_eq!(out.reasons.len(), 1);
        assert_eq!(
            out.reasons[0].lines().next(),
            Some("gate 'lint' failed (exit 3)")
        );
    }
}
