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
fn readonly_state_preserves_trusted_tool_decisions_without_audit_writes() {
    use std::os::unix::fs::PermissionsExt;

    for (agent, schema_six) in [
        ("claude", false),
        ("codex", false),
        ("claude", true),
        ("codex", true),
    ] {
        let f = Fixture::new();
        f.snapshot("mode='enforce'\n[[gate]]\nname='review'\non='PreToolUse'\naction='deny'\nmatch_tool='Bash'\nmessage='review required'\n[[gate]]\nname='rewrite'\non='PreToolUse'\naction='rewrite'\nmatch_tool='exec_command'\nrewrite_pattern='test'\nrewrite_to='safe'");
        // A disk edit must never replace approved bytes during recovery.
        fs::write(f.project.join("yoke.toml"), "mode='observe'").unwrap();
        for args in [
            &["init", "-q"][..],
            &[
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "base",
            ],
        ] {
            assert!(Command::new("git")
                .args(args)
                .current_dir(&f.project)
                .status()
                .unwrap()
                .success());
        }
        // A second session owns a building plan, whose stop progress must be persisted.
        let key = f.project.canonicalize().unwrap();
        let writer = rusqlite::Connection::open(f.home.join(".yoke/state.db")).unwrap();
        writer.execute("INSERT INTO plans(id,project,slug,file,state,created_at,updated_at) VALUES(1,?1,'demo','plan.md','building',1,1)", [key.to_str()]).unwrap();
        writer.execute("INSERT INTO sessions(id,agent,project,started_at,active_plan) VALUES('planned-session',?1,?2,1,1)", [agent, key.to_str().unwrap()]).unwrap();
        if schema_six {
            // An upgraded binary on a full disk meets the older schema the real state still has.
            writer.pragma_update(None, "foreign_keys", false).unwrap();
            writer
                .execute_batch(include_str!(
                    "../../yoke-core/src/store/008-plan-lifecycle.down.sql"
                ))
                .unwrap();
            writer
                .execute_batch(include_str!(
                    "../../yoke-core/src/store/007-plan-base-ref.down.sql"
                ))
                .unwrap();
        }
        let paths = ["state.db", "state.db-wal", "state.db-shm"]
            .map(|name| f.home.join(".yoke").join(name));
        for path in &paths {
            fs::set_permissions(path, fs::Permissions::from_mode(0o444)).unwrap();
        }
        let outputs = ["request_user_input", "spawn_agent", "Bash", "exec_command"].map(|tool| {
            let mut event = payload();
            event["tool_name"] = json!(tool);
            event["cwd"] = json!(f.project);
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
            child.wait_with_output().unwrap()
        });
        let stopped = f.hook(agent, "Stop", json!({"session_id":"fixture-session"}));
        let planned = f.hook(agent, "Stop", json!({"session_id":"planned-session"}));
        let prompted = f.hook(
            agent,
            "UserPromptSubmit",
            json!({"session_id":"planned-session","prompt":"continue"}),
        );
        for path in &paths {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let saved: i64 = writer
            .query_row("SELECT count(*) FROM decisions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(saved, 0);
        // The open writer kept the WAL sidecars in place without migrating the fixture.
        drop(writer);
        for output in &outputs {
            success(output);
        }
        assert!(
            outputs[0].stdout.is_empty(),
            "{}",
            String::from_utf8_lossy(&outputs[0].stdout)
        );
        assert!(outputs[1].stdout.is_empty());
        let denial: Value = serde_json::from_slice(&outputs[2].stdout).unwrap();
        assert_eq!(denial["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(
            denial["hookSpecificOutput"]["permissionDecisionReason"],
            "review required"
        );
        let rewrite: Value = serde_json::from_slice(&outputs[3].stdout).unwrap();
        assert_eq!(
            rewrite["hookSpecificOutput"]["updatedInput"]["command"],
            "echo safe"
        );
        assert_eq!(
            rewrite["hookSpecificOutput"]["permissionDecision"],
            if agent == "codex" {
                json!("allow")
            } else {
                Value::Null
            }
        );
        for output in &outputs {
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("audit decision was not saved"),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        // Without plan state to update, only the audit write fails and the allowed Stop stands.
        success(&stopped);
        assert!(
            stopped.stdout.is_empty(),
            "{}",
            String::from_utf8_lossy(&stopped.stdout)
        );
        assert!(String::from_utf8_lossy(&stopped.stderr).contains("stop audit was not saved"));
        // Plan context is advisory: an unreadable plan lookup never denies the prompt.
        success(&prompted);
        assert!(
            !String::from_utf8_lossy(&prompted.stdout).contains("block"),
            "{}",
            String::from_utf8_lossy(&prompted.stdout)
        );
        assert_eq!(
            String::from_utf8_lossy(&prompted.stderr).contains("plan context unavailable"),
            schema_six
        );
        success(&planned);
        let blocked: Value = serde_json::from_slice(&planned.stdout).unwrap();
        assert_eq!(blocked["decision"], "block");
        assert!(
            blocked["reason"]
                .as_str()
                .unwrap()
                .starts_with("yoke error"),
            "{blocked}"
        );
    }
}

#[test]
fn writer_lock_uses_committed_trust_without_bypassing_denials() {
    let f = Fixture::new();
    f.snapshot("mode='enforce'\n[[gate]]\nname='review'\non='PreToolUse'\naction='deny'\nmatch_tool='Bash'\nmessage='review required'");
    let writer = rusqlite::Connection::open(f.home.join(".yoke/state.db")).unwrap();
    // Uncommitted policy changes must not leak through the read-only fallback.
    writer
        .execute_batch("BEGIN IMMEDIATE; DELETE FROM trust WHERE scope='project';")
        .unwrap();
    let mut event = payload();
    event["tool_name"] = json!("request_user_input");
    let allowed = f.hook("codex", "PreToolUse", event);
    let denied = f.hook("codex", "PreToolUse", payload());
    writer.execute_batch("ROLLBACK").unwrap();
    success(&allowed);
    assert!(
        allowed.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&allowed.stdout)
    );
    success(&denied);
    let denial: Value = serde_json::from_slice(&denied.stdout).unwrap();
    assert_eq!(denial["hookSpecificOutput"]["permissionDecision"], "deny");
    assert_eq!(
        denial["hookSpecificOutput"]["permissionDecisionReason"],
        "review required"
    );
    for output in [&allowed, &denied] {
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(
            diagnostic.contains("writable state unavailable"),
            "{diagnostic}"
        );
        assert!(
            diagnostic.contains("audit decision was not saved"),
            "{diagnostic}"
        );
    }
    assert!(f
        .store()
        .decisions(&DecisionFilter::default())
        .unwrap()
        .is_empty());
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
    for path in [".claude/settings.json", ".codex/hooks.json"] {
        let settings: Value =
            serde_json::from_str(&fs::read_to_string(f.home.join(path)).unwrap()).unwrap();
        for (event, groups) in settings["hooks"].as_object().unwrap() {
            assert_eq!(
                groups[0]["hooks"][0]["statusMessage"],
                format!("Yoke: {event}")
            );
        }
    }
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
fn stop_with_transcript_over_a_usage_bound_warns_and_keeps_the_policy_decision() {
    let f = Fixture::new();
    const MIB: usize = 1024 * 1024;
    let transcripts = [
        ("long-line", format!("{}\n", " ".repeat(MIB + 1))),
        (
            "total-size",
            format!("{}\n", " ".repeat(MIB / 2 - 1)).repeat(65),
        ),
    ];
    for (policy, expected) in [
        ("mode='enforce'", None),
        (
            "mode='enforce'\n[[gate]]\nname='hold'\non='Stop'\naction='deny'\nmessage='hold the stop'",
            Some("hold the stop"),
        ),
    ] {
        f.snapshot(policy);
        for (name, contents) in &transcripts {
            let transcript = f.home.join(name);
            fs::write(&transcript, contents).unwrap();
            let stop = f.hook(
                "claude",
                "Stop",
                json!({"session_id":"large","transcript_path":transcript}),
            );
            success(&stop);
            assert_eq!(
                String::from_utf8_lossy(&stop.stderr).contains("usage ingestion failed"),
                *name == "total-size",
                "{name}"
            );
            match expected {
                None => assert!(stop.stdout.is_empty(), "{name} must not block an allowed Stop"),
                Some(reason) => {
                    let wire: Value = serde_json::from_slice(&stop.stdout).unwrap();
                    assert_eq!(wire["decision"], "block");
                    assert_eq!(wire["reason"], reason, "{name} must not replace the gate decision");
                }
            }
        }
    }
    // Compaction and session end run on the longest transcripts; accounting never errors them.
    f.snapshot("mode='enforce'");
    for event in ["PreCompact", "SessionEnd"] {
        for (name, contents) in &transcripts {
            let transcript = f.home.join(name);
            fs::write(&transcript, contents).unwrap();
            let output = f.hook(
                "claude",
                event,
                json!({"session_id":"large","transcript_path":transcript}),
            );
            success(&output);
            assert!(output.stdout.is_empty(), "{event} {name}");
            assert_eq!(
                String::from_utf8_lossy(&output.stderr).contains("usage ingestion failed"),
                *name == "total-size",
                "{event} {name}"
            );
        }
    }
    let decisions = f
        .store()
        .decisions(&DecisionFilter {
            session: Some("large".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(decisions.len(), 8);
    assert!(
        decisions.iter().all(|decision| decision.outcome != "error"),
        "{decisions:?}"
    );
    assert!(yoke_core::usage::summaries(&f.store(), None, Some("large"))
        .unwrap()
        .is_empty());
}

#[test]
fn session_start_prunes_expired_history_in_a_bounded_pass() {
    let f = Fixture::new();
    f.snapshot("[log]\nretention_days=1");
    // History at ts=1 is older than any retention window. Retention must keep a pinned
    // memory and the check run of an unfinished plan whatever their age.
    let db = rusqlite::Connection::open(f.home.join(".yoke/state.db")).unwrap();
    db.execute_batch(
        "INSERT INTO sessions(id,agent,project,started_at) VALUES ('old','claude','/p',1);
         INSERT INTO decisions(ts,session_id,agent,project,event,mode,outcome,gates,payload) VALUES (1,'old','claude','/p','stop','observe','allow','[]','{}');
         INSERT INTO facts(id,ts,session_id,project,kind,detail) VALUES (1,1,'old','/p','edit','expired');
         INSERT INTO memories(id,ts,project,source,text,pinned) VALUES (1,1,'/p','fact','pinned lesson',1);
         INSERT INTO plans(id,project,slug,file,state,created_at,updated_at) VALUES
            (1,'/p','done','a.md','done',1,1),(2,'/p','open','b.md','building',1,1);
         INSERT INTO check_runs(id,plan_id,ts,name,command,output_tail,fingerprint,blocking,ms,result) VALUES
            (1,1,1,'t','c','','f',1,1,'pass'),(2,2,1,'t','c','','f',1,1,'pass');",
    )
    .unwrap();
    let start = f.hook(
        "claude",
        "SessionStart",
        json!({"session_id":"fresh","hook_event_name":"SessionStart"}),
    );
    success(&start);
    let ids = |sql: &str| -> Vec<i64> {
        let mut statement = db.prepare(sql).unwrap();
        let rows = statement.query_map([], |row| row.get(0)).unwrap();
        rows.collect::<rusqlite::Result<_>>().unwrap()
    };
    assert_eq!(ids("SELECT count(*) FROM decisions WHERE ts<2"), vec![0]);
    assert_eq!(ids("SELECT count(*) FROM facts WHERE ts<2"), vec![0]);
    assert_eq!(ids("SELECT pinned FROM memories WHERE id=1"), vec![1]);
    assert_eq!(ids("SELECT id FROM check_runs ORDER BY id"), vec![2]);
    let fresh = f
        .store()
        .decisions(&DecisionFilter {
            session: Some("fresh".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(fresh.len(), 1, "{fresh:?}");
    assert_eq!(fresh[0].outcome, "allow");
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

#[test]
fn completed_plan_reopens_when_its_tree_changes_and_a_fresh_check_completes_it_again() {
    use yoke_core::plan;
    let f = Fixture::new();
    fs::write(f.project.join("tracked"), "first\n").unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args([
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.invalid",
            ])
            .args(args)
            .current_dir(&f.project)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap()
    };
    git(&["init", "-q"]);
    git(&["add", "tracked"]);
    git(&["commit", "-qm", "base"]);
    f.snapshot("mode='enforce'\n");
    let session =
        json!({"session_id":"journey","tool_name":"Bash","tool_input":{"command":"true"}});
    success(&f.hook("codex", "PostToolUse", session));
    success(&f.run(&["plan", "new", "journey"], ""));
    let store = f.store();
    let file = plan::get(&store, &f.project, "journey").unwrap().file;
    fs::write(
        &file,
        "+++\ntitle='Journey'\n[[check]]\nname='tracked'\nrun='grep -q . tracked'\n+++\n",
    )
    .unwrap();
    // Approval is human-only on the CLI; the fixture uses the same exact-header API.
    plan::approve(
        &store,
        &plan::prepare_approval(&store, &f.project, "journey").unwrap(),
    )
    .unwrap();
    success(&f.run(&["plan", "start", "journey", "--session", "journey"], ""));
    let check = || {
        let output = f.run(&["plan", "check", "journey", "--agent", "codex"], "");
        success(&output);
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    let stop = || f.hook("codex", "Stop", json!({"session_id":"journey"}));
    // Checked work: one edited tracked file and one new file, both uncommitted.
    fs::write(f.project.join("tracked"), "checked work\n").unwrap();
    fs::write(f.project.join("added.txt"), "new file\n").unwrap();
    assert_eq!(check()["plan"]["state"], "done");
    let unchanged = stop();
    success(&unchanged);
    assert!(
        unchanged.stdout.is_empty(),
        "an unchanged completed plan must be accepted"
    );
    // Committing exactly the checked bytes keeps the plan done, and Stop writes no git objects.
    git(&["add", "-A"]);
    git(&["commit", "-qm", "checked work"]);
    let objects = git(&["count-objects", "-v"]);
    let committed = stop();
    success(&committed);
    assert!(
        committed.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&committed.stdout)
    );
    assert_eq!(git(&["count-objects", "-v"]), objects);

    fs::write(f.project.join("tracked"), "edited after completion\n").unwrap();
    let stale = stop();
    success(&stale);
    let blocked: Value = serde_json::from_slice(&stale.stdout).unwrap();
    assert_eq!(blocked["decision"], "block");
    assert!(blocked["reason"]
        .as_str()
        .unwrap()
        .contains("was done but the workspace changed"));
    let reopened = plan::detail(&store, &f.project, "journey").unwrap();
    assert_eq!(reopened.plan.state, "building");
    let last = reopened.transitions.last().unwrap();
    assert_eq!(
        (last.from_state.as_str(), last.actor.as_str()),
        ("done", "yoke")
    );

    let rechecked = check();
    assert_eq!(rechecked["plan"]["state"], "done");
    assert!(
        plan::detail(&store, &f.project, "journey")
            .unwrap()
            .checks
            .len()
            > reopened.checks.len()
    );
    let fresh = stop();
    success(&fresh);
    assert!(
        fresh.stdout.is_empty(),
        "a freshly checked tree must be accepted"
    );
}

#[test]
fn install_follows_agent_config_dirs_and_health_stays_read_only() {
    let f = Fixture::new();
    let claude = f.home.join("claude-work");
    let codex = f.home.join("codex-home");
    let state = f.home.join(".yoke/state.db");
    let run = |args: &[&str]| {
        f.command(args)
            .env("CLAUDE_CONFIG_DIR", &claude)
            .env("CODEX_HOME", &codex)
            .output()
            .unwrap()
    };
    let health = |expected: i32| {
        let output = run(&["health", "--json"]);
        assert_eq!(
            output.status.code(),
            Some(expected),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(!state.exists(), "health must never create state");
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    let missing = health(1);
    assert_eq!(missing["status"], "failing");
    assert!(missing["problems"][0]
        .as_str()
        .unwrap()
        .contains("not installed for any agent"));

    success(&run(&["install", "--agent", "claude,codex"]));
    assert!(!f.home.join(".claude").exists() && !f.home.join(".codex").exists());
    let installed = health(0);
    assert_eq!(installed["status"], "attention");
    for agent in installed["agents"].as_array().unwrap() {
        assert_eq!(agent["installed"], true, "{agent}");
        assert!(agent["settings"].as_str().unwrap().starts_with(
            if agent["agent"] == "claude" {
                &claude
            } else {
                &codex
            }
            .to_str()
            .unwrap()
        ));
    }

    // The installed commands themselves enforce one denial and one allowed call per agent.
    f.snapshot("mode='enforce'\n[[gate]]\nname='review'\non='PreToolUse'\naction='deny'\nmatch_tool='Bash'\nmessage='review required'\n");
    for (agent, settings) in [
        ("claude", claude.join("settings.json")),
        ("codex", codex.join("hooks.json")),
    ] {
        let settings: Value = serde_json::from_str(&fs::read_to_string(settings).unwrap()).unwrap();
        let command = settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(command.contains(&format!("--agent {agent} PreToolUse")));
        for (tool, denied) in [("Bash", true), ("Read", false)] {
            let mut child = Command::new("sh")
                .args(["-c", &command])
                .current_dir(&f.project)
                .env("HOME", &f.home)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let mut event = payload();
            event["tool_name"] = json!(tool);
            child
                .stdin
                .take()
                .unwrap()
                .write_all(event.to_string().as_bytes())
                .unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success());
            if denied {
                let wire: Value = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(wire["hookSpecificOutput"]["permissionDecision"], "deny");
            } else {
                assert!(output.stdout.is_empty(), "{agent} {tool}");
            }
        }
    }

    fs::write(claude.join("settings.json"), {
        let mut settings: Value =
            serde_json::from_str(&fs::read_to_string(claude.join("settings.json")).unwrap())
                .unwrap();
        settings["disableAllHooks"] = json!(true);
        settings.to_string()
    })
    .unwrap();
    let output = run(&["health", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    let disabled: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(disabled["problems"]
        .as_array()
        .unwrap()
        .iter()
        .any(|problem| problem.as_str().unwrap().contains("disableAllHooks")));

    success(&run(&["uninstall", "--agent", "claude,codex"]));
    assert!(!codex.join("hooks.json").exists());
    let remaining: Value =
        serde_json::from_str(&fs::read_to_string(claude.join("settings.json")).unwrap()).unwrap();
    assert_eq!(remaining, json!({"disableAllHooks": true}));
}

#[test]
fn a_killed_checks_leftover_child_never_keeps_the_plan_from_resuming() {
    use yoke_core::plan;
    let f = Fixture::new();
    for args in [
        &["init", "-q"][..],
        &["commit", "-q", "--allow-empty", "-m", "base"],
    ] {
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.invalid",
            ])
            .args(args)
            .current_dir(&f.project)
            .status()
            .unwrap();
        assert!(status.success());
    }
    let session = json!({"session_id":"orphan","tool_name":"Bash","tool_input":{"command":"true"}});
    success(&f.hook("codex", "PostToolUse", session));
    success(&f.run(&["plan", "new", "orphan"], ""));
    let store = f.store();
    let file = plan::get(&store, &f.project, "orphan").unwrap().file;
    // The check leaves a long-lived grandchild behind and kills its own `yoke plan check`.
    fs::write(&file, "+++\ntitle='Orphan'\n[[check]]\nname='leak'\nrun='sleep 30 >/dev/null 2>&1 & echo $! > \"$HOME/sleeper.pid\"; kill -9 $PPID'\n+++\n").unwrap();
    plan::approve(
        &store,
        &plan::prepare_approval(&store, &f.project, "orphan").unwrap(),
    )
    .unwrap();
    success(&f.run(&["plan", "start", "orphan", "--session", "orphan"], ""));
    let killed = f.run(&["plan", "check", "orphan", "--agent", "codex"], "");
    assert_eq!(
        killed.status.code(),
        None,
        "the check must have been killed"
    );
    assert_eq!(
        plan::get(&store, &f.project, "orphan").unwrap().state,
        "checking"
    );
    let sleeper: i32 = fs::read_to_string(f.home.join("sleeper.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let alive = |pid: i32| {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .unwrap()
            .success()
    };
    assert!(alive(sleeper), "the leftover child must still be running");
    let resumed = plan::resume(&store, &f.project, "orphan");
    Command::new("kill")
        .arg(sleeper.to_string())
        .status()
        .unwrap();
    assert_eq!(resumed.unwrap().state, "building");
}

#[test]
fn background_children_write_only_timestamped_error_log_lines() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};
    let f = Fixture::new();
    let bin = f.home.join("bin");
    fs::create_dir(&bin).unwrap();
    // Local transport fixture only: the update check fails without any network.
    fs::write(bin.join("curl"), "#!/bin/sh\nexit 7\n").unwrap();
    fs::set_permissions(bin.join("curl"), fs::Permissions::from_mode(0o755)).unwrap();
    yoke_core::jobs::enqueue(&f.store(), "unknown-kind", "fixture", "/fixture", "{}", 0).unwrap();
    let mut child = f
        .command(&["run-hook", "SessionStart"])
        .env("PATH", &bin)
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"session_id\":\"background\"}")
        .unwrap();
    success(&child.wait_with_output().unwrap());
    let log = f.home.join(".yoke/error.log");
    let deadline = Instant::now() + Duration::from_secs(10);
    let text = loop {
        let text = fs::read_to_string(&log).unwrap_or_default();
        if text.contains("release check failed") && text.contains("unknown job kind") {
            break text;
        }
        assert!(Instant::now() < deadline, "children did not report: {text}");
        std::thread::sleep(Duration::from_millis(20));
    };
    for line in text.lines() {
        let stamp = line.split_once(' ').map(|(stamp, _)| stamp);
        assert!(
            stamp.is_some_and(|stamp| stamp.parse::<i64>().is_ok()),
            "untimestamped error.log line: {line}"
        );
    }
}

#[test]
fn extensions_inventory_never_creates_or_migrates_state() {
    let f = Fixture::new();
    let state = f.home.join(".yoke/state.db");
    let _ = f.command(&["extensions", "--json"]).output().unwrap();
    assert!(!state.exists(), "extensions must not create state");
    drop(f.store());
    let writer = rusqlite::Connection::open(&state).unwrap();
    writer.pragma_update(None, "foreign_keys", false).unwrap();
    for down in [
        include_str!("../../yoke-core/src/store/012-usage-time.down.sql"),
        include_str!("../../yoke-core/src/store/011-notifications.down.sql"),
        include_str!("../../yoke-core/src/store/010-jobs-recall.down.sql"),
        include_str!("../../yoke-core/src/store/009-plan-acceptance.down.sql"),
        include_str!("../../yoke-core/src/store/008-plan-lifecycle.down.sql"),
        include_str!("../../yoke-core/src/store/007-plan-base-ref.down.sql"),
    ] {
        writer.execute_batch(down).unwrap();
    }
    let output = f.command(&["extensions", "--json"]).output().unwrap();
    assert!(
        serde_json::from_slice::<Value>(&output.stdout).is_ok(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let version: i64 = writer
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 6, "extensions must not migrate state");
}

fn git_in(dir: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "protocol.file.allow=always",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// A repository with one commit, an enforce policy and a started plan running `check`.
fn started_plan(f: &Fixture, check: &str) {
    use yoke_core::plan;
    git_in(&f.project, &["init", "-q"]);
    git_in(&f.project, &["add", "-A"]);
    git_in(&f.project, &["commit", "-q", "--allow-empty", "-m", "base"]);
    f.snapshot("mode='enforce'\n");
    let session = json!({"session_id":"anchor","tool_name":"Bash","tool_input":{"command":"true"}});
    success(&f.hook("codex", "PostToolUse", session));
    success(&f.run(&["plan", "new", "anchor"], ""));
    let store = f.store();
    let file = plan::get(&store, &f.project, "anchor").unwrap().file;
    fs::write(
        &file,
        format!("+++\ntitle='Anchor'\n[[check]]\nname='work'\nrun='{check}'\n+++\n"),
    )
    .unwrap();
    plan::approve(
        &store,
        &plan::prepare_approval(&store, &f.project, "anchor").unwrap(),
    )
    .unwrap();
    success(&f.run(&["plan", "start", "anchor", "--session", "anchor"], ""));
}

fn check_anchor_plan(f: &Fixture) -> Value {
    let output = f.run(&["plan", "check", "anchor", "--agent", "codex"], "");
    success(&output);
    let summary: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["plan"]["state"], "done", "{summary}");
    summary
}

/// Stop output for the anchor session: `None` when allowed, else the block reason.
fn stop_anchor_plan(f: &Fixture) -> Option<String> {
    let output = f.hook("codex", "Stop", json!({"session_id":"anchor"}));
    success(&output);
    (!output.stdout.is_empty()).then(|| {
        let wire: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(wire["decision"], "block");
        wire["reason"].as_str().unwrap().to_owned()
    })
}

fn assert_reopened(reason: Option<String>) {
    let reason = reason.expect("a changed tree must block");
    assert!(
        reason.contains("was done but the workspace changed") && !reason.contains("yoke error"),
        "{reason}"
    );
}

#[test]
fn large_binary_changes_complete_and_keep_or_reopen_a_done_plan() {
    let f = Fixture::new();
    // 8 MiB of binary content per version: its binary diff exceeds every git output bound.
    let binary = |seed: u64| {
        let mut state = seed;
        (0..8 * 1024 * 1024)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 24) as u8
            })
            .collect::<Vec<u8>>()
    };
    fs::write(f.project.join("big.bin"), binary(1)).unwrap();
    started_plan(&f, "test -s big.bin");
    fs::write(f.project.join("big.bin"), binary(2)).unwrap();
    check_anchor_plan(&f);
    assert_eq!(stop_anchor_plan(&f), None);
    git_in(&f.project, &["commit", "-qam", "same bytes"]);
    assert_eq!(
        stop_anchor_plan(&f),
        None,
        "committing the checked bytes keeps the plan done"
    );
    fs::write(f.project.join("big.bin"), binary(3)).unwrap();
    git_in(&f.project, &["commit", "-qam", "new bytes"]);
    assert_reopened(stop_anchor_plan(&f));
    check_anchor_plan(&f);
    assert_eq!(stop_anchor_plan(&f), None);
}

#[test]
fn submodules_and_nested_repositories_complete_and_keep_or_reopen_a_done_plan() {
    let f = Fixture::new();
    let library = f.home.join("library");
    fs::create_dir(&library).unwrap();
    git_in(&library, &["init", "-q"]);
    git_in(
        &library,
        &["commit", "-q", "--allow-empty", "-m", "library"],
    );
    started_plan(&f, "true");
    check_anchor_plan(&f);
    git_in(
        &f.project,
        &["submodule", "add", "-q", library.to_str().unwrap(), "sub"],
    );
    let nested = f.project.join("nested");
    fs::create_dir(&nested).unwrap();
    git_in(&nested, &["init", "-q"]);
    git_in(&nested, &["commit", "-q", "--allow-empty", "-m", "nested"]);
    assert_reopened(stop_anchor_plan(&f));
    check_anchor_plan(&f);
    assert_eq!(stop_anchor_plan(&f), None);
    git_in(&f.project, &["commit", "-qm", "add submodule"]);
    assert_eq!(
        stop_anchor_plan(&f),
        None,
        "committing the checked submodule keeps the plan done"
    );
}

#[test]
fn a_blocking_hook_answers_itself_before_the_agent_timeout() {
    let f = Fixture::new();
    for (mode, denied) in [("enforce", true), ("observe", false)] {
        // The gate leaves a background child in its process group, as a test runner would.
        f.snapshot(&format!(
            "mode='{mode}'\n[[gate]]\nname='slow'\non='PreToolUse'\nrun='sleep 10 & echo $! > \"$HOME/gate-{mode}.pid\"; wait'\ntimeout=20\n"
        ));
        let started = std::time::Instant::now();
        let output = f
            .command(&["run-hook", "--agent", "claude", "PreToolUse"])
            .env("YOKE_HOOK_DEADLINE_MS", "500")
            .spawn()
            .and_then(|mut child| {
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(payload().to_string().as_bytes())?;
                child.wait_with_output()
            })
            .unwrap();
        success(&output);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(8),
            "{mode}"
        );
        let child = fs::read_to_string(f.home.join(format!("gate-{mode}.pid"))).unwrap();
        let gone = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while Command::new("kill")
            .args(["-0", child.trim()])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
        {
            assert!(
                std::time::Instant::now() < gone,
                "{mode}: gate child {} outlived the hook",
                child.trim()
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if denied {
            // Exactly one decision: a second value would fail to parse as one JSON document.
            let wire: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(wire["hookSpecificOutput"]["permissionDecision"], "deny");
        } else {
            assert!(output.stdout.is_empty(), "{mode}");
        }
    }
    let log = fs::read_to_string(f.home.join(".yoke/error.log")).unwrap();
    assert_eq!(log.matches("hook evaluation exceeded").count(), 2, "{log}");
}

#[test]
fn watchdog_answered_stops_count_toward_the_stall_cap() {
    let f = Fixture::new();
    started_plan(&f, "true");
    f.snapshot("mode='enforce'\n[stop]\nmax_blocks=2\n[[gate]]\nname='slow'\non='Stop'\nrun='sleep 10'\ntimeout=20\n");
    let stop = || {
        f.command(&["run-hook", "--agent", "codex", "Stop"])
            .env("YOKE_HOOK_DEADLINE_MS", "300")
            .spawn()
            .and_then(|mut child| {
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(br#"{"session_id":"anchor"}"#)?;
                child.wait_with_output()
            })
            .unwrap()
    };
    for block in 1..=2 {
        let output = stop();
        success(&output);
        let wire: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(wire["decision"], "block");
        assert!(
            wire["reason"]
                .as_str()
                .unwrap()
                .contains(&format!("stop block {block}/2")),
            "{wire}"
        );
    }
    let stalled = stop();
    success(&stalled);
    assert!(stalled.stdout.is_empty(), "the cap lets the agent stop");
    let store = f.store();
    assert_eq!(
        yoke_core::plan::get(&store, &f.project, "anchor")
            .unwrap()
            .state,
        "stalled"
    );
    let outcomes: Vec<String> = store
        .decisions(&DecisionFilter {
            session: Some("anchor".into()),
            ..Default::default()
        })
        .unwrap()
        .into_iter()
        .filter(|row| row.event == "stop")
        .map(|row| row.outcome)
        .collect();
    assert_eq!(outcomes, ["allow", "deny", "deny"]);
}

#[test]
fn trust_refuses_gates_that_cannot_finish_before_the_agent_timeout() {
    let f = Fixture::new();
    for (config, accepted) in [
        ("[[gate]]\nname='suite'\non='Stop'\nrun='true'\ntimeout=300\n", true),
        ("[[gate]]\nname='suite'\non='Stop'\nrun='true'\ntimeout=600\n", false),
        ("[[gate]]\nname='a'\non='Stop'\nrun='true'\ntimeout=300\n[[gate]]\nname='b'\non='Stop'\nrun='true'\ntimeout=300\n", false),
        ("[[gate]]\nname='prompt'\non='UserPromptSubmit'\nrun='true'\n", false),
        ("[[gate]]\nname='prompt'\non='UserPromptSubmit'\nrun='true'\ntimeout=20\n", true),
        ("[[gate]]\nname='deny'\non='Stop'\naction='deny'\nmessage='no'\n", true),
    ] {
        fs::write(f.project.join("yoke.toml"), config).unwrap();
        let prepared = trust::prepare_project(&f.store(), &f.project);
        assert_eq!(prepared.is_ok(), accepted, "{config}");
        if let Err(error) = prepared {
            assert!(format!("{error:#}").contains("must answer within"), "{error:#}");
        }
    }
}
