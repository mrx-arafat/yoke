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

fn record(
    store: &Store,
    cap: usize,
    (ts, agent, project, event, tool): (i64, &str, &str, &str, Option<&str>),
    (outcome, reason, gates, payload): (&str, Option<&str>, Value, &str),
) {
    store
        .record_decision(
            &DecisionRecord {
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
    let all = text(&yoke(home.path(), home.path(), &["log"]));
    assert!(
        all.starts_with("decisions[3]{time,agent,project,event,tool,outcome,gate,detail}:\n"),
        "{all}"
    );
    for row in [
        r#"  "2026-10-16 16:02:00","claude","/work/beta","pre_tool_use","Bash","deny","no-force-push","git push --force - force push blocked""#,
        r#"  "2026-10-16 16:01:00","codex","/work/alpha","post_tool_use","Bash","allow","","git status second line""#,
        r#"  "2026-10-16 16:00:00","codex","/work/alpha","stop","","allow","","""#,
    ] {
        assert!(all.lines().any(|line| line == row), "{row}\n{all}");
    }
    let one = text(&yoke(
        home.path(),
        home.path(),
        &["log", "--project", "/work/alpha"],
    ));
    assert!(
        one.starts_with("decisions[2]{time,agent,event,tool,outcome,gate,detail}:\n"),
        "{one}"
    );

    let json: Value =
        serde_json::from_str(&text(&yoke(home.path(), home.path(), &["log", "--json"]))).unwrap();
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
    assert!(dot.starts_with("decisions[1]"), "{dot}");

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

    let none = text(&yoke(&home, &home, &["log", "--tool", "bash"]));
    assert!(none.starts_with("decisions[0]"), "{none}");
    assert!(none.contains("case-sensitive"), "{none}");
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
    assert!(!dry.contains("\\\""), "escaped JSON leaked:\n{dry}");
    assert!(dry.lines().any(|line| line == "+   \"hooks\": {"), "{dry}");
    assert!(dry.contains("  \"model\": \"opus\""), "{dry}");
    assert_eq!(
        fs::read_to_string(home.join(".claude/settings.json")).unwrap(),
        "{\"model\":\"opus\"}\n",
        "a dry run writes nothing"
    );
}
