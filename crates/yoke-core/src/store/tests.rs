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
    assert_eq!(
        store.prune_history(172_800, 1, Duration::ZERO)?.decisions,
        1
    );
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
fn session_project_follows_a_project_change() -> Result<()> {
    let store = Store::open(":memory:")?;
    store.record_decision(&decision(1, "allow"), 4096)?;
    let mut moved = decision(2, "allow");
    moved.project = "/test/moved".into();
    store.record_decision(&moved, 4096)?;
    let project: String = store.connection.query_row(
        "SELECT project FROM sessions WHERE id='session-one'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(project, "/test/moved");
    Ok(())
}

#[test]
fn retention_drains_a_large_backlog_in_bounded_passes_and_keeps_protected_rows() -> Result<()> {
    let store = Store::open(":memory:")?;
    let old = PRUNE_BATCH * 2 + 7;
    store.connection.execute_batch(&format!(
        "INSERT INTO sessions(id,agent,project,started_at) VALUES ('s','codex','/p',1);
         WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<{old})
         INSERT INTO decisions(ts,session_id,agent,project,event,mode,outcome,gates,payload)
         SELECT x,'s','codex','/p','pre_tool_use','observe','allow','[]','{{}}' FROM n;
         WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<{old})
         INSERT INTO facts(id,ts,session_id,project,kind,detail) SELECT x,x,'s','/p','edit','old' FROM n;
         INSERT INTO decisions(ts,session_id,agent,project,event,mode,outcome,gates,payload) VALUES (200000,'s','codex','/p','stop','observe','allow','[]','{{}}');
         INSERT INTO facts(id,ts,session_id,project,kind,detail) VALUES (1000000,200000,'s','/p','edit','new');
         INSERT INTO imported_identities(identity,kind,local_id) VALUES ('old-fact','fact',1),('new-fact','fact',1000000);
         INSERT INTO memories(id,ts,project,source,text,pinned) VALUES (1,1,'/p','fact','pinned lesson',1);
         INSERT INTO memory_facts(memory_id,fact_id) VALUES (1,1);
         INSERT INTO trust(scope,root,file,sha256,content,trusted_at) VALUES ('project','/p','/p/yoke.toml','digest','trusted policy',1);
         INSERT INTO plans(id,project,slug,file,state,created_at,updated_at) VALUES
            (1,'/p','done','a.md','done',1,1),(2,'/p','open','b.md','building',1,1),(3,'/p','dropped','c.md','cancelled',1,1);
         INSERT INTO check_runs(id,plan_id,ts,name,command,output_tail,fingerprint,blocking,ms,result) VALUES
            (1,1,10,'t','c','','f',1,1,'pass'),(2,1,200000,'t','c','','f',1,1,'pass'),(3,2,10,'t','c','','f',1,1,'pass'),(4,3,10,'t','c','','f',1,1,'pass');
         INSERT INTO evidence(check_run_id,path,content_hash) VALUES (1,'/p/old.png','h'),(3,'/p/open.png','h');"
    ))?;
    let mut total = PruneReport::default();
    let mut passes = 0;
    while !total.complete {
        // A zero budget still finishes one batch, so each pass is bounded but makes progress.
        let pass = store.prune_history(200_000, 1, Duration::ZERO)?;
        assert!(pass.decisions <= PRUNE_BATCH && pass.facts <= PRUNE_BATCH);
        total.decisions += pass.decisions;
        total.facts += pass.facts;
        total.check_runs += pass.check_runs;
        total.complete = pass.complete;
        passes += 1;
    }
    assert!(passes > 2, "{passes}");
    assert_eq!(
        (total.decisions, total.facts, total.check_runs),
        (old, old, 2)
    );
    let ids = |sql: &str| -> Result<Vec<i64>> {
        let mut statement = store.connection.prepare(sql)?;
        let rows = statement.query_map([], |row| row.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    };
    assert_eq!(ids("SELECT ts FROM decisions")?, vec![200_000]);
    assert_eq!(ids("SELECT decision_count FROM sessions")?, vec![1]);
    assert_eq!(ids("SELECT id FROM facts")?, vec![1_000_000]);
    assert_eq!(
        ids("SELECT local_id FROM imported_identities")?,
        vec![1_000_000]
    );
    assert_eq!(ids("SELECT pinned FROM memories WHERE id=1")?, vec![1]);
    assert_eq!(ids("SELECT count(*) FROM memory_facts")?, vec![0]);
    assert_eq!(ids("SELECT trusted_at FROM trust")?, vec![1]);
    assert_eq!(ids("SELECT id FROM check_runs ORDER BY id")?, vec![2, 3]);
    assert_eq!(ids("SELECT check_run_id FROM evidence")?, vec![3]);
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
    assert!(Store::open_read_only(&path).is_err());
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
    assert_eq!(store.prune_history(3, 0, Duration::ZERO)?.decisions, 2);
    assert_eq!(store.sessions(Some("/bulk"), 1)?[0].decision_count, 100003);
    assert_eq!(store.sessions(Some("/other"), 1)?[0].decision_count, 0);
    store.record_decision(&decision(10, "allow"), 4096)?;
    assert_eq!(
        store.sessions(Some("/test/project"), 1)?[0].decision_count,
        1
    );
    Ok(())
}

const DOWNS: &[&str] = &[
    include_str!("001-core.down.sql"),
    include_str!("002-memory.down.sql"),
    include_str!("003-workflow.down.sql"),
    include_str!("004-guard-usage.down.sql"),
    include_str!("005-read-models.down.sql"),
    include_str!("006-session-counts.down.sql"),
    include_str!("007-plan-base-ref.down.sql"),
    include_str!("008-plan-lifecycle.down.sql"),
    include_str!("009-plan-acceptance.down.sql"),
    include_str!("010-jobs-recall.down.sql"),
    include_str!("011-notifications.down.sql"),
    include_str!("012-usage-time.down.sql"),
    include_str!("013-scale-indexes.down.sql"),
    include_str!("014-session-anchors.down.sql"),
    include_str!("015-usage-summaries.down.sql"),
    include_str!("016-quota-warnings.down.sql"),
];

#[test]
fn quota_warning_migration_preserves_acknowledgements_and_rolls_back_only_new_kind() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("quota-upgrade.db");
    {
        let db = Connection::open(&path)?;
        for migration in &MIGRATIONS[..15] {
            db.execute_batch(migration)?;
        }
        db.pragma_update(None, "user_version", 15)?;
        db.execute_batch("INSERT INTO notifications(id,key,kind,project,subject,title,detail,first_seen,last_seen,state,acknowledged_at,actor) VALUES(42,'keep','usage_partial','','s','partial','detail',1,2,'acknowledged',2,'human');")?;
    }
    let store = Store::open(&path)?;
    store.connection().execute_batch("INSERT INTO notifications(key,kind,project,subject,title,detail,first_seen,last_seen) VALUES('new','usage_limit','','codex','quota','detail',3,3);")?;
    store.transaction(|tx| {
        tx.execute_batch(DOWNS[15])?;
        Ok(())
    })?;
    let saved = store.connection().query_row(
        "SELECT id,state,actor FROM notifications WHERE key='keep'",
        [],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    )?;
    assert_eq!(saved, (42, "acknowledged".into(), "human".into()));
    assert_eq!(
        store
            .connection()
            .query_row("SELECT count(*) FROM notifications", [], |row| row
                .get::<_, i64>(0))?,
        1
    );
    Ok(())
}

#[test]
fn usage_summaries_upgrade_schemas_12_to_14_and_follow_mutation_and_rollback() -> Result<()> {
    for version in 12..=14 {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("upgrade.db");
        {
            let db = Connection::open(&path)?;
            for migration in &MIGRATIONS[..version] {
                db.execute_batch(migration)?;
            }
            db.pragma_update(None, "user_version", version)?;
            db.execute_batch("INSERT INTO usage_samples VALUES
                ('claude','a','/project','1','alpha',3,2,1,0,10,1),
                ('claude','a','/project','2','alpha',3,2,1,0,10,86401),
                ('claude','b','/project','3','beta',3,2,1,0,NULL,172801),
                ('claude','b','/project','4','beta',3,2,1,0,NULL,NULL);
                INSERT INTO notifications(key,kind,project,subject,title,detail,first_seen,last_seen,state,acknowledged_at,actor)
                VALUES('preserved','hook_error','/project','test','test','test',1,2,'acknowledged',2,'human');")?;
            let legacy = Store::open_read_only(&path)?;
            assert_eq!(
                crate::usage::recent_sessions(&legacy, 1)?[0].session_id,
                "b"
            );
            assert_eq!(
                crate::usage::totals_by_agent(&legacy, 2)?[0]
                    .totals
                    .input_tokens,
                6
            );
        }
        let store = Store::open(&path)?;
        let totals = crate::usage::totals_by_agent(&store, 2)?;
        assert_eq!(
            (
                totals[0].totals.samples,
                totals[0].totals.input_tokens,
                totals[0].totals.unknown_pricing_samples
            ),
            (2, 6, 1)
        );
        assert_eq!(
            totals[0].totals.known_cost_usd.as_deref(),
            Some("0.000000010")
        );
        let sessions = crate::usage::recent_sessions(&store, 2)?;
        assert_eq!(
            (sessions[0].session_id.as_str(), sessions[0].totals.samples),
            ("b", 2)
        );
        assert_eq!(sessions[0].totals.known_cost_usd, None);
        store.connection().execute_batch("UPDATE usage_samples SET ts=172802,input_tokens=7,model='zeta' WHERE session_id='a' AND record_key='1';")?;
        let sessions = crate::usage::recent_sessions(&store, 2)?;
        assert_eq!(sessions[0].session_id, "a");
        assert_eq!(sessions[0].totals.input_tokens, 10);
        assert_eq!(sessions[0].models, ["alpha", "zeta"]);
        store
            .connection()
            .execute_batch("BEGIN; DELETE FROM usage_samples; ROLLBACK;")?;
        assert_eq!(
            crate::usage::recent_sessions(&store, 2)?[0]
                .totals
                .input_tokens,
            10
        );
        store
            .connection()
            .execute_batch("DELETE FROM usage_samples WHERE session_id='a' AND record_key='1';")?;
        let sessions = crate::usage::recent_sessions(&store, 2)?;
        assert_eq!(sessions[1].last_at, Some(86401));
        assert_eq!(sessions[1].models, ["alpha"]);
        assert_eq!(
            store.connection().query_row(
                "SELECT actor FROM notifications WHERE key='preserved'",
                [],
                |row| row.get::<_, String>(0)
            )?,
            "human"
        );
    }
    Ok(())
}

#[test]
fn migrations_reverse_only_in_disposable_fixture_and_preserve_core_rows() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("rollback-fixture.db");
    let store = Store::open(&path)?;
    store.record_decision(&decision(10, "allow"), 4096)?;
    assert_eq!(
        DOWNS.len(),
        MIGRATIONS.len(),
        "every migration needs a down"
    );
    for index in (0..DOWNS.len()).rev() {
        store.transaction(|tx| {
            tx.execute_batch(DOWNS[index])?;
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

#[test]
fn plan_migrations_upgrade_released_schemas_atomically_and_round_trip() -> Result<()> {
    for seeded in [6, 7] {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("lifecycle.db");
        let connection = Connection::open(&path)?;
        for sql in MIGRATIONS.iter().take(seeded) {
            connection.execute_batch(sql)?;
        }
        connection.pragma_update(None, "user_version", seeded)?;
        connection.execute_batch(
            "INSERT INTO plans(id,project,slug,file,state,created_at,updated_at) VALUES (1,'/p','finished','a.md','done',1,1),(2,'/p','example','b.md','stalled',1,1);
             INSERT INTO sessions(id,agent,project,started_at,active_plan) VALUES ('s','codex','/p',1,2);
             INSERT INTO transitions(plan_id,ts,from_state,to_state,actor) VALUES (2,1,'building','stalled','yoke');
             INSERT INTO check_runs(plan_id,ts,name,command,output_tail,fingerprint,blocking,ms,result) VALUES (1,1,'t','c','','old',1,1,'pass'),(1,2,'t','c','','tree',1,1,'pass'),(2,1,'t','c','','f',1,1,'fail');
             INSERT INTO stop_guard(session_id,plan_id,last_fingerprint,last_block_at) VALUES ('s',2,'f',1);
             -- A later migration that fails must leave the released schema untouched.
             CREATE TABLE evidence(conflict INTEGER);",
        )?;
        if seeded == 7 {
            connection.execute("UPDATE plans SET base_ref='abc' WHERE id=2", [])?;
        }
        assert!(Store::open(&path).is_err());
        let version: usize =
            connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        assert_eq!(version, seeded);
        assert!(connection
            .execute("UPDATE plans SET state='cancelled' WHERE id=2", [])
            .is_err());
        connection.execute_batch("DROP TABLE evidence")?;
        let rows = |store: &Store| -> Result<(i64, i64, i64, Option<i64>)> {
            Ok(store.connection.query_row(
                "SELECT (SELECT count(*) FROM transitions),(SELECT count(*) FROM check_runs),(SELECT count(*) FROM stop_guard),(SELECT active_plan FROM sessions WHERE id='s')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?)
        };
        let store = Store::open(&path)?;
        assert_eq!(rows(&store)?, (1, 3, 1, Some(2)));
        let plans: Vec<(Option<String>, Option<String>)> = store
            .connection
            .prepare("SELECT base_ref,done_fingerprint FROM plans ORDER BY id")?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let base_ref = (seeded == 7).then(|| "abc".to_owned());
        assert_eq!(plans, [(None, Some("tree".into())), (base_ref, None)]);
        // Down migrations run with foreign keys off, as their rebuilds require.
        drop(store);
        let connection = Connection::open(&path)?;
        connection.pragma_update(None, "foreign_keys", false)?;
        for down in DOWNS[seeded..].iter().rev() {
            connection.execute_batch(down)?;
        }
        let version: usize =
            connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        assert_eq!(version, seeded);
        drop(connection);
        let store = Store::open(&path)?;
        assert_eq!(rows(&store)?, (1, 3, 1, Some(2)));
        store
            .connection
            .execute("UPDATE plans SET state='cancelled' WHERE id=2", [])?;
        assert_eq!(
            store
                .connection
                .pragma_query_value(None, "foreign_keys", |row| row.get::<_, i64>(0))?,
            1
        );
    }
    Ok(())
}

#[test]
fn last_close_leaves_an_empty_persistent_wal_that_readers_still_open() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("state.db");
    let wal = directory.path().join("state.db-wal");
    let shm = directory.path().join("state.db-shm");
    for ts in 1..=3 {
        // Each hook process is the only connection: it writes, closes, and the next one opens.
        let store = Store::open(&path)?;
        store.record_decision(&decision(ts, "allow"), 4096)?;
        drop(store);
        assert_eq!(
            std::fs::metadata(&wal)?.len(),
            0,
            "WAL must be truncated when the last connection closes, not grow across processes"
        );
        assert!(shm.exists(), "WAL index must persist for read-only readers");
    }
    let reader = Store::open_read_only(&path)?;
    assert_eq!(reader.decisions(&DecisionFilter::default())?.len(), 3);
    Ok(())
}

#[test]
fn read_only_open_without_sidecars_names_the_way_back_and_creates_nothing() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("state.db");
    drop(Store::open(&path)?);
    // Another SQLite client's last close checkpoints the WAL and deletes both sidecars.
    for sidecar in ["state.db-wal", "state.db-shm"] {
        std::fs::remove_file(directory.path().join(sidecar))?;
    }
    let error = Store::open_read_only(&path).err().expect("no sidecars");
    let missing = error
        .downcast_ref::<MissingSidecars>()
        .expect("typed error");
    assert_eq!(missing.missing, ["state.db-wal", "state.db-shm"]);
    assert!(
        format!("{error:#}").contains("yoke log --limit 1"),
        "{error:#}"
    );
    // SQLite alone would have left a zero-byte state.db-wal behind.
    let files: Vec<_> = std::fs::read_dir(directory.path())?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<std::io::Result<_>>()?;
    assert_eq!(files, ["state.db"]);
    Ok(())
}
