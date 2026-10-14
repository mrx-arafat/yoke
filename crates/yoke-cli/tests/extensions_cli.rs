use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

fn yoke(home: &Path, project: &Path, args: &[&str], input: &[u8]) -> Output {
    use std::io::Write;
    let mut child = Command::new(env!("CARGO_BIN_EXE_yoke"))
        .args(args)
        .env("HOME", home)
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME")
        .current_dir(project)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

fn git(project: &Path, args: &[&str]) {
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

fn dirs() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let project = temp.path().join("project");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&project).unwrap();
    (temp, home, project)
}

#[test]
fn extensions_marks_owned_hooks_and_exits_one_for_a_stale_managed_hook() {
    let (_temp, home, project) = dirs();
    assert!(
        yoke(&home, &project, &["install", "--agent", "claude"], b"")
            .status
            .success()
    );
    let settings = home.join(".claude/settings.json");
    let mut value: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
    value["hooks"].as_object_mut().unwrap().remove("Stop");
    fs::write(&settings, value.to_string()).unwrap();
    let before = fs::read(&settings).unwrap();

    let output = yoke(&home, &project, &["extensions", "--json"], b"");

    assert_eq!(output.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let stale: Vec<_> = report["drift"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|drift| drift["kind"] == "stale")
        .collect();
    assert_eq!(stale.len(), 1, "{stale:?}");
    assert_eq!(stale[0]["name"], "Stop");
    assert!(stale[0]["detail"]
        .as_str()
        .unwrap()
        .starts_with("managed hook missing or edited"));
    let managed = report["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["kind"] == "hook" && item["managed"] == true)
        .count();
    assert_eq!(managed, 8, "9 Claude hooks installed, Stop removed");
    assert_eq!(fs::read(&settings).unwrap(), before);
}

#[test]
fn extensions_never_prints_command_arguments_or_url_paths_in_text_or_json() {
    let (_temp, home, project) = dirs();
    let hook =
        |command: &str| serde_json::json!([{"hooks": [{"type": "command", "command": command}]}]);
    let settings = serde_json::json!({"hooks": {
        "Stop": hook("curl -u admin:Hn4Rt8Wq1bb https://hooks.slack.com/services/T0001/B0001/Xq7Lk2Pz9aa"),
        "Notification": hook("curl -H 'Cookie: session=CookieSecret77' https://discord.com/api/webhooks/123456/DiscordSecretTok"),
        "PreToolUse": hook("/missing/notify.sh --key PlainKeySecret"),
    }});
    fs::create_dir(home.join(".claude")).unwrap();
    fs::write(home.join(".claude/settings.json"), settings.to_string()).unwrap();
    let servers = serde_json::json!({"mcpServers": {
        "pay": {"command": "/bin/sh", "args": ["-c", "stripe-mcp --api=sk_live_51StripeSecret"]},
        "remote": {"type": "sse", "url": "https://mcp.example/k/PathKeySecret9/sse"},
        "relay": {"command": "/bin/sh", "args": ["--webhook", "https://hooks.slack.com/services/T1/B1/ArgPathSecret"]},
    }});
    fs::write(home.join(".claude.json"), servers.to_string()).unwrap();

    let text = yoke(&home, &project, &["extensions"], b"");
    let json = yoke(&home, &project, &["extensions", "--json"], b"");

    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.contains("hook runs /missing/notify.sh"), "{text}");
    let report: Value = serde_json::from_slice(&json.stdout).unwrap();
    let detail = |name: &str| {
        report["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["name"] == name)
            .map(|item| item["detail"].as_str().unwrap().to_owned())
    };
    assert_eq!(detail("Stop").as_deref(), Some("curl (3 args)"));
    assert_eq!(
        detail("remote").as_deref(),
        Some("sse: https://mcp.example")
    );
    assert_eq!(detail("pay").as_deref(), Some("/bin/sh (2 args)"));
    let json = String::from_utf8(json.stdout).unwrap();
    for secret in [
        "Hn4Rt8Wq1bb",
        "Xq7Lk2Pz9aa",
        "CookieSecret77",
        "DiscordSecretTok",
        "PlainKeySecret",
        "StripeSecret",
        "PathKeySecret9",
        "ArgPathSecret",
    ] {
        assert!(!text.contains(secret), "{secret} in text: {text}");
        assert!(!json.contains(secret), "{secret} in JSON: {json}");
    }
}

#[test]
fn extensions_keeps_redacted_names_distinct_and_skips_symlinked_walk_roots() {
    let (temp, home, project) = dirs();
    fs::create_dir(home.join(".claude")).unwrap();
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("hidden.md"), "outside command").unwrap();
    std::os::unix::fs::symlink(&outside, home.join(".claude/commands")).unwrap();
    fs::write(
        home.join(".claude.json"),
        serde_json::json!({"mcpServers": {
            "ghp_distinctalpha": {"env": {"ghp_keysecret": "value"}},
            "ghp_distinctbeta": {}
        }})
        .to_string(),
    )
    .unwrap();

    let output = yoke(&home, &project, &["extensions", "--json"], b"");

    assert!(output.status.success(), "{:?}", output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["drift"], serde_json::json!([]));
    let items = report["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|item| item["name"] == "[REDACTED]"));
    assert!(report["notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|note| { note["note"] == "symlinked directory not walked" }));
    let shown = String::from_utf8(output.stdout).unwrap();
    for secret in ["ghp_distinctalpha", "ghp_distinctbeta", "ghp_keysecret"] {
        assert!(!shown.contains(secret), "metadata leaked {secret}");
    }
}

#[test]
fn extensions_redacts_file_origins_and_symlink_notes_in_text_and_json() {
    let (temp, home, project) = dirs();
    let user_agents = home.join(".claude/agents");
    let project_agents = project.join(".claude/agents");
    fs::create_dir_all(&user_agents).unwrap();
    fs::create_dir_all(&project_agents).unwrap();
    for path in [
        user_agents.join("ghp_filenamealpha.md"),
        project_agents.join("ghp_filenamebeta.md"),
    ] {
        fs::write(path, "---\nname: reviewer\n---\n").unwrap();
    }
    let outside = temp.path().join("ghp_linktargetsecret");
    fs::create_dir(&outside).unwrap();
    let rules = home.join(".claude/rules");
    fs::create_dir(&rules).unwrap();
    std::os::unix::fs::symlink(&outside, rules.join("ghp_linknamesecret")).unwrap();

    for args in [vec!["extensions"], vec!["extensions", "--json"]] {
        let output = yoke(&home, &project, &args, b"");
        assert_eq!(output.status.code(), Some(1));
        let shown = String::from_utf8(output.stdout).unwrap();
        for secret in [
            "ghp_filenamealpha",
            "ghp_filenamebeta",
            "ghp_linktargetsecret",
            "ghp_linknamesecret",
        ] {
            assert!(!shown.contains(secret), "path leaked {secret}: {shown}");
        }
        assert!(shown.contains("[REDACTED].md"));
        if args.len() == 2 {
            let report: Value = serde_json::from_str(&shown).unwrap();
            assert_eq!(report["drift"][0]["origins"].as_array().unwrap().len(), 2);
        }
    }
}

#[test]
fn plan_diff_shows_committed_and_untracked_changes_since_the_plan_base() {
    use yoke_core::{plan, store::Store, trust};
    let (_temp, home, project) = dirs();
    fs::write(project.join("tracked"), "fixture\n").unwrap();
    git(&project, &["init", "-q"]);
    git(&project, &["add", "tracked"]);
    git(&project, &["commit", "-qm", "fixture"]);
    fs::write(project.join("yoke.toml"), "mode='observe'\n").unwrap();
    fs::create_dir(home.join(".yoke")).unwrap();
    let store = Store::open(home.join(".yoke/state.db")).unwrap();
    trust::prepare_project(&store, &project)
        .unwrap()
        .commit(&store)
        .unwrap();
    assert!(yoke(&home, &project, &["plan", "new", "fixture"], b"")
        .status
        .success());
    let row = plan::get(&store, &project, "fixture").unwrap();
    fs::write(&row.file, "+++\ntitle='Fixture'\nreview=true\n+++\nGoal.\n").unwrap();
    git(&project, &["add", "-A"]);
    git(&project, &["commit", "-qm", "plan"]);
    plan::approve(
        &store,
        &plan::prepare_approval(&store, &project, "fixture").unwrap(),
    )
    .unwrap();
    assert!(yoke(
        &home,
        &project,
        &["run-hook", "PostToolUse", "--agent", "codex"],
        br#"{"session_id":"diff-fixture","tool_name":"Bash","tool_input":{"command":"true"}}"#,
    )
    .status
    .success());
    let started = yoke(
        &home,
        &project,
        &["plan", "start", "fixture", "--session", "diff-fixture"],
        b"",
    );
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );
    let base = plan::get(&store, &project, "fixture")
        .unwrap()
        .base_ref
        .unwrap();
    fs::write(project.join("tracked"), "committed during plan\n").unwrap();
    git(&project, &["commit", "-qam", "work"]);
    fs::write(project.join("new.txt"), "untracked work\n").unwrap();

    let output = yoke(&home, &project, &["plan", "diff", "fixture", "--json"], b"");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let shown: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(shown["base"], base.as_str());
    assert_eq!(shown["untracked_files"], 1);
    let diff = shown["diff"].as_str().unwrap();
    assert!(
        diff.contains("-fixture\n+committed during plan\n"),
        "{diff}"
    );
    assert!(
        diff.contains(r#""kind":"file","path":"new.txt""#) && diff.contains("untracked work"),
        "{diff}"
    );
}
