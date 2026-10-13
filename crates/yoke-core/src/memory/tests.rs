use super::*;
use serde_json::json;
use std::{collections::BTreeMap, process::Command, time::Duration};

fn fixture_output(text: &str) -> Command {
    let mut command = Command::new("sh");
    command.args([
        "-c",
        "test \"$YOKE_OFF\" = 1 || exit 2; cat >/dev/null; printf '%s' \"$1\"",
        "digest-fixture",
        text,
    ]);
    command
}

#[test]
fn notes_facts_context_and_share_follow_project_boundaries() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let project = directory.path().to_str().unwrap();
    let store = Store::open(directory.path().join("state.db"))?;
    let first = remember(&store, project, "SQLite migration evidence", false, 1)?;
    let second = remember(
        &store,
        project,
        "Pinned safety password=hidden-value",
        true,
        2,
    )?;
    remember(&store, "/other-project", "SQLite other project", false, 3)?;
    assert_eq!(
        recall(&store, project, "SQLite", 10)?
            .iter()
            .map(|row| row.id)
            .collect::<Vec<_>>(),
        vec![first.id]
    );
    assert!(!get(&store, second.id)?.unwrap().text.contains("hidden"));
    assert_eq!(recall(&store, project, "", 10)?[0].id, second.id);
    assert_eq!(context(&store, project, "", "trusted knowledge", 0)?, "");
    let small = context(&store, project, "", "trusted knowledge", 2)?;
    assert_eq!(small, "Pinned s");
    let full = context(&store, project, "SQLite", "trusted knowledge", 100)?;
    assert!(full.find("Pinned").unwrap() < full.find("trusted knowledge").unwrap());
    assert!(full.find("trusted knowledge").unwrap() < full.find("SQLite").unwrap());
    pin(&store, first.id, true)?;
    let path = share(&store, first.id)?;
    let before = fs::read_to_string(&path)?;
    share(&store, first.id)?;
    assert_eq!(fs::read_to_string(&path)?, before);
    assert!(get(&store, first.id)?.unwrap().shared);
    assert!(store.list_trust("project", project)?.is_empty());
    let row = DecisionRecord {
        ts: 4,
        session_id: "s".into(),
        agent: "codex".into(),
        project: project.into(),
        event: "post_tool_use".into(),
        tool: Some("apply_patch".into()),
        mode: "observe".into(),
        outcome: "allow".into(),
        reason: None,
        gates: json!([]),
        payload: String::new(),
    };
    assert_eq!(
        record_hook_facts(
            &store,
            &row,
            &json!({"tool_input":{"command":"patch evidence"}})
        )?,
        1
    );
    let mut command = row.clone();
    command.tool = Some("Bash".into());
    assert_eq!(
        record_hook_facts(
            &store,
            &command,
            &json!({"tool_response":{"text":"failed"}})
        )?,
        0
    );
    assert_eq!(
        record_hook_facts(&store, &command, &json!({"tool_response":{"exit_code":1}}))?,
        1
    );
    assert!(forget(&store, first.id)?);
    assert!(recall(&store, project, "SQLite", 10)?.is_empty());
    Ok(())
}

#[test]
fn digest_fixtures_are_bounded_atomic_and_undo_preserves_provenance() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let project = directory.path().to_str().unwrap();
    let store = Store::open(directory.path().join("state.db"))?;
    let fact = record_fact(&store, "s", project, "edit", "Changed migration", 10)?;
    assert!(digest_due(&store, "s", project, 10, 1800)?);
    let result = json!({"memories":[{"text":"Migration fixed password=hidden","fact_ids":[fact]}]})
        .to_string();
    let mut malformed = fixture_output("not json");
    assert!(digest_with_command(
        &store,
        "s",
        project,
        "",
        11,
        &mut malformed,
        Duration::from_secs(1)
    )
    .is_err());
    assert!(digest_due(&store, "s", project, 11, 1800)?);
    store.connection().execute_batch("CREATE TEMP TRIGGER fail_digest BEFORE INSERT ON memories BEGIN SELECT RAISE(ABORT,'fixture'); END;")?;
    assert!(digest_with_command(
        &store,
        "s",
        project,
        "",
        11,
        &mut fixture_output(&result),
        Duration::from_secs(1)
    )
    .is_err());
    assert!(digest_due(&store, "s", project, 11, 1800)?);
    store
        .connection()
        .execute_batch("DROP TRIGGER fail_digest;")?;
    let captured = directory.path().join("stdin.json");
    let mut command = Command::new("sh");
    command.args([
        "-c",
        "test \"$YOKE_OFF\" = 1 || exit 2; cat >\"$2\"; printf '%s' \"$1\"",
        "fixture",
        &result,
        captured.to_str().unwrap(),
    ]);
    assert_eq!(
        digest_with_command(
            &store,
            "s",
            project,
            "token=hidden-transcript",
            12,
            &mut command,
            Duration::from_secs(1)
        )?,
        1
    );
    assert!(!fs::read_to_string(captured)?.contains("hidden-transcript"));
    let memory = recall(&store, project, "Migration", 10)?.remove(0);
    assert!(!memory.text.contains("hidden"));
    assert_eq!(memory.source, "digest");
    let snapshot = forget_with_snapshot(&store, memory.id)?.unwrap();
    assert_eq!(snapshot.fact_ids, vec![fact]);
    restore_snapshot(&store, &snapshot)?;
    assert_eq!(
        store.connection().query_row(
            "SELECT fact_id FROM memory_facts WHERE memory_id=?1",
            [memory.id],
            |row| row.get::<_, i64>(0)
        )?,
        fact
    );
    assert!(!digest_due(&store, "s", project, 2000, 1800)?);
    record_fact(&store, "s", project, "edit", "Another migration", 13)?;
    assert!(!digest_due(&store, "s", project, 14, 1800)?);
    assert!(digest_due(&store, "s", project, 1812, 1800)?);
    let mut timeout = Command::new("sh");
    timeout.args(["-c", "sleep 1"]);
    assert!(digest_with_command(
        &store,
        "s",
        project,
        "",
        14,
        &mut timeout,
        Duration::from_millis(20)
    )
    .is_err());
    assert!(digest_due(&store, "s", project, 1812, 1800)?);
    Ok(())
}

#[test]
fn portable_archive_maps_projects_is_idempotent_and_dry_run_does_not_write() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let source_path = directory.path().join("source.db");
    let destination_path = directory.path().join("destination.db");
    let archive = directory.path().join("archive.json");
    let source = Store::open(&source_path)?;
    let original = remember(
        &source,
        "/source/project",
        "Migration searchable evidence",
        true,
        123,
    )?;
    let fact = record_fact(
        &source,
        "old-session",
        "/source/project",
        "edit",
        "Applied migration",
        120,
    )?;
    let output = json!({"memories":[{"text":"Migration digest","fact_ids":[fact]}]}).to_string();
    digest_with_command(
        &source,
        "old-session",
        "/source/project",
        "",
        124,
        &mut fixture_output(&output),
        Duration::from_secs(1),
    )?;
    export_archive(&source, &archive, Some("/source/project"), false)?;
    let bytes = fs::read(&archive)?;
    assert!(export_archive(&source, &archive, None, false).is_err());
    assert_eq!(fs::read(&archive)?, bytes);
    let mapping = BTreeMap::from([("/source/project".into(), "/destination/project".into())]);
    let destination = Store::open(&destination_path)?;
    remember(&destination, "/unrelated", "Preexisting note", false, 1)?;
    drop(destination);
    let before = snapshot_directory(directory.path())?;
    let readonly = Store::open_read_only(&destination_path)?;
    let preview = import_archive(&readonly, &archive, &mapping, true)?;
    assert_eq!(preview.additions, 3);
    drop(readonly);
    assert_eq!(snapshot_directory(directory.path())?, before);
    let destination = Store::open(&destination_path)?;
    assert!(import_archive(&destination, &archive, &BTreeMap::new(), false).is_err());
    assert_eq!(
        import_archive(&destination, &archive, &mapping, false)?.additions,
        3
    );
    let recalled = recall(&destination, "/destination/project", "searchable", 10)?;
    assert_eq!(recalled.len(), 1);
    assert_eq!(recalled[0].identity, original.identity);
    assert_eq!(recalled[0].ts, 123);
    assert!(recalled[0].pinned);
    assert_ne!(recalled[0].id, original.id);
    assert!(!recalled[0].shared);
    let repeated = import_archive(&destination, &archive, &mapping, false)?;
    assert_eq!((repeated.additions, repeated.duplicates), (0, 3));
    let digest = recall(&destination, "/destination/project", "digest", 10)?
        .pop()
        .unwrap();
    let snapshot = forget_with_snapshot(&destination, digest.id)?.unwrap();
    assert!(snapshot.imported);
    assert!(!snapshot.fact_ids.is_empty());
    let replacement = remember(&destination, "/destination/project", "New note", false, 200)?;
    assert_eq!(replacement.id, digest.id);
    restore_snapshot(&destination, &snapshot)?;
    let restored = recall(&destination, "/destination/project", "digest", 10)?
        .pop()
        .unwrap();
    assert_eq!(restored.identity, digest.identity);
    assert_ne!(restored.id, replacement.id);
    assert_eq!(get(&destination, replacement.id)?.unwrap().text, "New note");
    let links = destination
        .connection()
        .prepare("SELECT fact_id FROM memory_facts WHERE memory_id=?1 ORDER BY fact_id")?
        .query_map([restored.id], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    assert_eq!(links, snapshot.fact_ids);
    let imported_id: i64 = destination.connection().query_row(
        "SELECT local_id FROM imported_identities WHERE identity=?1 AND kind='memory'",
        [&restored.identity],
        |row| row.get(0),
    )?;
    assert_eq!(imported_id, restored.id);
    let repeated = import_archive(&destination, &archive, &mapping, false)?;
    assert_eq!((repeated.additions, repeated.duplicates), (0, 3));
    assert!(!digest_due(
        &destination,
        "old-session",
        "/destination/project",
        2000,
        0
    )?);
    pin(&destination, recalled[0].id, false)?;
    assert_eq!(
        import_archive(&destination, &archive, &mapping, true)?
            .conflicts
            .len(),
        1
    );
    assert!(import_archive(&destination, &archive, &mapping, false).is_err());
    assert!(!get(&destination, recalled[0].id)?.unwrap().pinned);
    let mut malformed: Value = serde_json::from_slice(&bytes)?;
    malformed["version"] = json!(999);
    let invalid = directory.path().join("invalid.json");
    fs::write(&invalid, serde_json::to_vec(&malformed)?)?;
    assert!(import_archive(&destination, &invalid, &mapping, false).is_err());
    malformed = serde_json::from_slice(&bytes)?;
    malformed["links"][0]["fact"] = json!("00000000000000000000000000000000");
    fs::write(&invalid, serde_json::to_vec(&malformed)?)?;
    assert!(import_archive(&destination, &invalid, &mapping, false).is_err());
    let rollback = Store::open(directory.path().join("rollback.db"))?;
    rollback.connection().execute_batch("CREATE TEMP TRIGGER fail_fact BEFORE INSERT ON facts BEGIN SELECT RAISE(ABORT,'fixture'); END;")?;
    assert!(import_archive(&rollback, &archive, &mapping, false).is_err());
    assert!(recall(&rollback, "/destination/project", "", 10)?.is_empty());
    assert_eq!(
        rollback
            .connection()
            .query_row("SELECT count(*) FROM imported_identities", [], |row| row
                .get::<_, i64>(0))?,
        0
    );
    Ok(())
}

#[test]
fn archive_rejects_special_files_and_oversized_scope_before_loading_rows() -> Result<()> {
    use std::os::unix::{ffi::OsStrExt, fs::symlink};
    let directory = tempfile::tempdir()?;
    let store = Store::open(directory.path().join("state.db"))?;
    let fifo = directory.path().join("pipe");
    let name = std::ffi::CString::new(fifo.as_os_str().as_bytes())?;
    // SAFETY: fixture-owned path is NUL-terminated and mkfifo does not retain it.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(import_archive(&store, &fifo, &BTreeMap::new(), true).is_err());
    let link = directory.path().join("link");
    symlink(&fifo, &link)?;
    assert!(import_archive(&store, &link, &BTreeMap::new(), true).is_err());
    store.connection().execute("INSERT INTO memories(ts,project,source,text) VALUES (1,'/oversized','remember',zeroblob(17000000))",[])?;
    let archive = directory.path().join("oversized.json");
    assert!(export_archive(&store, &archive, Some("/oversized"), false).is_err());
    assert!(!archive.exists());
    Ok(())
}

fn snapshot_directory(path: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    fs::read_dir(path)?
        .map(|entry| {
            let entry = entry?;
            Ok((
                entry.file_name().to_string_lossy().into_owned(),
                fs::read(entry.path())?,
            ))
        })
        .collect()
}
