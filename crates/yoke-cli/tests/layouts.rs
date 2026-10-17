//! Yoke in the project layouts people really have: where `init` writes and what a hook does
//! when the directory an agent reports is gone.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use yoke_core::{store::Store, trust};

fn yoke(home: &Path, dir: &Path, args: &[&str]) -> Command {
    // Never run against the real home: the isolated one must be a distinct, existing directory.
    assert!(home.is_dir() && Some(home.to_path_buf()) != std::env::home_dir());
    let mut command = Command::new(env!("CARGO_BIN_EXE_yoke"));
    command
        .args(args)
        .env("HOME", home)
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME")
        .env_remove("YOKE_OFF")
        .current_dir(dir);
    command
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn repository(root: &Path) {
    fs::create_dir_all(root).unwrap();
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .status()
        .unwrap();
    assert!(status.success());
}

fn hook(home: &Path, dir: &Path, payload: &serde_json::Value) -> Output {
    let mut child = yoke(home, dir, &["run-hook", "PreToolUse"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn init_in_a_repository_subdirectory_writes_one_config_at_the_repository_root() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap().join("home");
    let repo = temp.path().canonicalize().unwrap().join("billing-api");
    fs::create_dir(&home).unwrap();
    repository(&repo);
    let sub = repo.join("pkg/sub");
    fs::create_dir_all(&sub).unwrap();

    let first = yoke(&home, &sub, &["init"]).output().unwrap();

    assert!(first.status.success(), "{}", text(&first));
    assert!(
        text(&first).contains(&repo.display().to_string()),
        "{}",
        text(&first)
    );
    let written = fs::read_to_string(repo.join("yoke.toml")).unwrap();
    assert!(written.contains("name = \"billing-api\""), "{written}");
    assert!(
        !sub.join("yoke.toml").exists(),
        "init forked a nested project"
    );

    let again = yoke(&home, &sub, &["init"]).output().unwrap();
    assert!(again.status.success(), "{}", text(&again));
    assert!(text(&again).contains("already exists"), "{}", text(&again));
    assert!(
        text(&again).contains(&repo.display().to_string()),
        "{}",
        text(&again)
    );
    assert!(!sub.join("yoke.toml").exists());

    // `yoke config` names the project it resolved, the same one from every subdirectory.
    let config = yoke(&home, &sub, &["config"]).output().unwrap();
    let expected = format!("project: \"{}\"", repo.display());
    assert!(text(&config).starts_with(&expected), "{}", text(&config));
}

#[test]
fn a_hook_whose_reported_directory_is_gone_still_applies_the_policy_it_runs_under() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let home = root.join("home");
    let project: PathBuf = root.join("project");
    fs::create_dir_all(home.join(".yoke")).unwrap();
    fs::create_dir(&project).unwrap();
    fs::write(
        project.join("yoke.toml"),
        "mode='enforce'\n[[gate]]\nname='no-secret-word'\non='pre_tool_use'\nmatch_tool='Bash'\nmatch_contains='secret-word'\naction='deny'\nmessage='that word is blocked'\n",
    )
    .unwrap();
    let store = Store::open(home.join(".yoke/state.db")).unwrap();
    trust::prepare_project(&store, &project)
        .unwrap()
        .commit(&store)
        .unwrap();
    drop(store);
    let gone = root.join("renamed-away");
    let event = |command: &str| {
        serde_json::json!({
            "session_id": "s1",
            "tool_name": "Bash",
            "tool_input": {"command": command},
            "cwd": gone,
        })
    };

    let allowed = hook(&home, &project, &event("echo hello"));
    assert!(allowed.status.success(), "{}", text(&allowed));
    assert!(allowed.stdout.is_empty(), "{}", text(&allowed));

    let denied = hook(&home, &project, &event("echo secret-word"));
    let stdout = String::from_utf8_lossy(&denied.stdout);
    assert!(
        stdout.contains("\"permissionDecision\":\"deny\""),
        "{}",
        text(&denied)
    );
    assert!(stdout.contains("that word is blocked"), "{}", text(&denied));
}
