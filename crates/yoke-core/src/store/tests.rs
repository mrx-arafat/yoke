use super::*;
use serde_json::json;

fn decision(ts: i64, outcome: &str) -> DecisionRecord {
    DecisionRecord {
        ts,
        session_id: "session-one".into(),
        agent: "claude".into(),
        project: "/test/project".into(),
        event: "pre_tool_use".into(),
        tool: Some("Bash".into()),
        mode: "observe".into(),
        outcome: outcome.into(),
        reason: Some("password=hidden-reason".into()),
        gates: json!([{"name":"test", "output":"Bearer hidden-output", "token":"hidden-token"}]),
        payload: "🙂 --token hidden-payload".into(),
    }
}

#[test]
fn persists_redacted_decisions_filters_and_retention() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("state.db");
    {
        let store = Store::open(&path)?;
        assert_eq!(
            store
                .connection
                .pragma_query_value(None, "journal_mode", |row| row.get::<_, String>(0))?,
            "wal"
        );
        assert_eq!(
            store
                .connection
                .pragma_query_value(None, "busy_timeout", |row| row.get::<_, i64>(0))?,
            2000
        );
        store.record_decision(&decision(1, "allow"), 4096)?;
        store.record_decision(&decision(86_400, "would_deny"), 4096)?;
        let mut other = decision(86_401, "deny");
        other.project = "/test/other".into();
        other.session_id = "session-two".into();
        other.tool = Some("Edit".into());
        store.record_decision(&other, 7)?;
    }
    let store = Store::open(&path)?;
    let rows = store.decisions(&DecisionFilter::default())?;
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].ts, 86_401);
    assert!(rows[0].payload.len() <= 7);
    assert!(!serde_json::to_string(&rows)?.contains("hidden"));
    for filter in [
        DecisionFilter {
            project: Some("/test/other".into()),
            ..Default::default()
        },
        DecisionFilter {
            session: Some("session-two".into()),
            ..Default::default()
        },
        DecisionFilter {
            outcome: Some("deny".into()),
            ..Default::default()
        },
        DecisionFilter {
            tool: Some("Edit".into()),
            ..Default::default()
        },
    ] {
        assert_eq!(store.decisions(&filter)?.len(), 1);
    }
    assert!(store
        .decisions(&DecisionFilter {
            project: Some("' OR 1=1 --".into()),
            ..Default::default()
        })?
        .is_empty());
    assert_eq!(store.prune_decisions(172_800, 1)?, 1);
    assert_eq!(store.decisions(&DecisionFilter::default())?.len(), 2);
    let mut end = decision(100_000, "allow");
    end.event = "session_end".into();
    store.record_decision(&end, 4096)?;
    assert_eq!(store.decisions(&DecisionFilter::default())?.len(), 3);
    assert_eq!(
        store.connection.query_row(
            "SELECT ended_at FROM sessions WHERE id='session-one'",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        100_000
    );
    store.connection.execute_batch(
        "CREATE TEMP TRIGGER fail_session_end BEFORE UPDATE OF ended_at ON sessions
         BEGIN SELECT RAISE(ABORT, 'session end fixture failure'); END;",
    )?;
    end.ts = 100_001;
    assert!(store.record_decision(&end, 4096).is_err());
    assert_eq!(store.decisions(&DecisionFilter::default())?.len(), 3);
    assert_eq!(
        store.connection.query_row(
            "SELECT ended_at FROM sessions WHERE id='session-one'",
            [],
            |row| row.get::<_, i64>(0),
        )?,
        100_000
    );
    let mut invalid = decision(200_000, "invalid");
    invalid.session_id = "rollback".into();
    assert!(store.record_decision(&invalid, 100).is_err());
    assert_eq!(
        store.connection.query_row(
            "SELECT count(*) FROM sessions WHERE id='rollback'",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        0
    );
    Ok(())
}

#[test]
fn trust_batches_are_exact_atomic_and_kv_claims_survive_reopen() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("state.db");
    let store = Store::open(&path)?;
    let row = TrustRow {
        scope: "project".into(),
        root: "/repo".into(),
        file: "yoke.toml".into(),
        sha256: "test-hash".into(),
        content: "password='deliberate-approved-value'\n".into(),
        trusted_at: 10,
    };
    store.put_trust(&row)?;
    assert_eq!(
        store.get_trust("project", "/repo", "yoke.toml")?,
        Some(row.clone())
    );
    let mut changed = row.clone();
    changed.content = "changed".into();
    let mut invalid = row.clone();
    invalid.scope = "invalid".into();
    assert!(store.put_trust_batch(&[changed, invalid]).is_err());
    assert_eq!(store.list_trust("project", "/repo")?, vec![row]);
    store.set_kv("version", "0.2.0")?;
    assert!(store.claim_interval("last-check", 100, 50)?);
    let reopened = Store::open(&path)?;
    assert_eq!(reopened.get_kv("version")?, Some("0.2.0".into()));
    assert!(!reopened.claim_interval("last-check", 149, 50)?);
    assert!(reopened.claim_interval("last-check", 150, 50)?);
    assert!(!store.claim_interval("last-check", 140, 50)?);
    store.set_kv("bad-check", "not a timestamp")?;
    assert!(store.claim_interval("bad-check", 100, 50).is_err());
    let contenders: Vec<_> = (0..4)
        .map(|_| {
            let contender = Store::open(&path).unwrap();
            std::thread::spawn(move || contender.claim_interval("concurrent", 200, 50).unwrap())
        })
        .collect();
    assert_eq!(
        contenders
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .filter(|claimed| *claimed)
            .count(),
        1
    );
    assert_eq!(
        store
            .connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?,
        MIGRATIONS.len() as i64
    );
    store.connection.pragma_update(None, "user_version", 99)?;
    assert!(Store::open(&path).is_err());
    Ok(())
}

#[test]
fn session_counts_backfill_and_follow_atomic_decision_changes() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("session-counts.db");
    let connection = Connection::open(&path)?;
    // Seed the previously released schema to exercise real upgrade backfill.
    for sql in MIGRATIONS.iter().take(5) {
        connection.execute_batch(sql)?;
    }
    connection.pragma_update(None, "user_version", 5)?;
    connection.execute_batch("INSERT INTO sessions(id,agent,project,started_at) VALUES ('bulk','codex','/bulk',1),('other','codex','/other',2);
        WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<100005)
        INSERT INTO decisions(ts,session_id,agent,project,event,mode,outcome,gates,payload)
        SELECT x,'bulk','codex','/bulk','pre_tool_use','observe','allow','[]','{}' FROM n;")?;
    drop(connection);
    let store = Store::open(&path)?;
    let start = std::time::Instant::now();
    let rows = store.sessions(Some("/bulk"), 200)?;
    eprintln!("100005-decision Sessions read: {:?}", start.elapsed());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].decision_count, 100005);
    // A stored exact count removes the O(decisions) read from the console path.
    let count: i64 = store.connection.query_row(
        "SELECT decision_count FROM sessions WHERE id='bulk'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(count, 100005);
    store.connection.execute(
        "UPDATE decisions SET session_id=?1 WHERE id=?2",
        params!["other", 1],
    )?;
    assert_eq!(store.sessions(Some("/other"), 1)?[0].decision_count, 1);
    assert_eq!(store.sessions(Some("/bulk"), 1)?[0].decision_count, 100004);
    let failure: Result<()> = store.transaction(|tx| {
        tx.execute("DELETE FROM decisions WHERE id=?1", [2])?;
        tx.execute(
            "UPDATE decisions SET session_id=?1 WHERE id=?2",
            params!["other", 3],
        )?;
        Store::record_decision_in(tx, &decision(10, "allow"), 4096)?;
        bail!("fixture rollback")
    });
    assert!(failure.is_err());
    assert_eq!(store.sessions(Some("/bulk"), 1)?[0].decision_count, 100004);
    assert!(store.sessions(Some("/test/project"), 1)?.is_empty());
    assert_eq!(store.prune_decisions(3, 0)?, 2);
    assert_eq!(store.sessions(Some("/bulk"), 1)?[0].decision_count, 100003);
    assert_eq!(store.sessions(Some("/other"), 1)?[0].decision_count, 0);
    store.record_decision(&decision(10, "allow"), 4096)?;
    assert_eq!(
        store.sessions(Some("/test/project"), 1)?[0].decision_count,
        1
    );
    Ok(())
}

#[test]
fn migrations_reverse_only_in_disposable_fixture_and_preserve_core_rows() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("rollback-fixture.db");
    let store = Store::open(&path)?;
    store.record_decision(&decision(10, "allow"), 4096)?;
    let downs = [
        include_str!("001-core.down.sql"),
        include_str!("002-memory.down.sql"),
        include_str!("003-workflow.down.sql"),
        include_str!("004-guard-usage.down.sql"),
        include_str!("005-read-models.down.sql"),
        include_str!("006-session-counts.down.sql"),
    ];
    for index in (0..downs.len()).rev() {
        store.transaction(|tx| {
            tx.execute_batch(downs[index])?;
            Ok(())
        })?;
        let version: usize = store
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        assert_eq!(version, index);
        if index > 0 {
            assert_eq!(store.decisions(&DecisionFilter::default())?.len(), 1);
        }
    }
    assert_eq!(
        store.connection.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        0
    );
    drop(store);
    let reopened = Store::open(&path)?;
    assert!(reopened.decisions(&DecisionFilter::default())?.is_empty());
    assert_eq!(
        reopened
            .connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, usize>(0))?,
        MIGRATIONS.len()
    );
    Ok(())
}
