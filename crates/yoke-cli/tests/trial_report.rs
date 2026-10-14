use std::fs;
use std::process::Command;

use serde_json::{json, Value};
use yoke_core::store::{DecisionRecord, Store};

fn report(home: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_yoke"))
        .args(["trial", "report"])
        .args(args)
        .env("HOME", home)
        .output()
        .unwrap()
}

#[test]
fn trial_report_reads_existing_state_and_never_creates_it() {
    let temp = tempfile::tempdir().unwrap();
    let missing = report(temp.path(), &["--json"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&missing.stdout).contains("no state at"));
    assert!(!temp.path().join(".yoke").exists());

    fs::create_dir(temp.path().join(".yoke")).unwrap();
    let path = temp.path().join(".yoke/state.db");
    let store = Store::open(&path).unwrap();
    for (ts, outcome) in [(1_791_800_000, "allow"), (1_791_800_060, "would_deny")] {
        store
            .record_decision(
                &DecisionRecord {
                    ts,
                    session_id: "trial".into(),
                    agent: "codex".into(),
                    project: "/trial".into(),
                    event: "pre_tool_use".into(),
                    tool: Some("Bash".into()),
                    mode: "observe".into(),
                    outcome: outcome.into(),
                    reason: Some("gate said no".into()),
                    gates: json!([]),
                    payload: "{}".into(),
                },
                4096,
            )
            .unwrap();
    }
    drop(store);
    fs::write(
        temp.path().join(".yoke/error.log"),
        "1791800030 yoke: audit decision was not saved: readonly\n",
    )
    .unwrap();
    let before = fs::read(&path).unwrap();
    let output = report(temp.path(), &["--json", "--agent", "codex"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["cohorts"][0]["decisions"], 2);
    assert_eq!(report["flagged_total"], 1);
    assert_eq!(report["incidents"][0]["kind"], "state_write_failed");
    assert_eq!(report["acceptance"].as_array().unwrap().len(), 4);
    assert_eq!(fs::read(&path).unwrap(), before);
}
