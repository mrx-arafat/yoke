use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{json, Value};
use yoke_core::{notifications, quota, store::Store, trust};

#[test]
fn codex_prompt_imports_live_quota_and_preserves_gate_decisions_without_repeat_alerts() {
    for mode in ["observe", "enforce"] {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let project = temp.path().join("project");
        std::fs::create_dir_all(home.join(".yoke")).unwrap();
        std::fs::create_dir(&project).unwrap();
        std::fs::write(project.join("yoke.toml"), format!("mode='{mode}'\n[update]\ncheck=false\n[usage]\nwarn_percent=75\n[[gate]]\nname='prompt-policy'\non='user_prompt_submit'\naction='deny'\nmessage='test policy'\n")).unwrap();
        let store = Store::open(home.join(".yoke/state.db")).unwrap();
        trust::prepare_project(&store, &project)
            .unwrap()
            .commit(&store)
            .unwrap();
        let clock = rusqlite::Connection::open_in_memory().unwrap();
        let now: i64 = clock
            .query_row("SELECT unixepoch()", [], |row| row.get(0))
            .unwrap();
        let transcript = temp.path().join("rollout.jsonl");
        let rows = [(now - 600, 20), (now, 80)].map(|(at, percent)| {
            let timestamp: String = clock.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%SZ',?1,'unixepoch')", [at], |row| row.get(0)).unwrap();
            json!({"timestamp":timestamp,"type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":percent,"window_minutes":300,"resets_at":now+3600}}}})
        });
        std::fs::write(
            &transcript,
            rows.iter()
                .map(|row| format!("{row}\n"))
                .collect::<String>(),
        )
        .unwrap();
        let hook = || {
            let mut child = Command::new(env!("CARGO_BIN_EXE_yoke"))
                .args(["run-hook", "--agent", "codex", "UserPromptSubmit"])
                .env("HOME", &home)
                .env_remove("YOKE_OFF")
                .env_remove("CODEX_HOME")
                .current_dir(&project)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let input = json!({"session_id":"quota-session","cwd":project,"hook_event_name":"UserPromptSubmit","transcript_path":transcript});
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.to_string().as_bytes())
                .unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success(), "{output:?}");
            if output.stdout.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&output.stdout).unwrap()
            }
        };
        let first = hook();
        assert!(first["systemMessage"].as_str().unwrap().contains("5h 80%"));
        assert_eq!(
            first["decision"].as_str(),
            (mode == "enforce").then_some("block")
        );
        let second = hook();
        assert!(second["systemMessage"].is_null());
        assert_eq!(
            second["decision"].as_str(),
            (mode == "enforce").then_some("block")
        );
        let saved = quota::load(&store, "codex").unwrap().unwrap();
        assert_eq!(saved.history.len(), 2);
        assert_eq!(
            saved.trend(&saved.windows[0], now).percent_per_hour,
            Some(360.0)
        );
        let notices = notifications::list(&store, &project, false, 100).unwrap();
        assert_eq!(
            notices.iter().filter(|n| n.kind == "usage_limit").count(),
            1
        );
        let report = Command::new(env!("CARGO_BIN_EXE_yoke"))
            .args(["usage", "--json"])
            .env("HOME", &home)
            .current_dir(&project)
            .output()
            .unwrap();
        assert!(report.status.success(), "{report:?}");
        let report: Value = serde_json::from_slice(&report.stdout).unwrap();
        assert_eq!(report["quota"][0]["percent_per_hour"], 360.0);
        assert_eq!(report["quota"][0]["stale"], false);
        assert_eq!(report["quota"][0]["history"].as_array().unwrap().len(), 2);
    }
}
