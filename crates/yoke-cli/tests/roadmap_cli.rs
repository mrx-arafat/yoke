use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

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
    let mut command = isolated_command(Path::new(env!("CARGO_BIN_EXE_yoke")), home);
    // A person at a shell: pinning and importing refuse an agent's tool process, and this run
    // may itself have been started by one.
    for marker in [
        "CLAUDECODE",
        "CLAUDE_CODE_CHILD_SESSION",
        "CODEX_THREAD_ID",
        "CODEX_SESSION_ID",
    ] {
        command.env_remove(marker);
    }
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
        &[
            "remember",
            "Keep the migration reversible",
            "--pinned",
            "--json",
        ],
        b"",
    ));
    let archive = temp.path().join("memory.json");
    let exported = json(run(
        &home,
        &source,
        &["memory", "export", archive.to_str().unwrap(), "--json"],
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
        // Restoring one's own export keeps its pin; an import drops pins by default.
        "--keep-pins",
        "--json",
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

/// Run `yoke statusline` as a person's shell would, with colors off and a fixed width so the
/// line does not depend on the developer's terminal.
fn statusline(home: &Path, cwd: &Path, args: &[&str], input: &[u8]) -> Output {
    let mut command = isolated_command(Path::new(env!("CARGO_BIN_EXE_yoke")), home);
    command
        .arg("statusline")
        .args(args)
        .env("HOME", home)
        .env("NO_COLOR", "1")
        .env("YOKE_STATUSLINE_WIDTH", "200")
        .env_remove("YOKE_ASCII")
        .env_remove("COLUMNS")
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

/// Every file and directory below `dir`, as paths relative to it.
fn entries_under(dir: &Path) -> Vec<String> {
    fn walk(dir: &Path, base: &Path, found: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            found.push(path.strip_prefix(base).unwrap().display().to_string());
            if path.is_dir() {
                walk(&path, base, found);
            }
        }
    }
    let mut found = Vec::new();
    walk(dir, dir, &mut found);
    found.sort();
    found
}

#[test]
fn statusline_fresh_home_and_wrapper_are_read_only() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let work = temp.path().join("work");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&work).unwrap();
    let input = br#"{"context_window":{"used_percentage":42}}"#;
    // The wrapper has a fixed 250 ms budget that a loaded host can miss once; a wrapper that
    // really cannot finish fails every attempt.
    let mut output = statusline(&home, &work, &["--wrap", "cat"], input);
    for _ in 0..9 {
        if output.status.success() {
            break;
        }
        output = statusline(&home, &work, &["--wrap", "cat"], input);
    }
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!(
            "yoke (no data yet) | ctx 42% \u{2588}\u{2588}\u{2588}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591} | {}\n",
            String::from_utf8_lossy(input)
        )
    );
    assert_eq!(entries_under(&home), Vec::<String>::new());

    // A git repository in the callback adds the branch, and still creates nothing: the count of
    // changed files is cached only where `~/.yoke` already exists, and no helper starts without it.
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    let callback = serde_json::json!({
        "session_id": "fresh",
        "workspace": {"current_dir": repo},
        "cost": {"total_cost_usd": 0.5, "total_duration_ms": 120_000},
    });
    let output = statusline(&home, &work, &[], callback.to_string().as_bytes());
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "yoke (no data yet) | main | $0.50 \u{b7} 2m\n"
    );
    // Observe the no-writes property for the worker window, failing as soon as a write appears.
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    while entries_under(&home).is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(entries_under(&home), Vec::<String>::new());
}

#[test]
fn statusline_helpers_write_only_inside_an_existing_state_directory() {
    use std::time::{Duration, Instant};
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let repo = temp.path().join("repo");
    fs::create_dir_all(home.join(".yoke")).unwrap();
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    let callback = serde_json::json!({"workspace": {"current_dir": repo}}).to_string();

    // The first paint answers at once with the branch alone and starts the detached count.
    let output = statusline(&home, &repo, &[], callback.as_bytes());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "yoke (no data yet) | main\n"
    );
    // The paint notes that a count was started; the helper's own result adds a `state`.
    let cache = home.join(".yoke/statusline-git.json");
    let deadline = Instant::now() + Duration::from_secs(15);
    while !fs::read_to_string(&cache).is_ok_and(|text| text.contains("\"state\"")) {
        assert!(
            Instant::now() < deadline,
            "the helper never wrote its result"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // Only the cache appeared, and only inside the state directory: no state database, no
    // quota file, nothing else under the home.
    assert_eq!(
        entries_under(&home),
        [".yoke".to_owned(), ".yoke/statusline-git.json".to_owned()]
    );
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
    // Establish the real hook anchor before invoking digest from an agent tool process.
    let hook = run(
        &home,
        &project,
        &["run-hook", "PreToolUse", "--agent", "claude"],
        serde_json::json!({
            "session_id": "fixture",
            "cwd": project,
            "tool_name": "Bash",
            "tool_input": {"command": "true"}
        })
        .to_string()
        .as_bytes(),
    );
    assert!(
        hook.status.success(),
        "{}",
        String::from_utf8_lossy(&hook.stderr)
    );
    let fifo = temp.path().join("transcript.fifo");
    assert!(Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    let mut child = isolated_command(Path::new(env!("CARGO_BIN_EXE_yoke")), &home)
        .args(["memory", "digest", "fixture", "--transcript"])
        .arg(&fifo)
        .env("HOME", &home)
        .env("CLAUDECODE", "1")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("CODEX_SESSION_ID")
        .current_dir(&project)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
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
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("regular file"), "{stderr}");
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
        let mut child = isolated_command(Path::new(env!("CARGO_BIN_EXE_yoke")), &home)
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
    let deadline = Instant::now() + Duration::from_secs(30);
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
        // Session start also queues the hourly retention job; this test is about the digest.
        let jobs: Vec<_> = jobs
            .as_array()
            .unwrap()
            .iter()
            .filter(|job| job["kind"] == "memory_digest")
            .collect();
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
        &[
            "plan",
            "start",
            "fixture",
            "--session",
            "plan-fixture",
            "--json",
        ],
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
            "--json",
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
        &["plan", "check", "fixture", "--agent", "codex", "--json"],
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
        &["review", "fixture", "--agent", "codex", "--json"],
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
fn plan_commands_explain_a_missing_first_commit_and_observe_mode() {
    use yoke_core::{plan, store::Store};
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let project = temp.path().join("project");
    fs::create_dir_all(home.join(".yoke")).unwrap();
    fs::create_dir(&project).unwrap();
    assert!(project.starts_with(std::env::temp_dir()));
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .current_dir(&project)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
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
    };
    git(&["init", "-q"]);

    let created = run(&home, &project, &["plan", "new", "fixture"], b"");
    let text = String::from_utf8_lossy(&created.stdout);
    assert!(
        created.status.success() && text.starts_with("plan: \"fixture\"\nstate: draft\n"),
        "{text}"
    );
    assert!(text.contains("no commit yet"), "{text}");
    let diffed = run(&home, &project, &["plan", "diff", "fixture"], b"");
    assert!(!diffed.status.success());
    assert!(String::from_utf8_lossy(&diffed.stdout).contains("no commit yet"));

    fs::write(project.join("tracked"), "fixture\n").unwrap();
    git(&["add", "tracked"]);
    git(&["commit", "-qm", "fixture"]);
    let store = Store::open(home.join(".yoke/state.db")).unwrap();
    plan::approve(
        &store,
        &plan::prepare_approval(&store, &project, "fixture").unwrap(),
    )
    .unwrap();
    let hook = run(
        &home,
        &project,
        &["run-hook", "PostToolUse", "--agent", "codex"],
        br#"{"session_id":"observed","tool_name":"Bash","tool_input":{"command":"true"}}"#,
    );
    assert!(hook.status.success());
    let started = run(
        &home,
        &project,
        &["plan", "start", "fixture", "--session", "observed"],
        b"",
    );
    assert!(started.status.success());
    // The default view is text with the next step; the observe note stays on stderr.
    let text = String::from_utf8_lossy(&started.stdout);
    assert!(
        text.starts_with("plan: \"fixture\"\nstate: building\nsession: \"observed\"\nfile: "),
        "{text}"
    );
    assert!(text.contains("yoke plan check fixture --agent"), "{text}");
    assert!(String::from_utf8_lossy(&started.stderr).contains("mode is observe"));
    let checked = run(
        &home,
        &project,
        &["plan", "check", "fixture", "--agent", "codex"],
        b"",
    );
    let text = String::from_utf8_lossy(&checked.stdout);
    assert!(checked.status.success(), "{text}");
    assert!(
        text.starts_with("plan: \"fixture\"\nstate: done\nresult: pass\nchecks[1]{name,result,blocking,ms,detail}:\n  \"diff\",pass,true,"),
        "{text}"
    );
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
        &["plan", "validate", "fixture", "--agent", "claude", "--json"],
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
            "--json",
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
    assert!(run(
        &home,
        &project,
        &["plan", "start", "fixture", "--session", "tasks"],
        b"",
    )
    .status
    .success());
    let done = run(
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
    );
    let text = String::from_utf8_lossy(&done.stdout);
    assert!(done.status.success(), "{text}");
    assert!(
        text.starts_with("plan: \"fixture\"\ntask: \"build\"\nstate: done\n")
            && text.contains("by: agent\nevidence: \"built\"\n"),
        "{text}"
    );
    let full = json(run(
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
            "--json",
        ],
        b"",
    ));
    assert_eq!(full["state"], "done");
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
    let comment_id = comment["id"].as_i64().unwrap().to_string();
    let reply_row = format!(",agent,reply,reply_to_{comment_id},\"heading:rollout\",\"Yes\"");
    assert!(run(
        &home,
        &project,
        &["plan", "reply", "fixture", &comment_id, "--text", "Yes"],
        b""
    )
    .status
    .success());
    let replied = run(&home, &project, &["plan", "status", "fixture"], b"");
    let text = String::from_utf8_lossy(&replied.stdout);
    assert!(
        text.lines().any(|line| line.ends_with(&reply_row)),
        "a reply must sit under the comment it answers in\n{text}"
    );
    assert!(
        text.contains("  feedback: 1 open (0 blocking) of 2, 1 reply"),
        "{text}"
    );
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
    let mut child = isolated_command(Path::new(env!("CARGO_BIN_EXE_yoke")), &home)
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

fn git_in(project: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(project)
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

/// A started `review=true` plan with one edit committed after `plan start`, and a fixture
/// `codex` reviewer that records what it is sent in `$HOME/review-input`.
/// Returns the temp dir, home, project and a `PATH` that finds the fixture reviewer.
fn plan_with_work_committed_after_start() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    use std::os::unix::fs::PermissionsExt;
    use yoke_core::{plan, store::Store, trust};
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let project = temp.path().join("project");
    let bin = temp.path().join("bin");
    for directory in [&home, &project, &bin] {
        fs::create_dir(directory).unwrap();
    }
    let project = project.canonicalize().unwrap();
    fs::write(project.join("tracked"), "fn base() {}\n").unwrap();
    git_in(&project, &["init", "-q"]);
    git_in(&project, &["add", "tracked"]);
    git_in(&project, &["commit", "-qm", "base"]);
    fs::write(
        project.join("yoke.toml"),
        "mode='enforce'\n[review]\nreviewer='codex'\n",
    )
    .unwrap();
    fs::create_dir(home.join(".yoke")).unwrap();
    let store = Store::open(home.join(".yoke/state.db")).unwrap();
    trust::prepare_project(&store, &project)
        .unwrap()
        .commit(&store)
        .unwrap();
    let reviewer = bin.join("codex");
    fs::write(
        &reviewer,
        "#!/bin/sh\n/bin/cat > \"$HOME/review-input\"\nprintf '[]'\n",
    )
    .unwrap();
    fs::set_permissions(&reviewer, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(run(&home, &project, &["plan", "new", "fixture"], b"")
        .status
        .success());
    let row = plan::get(&store, &project, "fixture").unwrap();
    fs::write(
        &row.file,
        "+++\ntitle='Fixture'\nreview=true\n[[check]]\nname='proof'\nrun='true'\n+++\nGoal.\n",
    )
    .unwrap();
    plan::approve(
        &store,
        &plan::prepare_approval(&store, &project, "fixture").unwrap(),
    )
    .unwrap();
    assert!(run(
        &home,
        &project,
        &["run-hook", "PostToolUse", "--agent", "claude"],
        br#"{"session_id":"review-seam","tool_name":"Bash","tool_input":{"command":"true"}}"#,
    )
    .status
    .success());
    assert!(run(
        &home,
        &project,
        &["plan", "start", "fixture", "--session", "review-seam"],
        b""
    )
    .status
    .success());
    fs::write(project.join("tracked"), "fn committed_during_plan() {}\n").unwrap();
    git_in(&project, &["commit", "-qam", "work during the plan"]);
    let path = std::path::PathBuf::from(format!("{}:/usr/bin:/bin", bin.display()));
    (temp, home, project, path)
}

fn assert_reviewer_saw_committed_work(home: &Path, launched: &Output) {
    let input = fs::read_to_string(home.join("review-input")).unwrap_or_else(|error| {
        panic!(
            "reviewer was never launched: {error}; stdout {} stderr {}",
            String::from_utf8_lossy(&launched.stdout),
            String::from_utf8_lossy(&launched.stderr)
        )
    });
    assert!(input.contains("committed_during_plan"), "{input}");
}

#[test]
fn plan_check_sends_work_committed_since_plan_start_to_the_reviewer() {
    let (_temp, home, project, path) = plan_with_work_committed_after_start();
    let checked = run_with_path(
        &home,
        &project,
        &["plan", "check", "fixture", "--agent", "claude"],
        b"",
        Some(&path),
    );
    assert_reviewer_saw_committed_work(&home, &checked);
}

#[test]
fn review_command_sends_work_committed_since_plan_start_to_the_reviewer() {
    let (_temp, home, project, path) = plan_with_work_committed_after_start();
    let reviewed = run_with_path(
        &home,
        &project,
        &["review", "fixture", "--agent", "claude"],
        b"",
        Some(&path),
    );
    assert_reviewer_saw_committed_work(&home, &reviewed);
}

#[test]
fn digest_with_more_than_one_batch_queues_a_follow_up_until_drained() {
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
    // One digest call drains at most 10 batches of 100 facts, so 1,050 facts need a follow-up job.
    for index in 0..1_050 {
        memory::record_fact(
            &store,
            "digest-session",
            project.to_str().unwrap(),
            "command_failure",
            &format!("exit {index}"),
            1,
        )
        .unwrap();
    }
    let script = bin.join("codex");
    fs::write(
        &script,
        // Shell expansions only: the child's PATH holds nothing but this script, and the
        // grep, head and cut paths differ between systems.
        "#!/bin/sh\ninput=$(/bin/cat)\nrest=${input#*'\"id\":'}\nid=${rest%%[!0-9]*}\nprintf '{\"memories\":[{\"text\":\"Batch digest\",\"fact_ids\":[%s]}]}' \"$id\"\n",
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let mut child = isolated_command(Path::new(env!("CARGO_BIN_EXE_yoke")), &home)
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
    assert!(child.wait_with_output().unwrap().status.success());

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let notes = memory::recall(&store, project.to_str().unwrap(), "Batch", 20).unwrap();
        if notes.len() == 11 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the follow-up batch never ran: {} digest memories, error log {:?}",
            notes.len(),
            fs::read_to_string(home.join(".yoke/error.log"))
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
