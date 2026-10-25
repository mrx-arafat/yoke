//! Human-facing output of the plain commands: pipes, errors, `yoke log` and `install --dry-run`.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::{json, Value};
use yoke_core::store::{DecisionRecord, Store};

/// 2026-10-16 10:00:00 UTC.
const T0: i64 = 1_792_144_800;

fn yoke(home: &Path, cwd: &Path, args: &[&str]) -> Output {
    assert!(home.is_dir() && Some(home.to_path_buf()) != std::env::home_dir());
    Command::new(env!("CARGO_BIN_EXE_yoke"))
        .args(args)
        .env("HOME", home)
        // Six hours east of UTC, so local time differs from UTC without needing tzdata.
        .env("TZ", "UTC-6")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME")
        .current_dir(cwd)
        .output()
        .unwrap()
}

fn text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn json_runtime_errors_are_one_value_with_diagnostics_on_stderr() {
    let home = tempfile::tempdir().unwrap();
    for args in [
        vec!["trial", "report", "--json"],
        vec!["log", "--outcome", "invalid", "--json"],
        vec!["plan", "check", "missing", "--agent", "codex", "--json"],
    ] {
        let output = yoke(home.path(), home.path(), &args);
        assert_eq!(output.status.code(), Some(1), "{args:?}: {output:?}");
        let error: Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("{args:?}: {error}; {output:?}"));
        assert!(error["error"].is_string(), "{error}");
        assert!(error["help"].as_str().unwrap().contains("--help"));
        assert!(String::from_utf8_lossy(&output.stderr).contains("error:"));
    }
}

#[test]
fn onboarding_help_shows_examples_and_boundaries_without_creating_state() {
    let home = tempfile::tempdir().unwrap();
    for (args, expected) in [
        (
            vec!["--help"],
            vec!["Getting started:", "yoke init", "yoke trust", "yoke health"],
        ),
        (
            vec!["init", "--help"],
            vec!["Examples:", "yoke init", "yoke trust"],
        ),
        (
            vec!["trust", "--help"],
            vec![
                "Examples:",
                "yoke trust --global",
                "interactive",
                "agent tool process",
            ],
        ),
        (
            vec!["install", "--help"],
            vec![
                "Examples:",
                "yoke install --agent claude --dry-run",
                "CLAUDE_CONFIG_DIR",
                "CODEX_HOME",
                "/hooks",
                "own terminal",
            ],
        ),
        (
            vec!["health", "--help"],
            vec![
                "Examples:",
                "yoke health --json",
                "read-only",
                "exits 1",
                "exits 0",
            ],
        ),
    ] {
        let output = yoke(home.path(), home.path(), &args);
        assert!(output.status.success(), "{args:?}: {output:?}");
        assert!(output.stderr.is_empty(), "{args:?}: {output:?}");
        let help = text(&output);
        for phrase in expected {
            assert!(
                help.contains(phrase),
                "{args:?}: missing {phrase:?}\n{help}"
            );
        }
    }
    assert!(!home.path().join(".yoke").exists());
    assert!(!home.path().join("yoke.toml").exists());
}

fn record(
    store: &Store,
    cap: usize,
    (ts, agent, project, event, tool): (i64, &str, &str, &str, Option<&str>),
    (outcome, reason, gates, payload): (&str, Option<&str>, Value, &str),
) {
    store
        .record_decision(
            &DecisionRecord {
                policy_hash: None,
                ts,
                session_id: "s1".into(),
                agent: agent.into(),
                project: project.into(),
                event: event.into(),
                tool: tool.map(Into::into),
                mode: "observe".into(),
                outcome: outcome.into(),
                reason: reason.map(Into::into),
                gates,
                payload: payload.into(),
            },
            cap,
        )
        .unwrap();
}

fn seeded_home() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join(".yoke")).unwrap();
    let store = Store::open(temp.path().join(".yoke/state.db")).unwrap();
    // Cut at 40 bytes: the stored payload is no longer valid JSON.
    record(
        &store,
        40,
        (T0, "codex", "/work/alpha", "stop", None),
        (
            "allow",
            None,
            json!([]),
            r#"{"tool_input":{"command":"echo aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}"#,
        ),
    );
    record(
        &store,
        4096,
        (
            T0 + 60,
            "codex",
            "/work/alpha",
            "post_tool_use",
            Some("Bash"),
        ),
        (
            "allow",
            None,
            json!([]),
            r#"{"tool_input":{"command":"git status\nsecond line"}}"#,
        ),
    );
    record(
        &store,
        4096,
        (
            T0 + 120,
            "claude",
            "/work/beta",
            "pre_tool_use",
            Some("Bash"),
        ),
        (
            "deny",
            Some("force push blocked"),
            json!([{"name": "no-force-push", "result": "fail", "ms": 3, "exit_code": 1}]),
            r#"{"tool_input":{"command":"git push --force"}}"#,
        ),
    );
    temp
}

#[cfg(unix)]
#[test]
fn a_closed_output_pipe_ends_the_command_quietly() {
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::os::unix::process::ExitStatusExt;
    use std::process::Stdio;

    let home = tempfile::tempdir().unwrap();
    let mut fds = [0; 2];
    // SAFETY: pipe fills two fresh descriptors; the reader is closed and the writer is owned once.
    let writer = unsafe {
        assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
        libc::close(fds[0]);
        OwnedFd::from_raw_fd(fds[1])
    };
    let output = Command::new(env!("CARGO_BIN_EXE_yoke"))
        .arg("log")
        .env("HOME", home.path())
        .current_dir(home.path())
        .stdout(Stdio::from(writer))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert_eq!(output.status.signal(), Some(libc::SIGPIPE), "{stderr}");
}

#[test]
fn log_rows_name_agent_tool_and_command_in_local_time_and_json_flags_cut_payloads() {
    let home = seeded_home();
    let all = text(&yoke(home.path(), home.path(), &["log", "--all"]));
    assert!(
        all.starts_with("project: all\ndecisions[3]{time,agent,session,project,event,tool,outcome,gate,detail}:\n"),
        "{all}"
    );
    for row in [
        r#"  "2026-10-16 16:02:00","claude","s1","/work/beta","pre_tool_use","Bash","deny","no-force-push","git push --force - force push blocked""#,
        r#"  "2026-10-16 16:01:00","codex","s1","/work/alpha","post_tool_use","Bash","allow","","git status second line""#,
        r#"  "2026-10-16 16:00:00","codex","s1","/work/alpha","stop","","allow","","""#,
    ] {
        assert!(all.lines().any(|line| line == row), "{row}\n{all}");
    }
    let one = text(&yoke(
        home.path(),
        home.path(),
        &["log", "--project", "/work/alpha"],
    ));
    assert!(
        one.starts_with("project: \"/work/alpha\"\ndecisions[2]{time,agent,session,event,tool,outcome,gate,detail}:\n"),
        "{one}"
    );

    let json: Value = serde_json::from_str(&text(&yoke(
        home.path(),
        home.path(),
        &["log", "--all", "--json"],
    )))
    .unwrap();
    assert_eq!(json[0]["ts"], T0 + 120);
    assert_eq!(json[0]["payload_truncated"], false);
    assert_eq!(json[2]["payload_truncated"], true);
}

#[test]
fn log_filters_resolve_the_project_and_say_when_nothing_matches() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let project = home.join("proj");
    let deep = project.join("sub/deep");
    fs::create_dir_all(&deep).unwrap();
    fs::create_dir(project.join(".git")).unwrap();
    fs::create_dir(home.join(".yoke")).unwrap();
    let store = Store::open(home.join(".yoke/state.db")).unwrap();
    record(
        &store,
        4096,
        (
            T0,
            "claude",
            project.to_str().unwrap(),
            "pre_tool_use",
            Some("Bash"),
        ),
        ("allow", None, json!([]), "{}"),
    );
    drop(store);

    let dot = text(&yoke(&home, &deep, &["log", "--project", "."]));
    assert!(dot.contains("\ndecisions[1]"), "{dot}");

    let typo = yoke(&home, &home, &["log", "--outcome", "wouldeny"]);
    assert_eq!(typo.status.code(), Some(1));
    let out = text(&typo);
    assert!(
        out.contains("would_deny") && out.contains("unknown outcome"),
        "{out}"
    );
    assert!(
        out.contains(r#"help: "Run yoke log --help for usage.""#),
        "{out}"
    );

    let none = text(&yoke(&home, &home, &["log", "--all", "--tool", "bash"]));
    assert!(none.contains("decisions[0]"), "{none}");
    assert!(none.contains("case-sensitive"), "{none}");
}

/// Two Git projects with one decision each and a third with none; returns the canonical home.
fn two_projects() -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    fs::create_dir(home.join(".yoke")).unwrap();
    let store = Store::open(home.join(".yoke/state.db")).unwrap();
    for (name, session) in [("alpha", "session-a"), ("beta", "session-b")] {
        fs::create_dir_all(home.join(name).join("sub")).unwrap();
        fs::create_dir(home.join(name).join(".git")).unwrap();
        store
            .record_decision(
                &DecisionRecord {
                    policy_hash: None,
                    ts: T0,
                    session_id: session.into(),
                    agent: "claude".into(),
                    project: home.join(name).display().to_string(),
                    event: "pre_tool_use".into(),
                    tool: Some("Bash".into()),
                    mode: "observe".into(),
                    outcome: "allow".into(),
                    reason: None,
                    gates: json!([]),
                    payload: "{}".into(),
                },
                4096,
            )
            .unwrap();
    }
    fs::create_dir_all(home.join("gamma/.git")).unwrap();
    (temp, home)
}

#[test]
fn log_defaults_to_the_current_project_and_all_widens_it() {
    let (_temp, home) = two_projects();
    let alpha = home.join("alpha");

    let here = text(&yoke(&home, &alpha.join("sub"), &["log"]));
    assert!(
        here.starts_with(&format!(
            "project: \"{}\"\ndecisions[1]{{time,agent,session,event,tool,outcome,gate,detail}}:\n",
            alpha.display()
        )),
        "{here}"
    );
    assert!(
        here.contains("\"session-a\"") && !here.contains("session-b"),
        "{here}"
    );
    assert!(here.contains("yoke log --all"), "{here}");

    let all = text(&yoke(&home, &alpha, &["log", "--all"]));
    assert!(all.starts_with("project: all\ndecisions[2]{"), "{all}");
    assert!(
        all.contains("session-a") && all.contains("session-b"),
        "{all}"
    );

    // A session id names one session, so it is searched everywhere unless a project is given.
    let session = text(&yoke(&home, &alpha, &["log", "--session", "session-b"]));
    assert!(
        session.contains("\ndecisions[1]") && session.contains("session-b"),
        "{session}"
    );

    let conflict = yoke(&home, &alpha, &["log", "--all", "--project", "."]);
    assert_eq!(conflict.status.code(), Some(2));

    let empty = text(&yoke(&home, &home.join("gamma"), &["log"]));
    assert!(empty.contains("decisions[0]"), "{empty}");
    assert!(
        empty.contains("No decisions recorded for this project yet"),
        "{empty}"
    );
    assert!(empty.contains("yoke log --all"), "{empty}");
}

#[test]
fn log_would_deny_with_no_rows_says_no_gate_has_run() {
    let (_temp, home) = two_projects();
    let gamma = home.join("gamma");

    let none = text(&yoke(&home, &gamma, &["log", "--outcome", "would_deny"]));
    assert!(none.contains("decisions[0]"), "{none}");
    assert!(
        none.contains("no gate has run") && none.contains("no trusted gates"),
        "{none}"
    );

    // A yoke.toml that no human trusted contributes nothing, and the answer names the way on.
    fs::write(gamma.join("yoke.toml"), "mode = \"enforce\"\n").unwrap();
    let untrusted = text(&yoke(&home, &gamma, &["log", "--outcome", "would_deny"]));
    assert!(untrusted.contains("no gate has run"), "{untrusted}");
    assert!(
        untrusted.contains("not trusted") && untrusted.contains("yoke trust"),
        "{untrusted}"
    );

    // Rows that exist are never explained away.
    let rows = text(&yoke(
        &home,
        &home.join("alpha"),
        &["log", "--outcome", "allow"],
    ));
    assert!(!rows.contains("no gate has run"), "{rows}");
}

#[test]
fn install_dry_run_shows_readable_changed_lines_not_escaped_files() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    fs::create_dir(home.join(".claude")).unwrap();
    fs::write(home.join(".claude/settings.json"), "{\"model\":\"opus\"}\n").unwrap();
    let dry = text(&yoke(
        &home,
        &home,
        &["install", "--agent", "claude", "--dry-run"],
    ));
    assert!(
        dry.contains("changes: 1") && dry.contains("dry_run: true"),
        "{dry}"
    );
    // Diagnostics quote paths too; only changed settings must be unescaped JSON.
    assert!(
        dry.lines()
            .filter(|line| line.starts_with("+ ") || line.starts_with("- "))
            .all(|line| !line.contains("\\\"")),
        "escaped JSON leaked:\n{dry}"
    );
    assert!(
        dry.lines().any(|line| {
            line.starts_with("+ hooks.PreToolUse: ")
                && line.contains("\"command\":")
                && line.contains(" run-hook --agent claude ")
        }),
        "{dry}"
    );
    // SEC24 previews show owned changes without exposing neighboring settings.
    assert!(!dry.contains("model") && !dry.contains("opus"), "{dry}");
    assert_eq!(
        fs::read_to_string(home.join(".claude/settings.json")).unwrap(),
        "{\"model\":\"opus\"}\n",
        "a dry run writes nothing"
    );
}
