//! `yoke watch` without a terminal: one read-only snapshot, exit 0, with or without state.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::json;
use yoke_core::store::{DecisionRecord, Store};

fn watch(home: &Path, cwd: &Path) -> Output {
    assert!(home.is_dir() && Some(home.to_path_buf()) != std::env::home_dir());
    Command::new(env!("CARGO_BIN_EXE_yoke"))
        .arg("watch")
        .env("HOME", home)
        .env("TZ", "UTC")
        .env("COLUMNS", "160")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME")
        .env_remove("NO_COLOR")
        .env_remove("YOKE_ASCII")
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn files(directory: &Path) -> BTreeMap<std::ffi::OsString, Vec<u8>> {
    fs::read_dir(directory)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (entry.file_name(), fs::read(entry.path()).unwrap())
        })
        .collect()
}

#[test]
fn prints_one_snapshot_and_exits_0_without_changing_or_creating_state() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let project = fs::canonicalize(project.path()).unwrap();
    fs::create_dir(project.join(".git")).unwrap();
    fs::write(project.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();

    // No state yet: the snapshot says so and nothing is created.
    let output = watch(home.path(), &project);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("No Yoke state yet"), "{text}");
    assert!(!home.path().join(".yoke").exists());

    let state = home.path().join(".yoke");
    fs::create_dir(&state).unwrap();
    let store = Store::open(state.join("state.db")).unwrap();
    let now = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap();
    for (offset, outcome, command) in [
        (30, "allow", "cargo test"),
        (20, "would_deny", "git push --force origin main"),
        (10, "error", "make lint"),
    ] {
        store
            .record_decision(
                &DecisionRecord {
                    policy_hash: None,
                    ts: now - offset,
                    session_id: "s1".into(),
                    agent: "claude".into(),
                    project: project.display().to_string(),
                    event: "pre_tool_use".into(),
                    tool: Some("Bash".into()),
                    mode: "observe".into(),
                    outcome: outcome.into(),
                    reason: None,
                    gates: json!([{"name": "no-force-push", "result": if outcome == "would_deny" { "fail" } else { "pass" }, "ms": 4, "exit_code": 1}]),
                    payload: json!({"tool_input": {"command": command}}).to_string(),
                },
                4096,
            )
            .unwrap();
    }
    drop(store);
    let before = files(&state);
    let output = watch(home.path(), &project);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{text}");
    assert!(!text.contains('\x1b'), "{text}");
    let name = project.file_name().unwrap().to_string_lossy();
    assert!(
        text.starts_with(&format!("yoke watch \u{b7} {name} \u{b7} main")),
        "{text}"
    );
    // Allowed, would-deny, denied and errors today, then the last event time.
    let today = text
        .lines()
        .find(|l| l.starts_with("claude  today"))
        .unwrap();
    let fields: Vec<&str> = today.split_whitespace().collect();
    assert_eq!(
        fields[..6],
        ["claude", "today", "1", "1", "0", "1"],
        "{text}"
    );
    assert!(
        text.contains("would-deny  no-force-push     git push --force origin main"),
        "{text}"
    );
    assert!(text.contains("make lint"), "{text}");
    assert!(
        text.contains("One snapshot; run yoke watch in a terminal"),
        "{text}"
    );
    assert_eq!(files(&state), before);
}
