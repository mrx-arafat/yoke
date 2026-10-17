//! Retention runs as a background job: session start only queues it, so no session waits for the
//! deletes. The existing worker leases the job, retries it, and queues a follow-up when one bounded
//! pass could not finish a large backlog.

use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use yoke_core::{
    jobs::{self, Job},
    store::Store,
};

use crate::jobs::Permanent;

pub(crate) const RETENTION_JOB: &str = "retention";
/// One retention job exists at a time; the dedupe key keeps repeated claims from stacking them.
pub(crate) const RETENTION_KEY: &str = "retention";
/// The throttle that predates the job; keeping its name keeps an upgraded installation's hour.
const CLAIM_KEY: &str = "retention.last_prune";
const INTERVAL_SECS: i64 = 3600;
/// Time one job spends deleting; a larger backlog continues in a follow-up job.
const BUDGET: Duration = Duration::from_secs(30);
/// Sleep between batches so a hook waiting for the write lock gets it (see `prune_history_paced`).
const PAUSE: Duration = Duration::from_millis(110);

#[derive(Serialize, Deserialize)]
struct Payload {
    retention_days: u32,
}

/// Queue the hourly pass when the hour has passed; returns whether it was queued.
/// Cheap by design: one interval claim and one insert, no deletes.
pub(crate) fn schedule(store: &Store, now: i64, retention_days: u32) -> Result<bool> {
    if !store.claim_interval(CLAIM_KEY, now, INTERVAL_SECS)? {
        return Ok(false);
    }
    let payload = serde_json::to_string(&Payload { retention_days })?;
    jobs::enqueue(store, RETENTION_JOB, RETENTION_KEY, "", &payload, now)?;
    Ok(true)
}

/// Worker entry: returns true when the backlog is not finished and a follow-up should be queued.
pub(crate) fn run_job(store: &Store, job: &Job, now: i64) -> Result<bool> {
    run(store, job, now, BUDGET, PAUSE)
}

fn run(store: &Store, job: &Job, now: i64, budget: Duration, pause: Duration) -> Result<bool> {
    let payload: Payload = serde_json::from_str(&job.payload)
        .map_err(|error| Permanent(format!("retention job payload is invalid: {error}")))?;
    let report = store.prune_history_paced(now, payload.retention_days, budget, pause)?;
    Ok(!report.complete)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;

    struct Fixture {
        _directory: tempfile::TempDir,
        store: Store,
        db: rusqlite::Connection,
    }

    fn seeded(old: i64) -> Fixture {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.db");
        let store = Store::open(&path).unwrap();
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(&format!(
            "INSERT INTO sessions(id,agent,project,started_at) VALUES ('s','codex','/p',1);
             WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<{old})
             INSERT INTO decisions(ts,session_id,agent,project,event,mode,outcome,gates,payload)
             SELECT x,'s','codex','/p','pre_tool_use','observe','allow','[]','{{}}' FROM n;
             INSERT INTO decisions(ts,session_id,agent,project,event,mode,outcome,gates,payload)
             VALUES (900000,'s','codex','/p','stop','observe','allow','[]','{{}}');
             INSERT INTO memories(id,ts,project,source,text,pinned) VALUES (1,1,'/p','fact','pinned',1);"
        ))
        .unwrap();
        Fixture {
            _directory: directory,
            store,
            db,
        }
    }

    fn count(f: &Fixture, sql: &str) -> i64 {
        f.db.query_row(sql, [], |row| row.get(0)).unwrap()
    }

    #[test]
    fn schedule_only_queues_one_job_an_hour_and_deletes_nothing() {
        let f = seeded(50);
        let store = &f.store;
        let now = 900_000 + DAY;
        assert!(schedule(store, now, 1).unwrap());
        assert!(!schedule(store, now + 10, 1).unwrap());
        assert!(schedule(store, now + INTERVAL_SECS, 1).unwrap());
        let queued = jobs::list(store, true, 10).unwrap();
        assert_eq!(queued.len(), 1, "{queued:?}");
        assert_eq!(
            (queued[0].kind.as_str(), queued[0].state.as_str()),
            (RETENTION_JOB, "queued")
        );
        assert_eq!(queued[0].payload, r#"{"retention_days":1}"#);
        assert_eq!(count(&f, "SELECT count(*) FROM decisions"), 51);
    }

    #[test]
    fn a_pass_that_runs_out_of_budget_asks_for_a_follow_up_until_the_backlog_is_gone() {
        let f = seeded(12_000);
        let store = &f.store;
        schedule(store, 900_000 + DAY, 1).unwrap();
        let job = jobs::list(store, true, 1).unwrap().remove(0);
        let mut passes = 0;
        // A zero budget still finishes one batch, so every pass is bounded and makes progress.
        while run(store, &job, 900_000 + DAY, Duration::ZERO, Duration::ZERO).unwrap() {
            passes += 1;
            assert!(passes < 100);
        }
        assert!(passes >= 1, "a 12,000-row backlog must not fit one batch");
        assert_eq!(count(&f, "SELECT count(*) FROM decisions WHERE ts<2"), 0);
        assert_eq!(
            count(&f, "SELECT count(*) FROM decisions WHERE ts=900000"),
            1
        );
        assert_eq!(count(&f, "SELECT pinned FROM memories WHERE id=1"), 1);
        let broken = Job {
            payload: "{}".into(),
            ..job
        };
        let error = run(
            store,
            &broken,
            900_000 + DAY,
            Duration::ZERO,
            Duration::ZERO,
        )
        .unwrap_err();
        assert!(error.downcast_ref::<Permanent>().is_some(), "{error:#}");
    }
}
