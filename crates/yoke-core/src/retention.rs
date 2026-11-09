//! What a retention pass would delete now, counted without deleting. The predicates mirror
//! `Store::prune_history` (decisions, facts, check runs of finished plans) and `jobs::prune`
//! (finished jobs); a test pins that the counts equal what those functions delete.

use anyhow::Result;
use serde::Serialize;

use crate::store::Store;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Preview {
    /// Rows strictly older than this Unix time are removed.
    pub cutoff: i64,
    pub decisions: i64,
    pub facts: i64,
    /// Check runs of done or cancelled plans; unfinished plans keep their whole history.
    pub check_runs: i64,
    /// Succeeded and dead background jobs, removed at session start rather than by the job.
    pub jobs: i64,
    pub total_decisions: i64,
    pub total_facts: i64,
}

pub fn preview(store: &Store, now: i64, retention_days: u32) -> Result<Preview> {
    let cutoff = now.saturating_sub(i64::from(retention_days) * 86_400);
    let count = |sql: &str, bound: bool| -> Result<i64> {
        let connection = store.connection();
        Ok(if bound {
            connection.query_row(sql, [cutoff], |row| row.get(0))?
        } else {
            connection.query_row(sql, [], |row| row.get(0))?
        })
    };
    Ok(Preview {
        cutoff,
        decisions: count("SELECT count(*) FROM decisions WHERE ts < ?1", true)?,
        facts: count("SELECT count(*) FROM facts WHERE ts < ?1", true)?,
        check_runs: count(
            "SELECT count(*) FROM check_runs WHERE ts < ?1 AND plan_id IN (SELECT id FROM plans WHERE state IN ('done','cancelled'))",
            true,
        )?,
        jobs: count(
            "SELECT count(*) FROM jobs WHERE state IN ('succeeded','dead') AND updated_at < ?1",
            true,
        )?,
        total_decisions: count("SELECT count(*) FROM decisions", false)?,
        total_facts: count("SELECT count(*) FROM facts", false)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn preview_counts_exactly_what_a_retention_pass_and_job_pruning_delete() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path().join("state.db")).unwrap();
        const DAY: i64 = 86_400;
        let now = 100 * DAY;
        store
            .connection()
            .execute_batch(&format!(
                "INSERT INTO sessions(id,agent,project,started_at) VALUES ('s','codex','/p',1);
                 INSERT INTO decisions(ts,session_id,agent,project,event,mode,outcome,gates,payload)
                 VALUES (1,'s','codex','/p','pre_tool_use','observe','allow','[]','{{}}'),
                        ({old},'s','codex','/p','pre_tool_use','observe','allow','[]','{{}}'),
                        ({edge},'s','codex','/p','pre_tool_use','observe','allow','[]','{{}}'),
                        ({now},'s','codex','/p','pre_tool_use','observe','allow','[]','{{}}');
                 INSERT INTO facts(identity,ts,session_id,project,kind,detail,digested)
                 VALUES ('f1',1,'s','/p','edit','a',1),('f2',{now},'s','/p','edit','b',0);
                 INSERT INTO plans(id,project,slug,file,state,created_at,updated_at)
                 VALUES (1,'/p','done','d.md','done',1,1),(2,'/p','open','o.md','building',1,1);
                 INSERT INTO check_runs(plan_id,ts,name,command,output_tail,fingerprint,blocking,ms,result)
                 VALUES (1,1,'u','t','','f',1,1,'pass'),(2,1,'u','t','','f',1,1,'pass');",
                old = now - 31 * DAY,
                edge = now - 30 * DAY,
            ))
            .unwrap();
        for (key, state) in [("a", "succeeded"), ("b", "dead"), ("c", "queued")] {
            store
                .connection()
                .execute(
                    "INSERT INTO jobs(kind,dedupe_key,project,payload,state,max_attempts,run_after,created_at,updated_at) VALUES ('k',?1,'','{}',?2,3,1,1,1)",
                    [key, state],
                )
                .unwrap();
        }

        let preview = preview(&store, now, 30).unwrap();
        assert_eq!(
            (
                preview.decisions,
                preview.facts,
                preview.check_runs,
                preview.jobs
            ),
            (2, 1, 1, 2),
            "the row exactly at the boundary stays"
        );
        assert_eq!((preview.total_decisions, preview.total_facts), (4, 2));
        let report = store
            .prune_history(now, 30, Duration::from_secs(5))
            .unwrap();
        let jobs = crate::jobs::prune(&store, now, 30).unwrap();
        assert_eq!(
            (
                report.decisions as i64,
                report.facts as i64,
                report.check_runs as i64,
                jobs as i64
            ),
            (
                preview.decisions,
                preview.facts,
                preview.check_runs,
                preview.jobs
            )
        );
    }
}
