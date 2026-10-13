use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};
use tempfile::TempDir;
use yoke_core::{
    store::{DecisionFilter, Store},
    trust,
};

struct Fixture {
    _temp: TempDir,
    home: PathBuf,
    project: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let project = temp.path().join("project");
        fs::create_dir_all(home.join(".yoke")).unwrap();
        fs::create_dir(&project).unwrap();
        Self {
            _temp: temp,
            home,
            project,
        }
    }

    fn store(&self) -> Store {
        Store::open(self.home.join(".yoke/state.db")).unwrap()
    }

    fn snapshot(&self, contents: &str) {
        fs::write(self.project.join("yoke.toml"), contents).unwrap();
        let store = self.store();
        trust::prepare_project(&store, &self.project)
            .unwrap()
            .commit(&store)
            .unwrap();
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_yoke"));
        cmd.args(args)
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env_remove("YOKE_OFF")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }

    fn run(&self, args: &[&str], payload: &str) -> Output {
        let mut child = self.command(args).spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn hook(&self, agent: &str, event: &str, payload: Value) -> Output {
        self.run(&["run-hook", "--agent", agent, event], &payload.to_string())
    }
}

fn payload() -> Value {
    json!({"session_id":"fixture-session","tool_name":"Bash","tool_input":{"command":"echo test","token":"private-token-value"}})
}
fn success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn untrusted_and_observe_are_silent_then_enforce_uses_snapshot_and_redacted_log() {
    let f = Fixture::new();
    let deny =
        "[[gate]]\nname='review'\non='PreToolUse'\naction='deny'\nmessage='review required'\n";
    fs::write(f.project.join("yoke.toml"), deny).unwrap();
    let untrusted = f.hook("claude", "PreToolUse", payload());
    success(&untrusted);
    assert!(untrusted.stdout.is_empty());
    assert_eq!(
        f.store().decisions(&DecisionFilter::default()).unwrap()[0].outcome,
        "allow"
    );
    f.snapshot(deny);
    let observed = f.hook("codex", "PreToolUse", payload());
    success(&observed);
    assert!(observed.stdout.is_empty());
    assert_eq!(
        f.store().decisions(&DecisionFilter::default()).unwrap()[0].outcome,
        "would_deny"
    );
    f.snapshot(&format!("mode='enforce'\n{deny}"));
    fs::write(f.project.join("yoke.toml"), "mode='observe'").unwrap();
    for agent in ["claude", "codex"] {
        let blocked = f.hook(agent, "PreToolUse", payload());
        success(&blocked);
        assert_eq!(
            serde_json::from_slice::<Value>(&blocked.stdout).unwrap(),
            json!({"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"review required"}})
        );
    }
    let log = f.run(
        &[
            "log",
            "--json",
            "--session",
            "fixture-session",
            "--outcome",
            "deny",
            "--limit",
            "1",
        ],
        "",
    );
    success(&log);
    let rows: Vec<Value> = serde_json::from_slice(&log.stdout).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["agent"], "codex");
    assert!(!String::from_utf8_lossy(&log.stdout).contains("private-token-value"));
    let config = f.run(&["config", "--json"], "");
    success(&config);
    let config: Value = serde_json::from_slice(&config.stdout).unwrap();
    assert_eq!(config["config"]["mode"], "enforce");
    assert!(config["snapshots"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["stale"] == true));
}

#[test]
fn malformed_payload_and_store_failure_follow_mode_and_fallback_rules() {
    let f = Fixture::new();
    let observe = f.run(&["run-hook", "Stop"], "not-json");
    success(&observe);
    assert!(observe.stdout.is_empty());
    assert_eq!(
        f.store().decisions(&DecisionFilter::default()).unwrap()[0].outcome,
        "error"
    );
    f.snapshot("mode='enforce'");
    let enforce = f.run(&["run-hook", "--agent", "codex", "Stop"], "not-json");
    success(&enforce);
    let result: Value = serde_json::from_slice(&enforce.stdout).unwrap();
    assert_eq!(result["decision"], "block");
    assert!(result["reason"]
        .as_str()
        .unwrap()
        .contains("parse hook JSON"));
    // Make the database path unusable without touching any existing database.
    let broken = Fixture::new();
    fs::create_dir(broken.home.join(".yoke/state.db")).unwrap();
    let output = broken.hook("claude", "PreToolUse", payload());
    success(&output);
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["hookSpecificOutput"]
            ["permissionDecision"],
        "deny"
    );
    assert!(fs::read_to_string(broken.home.join(".yoke/error.log"))
        .unwrap()
        .contains("open state database"));
}

#[test]
fn off_exits_before_parse_or_state_and_trust_has_no_agent_bypass() {
    let f = Fixture::new();
    let off_home = f.home.join("does-not-exist");
    let output = f
        .command(&["run-hook", "--unknown"])
        .env("YOKE_OFF", "1")
        .env("HOME", &off_home)
        .output()
        .unwrap();
    success(&output);
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
    assert!(!off_home.exists());
    let refused = f
        .command(&["trust"])
        .env("CODEX_THREAD_ID", "test-agent")
        .output()
        .unwrap();
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stdout).contains("human outside an agent"));
    let flag = f.run(&["trust", "--yes"], "trust\n");
    assert_eq!(flag.status.code(), Some(2));
    assert!(!f.home.join(".yoke/state.db").exists());
}

#[test]
fn hook_uses_payload_cwd_and_preserves_permission_checks_on_rewrites() {
    let f = Fixture::new();
    f.snapshot("mode='enforce'\n[[gate]]\nname='rewrite'\non='pre_tool_use'\naction='rewrite'\nrewrite_pattern='test'\nrewrite_to='safe'");
    let mut event = payload();
    event["cwd"] = json!(f.project);
    for agent in ["claude", "codex"] {
        let mut child = f
            .command(&["run-hook", "--agent", agent, "PreToolUse"])
            .current_dir(&f.home)
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(event.to_string().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        success(&output);
        let wire: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            f.store().decisions(&DecisionFilter::default()).unwrap()[0].outcome,
            "rewrite"
        );
        assert_eq!(
            wire["hookSpecificOutput"]["updatedInput"]["command"],
            "echo safe"
        );
        assert_eq!(
            wire["hookSpecificOutput"]["updatedInput"]["token"],
            "private-token-value"
        );
        assert_eq!(
            wire["hookSpecificOutput"]["permissionDecision"],
            if agent == "codex" {
                json!("allow")
            } else {
                Value::Null
            }
        );
    }
    let before = f
        .store()
        .decisions(&DecisionFilter::default())
        .unwrap()
        .len();
    let mut closed = f.command(&["run-hook", "PreToolUse"]).spawn().unwrap();
    drop(closed.stdout.take());
    closed
        .stdin
        .take()
        .unwrap()
        .write_all(event.to_string().as_bytes())
        .unwrap();
    assert_eq!(closed.wait_with_output().unwrap().status.code(), Some(2));
    assert_eq!(
        f.store()
            .decisions(&DecisionFilter::default())
            .unwrap()
            .len(),
        before + 1
    );
    assert!(fs::read_to_string(f.home.join(".yoke/error.log"))
        .unwrap()
        .contains("writing hook output"));
    f.snapshot("[[gate]]\nname='rewrite'\non='PreToolUse'\naction='rewrite'\nrewrite_pattern='test'\nrewrite_to='safe'");
    let observed = f.hook("claude", "PreToolUse", event.clone());
    success(&observed);
    assert!(observed.stdout.is_empty());
    assert_eq!(
        f.store().decisions(&DecisionFilter::default()).unwrap()[0].outcome,
        "allow"
    );
    f.snapshot("mode='enforce'");
    event["hook_event_name"] = json!("Stop");
    let mut child = f
        .command(&["run-hook", "PreToolUse"])
        .current_dir(&f.home)
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(event.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    success(&output);
    assert!(
        !output.stdout.is_empty(),
        "payload-cwd enforce mode must deny mismatched events"
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["hookSpecificOutput"]
            ["permissionDecision"],
        "deny"
    );
}

#[test]
fn install_dry_run_roundtrip_and_agent_specific_events_are_isolated() {
    let f = Fixture::new();
    success(&f.run(&["init"], ""));
    let starter =
        yoke_core::Config::parse(&fs::read_to_string(f.project.join("yoke.toml")).unwrap())
            .unwrap();
    assert_eq!(starter.mode, yoke_core::config::Mode::Observe);
    let dry = f.run(&["install", "--agent", "claude,codex", "--dry-run"], "");
    success(&dry);
    assert!(!f.home.join(".claude").exists());
    assert!(!f.home.join(".codex").exists());
    let install = f.run(&["install", "--agent", "claude,codex"], "");
    success(&install);
    let codex: Value =
        serde_json::from_str(&fs::read_to_string(f.home.join(".codex/hooks.json")).unwrap())
            .unwrap();
    assert!(codex["hooks"]["SessionEnd"].is_array());
    assert!(codex["hooks"].get("Notification").is_none());
    let again = f.run(&["install"], "");
    success(&again);
    assert!(String::from_utf8_lossy(&again.stdout).contains("changes: 0"));
    let uninstall = f.run(&["uninstall"], "");
    success(&uninstall);
    assert!(!f.home.join(".claude/settings.json").exists());
    assert!(!f.home.join(".codex/hooks.json").exists());
    assert!(!f.home.join(".codex/config.toml").exists());
}

#[test]
fn lifecycle_logs_and_internal_gate_errors_deny_only_blocking_events() {
    let f = Fixture::new();
    // Claim the current update interval so this scenario never starts network work.
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    f.store()
        .set_kv("update.last_attempt", &timestamp.to_string())
        .unwrap();
    let start = f.hook(
        "codex",
        "SessionStart",
        json!({"session_id":"life","hook_event_name":"SessionStart"}),
    );
    success(&start);
    assert!(start.stdout.is_empty());
    let transcript = f.home.join("usage.jsonl");
    fs::write(&transcript, "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":150,\"cached_input_tokens\":30,\"cache_write_input_tokens\":20,\"output_tokens\":20}}}}\n").unwrap();
    let end = f.hook(
        "codex",
        "SessionEnd",
        json!({"session_id":"life","hook_event_name":"SessionEnd","transcript_path":transcript}),
    );
    success(&end);
    assert!(end.stdout.is_empty());
    let usage = yoke_core::usage::summaries(&f.store(), None, Some("life")).unwrap();
    assert_eq!(usage.len(), 1);
    assert_eq!(usage[0].input_tokens, 100);
    assert_eq!(usage[0].cache_read_tokens, 30);
    assert_eq!(usage[0].cache_write_tokens, 20);
    assert_eq!(usage[0].output_tokens, 20);
    assert!(usage[0].estimated_cost_usd.is_none());
    assert_eq!(
        f.store()
            .decisions(&DecisionFilter::default())
            .unwrap()
            .len(),
        2
    );
    f.snapshot("mode='enforce'\n[[gate]]\nname='missing-shell'\non='Stop'\nrun='true'\nshell='/definitely-not-a-yoke-test-shell'");
    let error = f.hook("claude", "Stop", json!({"session_id":"life"}));
    success(&error);
    assert_eq!(
        serde_json::from_slice::<Value>(&error.stdout).unwrap()["decision"],
        "block"
    );
    assert_eq!(
        f.store().decisions(&DecisionFilter::default()).unwrap()[0].outcome,
        "error"
    );
    f.snapshot("mode='enforce'\n[[gate]]\nname='advisory'\non='PostToolUse'\naction='deny'");
    let advisory = f.hook("claude", "PostToolUse", payload());
    success(&advisory);
    assert!(advisory.stdout.is_empty());
    let invalid = f.hook("claude", "Stop", json!({"hook_event_name":"PreToolUse"}));
    success(&invalid);
    assert_eq!(
        serde_json::from_slice::<Value>(&invalid.stdout).unwrap()["decision"],
        "block"
    );
}

#[test]
fn daily_update_child_is_check_only_off_and_throttled() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};
    let f = Fixture::new();
    let bin = f.home.join("bin");
    fs::create_dir(&bin).unwrap();
    let curl = bin.join("curl");
    // Local transport fixture only: no network or executable replacement.
    fs::write(&curl, "#!/bin/sh\nprintf '%s\\n' \"$YOKE_OFF\" >> \"$HOME/checks\"\nprintf '%s' '{\"tag_name\":\"v0.2.0\",\"draft\":false,\"prerelease\":false,\"assets\":[]}'\n").unwrap();
    fs::set_permissions(&curl, fs::Permissions::from_mode(0o755)).unwrap();
    for _ in 0..2 {
        let mut child = f
            .command(&["run-hook", "SessionStart"])
            .env("PATH", &bin)
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"{\"session_id\":\"daily\"}")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        success(&output);
        assert!(output.stdout.is_empty());
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if f.store()
            .get_kv("update.latest_version")
            .unwrap()
            .as_deref()
            == Some("v0.2.0")
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "daily check child did not record fixture version"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(fs::read_to_string(f.home.join("checks")).unwrap(), "1\n");
    assert_eq!(
        fs::metadata(&curl).unwrap().permissions().mode() & 0o777,
        0o755
    );
}
