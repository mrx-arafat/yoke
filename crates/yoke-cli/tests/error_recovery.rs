use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
};

use serde_json::json;
use yoke_core::{store::Store, trust};

#[test]
fn malformed_hook_payloads_name_repairs_without_changing_observe_or_enforce() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let home = root.join("home");
    let project = root.join("project");
    fs::create_dir_all(home.join(".yoke")).unwrap();
    fs::create_dir(&project).unwrap();
    let store = Store::open(home.join(".yoke/state.db")).unwrap();
    for mode in ["observe", "enforce"] {
        fs::write(
            project.join("yoke.toml"),
            format!("mode='{mode}'\n[update]\ncheck=false\n"),
        )
        .unwrap();
        trust::prepare_project(&store, &project)
            .unwrap()
            .commit(&store)
            .unwrap();
        for payload in [
            json!({"cwd":"relative","session_id":"w4-errors"}),
            json!({"cwd":project,"session_id":"w4-errors","hook_event_name":"Stop"}),
        ] {
            assert!(home.is_dir() && home.starts_with(&root));
            let mut child = Command::new(env!("CARGO_BIN_EXE_yoke"))
                .args(["run-hook", "PreToolUse", "--agent", "codex"])
                .env("HOME", &home)
                .env_remove("CODEX_HOME")
                .env_remove("CLAUDE_CONFIG_DIR")
                .current_dir(&project)
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
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success(), "{output:?}");
            let diagnostic = String::from_utf8_lossy(&output.stderr);
            assert!(
                diagnostic.contains("repair the hook"),
                "{mode}: {diagnostic}"
            );
            if mode == "observe" {
                assert!(output.stdout.is_empty(), "{output:?}");
            } else {
                let wire: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(wire["hookSpecificOutput"]["permissionDecision"], "deny");
            }
        }
    }
}
