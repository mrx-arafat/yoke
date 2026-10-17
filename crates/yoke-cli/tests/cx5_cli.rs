use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn run(home: &Path, project: &Path, args: &[&str]) -> Output {
    assert!(home.is_dir() && home.starts_with(std::env::temp_dir()));
    Command::new(env!("CARGO_BIN_EXE_yoke"))
        .args(args)
        .env("HOME", home)
        .env_remove("CODEX_HOME")
        .env_remove("CLAUDE_CONFIG_DIR")
        // These output journeys represent a person; CX2 separately tests anchored agent scope.
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_CHILD_SESSION")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("CODEX_SESSION_ID")
        .current_dir(project)
        .output()
        .unwrap()
}

fn text(output: &Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).unwrap()
}

#[test]
fn memory_reports_are_text_by_default_and_json_on_request() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let project = temp.path().join("project");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&project).unwrap();
    let saved = text(&run(
        &home,
        &project,
        &["remember", "Keep reversible migrations"],
    ));
    assert!(saved.starts_with("id: "), "{saved}");
    assert!(saved.contains("\ntime: \"20"), "{saved}");
    let json = text(&run(
        &home,
        &project,
        &["remember", "Second note", "--json"],
    ));
    let row: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(row["text"], "Second note");
    assert!(row["ts"].as_i64().unwrap() > 0);
    let missing = run(&home, &project, &["forget", "999"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&missing.stdout).contains("yoke recall"));
    let archive = temp.path().join("memory.json");
    let archive = archive.to_str().unwrap();
    let exported = text(&run(&home, &project, &["memory", "export", archive]));
    assert!(exported.contains("memories: 2\n"), "{exported}");
    let exported = text(&run(
        &home,
        &project,
        &["memory", "export", archive, "--overwrite", "--json"],
    ));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&exported).unwrap()["memories"],
        2
    );
    let project = project.canonicalize().unwrap();
    let map = format!("{}={}", project.display(), project.display());
    let imported = text(&run(
        &home,
        &project,
        &["memory", "import", archive, "--map", &map, "--dry-run"],
    ));
    assert!(imported.contains("duplicates: 2\n"), "{imported}");
    let imported = text(&run(
        &home,
        &project,
        &[
            "memory",
            "import",
            archive,
            "--map",
            &map,
            "--dry-run",
            "--json",
        ],
    ));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&imported).unwrap()["duplicates"],
        2
    );
}

#[test]
fn plan_new_outside_git_tells_the_user_to_initialize_git() {
    let temp = tempfile::tempdir().unwrap();
    let missing = run(temp.path(), temp.path(), &["plan", "status", "missing"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&missing.stdout).contains("yoke plan list"));
    let output = text(&run(
        temp.path(),
        temp.path(),
        &["plan", "new", "first-plan"],
    ));
    assert!(output.contains("git init"), "{output}");
    let listed = text(&run(temp.path(), temp.path(), &["plan", "list"]));
    assert!(listed.contains("{slug,state,session,file}"), "{listed}");
    assert!(listed.contains(",none,"), "{listed}");
    let detail = text(&run(
        temp.path(),
        temp.path(),
        &["plan", "status", "first-plan"],
    ));
    assert!(detail.contains("  session: none\n"), "{detail}");

    // Prepare a binding through the core API, without invoking a human-only CLI action.
    for args in [
        vec!["init", "-q"],
        vec!["commit", "--allow-empty", "-qm", "fixture"],
    ] {
        let output = Command::new("git")
            .current_dir(temp.path())
            .env("HOME", temp.path())
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgSign=false",
                "-c",
                "user.name=Fixture",
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
    let project = temp.path().canonicalize().unwrap();
    let store = yoke_core::store::Store::open(project.join(".yoke/state.db")).unwrap();
    let approval = yoke_core::plan::prepare_approval(&store, &project, "first-plan").unwrap();
    yoke_core::plan::approve(&store, &approval).unwrap();
    store
        .record_decision(
            &yoke_core::store::DecisionRecord {
                ts: 1,
                session_id: "bound-session".into(),
                agent: "codex".into(),
                project: project.to_str().unwrap().into(),
                event: "session_start".into(),
                tool: None,
                mode: "observe".into(),
                outcome: "allow".into(),
                reason: None,
                gates: serde_json::json!([]),
                payload: "{}".into(),
            },
            4096,
        )
        .unwrap();
    yoke_core::plan::start(&store, &project, "first-plan", "bound-session").unwrap();
    let listed = text(&run(temp.path(), &project, &["plan", "list"]));
    assert!(listed.contains(",\"bound-session\","), "{listed}");
    let detail = text(&run(
        temp.path(),
        &project,
        &["plan", "status", "first-plan"],
    ));
    assert!(
        detail.contains("  session: \"bound-session\"\n"),
        "{detail}"
    );
    let listed = text(&run(temp.path(), &project, &["plan", "status", "--json"]));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&listed).unwrap()[0]["session"],
        "bound-session"
    );
    let listed = text(&run(temp.path(), &project, &["plan", "list", "--json"]));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&listed).unwrap()[0]["session"],
        "bound-session"
    );
}
