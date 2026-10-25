use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
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
        self.command_at(Path::new(env!("CARGO_BIN_EXE_yoke")), args)
    }

    fn command_at(&self, bin: &Path, args: &[&str]) -> Command {
        let mut cmd = isolated_command(bin, &self.home);
        cmd.args(args)
            .current_dir(&self.project)
            .env("HOME", &self.home)
            // The agents' config dirs follow these, so an inherited value would point install
            // and uninstall at the caller's real configuration.
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME")
            .env_remove("YOKE_OFF")
            // Installing and uninstalling refuse an agent's process, so tests cannot inherit it.
            .env_remove("CLAUDECODE")
            .env_remove("CLAUDE_CODE_CHILD_SESSION")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("CODEX_SESSION_ID")
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

fn isolated_command(bin: &Path, home: &Path) -> Command {
    let home = home.canonicalize().expect("fixture home must exist");
    let temporary = std::env::temp_dir().canonicalize().unwrap();
    assert!(
        home.starts_with(&temporary) && home != temporary,
        "fixture HOME must be inside a temporary directory"
    );
    let curl = home.parent().unwrap().join(".fixture-curl");
    if !curl.exists() {
        fs::write(&curl, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(&curl, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut command = Command::new(bin);
    command
        .env("HOME", &home)
        .env_remove("CODEX_HOME")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env("YOKE_CURL", curl);
    command
}

fn payload() -> Value {
    json!({"session_id":"fixture-session","tool_name":"Bash","tool_input":{"command":"echo test","token":"private-token-value"}})
}

#[test]
fn hook_records_the_loaded_policy_across_retrust_and_untrusted_disk_changes() {
    let f = Fixture::new();
    let first = "mode='enforce'\n[log]\npayload_cap=32\n[[gate]]\nname='first-policy'\non='pre_tool_use'\naction='deny'\nmatch_contains='echo test'\nmessage='First policy denied this command'\n";
    f.snapshot(first);
    let mut input = payload();
    input["policy_hash"] = json!("f".repeat(64));
    let output = f.hook("claude", "PreToolUse", input);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("First policy denied"));
    let db = rusqlite::Connection::open(f.home.join(".yoke/state.db")).unwrap();
    let hash: String = db
        .query_row(
            "SELECT policy_hash FROM decisions ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let content: String = db
        .query_row(
            "SELECT content FROM policy_versions WHERE sha256=?1",
            [&hash],
            |r| r.get(0),
        )
        .unwrap();
    let archived: Value = serde_json::from_str(&content).unwrap();
    assert_eq!(archived["sources"][0]["content"], first);
    assert_eq!(archived["config"]["mode"], "enforce");
    f.snapshot("mode='observe'\n");
    // Disk content that was not trusted cannot become the next decision's policy.
    fs::write(f.project.join("yoke.toml"), first).unwrap();
    assert!(f.hook("codex", "PreToolUse", payload()).status.success());
    let next: String = db
        .query_row(
            "SELECT policy_hash FROM decisions ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_ne!(next, hash);
    let still_archived: String = db
        .query_row(
            "SELECT content FROM policy_versions WHERE sha256=?1",
            [&hash],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(still_archived, content);
    assert_ne!(hash, "f".repeat(64), "hook input cannot forge provenance");
    // The preliminary policy is enough to attribute malformed-input errors; a normal
    // Stop uses the same stamp through its atomic guard/decision transaction.
    success(&f.run(&["run-hook", "--agent", "claude", "PreToolUse"], "{"));
    let error = f
        .store()
        .decisions(&DecisionFilter::default())
        .unwrap()
        .remove(0);
    assert_eq!(error.outcome, "error");
    assert_eq!(error.policy_hash.as_deref(), Some(next.as_str()));
    success(&f.hook("codex", "Stop", json!({"session_id":"policy-stop"})));
    let stop = f
        .store()
        .decisions(&DecisionFilter::default())
        .unwrap()
        .remove(0);
    assert_eq!(stop.event, "stop");
    assert_eq!(stop.policy_hash.as_deref(), Some(next.as_str()));
}

#[test]
fn policy_archive_failure_preserves_the_gate_outcome_and_marks_provenance_missing() {
    let f = Fixture::new();
    f.snapshot("mode='enforce'\n[[gate]]\nname='deny'\non='pre_tool_use'\naction='deny'\nmessage='policy says no'\n");
    let db = rusqlite::Connection::open(f.home.join(".yoke/state.db")).unwrap();
    db.execute_batch("CREATE TRIGGER reject_archive BEFORE INSERT ON policy_versions BEGIN SELECT RAISE(ABORT,'fixture archive unavailable'); END;").unwrap();
    let output = f.hook("claude", "PreToolUse", payload());
    success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("policy says no"));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("decision policy version was not saved")
    );
    let rows = f.store().decisions(&DecisionFilter::default()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].outcome, "deny");
    assert!(rows[0].policy_hash.is_none());
}
/// True while the process runs. A killed orphan that nothing has reaped is a zombie on hosts
/// whose PID 1 does not reap children (containers), and a zombie is not running.
fn process_exists(pid: i32) -> bool {
    // SAFETY: signal zero only probes whether the process exists.
    if unsafe { libc::kill(pid, 0) } == -1 {
        return std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH);
    }
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => !stat
            .rsplit_once(')')
            .is_some_and(|(_, rest)| rest.trim_start().starts_with(['Z', 'X'])),
        Err(_) => true,
    }
}

/// Read-only files stop writes for every user except root, which ignores file modes.
fn file_modes_are_enforced() -> bool {
    use std::os::unix::fs::PermissionsExt;
    let scratch = tempfile::tempdir().unwrap();
    let probe = scratch.path().join("mode-probe");
    fs::write(&probe, "").unwrap();
    fs::set_permissions(&probe, fs::Permissions::from_mode(0o444)).unwrap();
    fs::OpenOptions::new().write(true).open(&probe).is_err()
}

fn success(output: &Output) {
    assert!(
        output.status.success(),
        "status: {:?}; stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn trusted_output_capture_is_logged_without_changing_hook_decisions() {
    let f = Fixture::new();
    for mode in ["observe", "enforce"] {
        let mut baseline = None;
        for capture in [false, true] {
            f.snapshot(&format!("mode='{mode}'\n[[gate]]\nname='checks'\non='pre_tool_use'\ncapture_output={capture}\nrun=\"printf 'report token=stdout-hidden\\n'; printf 'diagnostic password=stderr-hidden\\n' >&2; exit 1\"\nmessage='Run checks first.'"));
            // Only the captured, approved setting may decide whether output is retained.
            fs::write(
                f.project.join("yoke.toml"),
                format!(
                    "[[gate]]\nname='checks'\non='pre_tool_use'\ncapture_output={}\nrun='true'",
                    !capture
                ),
            )
            .unwrap();
            for agent in ["claude", "codex"] {
                let response = f.hook(agent, "PreToolUse", payload());
                success(&response);
                if let Some(expected) = &baseline {
                    assert_eq!(&response.stdout, expected);
                } else {
                    baseline = Some(response.stdout.clone());
                }
                if mode == "observe" {
                    assert!(response.stdout.is_empty());
                } else {
                    let response: Value = serde_json::from_slice(&response.stdout).unwrap();
                    assert!(response["hookSpecificOutput"]["permissionDecisionReason"]
                        .as_str()
                        .unwrap()
                        .starts_with("Run checks first."));
                }
                let log = f.run(&["log", "--json", "--limit", "1"], "");
                success(&log);
                let rows: Value = serde_json::from_slice(&log.stdout).unwrap();
                assert_eq!(
                    rows[0]["outcome"],
                    if mode == "observe" {
                        "would_deny"
                    } else {
                        "deny"
                    }
                );
                let gate = &rows[0]["gates"][0];
                assert_eq!(gate["exit_code"], 1);
                if capture {
                    assert_eq!(gate["output"]["stdout"], "report token=[REDACTED]\n");
                    assert_eq!(gate["output"]["stderr"], "diagnostic password=[REDACTED]\n");
                } else {
                    assert!(gate.get("output").is_none());
                }
                assert!(!String::from_utf8(log.stdout).unwrap().contains("hidden"));
            }
        }
    }
}

#[test]
fn gates_inspection_exposes_trusted_commands_and_execution_settings() {
    let f = Fixture::new();
    let unapproved = "[[gate]]\nname='unapproved'\non='stop'\nrun='touch ran'\n";
    fs::write(f.project.join("yoke.toml"), unapproved).unwrap();
    let output = f.run(&["gates", "--json"], "");
    success(&output);
    let empty: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(empty["gates"], json!([]));
    assert_eq!(empty["mode"], "observe");

    fs::write(f.home.join(".yoke/config.toml"), "[[gate]]\nname='checks'\non='stop'\nrun='touch replaced'\n[[gate]]\nname='global-check'\non='stop'\nrun='touch global-ran'\nshell='bash'\ntimeout=3\n").unwrap();
    let store = f.store();
    trust::prepare_global(&store, &f.home)
        .unwrap()
        .commit(&store)
        .unwrap();
    drop(store);
    f.snapshot("mode='enforce'\n[gates]\npass_env=['DATABASE_URL']\n[[gate]]\nname='checks'\non='stop'\nrun='printf trusted'\nmatch_contains='finish'\npass_env=['SERVICE_TOKEN']\n[[gate]]\nname='deny'\non='pre_tool_use'\naction='deny'\nmessage='blocked'\n");
    fs::write(f.project.join("yoke.toml"), unapproved).unwrap();
    let output = f.run(&["gates", "--verbose"], "");
    success(&output);
    let text = String::from_utf8(output.stdout).unwrap();
    for expected in [
        "mode: enforce",
        "printf trusted",
        "/bin/sh",
        "expected_exit",
        "timeout",
        "finish",
        "DATABASE_URL",
        "SERVICE_TOKEN",
        "changed or is missing after it was trusted",
        "yoke log --json",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert!(!text.contains("touch ran"), "{text}");

    let output = f.run(&["gates", "--json"], "");
    success(&output);
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["mode"], "enforce");
    assert_eq!(value["pass_env"], json!(["DATABASE_URL"]));
    let gate = &value["gates"][0];
    assert_eq!(gate["run"], "printf trusted");
    assert_eq!(gate["shell"], "/bin/sh");
    assert_eq!(gate["timeout"], 30);
    assert_eq!(gate["expected_exit"], 0);
    assert_eq!(gate["pass_env"], json!(["SERVICE_TOKEN"]));
    assert_eq!(
        gate["source"],
        f.project
            .canonicalize()
            .unwrap()
            .join("yoke.toml")
            .to_str()
            .unwrap()
    );
    assert_eq!(value["gates"].as_array().unwrap().len(), 3);
    let global = &value["gates"][1];
    assert_eq!(global["name"], "global-check");
    assert_eq!(global["shell"], "bash");
    assert_eq!(global["timeout"], 3);
    assert_eq!(
        global["source"],
        f.home
            .canonicalize()
            .unwrap()
            .join(".yoke/config.toml")
            .to_str()
            .unwrap()
    );
    assert_eq!(value["gates"][2]["expected_exit"], Value::Null);
    assert!(value["snapshots"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["stale"] == true));
    assert!(!f.project.join("ran").exists());
    assert!(!f.project.join("global-ran").exists());
    assert!(!f.project.join("replaced").exists());
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
fn a_linked_state_directory_cannot_redirect_hook_writes() {
    let f = Fixture::new();
    f.snapshot("mode='enforce'");
    let state = f.home.join(".yoke");
    let outside = f.home.join("outside");
    fs::rename(&state, &outside).unwrap();
    std::os::unix::fs::symlink(&outside, &state).unwrap();
    let contents = || {
        let mut files: Vec<_> = fs::read_dir(&outside)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.file_name(), fs::read(entry.path()).unwrap())
            })
            .collect();
        files.sort_by(|a, b| a.0.cmp(&b.0));
        files
    };
    let before = contents();
    let output = f.hook("claude", "PreToolUse", payload());
    success(&output);
    let wire: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(wire["hookSpecificOutput"]["permissionDecision"], "deny");
    assert_eq!(contents(), before);
}

#[test]
fn unsafe_bypass_files_cannot_skip_or_hang_an_enforcing_hook() {
    use std::os::unix::fs::symlink;
    use std::time::{Duration, Instant};
    for name in ["off-token", "paused-until"] {
        let f = Fixture::new();
        f.snapshot("mode='enforce'\n[[gate]]\nname='deny'\non='PreToolUse'\naction='deny'\nmatch_tool='Bash'");
        let path = f.home.join(".yoke").join(name);
        let target = f.home.join("control");
        let token = "ab".repeat(32);
        fs::write(
            &target,
            if name == "off-token" {
                &token
            } else {
                "99999999999"
            },
        )
        .unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&target, &path).unwrap();
        for fifo in [false, true] {
            if fifo {
                fs::remove_file(&path).unwrap();
                let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
                // SAFETY: path is a live, NUL-terminated path.
                assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
            }
            let mut command = f.command(&["run-hook", "--agent", "claude", "PreToolUse"]);
            command.env("YOKE_OFF", &token);
            let mut child = command.spawn().unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(payload().to_string().as_bytes())
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            let finished = child.try_wait().unwrap().is_some();
            if !finished {
                child.kill().unwrap();
            }
            let output = child.wait_with_output().unwrap();
            assert!(finished, "hook blocked reading {name}");
            success(&output);
            let wire: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                wire["hookSpecificOutput"]["permissionDecision"], "deny",
                "{name}, fifo={fifo}"
            );
        }
    }
}

#[test]
fn off_needs_the_install_token_or_a_pause_and_agents_cannot_trust_or_reinstall() {
    let f = Fixture::new();
    f.snapshot("mode='enforce'\n[[gate]]\nname='review'\non='PreToolUse'\naction='deny'\nmatch_tool='Bash'\nmessage='review required'");
    let token = "ab".repeat(32);
    fs::write(f.home.join(".yoke/off-token"), format!("{token}\n")).unwrap();
    fs::set_permissions(
        f.home.join(".yoke/off-token"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let denies = |output: &Output| {
        success(output);
        serde_json::from_slice::<Value>(&output.stdout)
            .is_ok_and(|result| result["hookSpecificOutput"]["permissionDecision"] == "deny")
    };
    let hook_with = |off: Option<&str>| {
        let mut command = f.command(&["run-hook", "--agent", "claude", "PreToolUse"]);
        if let Some(off) = off {
            command.env("YOKE_OFF", off);
        }
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload().to_string().as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    };
    // The old value, a guess and no value at all leave the gate in force.
    for off in [Some("1"), Some("true"), Some(&token[1..]), None] {
        assert!(denies(&hook_with(off)), "{off:?}");
    }
    let refusals = fs::read_to_string(f.home.join(".yoke/error.log")).unwrap();
    assert_eq!(
        refusals
            .matches("YOKE_OFF is set but does not match")
            .count(),
        3
    );
    // The token skips the hook before it reads the payload, the configuration or the database.
    let decisions = f
        .store()
        .decisions(&DecisionFilter::default())
        .unwrap()
        .len();
    let skipped = hook_with(Some(&token));
    success(&skipped);
    assert!(skipped.stdout.is_empty() && skipped.stderr.is_empty());
    assert_eq!(
        f.store()
            .decisions(&DecisionFilter::default())
            .unwrap()
            .len(),
        decisions
    );
    assert!(fs::read_to_string(f.home.join(".yoke/error.log"))
        .unwrap()
        .contains("YOKE_OFF matched the bypass token"));
    // A pause skips every hook until it ends.
    let pause = f.home.join(".yoke/paused-until");
    fs::write(&pause, "99999999999\n").unwrap();
    fs::set_permissions(&pause, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(hook_with(None).stdout.is_empty());
    fs::write(&pause, "1\n").unwrap();
    assert!(denies(&hook_with(None)));

    let refused = f
        .command(&["trust"])
        .env("CODEX_THREAD_ID", "test-agent")
        .output()
        .unwrap();
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stdout).contains("human outside an agent"));
    for args in [&["uninstall"][..], &["install"], &["update"], &["pause"]] {
        let refused = f.command(args).env("CLAUDECODE", "1").output().unwrap();
        assert_eq!(refused.status.code(), Some(1), "{args:?}");
    }
    let dry = f
        .command(&["install", "--agent", "claude", "--dry-run"])
        .env("CLAUDECODE", "1")
        .output()
        .unwrap();
    success(&dry);
    assert!(!f.home.join(".claude").exists());
    let flag = f.run(&["trust", "--yes"], "trust\n");
    assert_eq!(flag.status.code(), Some(2));
}

#[test]
fn readonly_state_preserves_trusted_tool_decisions_without_audit_writes() {
    use std::os::unix::fs::PermissionsExt;

    if !file_modes_are_enforced() {
        eprintln!(
            "skipped: this user can write read-only files, so read-only state cannot be simulated"
        );
        return;
    }
    for (agent, schema_six) in [
        ("claude", false),
        ("codex", false),
        ("claude", true),
        ("codex", true),
    ] {
        let f = Fixture::new();
        f.snapshot("mode='enforce'\n[[gate]]\nname='review'\non='PreToolUse'\naction='deny'\nmatch_tool='Bash'\nmessage='review required'\n[[gate]]\nname='rewrite'\non='PreToolUse'\naction='rewrite'\nmatch_tool='exec_command'\nmatch_contains='test'\nrewrite_pattern='test'\nrewrite_to='safe'");
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
    f.snapshot("mode='enforce'\n[[gate]]\nname='rewrite'\non='pre_tool_use'\naction='rewrite'\nmatch_tool='Bash|exec_command'\nmatch_contains='test'\nrewrite_pattern='test'\nrewrite_to='safe'");
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
    f.snapshot("mode='enforce'\n[[gate]]\nname='broken-check'\non='Stop'\nrun='yoke-nonexistent-gate-command'\nmessage='Run checks first.'");
    for agent in ["claude", "codex"] {
        let failed = f.hook(agent, "Stop", json!({"session_id":"life"}));
        success(&failed);
        let response: Value = serde_json::from_slice(&failed.stdout).unwrap();
        assert_eq!(response["decision"], "block");
        let reason = response["reason"].as_str().unwrap();
        assert!(
            reason.contains("Run checks first.")
                && reason.contains("yoke-nonexistent-gate-command")
                && reason.contains("yoke gates --verbose"),
            "{reason}"
        );
        let rows = f.store().decisions(&DecisionFilter::default()).unwrap();
        assert_eq!(rows[0].gates[0]["exit_code"], 127);
        assert!(rows[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("yoke gates --verbose"));
    }
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
fn stop_with_transcript_over_a_usage_bound_keeps_the_policy_decision() {
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
            // Oversized transcripts are imported from their newest part instead of failing,
            // so neither transcript warns; the size bound shows up as a partial-import record.
            assert!(
                !String::from_utf8_lossy(&stop.stderr).contains("usage ingestion failed"),
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
            assert!(
                !String::from_utf8_lossy(&output.stderr).contains("usage ingestion failed"),
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
fn session_start_queues_retention_and_a_background_job_prunes_expired_history() {
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
    // The hook only queues the pass; the background worker it starts deletes the history.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while ids("SELECT count(*) FROM jobs WHERE kind='retention' AND state='succeeded'") != vec![1]
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(
        ids("SELECT count(*) FROM jobs WHERE kind='retention' AND state='succeeded'"),
        vec![1]
    );
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
    // The hourly claim throttles the pass: a second session start in the same hour queues nothing.
    success(&f.hook(
        "claude",
        "SessionStart",
        json!({"session_id":"again","hook_event_name":"SessionStart"}),
    ));
    assert_eq!(
        ids("SELECT count(*) FROM jobs WHERE kind='retention'"),
        vec![1]
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
    fs::write(&curl, "#!/bin/sh\nprintf '%s\\n' \"$YOKE_OFF\" >> \"$HOME/checks\"\nprintf '%s\\n200' '{\"tag_name\":\"v0.2.0\",\"draft\":false,\"prerelease\":false,\"assets\":[]}'\n").unwrap();
    fs::set_permissions(&curl, fs::Permissions::from_mode(0o755)).unwrap();
    for _ in 0..2 {
        let mut child = f
            .command(&["run-hook", "SessionStart"])
            // The updater runs curl from a fixed system path, so PATH cannot supply the fake.
            .env("YOKE_CURL", &curl)
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
    let deadline = Instant::now() + Duration::from_secs(30);
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
        let output = f.run(
            &["plan", "check", "journey", "--agent", "codex", "--json"],
            "",
        );
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
    use std::os::unix::fs::MetadataExt;
    let f = Fixture::new();
    // Health correctly warns about target/ executables; exercise a stable installed copy.
    let bin = f.home.join("bin");
    fs::create_dir(&bin).unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o775)).unwrap();
    let executable = bin.join("yoke");
    fs::copy(env!("CARGO_BIN_EXE_yoke"), &executable).unwrap();
    // Sticky temporary ancestors protect owned entries; group write is guidance only.
    let canonical = executable.canonicalize().unwrap();
    let mut shared_locations = Vec::new();
    let mut group_locations = Vec::new();
    // SAFETY: geteuid only reads the process's effective user ID.
    let uid = unsafe { libc::geteuid() };
    let mut child: Option<&Path> = None;
    for path in canonical.ancestors() {
        let meta = fs::metadata(path).unwrap();
        let mode = meta.permissions().mode();
        let protected = meta.is_dir()
            && mode & 0o1000 != 0
            && child.is_some_and(|child| {
                let owner = fs::metadata(child).unwrap().uid();
                owner == uid || owner == 0
            });
        if mode & 0o002 != 0 {
            if !protected {
                shared_locations.push(path.to_path_buf());
            }
        } else if mode & 0o020 != 0 {
            group_locations.push(path.to_path_buf());
        }
        child = Some(path);
    }
    let claude = f.home.join("claude-work");
    let codex = f.home.join("codex-home");
    let state = f.home.join(".yoke/state.db");
    let run = |args: &[&str]| {
        f.command_at(&executable, args)
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
    assert!(
        missing["problems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|problem| problem
                .as_str()
                .unwrap()
                .contains("not installed for any agent")),
        "{missing}"
    );

    success(&run(&["install", "--agent", "claude,codex"]));
    assert!(!f.home.join(".claude").exists() && !f.home.join(".codex").exists());
    let installed = health(i32::from(!shared_locations.is_empty()));
    assert_eq!(
        installed["status"],
        if shared_locations.is_empty() {
            "attention"
        } else {
            "failing"
        }
    );
    let problems = installed["problems"].as_array().unwrap();
    assert_eq!(problems.len(), shared_locations.len() * 2, "{installed}");
    for problem in problems {
        let problem = problem.as_str().unwrap();
        assert!(problem.contains("writable by others"), "{problem}");
        assert!(
            shared_locations
                .iter()
                .any(|path| problem.contains(&path.display().to_string())),
            "{problem}"
        );
    }
    for path in group_locations {
        assert!(
            installed["guidance"]
                .as_array()
                .unwrap()
                .iter()
                .any(|line| {
                    let line = line.as_str().unwrap();
                    line.contains(&path.display().to_string())
                        && line.contains("group-writable")
                        && line.contains("chmod g-w")
                }),
            "{installed}"
        );
    }
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
            let mut child = isolated_command(Path::new("sh"), &f.home)
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
    assert!(
        process_exists(sleeper),
        "the leftover child must still be running"
    );
    let resumed = plan::resume(&store, &f.project, "orphan");
    // SAFETY: SIGTERM goes to the sleeper this fixture started.
    unsafe { libc::kill(sleeper, libc::SIGTERM) };
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
        .env("YOKE_CURL", bin.join("curl"))
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
        if text.contains("release lookup failed") && text.contains("unknown job kind") {
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
        include_str!("../../yoke-core/src/store/015-usage-summaries.down.sql"),
        include_str!("../../yoke-core/src/store/014-session-anchors.down.sql"),
        include_str!("../../yoke-core/src/store/013-scale-indexes.down.sql"),
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
    let output = f.run(
        &["plan", "check", "anchor", "--agent", "codex", "--json"],
        "",
    );
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
        let child: i32 = fs::read_to_string(f.home.join(format!("gate-{mode}.pid")))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let gone = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while process_exists(child) {
            assert!(
                std::time::Instant::now() < gone,
                "{mode}: gate child {child} outlived the hook"
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
    for row in f.store().decisions(&DecisionFilter::default()).unwrap() {
        let hash = row.policy_hash.expect("watchdog retains the loaded policy");
        assert!(f.store().policy_version(&hash).unwrap().is_some());
    }
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
