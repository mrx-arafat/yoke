use std::{collections::BTreeMap, fs};

use serde_json::json;
use yoke_core::{config::Config, jobs, memory, plan, store::Store};

#[test]
fn unsafe_policy_files_name_backup_and_repair_without_replacing_them() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(":memory:").unwrap();
    let path = temp.path().join("yoke.toml");
    fs::create_dir(&path).unwrap();
    let error = format!(
        "{:#}",
        yoke_core::trust::prepare_project(&store, temp.path())
            .err()
            .unwrap()
    );
    assert!(error.contains("back up"), "{error}");
    assert!(path.is_dir());
    let error = format!(
        "{:#}",
        yoke_core::preset::add(&path, "git-safety", &Default::default())
            .err()
            .unwrap()
    );
    assert!(error.contains("back up"), "{error}");
    assert!(path.is_dir());
}

#[test]
fn unreadable_plan_file_names_backup_and_retry_without_advancing_state() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(":memory:").unwrap();
    let row = plan::create(&store, temp.path(), ".yoke/plans", "demo", "Demo").unwrap();
    fs::remove_file(&row.file).unwrap();
    fs::create_dir(&row.file).unwrap();
    let error = format!(
        "{:#}",
        plan::prepare_approval(&store, temp.path(), "demo").unwrap_err()
    );
    assert!(error.contains("back up"), "{error}");
    assert!(error.contains("yoke plan approve"), "{error}");
    assert_eq!(
        plan::get(&store, temp.path(), "demo").unwrap().state,
        "draft"
    );
}

#[test]
fn invalid_review_input_names_inspection_and_repair() {
    let temp = tempfile::tempdir().unwrap();
    let error = format!(
        "{:#}",
        yoke_core::review::input(temp.path(), Some("bad/ref")).unwrap_err()
    );
    assert!(error.contains("git status --short"), "{error}");
    assert!(error.contains("retry yoke review"), "{error}");
}

#[test]
fn unsafe_knowledge_and_duplicate_plan_name_a_preserving_retry() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().canonicalize().unwrap();
    let store = Store::open(":memory:").unwrap();
    let note = memory::remember(&store, project.to_str().unwrap(), "note", false, 100).unwrap();
    fs::create_dir(project.join(".yoke")).unwrap();
    fs::create_dir(project.join(".yoke/knowledge.md")).unwrap();
    let error = format!("{:#}", memory::share(&store, note.id).unwrap_err());
    assert!(error.contains("back up"), "{error}");
    assert!(!memory::get(&store, note.id).unwrap().unwrap().shared);
    plan::create(&store, &project, ".yoke/plans", "demo", "Demo").unwrap();
    let error = plan::create(&store, &project, ".yoke/plans", "demo", "Other")
        .unwrap_err()
        .to_string();
    assert!(error.contains("yoke plan status demo"), "{error}");
    assert_eq!(plan::get(&store, &project, "demo").unwrap().state, "draft");
}

#[test]
fn invalid_policy_explains_how_to_correct_and_retrust_it() {
    for text in [
        "mode = [",
        "[usage]\nwarn_percent=0",
        "[[gate]]\nname=''\non='stop'\nrun='true'",
    ] {
        let error = format!("{:#}", Config::parse(text).unwrap_err());
        assert!(error.contains("edit the policy"), "{error}");
        assert!(error.contains("yoke trust --global"), "{error}");
    }
}

#[test]
fn newer_state_schema_explains_safe_binary_recovery_without_changing_data() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.db");
    let store = Store::open(&path).unwrap();
    let version = store.schema_version().unwrap().1 + 1;
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .pragma_update(None, "user_version", version)
        .unwrap();
    for error in [
        Store::open(&path).err().unwrap(),
        Store::open_read_only(&path).err().unwrap(),
    ] {
        let error = format!("{error:#}");
        for hint in [
            "update every Yoke executable",
            "yoke health",
            "keep state.db",
        ] {
            assert!(error.contains(hint), "missing {hint}: {error}");
        }
    }
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, usize>(0))
            .unwrap(),
        version
    );
}

#[test]
fn invalid_memory_archive_preserves_records_and_names_a_fresh_export() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("state.db");
    let store = Store::open(&database).unwrap();
    let path = temp.path().join("archive.json");
    for archive in [
        json!({"version":2,"projects":[],"memories":[],"facts":[],"links":[]}),
        json!({"version":1,"projects":["/project","/project"],"memories":[],"facts":[],"links":[]}),
    ] {
        fs::write(&path, archive.to_string()).unwrap();
        let error = memory::import_archive(
            &store,
            &path,
            &BTreeMap::new(),
            &memory::ImportOptions {
                dry_run: true,
                keep_pins: false,
                now: 1000,
            },
        )
        .unwrap_err();
        let error = format!("{error:#}");
        assert!(error.contains("yoke memory export"), "{error}");
        assert!(error.contains("keep the original"), "{error}");
        let count: i64 = rusqlite::Connection::open(&database)
            .unwrap()
            .query_row("SELECT count(*) FROM memories", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
    memory::remember(&store, "/project", "invalid stored timestamp", false, -1).unwrap();
    let output = temp.path().join("export.json");
    let error = format!(
        "{:#}",
        memory::export_archive(&store, &output, Some("/project"), false).unwrap_err()
    );
    assert!(error.contains("keep state.db and its sidecars"), "{error}");
    assert!(!output.exists());
}

#[test]
fn invalid_plan_contract_names_its_file_repair_and_human_approval() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(":memory:").unwrap();
    let row = plan::create(&store, temp.path(), ".yoke/plans", "demo", "Demo").unwrap();
    for content in [
        "no header",
        "+++\ntitle='Demo'\n+++\n",
        "+++\ntitle='Demo'\nreview=true\n[[task]]\nid='x'\ntitle='X'\ndepends_on=['x']\n+++\n",
    ] {
        fs::write(&row.file, content).unwrap();
        let error = format!(
            "{:#}",
            plan::prepare_approval(&store, temp.path(), "demo").unwrap_err()
        );
        assert!(error.contains("edit the plan file"), "{error}");
        assert!(error.contains("yoke plan approve"), "{error}");
        assert_eq!(
            plan::get(&store, temp.path(), "demo").unwrap().state,
            "draft"
        );
    }
}

#[test]
fn job_retry_refusals_name_the_existing_job_list_and_preserve_state() {
    let store = Store::open(":memory:").unwrap();
    let (job, _) = jobs::enqueue(&store, "test", "one", "/project", "{}", 1).unwrap();
    for id in [job.id, job.id + 1] {
        let error = format!("{:#}", jobs::retry(&store, id, 2).unwrap_err());
        assert!(error.contains("yoke jobs --all"), "{error}");
    }
    assert_eq!(jobs::list(&store, true, 10).unwrap()[0].state, "queued");
    let error = jobs::enqueue(
        &store,
        "test",
        "oversize",
        "/project",
        &"x".repeat(65_537),
        2,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("repair the hook"), "{error}");
    assert_eq!(jobs::list(&store, true, 10).unwrap().len(), 1);
}

#[test]
fn missing_memory_explains_how_to_find_the_current_project_id() {
    let store = Store::open(":memory:").unwrap();
    let error = memory::pin_in_project(&store, "/project", 123, false).unwrap_err();
    assert!(
        error.to_string().contains("yoke recall --json"),
        "{error:#}"
    );
}

#[test]
fn digest_failure_names_retry_without_marking_facts_digested() {
    let store = Store::open(":memory:").unwrap();
    memory::record_fact(&store, "session", "/project", "test", "keep this fact", 1).unwrap();
    let mut command = std::process::Command::new("/bin/sh");
    command.args([
        "-c",
        "printf '%s' '{\"memories\":[{\"text\":\"note\",\"fact_ids\":[999]}]}'",
    ]);
    let error = memory::digest_with_command(
        &store,
        "session",
        "/project",
        "",
        2,
        &mut command,
        std::time::Duration::from_secs(5),
        &mut || Ok(()),
    )
    .unwrap_err();
    let error = format!("{error:#}");
    assert!(error.contains("yoke memory digest <session-id>"), "{error}");
    assert!(error.contains("yoke jobs --all"), "{error}");
    assert!(memory::digest_due(&store, "session", "/project", 2, 0).unwrap());
}
