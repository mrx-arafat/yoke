//! Migration 013 upgrades a schema 12 database in place, and each new index serves the query it
//! was built for (see the "Scale" section of docs/field-testing.md).

use anyhow::Result;
use rusqlite::Connection;
use yoke_core::store::Store;

const SCHEMA_12: [&str; 12] = [
    include_str!("../src/store/001-core.sql"),
    include_str!("../src/store/002-memory.sql"),
    include_str!("../src/store/003-workflow.sql"),
    include_str!("../src/store/004-guard-usage.sql"),
    include_str!("../src/store/005-read-models.sql"),
    include_str!("../src/store/006-session-counts.sql"),
    include_str!("../src/store/007-plan-base-ref.sql"),
    include_str!("../src/store/008-plan-lifecycle.sql"),
    include_str!("../src/store/009-plan-acceptance.sql"),
    include_str!("../src/store/010-jobs-recall.sql"),
    include_str!("../src/store/011-notifications.sql"),
    include_str!("../src/store/012-usage-time.sql"),
];

fn plan(connection: &Connection, sql: &str) -> Result<String> {
    let mut statement = connection.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
    let details = statement.query_map([], |row| row.get::<_, String>(3))?;
    Ok(details.collect::<rusqlite::Result<Vec<_>>>()?.join("\n"))
}

#[test]
fn schema_12_upgrades_and_new_indexes_serve_health_trial_and_retention() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("state.db");
    let connection = Connection::open(&path)?;
    for (index, sql) in SCHEMA_12.iter().enumerate() {
        connection.execute_batch(sql)?;
        connection.pragma_update(None, "user_version", index + 1)?;
    }
    connection.execute_batch(
        "INSERT INTO sessions(id,agent,project,started_at) VALUES ('s1','claude','/p',1),('s2','codex','/p',2);
         INSERT INTO decisions(ts,session_id,agent,project,event,mode,outcome,gates,payload) VALUES
            (10,'s1','claude','/p','pre_tool_use','observe','allow','[]','{}'),
            (20,'s2','codex','/p','pre_tool_use','enforce','deny','[]','{}'),
            (30,'s2','codex','/p','stop','enforce','allow','[]','{}');
         INSERT INTO facts(ts,session_id,project,kind,detail) VALUES (5,'s1','/p','file_write','a');",
    )?;
    drop(connection);

    let store = Store::open(&path)?;
    let (version, supported) = store.schema_version()?;
    assert!(
        version >= 13 && version == supported,
        "{version} of {supported}"
    );
    let newest = store.last_decision("codex")?.expect("codex has decisions");
    assert_eq!(
        newest,
        (30, "stop".into(), "enforce".into(), "allow".into())
    );
    drop(store);

    let connection = Connection::open(&path)?;
    let health = plan(
        &connection,
        "SELECT ts,event,mode,outcome FROM decisions WHERE agent='codex' ORDER BY ts DESC,id DESC LIMIT 1",
    )?;
    assert!(health.contains("decisions_agent"), "{health}");
    assert!(!health.contains("SCAN"), "{health}");
    let trial = plan(
        &connection,
        "SELECT agent,mode,event,sum(outcome='allow'),sum(outcome='deny') FROM decisions GROUP BY agent,mode,event",
    )?;
    assert!(trial.contains("COVERING INDEX decisions_trial"), "{trial}");
    let retention = plan(
        &connection,
        "SELECT id FROM facts WHERE ts < 5 ORDER BY ts,id LIMIT 5000",
    )?;
    assert!(retention.contains("facts_ts"), "{retention}");
    Ok(())
}
