//! First-run guidance a user reads straight from the binary: what is ignored, what is broken,
//! and the command that fixes it.

use std::fs;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use yoke_core::store::Store;

fn yoke(home: &Path, dir: &Path, args: &[&str]) -> Output {
    // Never run against the real home: the isolated one must be a distinct, existing directory.
    assert!(home.is_dir() && Some(home.to_path_buf()) != std::env::home_dir());
    Command::new(env!("CARGO_BIN_EXE_yoke"))
        .args(args)
        .env("HOME", home)
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME")
        .current_dir(dir)
        .output()
        .unwrap()
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn init_names_the_project_after_its_directory_and_offers_memory() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let project = home.join("billing-api");
    fs::create_dir(&project).unwrap();

    let output = yoke(&home, &project, &["init"]);

    assert!(output.status.success(), "{}", text(&output));
    let written = fs::read_to_string(project.join("yoke.toml")).unwrap();
    assert!(written.contains("name = \"billing-api\""), "{written}");
    assert!(!written.contains("my-project"), "{written}");
    assert!(written.contains("# [memory]"), "{written}");
}

#[test]
fn empty_tables_say_the_file_is_ignored_and_the_bare_command_points_at_health() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let project = home.join("proj");
    fs::create_dir(&project).unwrap();
    assert!(yoke(&home, &project, &["init"]).status.success());

    for command in [&["gates"][..], &["config"]] {
        let output = text(&yoke(&home, &project, command));
        assert!(
            output.contains("is not trusted, so its settings and gates are ignored"),
            "{command:?}: {output}"
        );
        assert!(output.contains("run yoke trust"), "{command:?}: {output}");
    }
    let bare = text(&yoke(&home, &project, &[]));
    assert!(bare.contains("yoke health"), "{bare}");
}

#[test]
fn a_state_database_from_a_newer_yoke_is_reported_as_that_and_how_to_leave() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    fs::create_dir(home.join(".yoke")).unwrap();
    let path = home.join(".yoke/state.db");
    drop(Store::open(&path).unwrap());
    // Held open so the -wal and -shm files that read-only commands need stay in place.
    let db = rusqlite::Connection::open(&path).unwrap();
    db.pragma_update(None, "user_version", 99).unwrap();

    let health = text(&yoke(&home, &home, &["health"]));
    assert!(health.contains("newer than supported"), "{health}");
    assert!(health.contains("yoke uninstall"), "{health}");
    assert!(!health.contains("free disk space"), "{health}");

    let mut hook = Command::new(env!("CARGO_BIN_EXE_yoke"))
        .args(["run-hook", "--agent", "claude", "PreToolUse"])
        .env("HOME", &home)
        .current_dir(&home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(
        hook.stdin.as_mut().unwrap(),
        br#"{"session_id":"s","hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"ls"},"cwd":"/"}"#,
    )
    .unwrap();
    let output = hook.wait_with_output().unwrap();
    drop(db);
    let reason = text(&output);
    assert!(reason.contains("yoke uninstall"), "{reason}");
    assert!(!reason.contains("free disk space"), "{reason}");
}

#[test]
fn session_start_warns_only_about_policy_files_that_exist() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let project = home.join("proj");
    fs::create_dir(&project).unwrap();
    let start = |dir: &Path| {
        let mut hook = Command::new(env!("CARGO_BIN_EXE_yoke"))
            .args(["run-hook", "--agent", "claude", "SessionStart"])
            .env("HOME", &home)
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME")
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let payload = serde_json::json!({
            "session_id": "s",
            "hook_event_name": "SessionStart",
            "source": "startup",
            "cwd": dir,
        });
        std::io::Write::write_all(hook.stdin.as_mut().unwrap(), payload.to_string().as_bytes())
            .unwrap();
        text(&hook.wait_with_output().unwrap())
    };

    let nothing = start(&project);
    assert!(!nothing.contains("yoke error"), "{nothing}");
    assert!(!nothing.contains("untrusted (ignored)"), "{nothing}");

    fs::write(project.join("yoke.toml"), "mode = \"observe\"\n").unwrap();
    let present = start(&project);
    assert!(
        present.contains("yoke.toml is untrusted (ignored)"),
        "{present}"
    );
}
