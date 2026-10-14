use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

fn run(home: &Path, project: &Path, args: &[&str], input: &[u8]) -> Output {
    run_with_path(home, project, args, input, None)
}

fn run_with_path(
    home: &Path,
    project: &Path,
    args: &[&str],
    input: &[u8],
    path: Option<&Path>,
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_yoke"));
    command
        .args(args)
        .env("HOME", home)
        .current_dir(project)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = path {
        command.env("PATH", path);
    }
    let mut child = command.spawn().unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

fn json(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn memory_transfer_preserves_identity_and_fresh_home_dry_run_writes_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    let home = temp.path().join("home");
    let fresh = temp.path().join("fresh");
    for path in [&source, &destination, &home, &fresh] {
        fs::create_dir(path).unwrap();
    }
    let note = json(run(
        &home,
        &source,
        &["remember", "Keep the migration reversible", "--pinned"],
        b"",
    ));
    let archive = temp.path().join("memory.json");
    let exported = json(run(
        &home,
        &source,
        &["memory", "export", archive.to_str().unwrap()],
        b"",
    ));
    assert_eq!(exported["memories"], 1);
    let mapping = format!(
        "{}={}",
        fs::canonicalize(&source).unwrap().display(),
        fs::canonicalize(&destination).unwrap().display()
    );
    let args = [
        "memory",
        "import",
        archive.to_str().unwrap(),
        "--map",
        &mapping,
    ];
    let mut dry_args = args.to_vec();
    dry_args.push("--dry-run");
    let dry = json(run(&fresh, &destination, &dry_args, b""));
    assert_eq!(dry["additions"], 1);
    assert_eq!(dry["dry_run"], true);
    assert_eq!(fs::read_dir(&fresh).unwrap().count(), 0);
    let imported = json(run(&fresh, &destination, &args, b""));
    assert_eq!(imported["additions"], 1);
    let repeated = json(run(&fresh, &destination, &args, b""));
    assert_eq!(repeated["additions"], 0);
    assert_eq!(repeated["duplicates"], 1);
    let recalled = json(run(
        &fresh,
        &destination,
        &["recall", "migration", "--json"],
        b"",
    ));
    assert_eq!(recalled.as_array().unwrap().len(), 1);
    for key in ["identity", "ts", "source", "text", "pinned"] {
        assert_eq!(recalled[0][key], note[key], "{key}");
    }
    assert!(!destination.join("yoke.toml").exists());
    assert!(!destination.join(".yoke").exists());
    let store = yoke_core::store::Store::open(fresh.join(".yoke/state.db")).unwrap();
    store
        .set_kv(
            "update.last_attempt",
            &std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                .to_string(),
        )
        .unwrap();
    for mode in ["enforce", "observe"] {
        fs::write(destination.join("yoke.toml"), format!("mode='{mode}'\n")).unwrap();
        yoke_core::trust::prepare_project(&store, &destination)
            .unwrap()
            .commit(&store)
            .unwrap();
        let output = run(
            &fresh,
            &destination,
            &["run-hook", "SessionStart", "--agent", "claude"],
            br#"{"session_id":"memory-context"}"#,
        );
        assert!(output.status.success());
        if mode == "enforce" {
            let wire: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert!(wire["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap()
                .contains("Keep the migration reversible"));
        } else {
            assert!(output.stdout.is_empty());
        }
    }
}

#[test]
fn statusline_fresh_home_and_wrapper_are_read_only() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let input = br#"{"context_window":{"used_percentage":42}}"#;
    let output = run(&home, temp.path(), &["statusline", "--wrap", "cat"], input);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!(
            "yoke unrecorded | ctx 42% | {}\n",
            String::from_utf8_lossy(input)
        )
    );
    assert_eq!(fs::read_dir(&home).unwrap().count(), 0);
}

#[test]
fn digest_rejects_fifo_without_waiting_for_a_writer() {
    use std::time::{Duration, Instant};
    use yoke_core::{store::Store, trust};
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let project = temp.path().join("project");
    fs::create_dir_all(home.join(".yoke")).unwrap();
    fs::create_dir(&project).unwrap();
    fs::write(project.join("yoke.toml"), "[memory]\ndigest=true\n").unwrap();
    let store = Store::open(home.join(".yoke/state.db")).unwrap();
    trust::prepare_project(&store, &project)
        .unwrap()
        .commit(&store)
        .unwrap();
    let fifo = temp.path().join("transcript.fifo");
    assert!(Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    let mut child = Command::new(env!("CARGO_BIN_EXE_yoke"))
        .args(["memory", "digest", "fixture", "--transcript"])
        .arg(&fifo)
        .env("HOME", &home)
        .current_dir(&project)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("digest blocked opening FIFO before its bounded process runner");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("regular file"));
}

#[test]
fn stop_schedules_bounded_digest_with_off_and_debounces_repeat() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};
    use yoke_core::{memory, store::Store, trust};
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let project = temp.path().join("project");
    let bin = temp.path().join("bin");
    fs::create_dir_all(home.join(".yoke")).unwrap();
    fs::create_dir(&project).unwrap();
    fs::create_dir(&bin).unwrap();
    let project = project.canonicalize().unwrap();
    fs::write(
        project.join("yoke.toml"),
        "mode='enforce'\n[memory]\ndigest=true\ndigest_agent='codex'\n",
    )
    .unwrap();
    let store = Store::open(home.join(".yoke/state.db")).unwrap();
    trust::prepare_project(&store, &project)
        .unwrap()
        .commit(&store)
        .unwrap();
    let fact = memory::record_fact(
        &store,
        "digest-session",
        project.to_str().unwrap(),
        "command_failure",
        "exit 7",
        1,
    )
    .unwrap();
    let script = bin.join("codex");
    fs::write(&script, format!("#!/bin/sh\nprintf '%s\\n' \"$YOKE_OFF\" >> \"$HOME/digest-calls\"\nprintf '%s\\n' \"$@\" > \"$HOME/digest-args\"\n/bin/cat > \"$HOME/digest-input\"\nprintf '%s' '{{\"memories\":[{{\"text\":\"Fixture digest\",\"fact_ids\":[{fact}]}}]}}'\n")).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    for _ in 0..2 {
        let mut child = Command::new(env!("CARGO_BIN_EXE_yoke"))
            .args(["run-hook", "Stop", "--agent", "codex"])
            .env("HOME", &home)
            .env("PATH", &bin)
            .env_remove("YOKE_OFF")
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
            .write_all(br#"{"session_id":"digest-session"}"#)
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let notes = memory::recall(&store, project.to_str().unwrap(), "Fixture", 10).unwrap();
        if let Some(note) = notes.first() {
            assert_eq!(note.source, "digest");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "digest did not persist: {:?}",
            fs::read_to_string(home.join(".yoke/error.log"))
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    loop {
        let jobs = json(run(&home, &project, &["jobs", "--all", "--json"], b""));
        let jobs = jobs.as_array().unwrap();
        assert_eq!(jobs.len(), 1, "one deduplicated digest job");
        if jobs[0]["state"] == "succeeded" {
            assert_eq!(jobs[0]["kind"], "memory_digest");
            assert_eq!(jobs[0]["attempts"], 1);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "digest job did not finish: {jobs:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        fs::read_to_string(home.join("digest-calls")).unwrap(),
        "1\n"
    );
    let args = fs::read_to_string(home.join("digest-args")).unwrap();
    assert!(args.contains("--ephemeral\n"));
    assert!(args.contains("read-only\n"));
    assert!(fs::read_to_string(home.join("digest-input"))
        .unwrap()
        .contains("exit 7"));
}

#[test]
fn plan_cli_records_session_waits_for_human_and_completes_real_check() {
    use yoke_core::{plan, store::Store, trust};
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let project = temp.path().join("project");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&project).unwrap();
    assert!(project.starts_with(std::env::temp_dir()));
    for args in [
        vec!["init", "-q"],
        vec!["add", "tracked"],
        vec!["commit", "-qm", "fixture"],
    ] {
        fs::write(project.join("tracked"), "fixture\n").unwrap();
        let output = Command::new("git")
            .current_dir(&project)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgSign=false",
                "-c",
                "user.name=Yoke Fixture",
                "-c",
                "user.email=fixture@example.invalid",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fs::write(project.join("yoke.toml"), "mode='enforce'\n").unwrap();
    fs::create_dir(home.join(".yoke")).unwrap();
    let store = Store::open(home.join(".yoke/state.db")).unwrap();
    trust::prepare_project(&store, &project)
        .unwrap()
        .commit(&store)
        .unwrap();
    let created = run(&home, &project, &["plan", "new", "fixture"], b"");
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let row = plan::get(&store, &project, "fixture").unwrap();
    fs::write(&row.file, "+++\ntitle='Fixture'\nreview=false\n[[check]]\nname='real shell'\nrun='printf checked'\n+++\nGoal: verify CLI workflow.\n").unwrap();
    let refused = run(
        &home,
        &project,
        &["plan", "approve", "fixture"],
        b"approve fixture\n",
    );
    assert!(!refused.status.success());
    assert_eq!(
        plan::get(&store, &project, "fixture").unwrap().state,
        "draft"
    );
    // Fixture preparation exercises the accepted exact-byte API, never bypasses CLI human checks.
    let prepared = plan::prepare_approval(&store, &project, "fixture").unwrap();
    plan::approve(&store, &prepared).unwrap();
    let hook = run(
        &home,
        &project,
        &["run-hook", "PostToolUse", "--agent", "codex"],
        br#"{"session_id":"plan-fixture","tool_name":"Bash","tool_input":{"command":"true"}}"#,
    );
    assert!(hook.status.success());
    assert!(hook.stdout.is_empty());
    let started = json(run(
        &home,
        &project,
        &["plan", "start", "fixture", "--session", "plan-fixture"],
        b"",
    ));
    assert_eq!(started["state"], "building");
    let asked = json(run(
        &home,
        &project,
        &[
            "plan",
            "ask",
            "Which behavior?",
            "--session",
            "plan-fixture",
        ],
        b"",
    ));
    assert_eq!(asked["state"], "waiting");
    let prompt = run(
        &home,
        &project,
        &["run-hook", "UserPromptSubmit", "--agent", "codex"],
        br#"{"session_id":"plan-fixture","prompt":"continue"}"#,
    );
    assert!(prompt.status.success());
    let context: Value = serde_json::from_slice(&prompt.stdout).unwrap();
    assert!(context["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .contains("yoke plan resume fixture"));
    assert_eq!(
        plan::get(&store, &project, "fixture").unwrap().state,
        "waiting"
    );
    assert!(!run(
        &home,
        &project,
        &["plan", "resume", "fixture"],
        b"resume fixture\n"
    )
    .status
    .success());
    for args in [
        &["plan", "cancel", "fixture"][..],
        &["plan", "reassign", "fixture", "--session", "other"][..],
    ] {
        let input = b"cancel fixture\nreassign fixture\n";
        assert!(!run(&home, &project, args, input).status.success());
    }
    assert_eq!(
        plan::get(&store, &project, "fixture").unwrap().state,
        "waiting"
    );
    plan::resume(&store, &project, "fixture").unwrap();
    use std::os::unix::fs::PermissionsExt;
    let fake_bin = temp.path().join("fake-bin");
    fs::create_dir(&fake_bin).unwrap();
    fs::write(fake_bin.join("codex"), "#!/bin/sh\n/bin/cat >/dev/null\nprintf called > \"$HOME/digest-started\"\nprintf '%s' '{\"memories\":[]}'\n").unwrap();
    fs::set_permissions(fake_bin.join("codex"), fs::Permissions::from_mode(0o755)).unwrap();
    let fake_path = std::path::PathBuf::from(format!("{}:/usr/bin:/bin", fake_bin.display()));
    for mode in ["observe", "enforce"] {
        fs::write(
            project.join("yoke.toml"),
            format!("mode='{mode}'\n[stop]\nmax_blocks=1\n[memory]\ndigest=true\ndigest_agent='codex'\n"),
        )
        .unwrap();
        trust::prepare_project(&store, &project)
            .unwrap()
            .commit(&store)
            .unwrap();
        for index in 0..2 {
            if mode == "enforce" && index == 1 {
                let before = plan::detail(&store, &project, "fixture").unwrap();
                let invalid = serde_json::json!({"session_id":"plan-fixture","transcript_path":home.join("missing-transcript")});
                let capped = run_with_path(
                    &home,
                    &project,
                    &["run-hook", "Stop", "--agent", "codex"],
                    invalid.to_string().as_bytes(),
                    Some(&fake_path),
                );
                assert!(capped.status.success());
                assert!(String::from_utf8_lossy(&capped.stderr).contains("usage ingestion failed"));
                assert!(
                    capped.stdout.is_empty(),
                    "unreadable usage data must not block a capped stop"
                );
                let after = plan::detail(&store, &project, "fixture").unwrap();
                assert_eq!(
                    after.plan.state, "stalled",
                    "the stop guard cap must still apply"
                );
                assert_eq!(after.transitions.len(), before.transitions.len() + 1);
            }
            let stopped = run_with_path(
                &home,
                &project,
                &["run-hook", "Stop", "--agent", "codex"],
                br#"{"session_id":"plan-fixture"}"#,
                Some(&fake_path),
            );
            assert!(stopped.status.success());
            if mode == "enforce" && index == 0 {
                let output: Value = serde_json::from_slice(&stopped.stdout).unwrap();
                assert_eq!(output["decision"], "block");
                assert!(output["reason"].as_str().unwrap().contains("--agent codex"));
            } else {
                assert!(stopped.stdout.is_empty());
            }
        }
        assert_eq!(
            plan::get(&store, &project, "fixture").unwrap().state,
            if mode == "observe" {
                "building"
            } else {
                "stalled"
            }
        );
    }
    plan::resume(&store, &project, "fixture").unwrap();
    let checked = json(run(
        &home,
        &project,
        &["plan", "check", "fixture", "--agent", "codex"],
        b"",
    ));
    assert_eq!(checked["passed"], true);
    assert_eq!(checked["plan"]["state"], "done");
    assert_eq!(checked["checks"][0]["output_tail"], "checked");
    fs::write(
        fake_bin.join("claude"),
        "#!/bin/sh\n/bin/cat >/dev/null\nprintf changed >> tracked\nprintf '%s' '[]'\n",
    )
    .unwrap();
    fs::set_permissions(fake_bin.join("claude"), fs::Permissions::from_mode(0o755)).unwrap();
    let reviewed = run_with_path(
        &home,
        &project,
        &["review", "fixture", "--agent", "codex"],
        b"",
        Some(&fake_path),
    );
    assert!(
        !reviewed.status.success(),
        "concurrent tree edits must fail standalone review"
    );
    let report: Value = serde_json::from_str(
        String::from_utf8_lossy(&reviewed.stdout)
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(report["passed"], false);
    let detail = plan::detail(&store, &project, "fixture").unwrap();
    assert!(detail
        .checks
        .iter()
        .any(|check| check.result == "void" && check.name == "review"));
}

#[test]
fn plan_cli_validates_tracks_tasks_and_keeps_human_actions_human() {
    use std::os::unix::fs::PermissionsExt;
    use yoke_core::{plan, store::Store, trust};
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let project = temp.path().join("project");
    fs::create_dir_all(home.join(".yoke")).unwrap();
    fs::create_dir(&project).unwrap();
    assert!(project.starts_with(std::env::temp_dir()));
    fs::write(project.join("yoke.toml"), "mode='enforce'\n").unwrap();
    for args in [
        &["init", "-q"][..],
        &["add", "yoke.toml"],
        &["commit", "-qm", "fixture"],
    ] {
        let output = Command::new("git")
            .current_dir(&project)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgSign=false",
            ])
            .args([
                "-c",
                "user.name=Yoke Fixture",
                "-c",
                "user.email=fixture@example.invalid",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let store = Store::open(home.join(".yoke/state.db")).unwrap();
    trust::prepare_project(&store, &project)
        .unwrap()
        .commit(&store)
        .unwrap();
    assert!(run(&home, &project, &["plan", "new", "fixture"], b"")
        .status
        .success());
    let row = plan::get(&store, &project, "fixture").unwrap();
    fs::write(&row.file, "+++\ntitle='Fixture'\n[[check]]\nname='unit'\nrun='true'\n[[requirement]]\nid='works'\ntext='Unit check passes'\nchecks=['unit']\n[[requirement]]\nid='copy'\ntext='A human approves the copy'\nhuman=true\n[[task]]\nid='build'\ntitle='Build it'\n[[task]]\nid='signoff'\ntitle='Sign off'\ndepends_on=['build']\nowner='human'\n+++\n\n## Rollout\n").unwrap();
    let fake_bin = temp.path().join("fake-bin");
    fs::create_dir(&fake_bin).unwrap();
    fs::write(fake_bin.join("codex"), "#!/bin/sh\n/bin/cat >/dev/null\nprintf '%s' '[{\"severity\":\"blocking\",\"anchor\":\"requirement:copy\",\"summary\":\"Name the copy reviewer\"}]'\n").unwrap();
    fs::set_permissions(fake_bin.join("codex"), fs::Permissions::from_mode(0o755)).unwrap();
    let fake_path = std::path::PathBuf::from(format!("{}:/usr/bin:/bin", fake_bin.display()));
    let validated = run_with_path(
        &home,
        &project,
        &["plan", "validate", "fixture", "--agent", "claude"],
        b"",
        Some(&fake_path),
    );
    assert!(
        validated.status.success(),
        "{}",
        String::from_utf8_lossy(&validated.stdout)
    );
    let stdout = String::from_utf8_lossy(&validated.stdout);
    let report: Value = serde_json::from_str(stdout.lines().next().unwrap()).unwrap();
    assert_eq!(report["blocking"], 1);
    let finding = report["findings"][0]["id"].as_i64().unwrap().to_string();
    for args in [
        &["plan", "resolve", "fixture", finding.as_str()][..],
        &["plan", "acknowledge", "fixture", finding.as_str()],
    ] {
        assert!(!run(&home, &project, args, b"resolve 1\nacknowledge 1\n")
            .status
            .success());
    }
    assert!(plan::prepare_approval(&store, &project, "fixture").is_err());
    let comment = json(run(
        &home,
        &project,
        &[
            "plan",
            "comment",
            "fixture",
            "--anchor",
            "heading:rollout",
            "--text",
            "Stage first?",
        ],
        b"",
    ));
    assert_eq!(
        (comment["author"].as_str(), comment["state"].as_str()),
        (Some("agent"), Some("open"))
    );
    // Fixture preparation uses the human core APIs, never bypassing CLI human checks.
    let id = finding.parse().unwrap();
    plan::close(
        &store,
        &project,
        "fixture",
        id,
        plan::Closure::Acknowledged,
        None,
    )
    .unwrap();
    plan::approve(
        &store,
        &plan::prepare_approval(&store, &project, "fixture").unwrap(),
    )
    .unwrap();
    let hook = run(
        &home,
        &project,
        &["run-hook", "PostToolUse", "--agent", "codex"],
        br#"{"session_id":"tasks","tool_name":"Bash","tool_input":{"command":"true"}}"#,
    );
    assert!(hook.status.success());
    json(run(
        &home,
        &project,
        &["plan", "start", "fixture", "--session", "tasks"],
        b"",
    ));
    let done = json(run(
        &home,
        &project,
        &[
            "plan",
            "task",
            "done",
            "fixture",
            "build",
            "--evidence",
            "built",
        ],
        b"",
    ));
    assert_eq!(done["state"], "done");
    for args in [
        &["plan", "task", "done", "fixture", "signoff"][..],
        &[
            "plan",
            "accept",
            "fixture",
            "copy",
            "--evidence",
            "approved",
        ],
    ] {
        assert!(!run(&home, &project, args, b"done signoff\naccept copy\n")
            .status
            .success());
    }
    let status = run(&home, &project, &["plan", "status", "fixture"], b"");
    let text = String::from_utf8_lossy(&status.stdout);
    assert!(status.status.success(), "{text}");
    for line in [
        "  tasks: 1/2 done",
        "  \"build\",agent,done,\"Build it\"",
        "  \"signoff\",human,ready,\"Sign off\"",
        "  \"copy\",unsatisfied,\"A human approves the copy\"",
        "open_feedback[1]{id,author,severity,state,anchor,text}:",
    ] {
        assert!(
            text.lines().any(|candidate| candidate == line),
            "missing {line:?} in\n{text}"
        );
    }
    let detail = json(run(
        &home,
        &project,
        &["plan", "status", "fixture", "--json"],
        b"",
    ));
    assert_eq!(
        detail["progress"]["tasks"][0]["completion"]["evidence"],
        "built"
    );
    assert_eq!(detail["feedback"][0]["state"], "acknowledged");
}

#[test]
fn console_refuses_agent_processes_before_serving_or_printing_a_url() {
    use std::time::{Duration, Instant};
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let project = temp.path().join("project");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir(&project).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_yoke"))
        .args(["console", "--no-open"])
        .env("HOME", &home)
        .current_dir(&project)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("console kept serving for a noninteractive caller");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "{stdout}");
    assert!(stdout.contains("console requires"), "{stdout}");
    assert!(!stdout.contains("http://"), "{stdout}");
}
