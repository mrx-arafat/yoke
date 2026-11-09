use std::path::Path;
use std::process::{Command, Stdio};

use yoke_core::store::Store;

fn decisions(path: &Path) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row("SELECT count(*) FROM decisions", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn backup_restore_refuses_agents_and_piped_input_without_touching_state() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    assert_ne!(Some(home.clone()), std::env::home_dir());
    std::fs::create_dir(home.join(".yoke")).unwrap();
    let live = home.join(".yoke/state.db");
    let store = Store::open(&live).unwrap();
    let backups = yoke_core::backup::directory(&home);
    let made = yoke_core::backup::create(&store, &backups, 1_794_200_000).unwrap();
    rusqlite::Connection::open(&live)
        .unwrap()
        .execute_batch(
            "INSERT INTO sessions(id,agent,project,started_at) VALUES ('s','codex','/p',1);
             INSERT INTO decisions(ts,session_id,agent,project,event,mode,outcome,gates,payload)
             VALUES (2,'s','codex','/p','pre_tool_use','observe','allow','[]','{}');",
        )
        .unwrap();
    drop(store);
    let restore = |agent: Option<&str>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_yoke"));
        command
            .args(["backup", "restore", &made.name])
            .env("HOME", &home)
            .current_dir(&home)
            .env_remove("CLAUDECODE")
            .env_remove("CLAUDE_CODE_CHILD_SESSION")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("CODEX_SESSION_ID")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(thread) = agent {
            command.env("CODEX_THREAD_ID", thread);
        }
        let mut child = command.spawn().unwrap();
        // Typing the confirmation into a pipe must not count as a human.
        use std::io::Write;
        child.stdin.take().unwrap().write_all(b"restore\n").unwrap();
        child.wait_with_output().unwrap()
    };

    let piped = restore(None);
    assert_eq!(piped.status.code(), Some(1));
    let text = String::from_utf8_lossy(&piped.stdout);
    assert!(text.contains("interactive human terminal"), "{text}");
    let agent = restore(Some("thread-1"));
    assert_eq!(agent.status.code(), Some(1));
    let text = String::from_utf8_lossy(&agent.stdout);
    assert!(
        text.contains("agent tool process (CODEX_THREAD_ID)"),
        "{text}"
    );
    assert_eq!(
        decisions(&live),
        1,
        "the live state keeps its newer decision"
    );
    assert_eq!(
        yoke_core::backup::list(&backups).unwrap().len(),
        1,
        "no safety backup was written"
    );
}
