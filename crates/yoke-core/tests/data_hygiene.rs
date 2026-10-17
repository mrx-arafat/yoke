//! What Yoke writes to disk: who can read it, and what it keeps of a tool's output.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use yoke_core::store::{DecisionFilter, DecisionRecord, Store};

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn set_mode(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn the_state_directory_database_sidecars_and_logs_are_owner_only() {
    let home = tempfile::tempdir().unwrap();
    let state = home.path().join(".yoke");
    let db = state.join("state.db");
    // `create_dir_all` under a 022 umask, as the CLI does before opening the store.
    std::fs::create_dir(&state).unwrap();
    set_mode(&state, 0o755);

    let store = Store::open(&db).unwrap();
    assert_eq!(mode(&state), 0o700);
    for name in ["state.db", "state.db-wal", "state.db-shm"] {
        assert_eq!(mode(&state.join(name)), 0o600, "{name}");
    }
    drop(store);

    // An installation made before this change: everything readable by others. The sidecars
    // persist after the last connection closes, so they are widened too.
    std::fs::write(state.join("error.log"), "1 old failure\n").unwrap();
    for name in ["state.db", "state.db-wal", "state.db-shm", "error.log"] {
        set_mode(&state.join(name), 0o644);
    }
    set_mode(&state, 0o755);
    let copy = tempfile::tempdir().unwrap();
    let copied = copy.path().join(".yoke");
    std::fs::create_dir(&copied).unwrap();
    for entry in std::fs::read_dir(&state).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), copied.join(entry.file_name())).unwrap();
    }
    set_mode(&copied, 0o755);
    for name in ["state.db", "state.db-wal", "state.db-shm", "error.log"] {
        set_mode(&copied.join(name), 0o644);
    }

    let _store = Store::open(copied.join("state.db")).unwrap();
    assert_eq!(mode(&copied), 0o700);
    for name in ["state.db", "state.db-wal", "state.db-shm", "error.log"] {
        assert_eq!(mode(&copied.join(name)), 0o600, "{name}");
    }
    // The original was not the copy that was opened.
    assert_eq!(mode(&state), 0o755);
}

#[test]
fn private_helpers_create_owner_only_paths_and_never_fail_on_a_missing_one() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("a/b/.yoke");
    yoke_core::private::create_dir_all(&nested).unwrap();
    assert_eq!(mode(&nested), 0o700);
    assert_eq!(mode(&dir.path().join("a")), 0o700);

    let log = nested.join("error.log");
    drop(yoke_core::private::open_append(&log).unwrap());
    assert_eq!(mode(&log), 0o600);
    set_mode(&log, 0o644);
    drop(yoke_core::private::open_append(&log).unwrap());
    assert_eq!(mode(&log), 0o600);

    yoke_core::private::tighten(&dir.path().join("does-not-exist"));
}

#[test]
fn the_decision_log_keeps_the_size_of_a_tool_response_not_its_content() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("state.db")).unwrap();
    let payload = serde_json::json!({
        "session_id": "s",
        "tool_name": "Read",
        "tool_input": {"file_path": "/work/people.csv"},
        "tool_response": {"content": "Ada Lovelace ada@example.com", "interrupted": false},
    })
    .to_string();
    store
        .record_decision(
            &DecisionRecord {
                ts: 1,
                session_id: "s".into(),
                agent: "claude".into(),
                project: "/work".into(),
                event: "post_tool_use".into(),
                tool: Some("Read".into()),
                mode: "observe".into(),
                outcome: "allow".into(),
                reason: None,
                gates: serde_json::json!([]),
                payload,
            },
            4096,
        )
        .unwrap();
    let stored = store.decisions(&DecisionFilter::default()).unwrap();
    let kept = &stored[0].payload;
    assert!(
        !kept.contains("Ada") && !kept.contains("example.com"),
        "{kept}"
    );
    assert!(
        kept.contains("/work/people.csv") && kept.contains("\"bytes\""),
        "{kept}"
    );
}
