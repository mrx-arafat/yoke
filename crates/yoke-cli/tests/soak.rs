//! Synthetic soak trial - does not replace the real-work trial.
//!
//! Drives the real `yoke` binary in compressed time with hook payloads shaped like the ones
//! Claude Code and Codex send, and compares every outcome with a scripted expectation.
//! Run: `cargo test -p yoke-cli --test soak -- --ignored --nocapture --test-threads=1`.
//! `YOKE_SOAK_BIN` selects another yoke build (use a release build for latency numbers);
//! `YOKE_SOAK_OUT` names a directory that receives each scenario's aggregate results as JSON.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tempfile::TempDir;
use yoke_core::store::{DecisionFilter, Store};
use yoke_core::{plan, trust};

const MIB: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Out {
    Silent,
    Deny,
    Block,
    Rewrite,
    Context,
}

/// The decision a hook must persist (None: nothing persisted) and what it must print.
#[derive(Clone, Copy, Debug)]
struct Expect(Option<&'static str>, Out);

const ALLOW: Expect = Expect(Some("allow"), Out::Silent);

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

fn classify(stdout: &[u8]) -> Result<Out, String> {
    if stdout.iter().all(u8::is_ascii_whitespace) {
        return Ok(Out::Silent);
    }
    let value: Value =
        serde_json::from_slice(stdout).map_err(|error| format!("invalid hook JSON: {error}"))?;
    let specific = &value["hookSpecificOutput"];
    Ok(if specific["permissionDecision"] == "deny" {
        Out::Deny
    } else if specific["updatedInput"].is_object() {
        Out::Rewrite
    } else if specific["additionalContext"].is_string() {
        Out::Context
    } else if value["decision"] == "block" && value["reason"].is_string() {
        Out::Block
    } else {
        return Err(format!("unrecognized hook output {value}"));
    })
}

#[derive(Clone)]
struct Session {
    agent: &'static str,
    id: String,
    cwd: PathBuf,
    transcript: PathBuf,
    turn: u32,
    tool: u32,
    tokens: [u64; 2],
    /// A plan is bound, so enforce-mode prompts carry plan context.
    plan: bool,
}

struct Trial {
    _temp: TempDir,
    name: &'static str,
    home: PathBuf,
    project: PathBuf,
    bin: PathBuf,
    path: OsString,
    store: Store,
    enforce: bool,
    /// While state files are read-only, decisions cannot be persisted or checked per event.
    readonly: bool,
    samples: BTreeMap<String, Vec<f64>>,
    mismatches: Vec<String>,
    events: usize,
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args([
            "-c",
            "user.name=Soak",
            "-c",
            "user.email=soak@example.invalid",
        ])
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

fn project(root: &Path, name: &str) -> PathBuf {
    let dir = root.join(name);
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("README.md"), "# soak\n").unwrap();
    fs::write(dir.join("src/lib.rs"), "pub fn soak() {}\n").unwrap();
    git(&dir, &["init", "-q"]);
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "base"]);
    dir.canonicalize().unwrap()
}

/// Spawn one hook exactly as an agent does: event argument, JSON on stdin, session cwd.
fn run_hook(
    trial: (&Path, &Path, &OsString),
    agent: &str,
    event: &str,
    payload: &Value,
    cwd: &Path,
) -> (Output, f64) {
    let (bin, home, path) = trial;
    let mut command = isolated_command(bin, home);
    command
        .args(["run-hook", "--agent", agent, event])
        .current_dir(cwd)
        .env("HOME", home)
        .env("PATH", path)
        .env_remove("YOKE_OFF")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if agent == "claude" {
        command.env("CLAUDE_PROJECT_DIR", cwd);
    }
    let started = Instant::now();
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    (output, started.elapsed().as_secs_f64() * 1000.0)
}

fn payload(session: &Session, event: &str, fields: Value) -> Value {
    let mut value = json!({
        "session_id": session.id,
        "transcript_path": session.transcript,
        "cwd": session.cwd,
        "hook_event_name": event,
    });
    if session.agent == "codex" {
        // Codex documents model on every event except SessionEnd, turn_id on turn events,
        // and permission_mode on a fixed subset (learn.chatgpt.com/docs/hooks).
        if event != "SessionEnd" {
            value["model"] = json!("fixture-model");
        }
        if matches!(
            event,
            "UserPromptSubmit" | "PreToolUse" | "PostToolUse" | "Stop" | "PreCompact"
        ) {
            value["turn_id"] = json!(format!("{}-turn-{}", session.id, session.turn));
        }
        if matches!(
            event,
            "SessionStart" | "UserPromptSubmit" | "PreToolUse" | "PostToolUse" | "Stop"
        ) {
            value["permission_mode"] = json!("default");
        }
    } else if !matches!(
        event,
        "SessionStart" | "SessionEnd" | "PreCompact" | "Notification"
    ) {
        value["permission_mode"] = json!("default");
    }
    for (key, field) in fields.as_object().unwrap() {
        value[key] = field.clone();
    }
    value
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    sorted[((sorted.len() as f64 * fraction).ceil() as usize).clamp(1, sorted.len()) - 1]
}

impl Trial {
    fn new(name: &'static str, enforce: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        fs::create_dir_all(home.join(".yoke")).unwrap();
        let project = project(temp.path(), "project");
        // Offline digest responders shadow any installed agent CLI for the job worker.
        let fake = temp.path().join("fake-bin");
        fs::create_dir(&fake).unwrap();
        for agent in ["claude", "codex"] {
            fs::write(
                fake.join(agent),
                "#!/bin/sh\n/bin/cat >/dev/null\nprintf '%s' '{\"memories\":[]}'\n",
            )
            .unwrap();
            fs::set_permissions(fake.join(agent), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut path = fake.into_os_string();
        path.push(":/usr/bin:/bin");
        let store = Store::open(home.join(".yoke/state.db")).unwrap();
        // Claim today's update check so session starts never reach the network.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        store
            .set_kv("update.last_attempt", &now.to_string())
            .unwrap();
        let bin = std::env::var_os("YOKE_SOAK_BIN")
            .map_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_yoke")), PathBuf::from);
        // The first exec of a fresh build pays one-time OS checks that installed hooks never see.
        assert!(isolated_command(&bin, &home)
            .arg("--version")
            .output()
            .unwrap()
            .status
            .success());
        Self {
            _temp: temp,
            name,
            home,
            project,
            bin,
            path,
            store,
            enforce,
            readonly: false,
            samples: BTreeMap::new(),
            mismatches: Vec::new(),
            events: 0,
        }
    }

    fn trust(&self, project: &Path, config: &str) {
        fs::write(project.join("yoke.toml"), config).unwrap();
        // The test acts as the human approving exact bytes; agents cannot reach this API.
        trust::prepare_project(&self.store, project)
            .unwrap()
            .commit(&self.store)
            .unwrap();
    }

    fn session(&self, agent: &'static str, id: &str, cwd: &Path) -> Session {
        let directory = self.home.join(if agent == "codex" {
            ".codex/sessions"
        } else {
            ".claude/projects/soak"
        });
        fs::create_dir_all(&directory).unwrap();
        let transcript = directory.join(format!("{id}.jsonl"));
        let header = if agent == "codex" {
            format!(
                "{}\n{}\n",
                json!({"type":"session_meta","payload":{"id":id}}),
                json!({"type":"turn_context","payload":{"model":"fixture-model"}})
            )
        } else {
            String::new()
        };
        fs::write(&transcript, header).unwrap();
        Session {
            agent,
            id: id.into(),
            cwd: cwd.to_path_buf(),
            transcript,
            turn: 0,
            tool: 0,
            tokens: [0, 0],
            plan: false,
        }
    }

    fn last_id(&self) -> i64 {
        let filter = DecisionFilter {
            limit: 1,
            ..Default::default()
        };
        self.store
            .decision_page(&filter, None)
            .unwrap()
            .first()
            .map_or(0, |row| row.id)
    }

    fn hook(&mut self, session: &Session, event: &str, fields: Value, expect: Expect) -> Output {
        let before = if self.readonly { 0 } else { self.last_id() };
        let event_payload = payload(session, event, fields);
        let (output, ms) = run_hook(
            (&self.bin, &self.home, &self.path),
            session.agent,
            event,
            &event_payload,
            &session.cwd,
        );
        self.events += 1;
        self.samples
            .entry(format!("{} {event}", session.agent))
            .or_default()
            .push(ms);
        let mut problems = Vec::new();
        if !output.status.success() {
            problems.push(format!("exit {:?}", output.status.code()));
        }
        match classify(&output.stdout) {
            Ok(out) if out == expect.1 => {}
            Ok(out) => problems.push(format!("stdout {out:?}, expected {:?}", expect.1)),
            Err(error) => problems.push(error),
        }
        // Read-only phases verify that nothing persisted once permissions are restored.
        if !self.readonly {
            let filter = DecisionFilter {
                limit: 1,
                ..Default::default()
            };
            let latest = self
                .store
                .decision_page(&filter, None)
                .unwrap()
                .into_iter()
                .next();
            let persisted = latest
                .filter(|row| row.id > before)
                .map(|row| row.record.outcome);
            if persisted.as_deref() != expect.0 {
                problems.push(format!("decision {persisted:?}, expected {:?}", expect.0));
            }
        }
        if !problems.is_empty() {
            self.mismatches.push(format!(
                "{} #{}: {} {} {}: {}; stderr: {}",
                self.name,
                self.events,
                session.agent,
                session.id,
                event,
                problems.join("; "),
                String::from_utf8_lossy(&output.stderr)
                    .chars()
                    .take(400)
                    .collect::<String>()
            ));
        }
        output
    }

    fn expect_stderr(&mut self, output: &Output, needle: &str) {
        if !String::from_utf8_lossy(&output.stderr).contains(needle) {
            self.mismatches.push(format!(
                "{}: stderr lacks {needle:?}: {}",
                self.name,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }

    fn cli(&self, args: &[&str], envs: &[(&str, &str)]) -> Output {
        isolated_command(&self.bin, &self.home)
            .args(args)
            .envs(envs.iter().copied())
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("PATH", &self.path)
            .env_remove("YOKE_OFF")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn start(&mut self, session: &Session, source: &str) {
        let out = if self.enforce && session.plan {
            Out::Context
        } else {
            Out::Silent
        };
        let fields = if session.agent == "claude" {
            json!({"source": source, "model": "fixture-model"})
        } else {
            json!({"source": source})
        };
        self.hook(session, "SessionStart", fields, Expect(Some("allow"), out));
    }

    fn end(&mut self, session: &Session, expect: Expect) -> Output {
        let reason = if session.agent == "claude" {
            "prompt_input_exit"
        } else {
            "other"
        };
        self.hook(session, "SessionEnd", json!({"reason": reason}), expect)
    }

    fn prompt(&mut self, session: &mut Session, text: &str) {
        session.turn += 1;
        let out = if self.enforce && session.plan {
            Out::Context
        } else {
            Out::Silent
        };
        self.hook(
            session,
            "UserPromptSubmit",
            json!({"prompt": text}),
            Expect(Some("allow"), out),
        );
    }

    /// One tool call: PreToolUse, then PostToolUse unless an enforced denial stopped the tool.
    fn tool(&mut self, session: &mut Session, kind: &str, arg: &str, pre: Expect) {
        session.tool += 1;
        let id = format!("{}-tool-{}", session.id, session.tool);
        let file = session.cwd.join(arg);
        let (name, input, response) = match (session.agent, kind) {
            ("claude", "bash") => (
                "Bash",
                json!({"command": arg, "description": "soak step", "timeout": 120000, "run_in_background": false}),
                json!({"stdout": "ok\n", "stderr": "", "interrupted": false, "isImage": false}),
            ),
            ("claude", "edit") => (
                "Edit",
                json!({"file_path": file, "old_string": "soak()", "new_string": "soak_v2()", "replace_all": false}),
                json!({"filePath": file, "success": true}),
            ),
            ("claude", "write") => (
                "Write",
                json!({"file_path": file, "content": "pub fn added() {}\n"}),
                json!({"filePath": file, "type": "create"}),
            ),
            ("claude", "read") => (
                "Read",
                json!({"file_path": file}),
                json!({"type": "text", "file": {"filePath": file}}),
            ),
            ("codex", "bash") => ("Bash", json!({"command": arg}), json!("ok\n")),
            ("codex", "read") => (
                "Bash",
                json!({"command": format!("cat {arg}")}),
                json!("pub fn soak() {}\n"),
            ),
            ("codex", _) => (
                "apply_patch",
                json!({"command": format!("*** Begin Patch\n*** Update File: {arg}\n@@\n-pub fn soak() {{}}\n+pub fn soak_v2() {{}}\n*** End Patch\n")}),
                json!("Success. Updated the following files:\nM src/lib.rs\n"),
            ),
            _ => unreachable!(),
        };
        let pre_output = self.hook(
            session,
            "PreToolUse",
            json!({"tool_name": name, "tool_input": input, "tool_use_id": id}),
            pre,
        );
        let rewritten: Value = serde_json::from_slice(&pre_output.stdout).unwrap_or(Value::Null);
        let input = rewritten["hookSpecificOutput"]["updatedInput"]
            .as_object()
            .map_or(input, |object| Value::Object(object.clone()));
        if !(self.enforce && pre.1 == Out::Deny) {
            let mut fields = json!({"tool_name": name, "tool_input": input, "tool_response": response, "tool_use_id": id});
            if session.agent == "claude" {
                fields["duration_ms"] = json!(12);
            }
            self.hook(session, "PostToolUse", fields, ALLOW);
        }
    }

    /// The assistant message that ended a turn, with usage recorded the way each agent writes it.
    fn record_usage(&self, session: &mut Session) {
        session.tokens[0] += 1200;
        session.tokens[1] += 300;
        let line = if session.agent == "claude" {
            json!({"type":"assistant","sessionId":session.id,"timestamp":"2026-10-14T09:00:00Z","message":{"id":format!("msg-{}-{}", session.id, session.turn),"model":"fixture-model","usage":{"input_tokens":1200,"output_tokens":300,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}})
        } else {
            json!({"type":"event_msg","timestamp":"2026-10-14T09:00:00Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":session.tokens[0],"cached_input_tokens":0,"output_tokens":session.tokens[1]}}}})
        };
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&session.transcript)
            .unwrap();
        writeln!(file, "{line}").unwrap();
    }

    fn stop(&mut self, session: &mut Session, expect: Expect) -> Output {
        self.record_usage(session);
        let mut fields =
            json!({"stop_hook_active": false, "last_assistant_message": "Done with this step."});
        if session.agent == "claude" {
            fields["background_tasks"] = json!([]);
            fields["session_crons"] = json!([]);
        }
        self.hook(session, "Stop", fields, expect)
    }

    /// A working hour: one prompt, a realistic mix of tools, then Stop.
    fn hour(&mut self, session: &mut Session, stop: Expect) {
        self.prompt(session, "Continue the soak task.");
        for (kind, arg) in [
            ("read", "src/lib.rs"),
            ("bash", "git status --short"),
            ("edit", "src/lib.rs"),
            ("bash", "cargo check"),
            ("read", "README.md"),
            ("write", "src/added.rs"),
            ("bash", "git diff --stat"),
            ("edit", "src/lib.rs"),
            ("bash", "ls src"),
        ] {
            self.tool(session, kind, arg, ALLOW);
        }
        self.stop(session, stop);
    }

    fn compact(&mut self, session: &Session, expect: Expect) -> Output {
        let fields = if session.agent == "claude" {
            json!({"trigger": "auto", "custom_instructions": null})
        } else {
            json!({"trigger": "auto"})
        };
        self.hook(session, "PreCompact", fields, expect)
    }

    /// Create and approve a plan whose single check sleeps for SOAK_CHECK_SLEEP seconds.
    fn approved_plan(&self, slug: &str) {
        let created = self.cli(&["plan", "new", slug], &[]);
        assert!(
            created.status.success(),
            "{}",
            String::from_utf8_lossy(&created.stderr)
        );
        let row = plan::get(&self.store, &self.project, slug).unwrap();
        fs::write(&row.file, "+++\ntitle='Soak'\nreview=false\n[[check]]\nname='unit'\nrun='sleep ${SOAK_CHECK_SLEEP:-0}; test -f README.md'\n+++\nGoal: exercise the plan lifecycle.\n").unwrap();
        let prepared = plan::prepare_approval(&self.store, &self.project, slug).unwrap();
        plan::approve(&self.store, &prepared).unwrap();
    }

    fn plan_start(&mut self, session: &mut Session, slug: &str) {
        let started = self.cli(&["plan", "start", slug, "--session", &session.id], &[]);
        if !started.status.success() {
            self.mismatches.push(format!(
                "{}: plan start {slug}: {}",
                self.name,
                String::from_utf8_lossy(&started.stdout)
            ));
        }
        session.plan = true;
    }

    fn plan_check(&mut self, slug: &str, agent: &str) {
        let checked = self.cli(&["plan", "check", slug, "--agent", agent], &[]);
        let state = plan::get(&self.store, &self.project, slug).unwrap().state;
        if !checked.status.success() || state != "done" {
            self.mismatches.push(format!(
                "{}: plan check {slug} left {state}: {}",
                self.name,
                String::from_utf8_lossy(&checked.stdout)
            ));
        }
    }

    fn plan_state(&mut self, slug: &str, expected: &str) {
        let state = plan::get(&self.store, &self.project, slug).unwrap().state;
        if state != expected {
            self.mismatches.push(format!(
                "{}: plan {slug} is {state}, expected {expected}",
                self.name
            ));
        }
    }

    fn set_readonly(&mut self, readonly: bool) {
        if readonly {
            // Reopening restores the persistent WAL sidecars a read-only fallback needs.
            drop(Store::open(self.home.join(".yoke/state.db")).unwrap());
        }
        let mode = if readonly { 0o444 } else { 0o600 };
        for name in ["state.db", "state.db-wal", "state.db-shm"] {
            fs::set_permissions(
                self.home.join(".yoke").join(name),
                fs::Permissions::from_mode(mode),
            )
            .unwrap();
        }
        self.readonly = readonly;
    }

    fn trial_report(&mut self) -> Value {
        let output = self.cli(&["trial", "report", "--json", "--limit", "1000"], &[]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    /// Print and optionally save aggregates, then fail on any unexpected outcome.
    fn finish(self, report: &Value) {
        let mut latency = BTreeMap::new();
        println!(
            "\n## {} (synthetic soak trial - does not replace the real-work trial)",
            self.name
        );
        println!(
            "events: {}, mismatches: {}",
            self.events,
            self.mismatches.len()
        );
        println!(
            "| agent event | n | p50 ms | p95 ms | p99 ms | max ms |\n|---|---|---|---|---|---|"
        );
        for (key, samples) in &self.samples {
            let mut sorted = samples.clone();
            sorted.sort_by(f64::total_cmp);
            let row = [
                percentile(&sorted, 0.5),
                percentile(&sorted, 0.95),
                percentile(&sorted, 0.99),
                *sorted.last().unwrap(),
            ];
            println!(
                "| {key} | {} | {:.1} | {:.1} | {:.1} | {:.1} |",
                sorted.len(),
                row[0],
                row[1],
                row[2],
                row[3]
            );
            latency.insert(key.clone(), json!({"n": sorted.len(), "p50": row[0], "p95": row[1], "p99": row[2], "max": row[3]}));
        }
        for mismatch in &self.mismatches {
            println!("MISMATCH {mismatch}");
        }
        let error_log = fs::read_to_string(self.home.join(".yoke/error.log")).unwrap_or_default();
        for line in error_log.lines() {
            println!("error.log: {}", line.chars().take(300).collect::<String>());
        }
        if let Some(directory) = std::env::var_os("YOKE_SOAK_OUT") {
            let summary = json!({
                "label": "Synthetic soak trial - does not replace the real-work trial",
                "scenario": self.name,
                "events": self.events,
                "mismatches": self.mismatches,
                "latency_ms": latency,
                "error_log_lines": error_log.lines().count(),
                "trial_report": {"cohorts": report["cohorts"], "flagged_total": report["flagged_total"], "incidents": report["incidents"].as_array().map(|list| list.iter().map(|incident| incident["kind"].clone()).collect::<Vec<_>>()), "acceptance": report["acceptance"]},
            });
            fs::write(
                Path::new(&directory).join(format!("{}.json", self.name)),
                serde_json::to_string_pretty(&summary).unwrap(),
            )
            .unwrap();
        }
        assert!(
            self.mismatches.is_empty(),
            "{} unexpected outcomes:\n{}",
            self.mismatches.len(),
            self.mismatches.join("\n")
        );
    }
}

const OBSERVE: &str = "mode='observe'\n[[gate]]\nname='no-force-push'\non='pre_tool_use'\nmatch_tool='Bash'\nmatch_contains='push --force'\naction='deny'\nmessage='Force push is blocked by Yoke.'\n[[gate]]\nname='readme-present'\non='stop'\nrun='test -f README.md'\nmessage='README.md is missing'\n";

#[test]
#[ignore = "synthetic soak trial; run explicitly with --ignored"]
fn observe_day() {
    let mut trial = Trial::new("observe-day", false);
    trial.trust(&trial.project.clone(), OBSERVE);
    let would_deny = Expect(Some("would_deny"), Out::Silent);
    let mut flagged = 0;
    for agent in ["claude", "codex"] {
        let cwd = trial.project.clone();
        // Morning and afternoon sessions: eight compressed working hours per agent.
        for (part, source) in [("morning", "startup"), ("afternoon", "resume")] {
            let mut session = trial.session(agent, &format!("{agent}-observe-{part}"), &cwd);
            trial.start(&session, source);
            for hour in 0..4 {
                if part == "afternoon" && hour == 0 && agent == "claude" {
                    trial.approved_plan("soak-observe");
                    trial.plan_start(&mut session, "soak-observe");
                }
                // Observe mode reports a building plan's Stop as would_deny and never mutates it.
                let plan_open = part == "afternoon" && agent == "claude" && hour < 3;
                trial.hour(&mut session, if plan_open { would_deny } else { ALLOW });
                flagged += usize::from(plan_open);
                match (part, hour) {
                    ("morning", 1) => {
                        trial.prompt(&mut session, "Publish the branch.");
                        trial.tool(
                            &mut session,
                            "bash",
                            "git push --force origin main",
                            would_deny,
                        );
                        trial.tool(
                            &mut session,
                            "bash",
                            "sqlite3 ~/.yoke/state.db 'select count(*) from decisions'",
                            would_deny,
                        );
                        trial.stop(&mut session, ALLOW);
                        flagged += 2;
                    }
                    ("morning", 2) => {
                        trial.compact(&session, ALLOW);
                    }
                    ("afternoon", 2) if agent == "claude" => {
                        trial.plan_check("soak-observe", "claude");
                    }
                    _ => {}
                }
                if agent == "claude" && hour == 3 {
                    trial.hook(&session, "Notification", json!({"message": "Claude is waiting for your input", "notification_type": "idle_prompt"}), ALLOW);
                }
            }
            trial.end(&session, ALLOW);
            let usage = yoke_core::usage::summaries(&trial.store, None, Some(&session.id)).unwrap();
            let total: u64 = usage
                .iter()
                .map(|row| row.input_tokens + row.output_tokens)
                .sum();
            if total != session.tokens[0] + session.tokens[1] {
                trial.mismatches.push(format!(
                    "usage for {} is {total}, expected {}",
                    session.id,
                    session.tokens[0] + session.tokens[1]
                ));
            }
        }
    }
    trial.plan_state("soak-observe", "done");
    let report = trial.trial_report();
    assert_eq!(report["flagged_total"], flagged, "{report}");
    for criterion in report["acceptance"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|criterion| criterion["agent"].is_string())
    {
        // Compressed time never covers a real working day.
        assert_eq!(criterion["status"], "not_met", "{criterion}");
    }
    trial.finish(&report);
}

const ENFORCE: &str = "mode='enforce'\n[stop]\nmax_blocks=2\n[memory]\ndigest=true\ndigest_agent='codex'\n[[gate]]\nname='no-force-push'\non='pre_tool_use'\nmatch_tool='Bash'\nmatch_contains='push --force'\naction='deny'\nmessage='Force push is blocked by Yoke.'\n[[gate]]\nname='quiet-tests'\non='pre_tool_use'\nmatch_tool='Bash'\nmatch_contains='cargo test'\naction='rewrite'\nrewrite_pattern='^cargo test$'\nrewrite_to='cargo test --quiet'\n[[gate]]\nname='readme-present'\non='stop'\nrun='test -f README.md'\nmessage='README.md is missing'\n";

#[test]
fn plan_check_faults_are_bounded_and_still_reach_the_stop_cap() {
    let mut trial = Trial::new("plan-check-faults", true);
    trial.trust(
        &trial.project.clone(),
        "mode='enforce'\n[stop]\nmax_blocks=2\n",
    );
    let mut session = trial.session("claude", "check-faults", &trial.project);
    trial.start(&session, "startup");
    trial.approved_plan("check-faults");
    let row = plan::get(&trial.store, &trial.project, "check-faults").unwrap();
    fs::write(&row.file, "+++\ntitle='Fault injection'\n[[check]]\nname='noisy'\nrun='yes noise | head -c 1048576'\n[[check]]\nname='hung'\nrun='sleep 30'\ntimeout=1\n[[check]]\nname='dirty'\nrun='printf changed >> README.md'\n+++\nGoal: failed checks cannot complete the plan.\n").unwrap();
    plan::approve(
        &trial.store,
        &plan::prepare_approval(&trial.store, &trial.project, "check-faults").unwrap(),
    )
    .unwrap();
    trial.plan_start(&mut session, "check-faults");
    let checked = trial.cli(
        &[
            "plan",
            "check",
            "check-faults",
            "--agent",
            "claude",
            "--json",
        ],
        &[],
    );
    assert!(!checked.status.success());
    let detail = plan::detail(&trial.store, &trial.project, "check-faults").unwrap();
    assert_eq!(detail.plan.state, "building");
    let runs = &detail.checks;
    assert_eq!(runs[0].result, "pass");
    assert!(runs[0].output_tail.len() <= 4096);
    assert!(runs[0].output_tail.contains("noise"));
    assert_eq!(runs[1].result, "timeout");
    assert!(runs[1].ms < 15_000);
    assert_eq!(runs[2].result, "pass");
    assert_eq!(runs[3].name, "evidence");
    assert!(runs[3].output_tail.contains("working tree changed"));
    trial.stop(&mut session, Expect(Some("deny"), Out::Block));
    trial.stop(&mut session, Expect(Some("deny"), Out::Block));
    trial.stop(&mut session, ALLOW);
    trial.plan_state("check-faults", "stalled");
    assert!(
        trial.mismatches.is_empty(),
        "{}",
        trial.mismatches.join("\n")
    );
}

#[test]
#[ignore = "synthetic soak trial; run explicitly with --ignored"]
fn enforce_week() {
    let mut trial = Trial::new("enforce-week", true);
    let main = trial.project.clone();
    trial.trust(&main, ENFORCE);
    let deny = Expect(Some("deny"), Out::Deny);
    for day in 1..=5 {
        for agent in ["claude", "codex"] {
            let mut session = trial.session(agent, &format!("{agent}-enforce-day{day}"), &main);
            trial.start(&session, "startup");
            for hour in 0..6 {
                trial.hour(&mut session, ALLOW);
                if hour == 2 {
                    trial.compact(&session, ALLOW);
                }
            }
            trial.prompt(&mut session, "Run the tests and publish.");
            trial.tool(
                &mut session,
                "bash",
                "cargo test",
                Expect(Some("rewrite"), Out::Rewrite),
            );
            trial.tool(&mut session, "bash", "git push --force origin main", deny);
            trial.tool(
                &mut session,
                "bash",
                "sqlite3 ~/.yoke/state.db .tables",
                deny,
            );
            trial.stop(&mut session, ALLOW);
            match (day, agent) {
                (1, "codex") => plan_stalls_and_resumes(&mut trial, &mut session),
                (2, "claude") => interrupted_check_resumes(&mut trial, &mut session),
                (2, "codex") => oversized_transcripts(&mut trial, &mut session),
                (3, "claude") => readonly_state(&mut trial, &mut session),
                (3, "codex") => stale_and_untrusted_config(&mut trial, &mut session),
                (4, "claude") => concurrent_sessions(&mut trial),
                (4, "codex") => session_moves_between_projects(&mut trial, &mut session),
                (5, "codex") => digest_over_a_thousand_facts(&mut trial),
                _ => {}
            }
            let ended = trial.end(&session, ALLOW);
            if session.id == "codex-enforce-day2" {
                assert!(!String::from_utf8_lossy(&ended.stderr).contains("usage ingestion failed"));
            }
        }
    }
    let report = trial.trial_report();
    trial.finish(&report);
}

fn plan_stalls_and_resumes(trial: &mut Trial, session: &mut Session) {
    let block = Expect(Some("deny"), Out::Block);
    trial.approved_plan("soak-stall");
    trial.plan_start(session, "soak-stall");
    trial.hour(session, block);
    trial.hour(session, block);
    // The third Stop without progress stalls the plan and lets the agent stop.
    trial.hour(session, ALLOW);
    trial.plan_state("soak-stall", "stalled");
    plan::resume(&trial.store, &trial.project, "soak-stall").unwrap();
    trial.hour(session, block);
    trial.plan_check("soak-stall", "codex");
    trial.hour(session, ALLOW);
}

fn interrupted_check_resumes(trial: &mut Trial, session: &mut Session) {
    trial.approved_plan("soak-interrupted");
    trial.plan_start(session, "soak-interrupted");
    let mut check = isolated_command(&trial.bin, &trial.home)
        .args(["plan", "check", "soak-interrupted", "--agent", "claude"])
        .current_dir(&trial.project)
        .env("HOME", &trial.home)
        .env("PATH", &trial.path)
        .env("SOAK_CHECK_SLEEP", "20")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while plan::get(&trial.store, &trial.project, "soak-interrupted")
        .unwrap()
        .state
        != "checking"
    {
        assert!(Instant::now() < deadline, "plan never reached checking");
        std::thread::sleep(Duration::from_millis(20));
    }
    check.kill().unwrap();
    check.wait().unwrap();
    // A crashed check leaves the plan checking; Stop keeps the agent working until a human recovers it.
    trial.hour(session, Expect(Some("deny"), Out::Block));
    if let Err(error) = plan::resume(&trial.store, &trial.project, "soak-interrupted") {
        trial
            .mismatches
            .push(format!("resume after a killed check failed: {error:#}"));
    }
    trial.plan_state("soak-interrupted", "building");
    trial.plan_check("soak-interrupted", "claude");
    trial.hour(session, ALLOW);
}

#[test]
#[ignore = "oversized transcript scenario; run explicitly with --ignored"]
fn oversized_transcripts_remain_non_fatal() {
    let mut trial = Trial::new("oversized-transcripts", true);
    let project = trial.project.clone();
    trial.trust(&project, "mode='enforce'");
    let mut session = trial.session("codex", "oversized", &project);
    trial.start(&session, "startup");
    oversized_transcripts(&mut trial, &mut session);
    let report = trial.trial_report();
    trial.finish(&report);
}

fn oversized_transcripts(trial: &mut Trial, session: &mut Session) {
    // The incremental importer skips oversized non-usage lines and reads beyond the old
    // whole-file bound, so these transcripts must not fail accounting or block hooks.
    for (name, contents) in [
        ("long-line", format!("{}\n", " ".repeat(MIB + 1))),
        (
            "total-size",
            format!("{}\n", " ".repeat(MIB / 2 - 1)).repeat(65),
        ),
    ] {
        session.transcript = session
            .transcript
            .with_file_name(format!("{}-{name}.jsonl", session.id));
        fs::write(&session.transcript, contents).unwrap();
        let stopped = trial.hour_returning_stop(session);
        assert!(!String::from_utf8_lossy(&stopped.stderr).contains("usage ingestion failed"));
        let compacted = trial.compact(session, ALLOW);
        assert!(!String::from_utf8_lossy(&compacted.stderr).contains("usage ingestion failed"));
    }
}

fn readonly_state(trial: &mut Trial, session: &mut Session) {
    let before = trial.last_id();
    trial.set_readonly(true);
    trial.prompt(session, "Continue.");
    trial.tool(session, "bash", "git status", ALLOW);
    trial.tool(
        session,
        "bash",
        "git push --force origin main",
        Expect(None, Out::Deny),
    );
    let stopped = trial.stop(session, ALLOW);
    trial.expect_stderr(&stopped, "stop audit was not saved");
    trial.set_readonly(false);
    if trial.last_id() != before {
        trial
            .mismatches
            .push("read-only state persisted a decision".into());
    }
}

fn stale_and_untrusted_config(trial: &mut Trial, session: &mut Session) {
    let main = trial.project.clone();
    let trusted = fs::read_to_string(main.join("yoke.toml")).unwrap();
    fs::write(main.join("yoke.toml"), "mode='observe'\n").unwrap();
    let mut stale = trial.session("codex", "codex-stale", &main);
    let started = trial.hook(&stale, "SessionStart", json!({"source": "startup"}), ALLOW);
    trial.expect_stderr(&started, "stale (using trusted bytes)");
    trial.prompt(&mut stale, "Push it.");
    trial.tool(
        &mut stale,
        "bash",
        "git push --force origin main",
        Expect(Some("deny"), Out::Deny),
    );
    trial.stop(&mut stale, ALLOW);
    fs::write(main.join("yoke.toml"), trusted).unwrap();
    let untrusted = project(trial._temp.path(), "untrusted");
    fs::write(
        untrusted.join("yoke.toml"),
        "mode='enforce'\n[[gate]]\nname='deny-all'\non='pre_tool_use'\naction='deny'\n",
    )
    .unwrap();
    let mut outside = trial.session("codex", "codex-untrusted", &untrusted);
    let started = trial.hook(
        &outside,
        "SessionStart",
        json!({"source": "startup"}),
        ALLOW,
    );
    trial.expect_stderr(&started, "untrusted (ignored)");
    trial.prompt(&mut outside, "Look around.");
    trial.tool(&mut outside, "bash", "ls", ALLOW);
    trial.stop(&mut outside, ALLOW);
    let _ = session;
}

fn concurrent_sessions(trial: &mut Trial) {
    let before = trial.last_id();
    let handles: Vec<_> = ["claude", "codex"]
        .into_iter()
        .map(|agent| {
            let session = trial.session(agent, &format!("{agent}-concurrent"), &trial.project);
            let context = (trial.bin.clone(), trial.home.clone(), trial.path.clone());
            std::thread::spawn(move || {
                let mut results = Vec::new();
                for step in 0..60 {
                    let (event, fields) = match step % 3 {
                        0 => ("PreToolUse", json!({"tool_name": "Bash", "tool_input": {"command": "git status"}, "tool_use_id": format!("c-{step}")})),
                        1 => ("PostToolUse", json!({"tool_name": "Bash", "tool_input": {"command": "git status"}, "tool_response": "ok", "tool_use_id": format!("c-{step}")})),
                        _ => ("UserPromptSubmit", json!({"prompt": "keep going"})),
                    };
                    let event_payload = payload(&session, event, fields);
                    let (output, ms) = run_hook((&context.0, &context.1, &context.2), agent, event, &event_payload, &session.cwd);
                    results.push((format!("{agent} {event} (concurrent)"), output, ms));
                }
                results
            })
        })
        .collect();
    for handle in handles {
        for (key, output, ms) in handle.join().unwrap() {
            trial.events += 1;
            trial.samples.entry(key.clone()).or_default().push(ms);
            if !output.status.success()
                || classify(&output.stdout) != Ok(Out::Silent)
                || !output.stderr.is_empty()
            {
                trial.mismatches.push(format!(
                    "{key}: exit {:?}, stdout {}, stderr {}",
                    output.status.code(),
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }
    }
    let rows = trial
        .store
        .decision_page(
            &DecisionFilter {
                limit: 1000,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let new: Vec<_> = rows.iter().filter(|row| row.id > before).collect();
    if new.len() != 120 || new.iter().any(|row| row.record.outcome != "allow") {
        trial.mismatches.push(format!(
            "concurrent sessions persisted {} decisions, expected 120 allows",
            new.len()
        ));
    }
}

fn session_moves_between_projects(trial: &mut Trial, session: &mut Session) {
    let other = project(trial._temp.path(), "other");
    trial.trust(&other, "mode='enforce'\n[[gate]]\nname='no-deploy'\non='pre_tool_use'\nmatch_tool='Bash'\nmatch_contains='make deploy'\naction='deny'\nmessage='Deploys need a human.'\n");
    trial.prompt(session, "Deploy from here.");
    trial.tool(session, "bash", "make deploy", ALLOW);
    trial.stop(session, ALLOW);
    // A cd cannot replace this session's anchored policy with another project's policy.
    session.cwd = other.clone();
    trial.prompt(session, "Deploy from the other repo.");
    trial.tool(session, "bash", "make deploy", ALLOW);
    trial.tool(
        session,
        "bash",
        "git push --force origin main",
        Expect(Some("deny"), Out::Deny),
    );
    trial.stop(session, ALLOW);
    let sessions = trial.store.sessions(None, 1000).unwrap();
    let moved = sessions.iter().find(|row| row.id == session.id).unwrap();
    if moved.project != trial.project.display().to_string() {
        trial.mismatches.push(format!(
            "cd changed the anchored project to {}",
            moved.project
        ));
    }
    session.cwd = trial.project.clone();
}

fn digest_over_a_thousand_facts(trial: &mut Trial) {
    let mut session = trial.session("codex", "codex-digest", &trial.project.clone());
    trial.start(&session, "startup");
    trial.prompt(&mut session, "Apply the large refactor.");
    for step in 0..1005 {
        session.tool += 1;
        let fields = json!({"tool_name": "apply_patch", "tool_input": {"command": format!("*** Begin Patch\n*** Add File: src/generated_{step}.rs\n+pub fn generated() {{}}\n*** End Patch\n")}, "tool_response": "Success.", "tool_use_id": format!("digest-{step}")});
        trial.hook(&session, "PostToolUse", fields, ALLOW);
    }
    trial.stop(&mut session, ALLOW);
    let connection = rusqlite::Connection::open(trial.home.join(".yoke/state.db")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let (pending, active): (i64, i64) = connection
            .query_row(
                "SELECT (SELECT count(*) FROM facts WHERE session_id=?1 AND digested=0),
                        (SELECT count(*) FROM jobs WHERE state IN ('queued','running','failed'))",
                [&session.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        if pending == 0 && active == 0 {
            break;
        }
        if Instant::now() > deadline {
            trial.mismatches.push(format!(
                "digest left {pending} facts and {active} active jobs after 120s"
            ));
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let dead: i64 = connection
        .query_row("SELECT count(*) FROM jobs WHERE state='dead'", [], |row| {
            row.get(0)
        })
        .unwrap();
    if dead != 0 {
        trial.mismatches.push(format!("{dead} dead digest jobs"));
    }
    trial.end(&session, ALLOW);
}

impl Trial {
    fn hour_returning_stop(&mut self, session: &mut Session) -> Output {
        self.prompt(session, "Continue.");
        self.tool(session, "bash", "git status", ALLOW);
        self.stop(session, ALLOW)
    }
}
