use std::path::Path;
use std::process::{Command, Output};

use yoke_core::store::Store;

fn yoke(home: &Path, command: &str) -> Output {
    assert!(home.is_dir() && Some(home.to_path_buf()) != std::env::home_dir());
    Command::new(env!("CARGO_BIN_EXE_yoke"))
        .arg(command)
        .env("HOME", home)
        .current_dir(home)
        .output()
        .unwrap()
}

#[test]
fn notifications_hide_long_check_secrets_and_refuse_noninteractive_ack() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    std::fs::create_dir(home.join(".yoke")).unwrap();
    let path = home.join(".yoke/state.db");
    drop(Store::open(&path).unwrap());
    let db = rusqlite::Connection::open(&path).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    db.execute(
        "INSERT INTO plans(id,project,slug,file,state,created_at,updated_at) VALUES(1,?1,'ship','p.md','building',?2,?2)",
        rusqlite::params![home.to_str().unwrap(), now],
    ).unwrap();
    db.execute(
        "INSERT INTO check_runs(plan_id,ts,name,command,output_tail,fingerprint,blocking,ms,result) VALUES(1,?1,'unit','test',?2,'f',1,3,'fail')",
        rusqlite::params![now, format!("API_KEY={}", "x".repeat(350))],
    ).unwrap();
    let list = || {
        Command::new(env!("CARGO_BIN_EXE_yoke"))
            .args(["notifications", "--json"])
            .env("HOME", &home)
            .current_dir(&home)
            .output()
            .unwrap()
    };
    let first = list();
    assert!(first.status.success());
    let rows: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    let id = rows[0]["id"].as_i64().unwrap();
    let ack = Command::new(env!("CARGO_BIN_EXE_yoke"))
        .args(["notifications", "ack", &id.to_string()])
        .env("HOME", &home)
        .current_dir(&home)
        .output()
        .unwrap();
    assert_eq!(ack.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&ack.stdout).contains("requires"));
    let second = list();
    assert!(second.status.success());
    assert_eq!(
        first.stdout, second.stdout,
        "listing and refused ack preserve state"
    );
    assert_eq!(rows[0]["state"], "unread");
    assert_eq!(rows[0]["count"], 1);
    assert!(!rows[0]["detail"]
        .as_str()
        .unwrap()
        .contains(&"x".repeat(20)));
    assert!(rows[0]["detail"].as_str().unwrap().contains("[REDACTED]"));
}

#[test]
fn notifications_cli_isolates_projects_with_identical_redacted_paths() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let alpha = home.join("ghp_alpha");
    let beta = home.join("ghp_beta");
    for path in [&alpha, &beta, &home.join(".yoke")] {
        std::fs::create_dir(path).unwrap();
    }
    let path = home.join(".yoke/state.db");
    drop(Store::open(&path).unwrap());
    let db = rusqlite::Connection::open(&path).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    for (id, project) in [(1, &alpha), (2, &beta)] {
        db.execute(
            "INSERT INTO plans(id,project,slug,file,state,created_at,updated_at) VALUES(?1,?2,'ship','p.md','waiting',?3,?3)",
            rusqlite::params![id, project.to_str().unwrap(), now],
        ).unwrap();
    }
    for (id, project) in [(1, &alpha), (2, &beta)] {
        let output = Command::new(env!("CARGO_BIN_EXE_yoke"))
            .args(["notifications", "--json"])
            .env("HOME", &home)
            .current_dir(project)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let rows: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert_eq!(rows[0]["plan_id"], id);
        assert_eq!(
            rows[0]["project"],
            yoke_core::redact::redact(project.to_str().unwrap())
        );
        let shown = String::from_utf8(output.stdout).unwrap();
        assert!(
            !shown.contains("sha256:")
                && !shown.contains("ghp_alpha")
                && !shown.contains("ghp_beta")
        );
    }
}

#[test]
fn listing_commands_never_create_or_migrate_state() {
    for (command, listed) in [("notifications", "unread: 0"), ("jobs", "jobs[0]")] {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let missing = yoke(&home, command);
        assert!(missing.status.success(), "{command}");
        assert!(String::from_utf8_lossy(&missing.stdout).contains("No Yoke state exists yet"));
        assert!(!home.join(".yoke").exists(), "{command} created state");

        std::fs::create_dir(home.join(".yoke")).unwrap();
        let path = home.join(".yoke/state.db");
        drop(Store::open(&path).unwrap());
        // Holding a connection keeps the WAL files a read-only open needs.
        let db = rusqlite::Connection::open(&path).unwrap();
        let version = |db: &rusqlite::Connection| -> i64 {
            db.pragma_query_value(None, "user_version", |row| row.get(0))
                .unwrap()
        };
        let current = version(&db);
        db.pragma_update(None, "user_version", 6).unwrap();
        let older = yoke(&home, command);
        assert_eq!(older.status.code(), Some(1), "{command}");
        assert!(String::from_utf8_lossy(&older.stdout).contains("does not upgrade it"));
        assert_eq!(version(&db), 6, "{command} must not migrate");

        db.pragma_update(None, "user_version", current).unwrap();
        let listed_output = yoke(&home, command);
        assert!(listed_output.status.success(), "{command}");
        assert!(String::from_utf8_lossy(&listed_output.stdout).contains(listed));
    }
}

#[test]
fn empty_state_file_is_reported_without_blaming_hook_versions() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    std::fs::create_dir(home.join(".yoke")).unwrap();
    let path = home.join(".yoke/state.db");
    std::fs::write(&path, b"").unwrap();
    for command in ["notifications", "jobs", "extensions"] {
        let output = yoke(&home, command);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(output.status.code(), Some(1), "{command}: {stdout}");
        assert!(
            stdout.contains("empty or incomplete"),
            "{command}: {stdout}"
        );
        assert!(!stdout.contains("older yoke") && !stdout.contains("no such table"));
    }
    assert_eq!(
        std::fs::metadata(&path).unwrap().len(),
        0,
        "nothing was written"
    );
}

#[test]
fn one_listing_reads_a_bounded_slice_and_says_more_is_pending() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    std::fs::create_dir(home.join(".yoke")).unwrap();
    let path = home.join(".yoke/state.db");
    drop(Store::open(&path).unwrap());
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(
        "INSERT INTO sessions(id,agent,project,started_at) VALUES('s','codex','/p',0);
         WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<60000)
         INSERT INTO decisions(ts,session_id,agent,project,event,tool,mode,outcome,reason,gates,payload)
         SELECT strftime('%s','now'),'s','codex','/p','pre_tool_use','Bash','enforce',
                CASE WHEN i%2 THEN 'error' ELSE 'allow' END,'disk full','[]','{}' FROM n;",
    )
    .unwrap();
    // `db` stays open: closing the last connection would delete the WAL files a read-only
    // open needs.
    let first = yoke(&home, "notifications");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stdout)
    );
    assert!(String::from_utf8_lossy(&first.stdout).contains("pending: true"));
    // Every run reads at least one 2,000-id window, so 60,000 ids need at most 30 runs.
    for run in 2..=30 {
        let next = yoke(&home, "notifications");
        let stdout = String::from_utf8_lossy(&next.stdout);
        assert!(next.status.success(), "{stdout}");
        if !stdout.contains("pending: true") {
            drop(db);
            return;
        }
        assert!(run < 30, "still pending after {run} runs");
    }
}

#[test]
fn statusline_refuses_a_relative_home() {
    let temp = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_yoke"))
        .arg("statusline")
        .env("HOME", "relative-home")
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    // The agent paints stdout in its prompt bar, so the reason is on stderr only.
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("HOME must be an absolute path"));
}

#[test]
fn statusline_treats_an_empty_state_file_like_missing_state() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let statusline = || {
        assert!(home.is_dir() && Some(home.clone()) != std::env::home_dir());
        Command::new(env!("CARGO_BIN_EXE_yoke"))
            .arg("statusline")
            .env("HOME", &home)
            .current_dir(&home)
            .output()
            .unwrap()
    };
    let missing = statusline();
    assert!(missing.status.success());
    std::fs::create_dir(home.join(".yoke")).unwrap();
    std::fs::write(home.join(".yoke/state.db"), b"").unwrap();
    let empty = statusline();
    assert!(
        empty.status.success(),
        "{}",
        String::from_utf8_lossy(&empty.stdout)
    );
    assert_eq!(empty.stdout, missing.stdout);
    assert_eq!(
        std::fs::metadata(home.join(".yoke/state.db"))
            .unwrap()
            .len(),
        0
    );
}
