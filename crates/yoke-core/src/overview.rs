//! Read-only summaries across projects, and plan statistics, for the console dashboard.
use std::collections::BTreeMap;

use anyhow::Result;
use rusqlite::params;
use serde::Serialize;

use crate::memory::{self, Memory};
use crate::store::Store;
use crate::timefmt;

/// One plan from any recorded project.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RecentPlan {
    pub project: String,
    pub slug: String,
    pub state: String,
    pub revision: i64,
    pub updated_at: i64,
}

/// Newest plans across every recorded project.
pub fn recent_plans(store: &Store, limit: usize) -> Result<Vec<RecentPlan>> {
    let mut statement = store.connection().prepare(
        "SELECT project,slug,state,revision,updated_at FROM plans ORDER BY updated_at DESC,id DESC LIMIT ?1",
    )?;
    let rows = statement.query_map(params![limit.min(1000) as i64], |row| {
        Ok(RecentPlan {
            project: row.get(0)?,
            slug: row.get(1)?,
            state: row.get(2)?,
            revision: row.get(3)?,
            updated_at: row.get(4)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Newest memories across every recorded project, pinned ones first.
pub fn recent_memories(store: &Store, limit: usize) -> Result<Vec<Memory>> {
    let mut statement = store.connection().prepare(&format!(
        "SELECT {} FROM memories ORDER BY pinned DESC,ts DESC,id DESC LIMIT ?1",
        memory::MEMORY_COLUMNS
    ))?;
    let rows = statement.query_map(params![limit.min(1000) as i64], memory::memory_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Plans completed on one local calendar day.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct DayCount {
    /// Unix seconds of the local midnight that starts the day.
    pub day_start: i64,
    pub count: i64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct DonePlan {
    pub slug: String,
    pub done_at: i64,
    /// Times the plan entered checking before it passed.
    pub check_rounds: i64,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct PlanStats {
    pub total: i64,
    pub by_state: BTreeMap<String, i64>,
    /// Plans in the done state; a plan only reaches it through passing checks.
    pub done: i64,
    /// Approved, building, checking, waiting or stalled.
    pub in_progress: i64,
    pub draft: i64,
    pub cancelled: i64,
    /// Mean times a done plan entered checking; None until a plan is done.
    pub avg_check_rounds: Option<f64>,
    /// Completions per local day over the last 30 days, oldest first, empty days omitted.
    pub timeline: Vec<DayCount>,
    /// Newest completions first, at most five.
    pub recently_done: Vec<DonePlan>,
}

const TIMELINE_DAYS: i64 = 30;

/// Totals, check rounds and completion history for one project's plans.
pub fn plan_stats(store: &Store, project: &str, now: i64) -> Result<PlanStats> {
    let mut stats = PlanStats::default();
    let mut statement = store
        .connection()
        .prepare("SELECT state,count(*) FROM plans WHERE project=?1 GROUP BY state")?;
    for row in statement.query_map([project], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })? {
        let (state, count) = row?;
        stats.total += count;
        match state.as_str() {
            "done" => stats.done += count,
            "draft" => stats.draft += count,
            "cancelled" => stats.cancelled += count,
            _ => stats.in_progress += count,
        }
        stats.by_state.insert(state, count);
    }
    let mut statement = store.connection().prepare(
        "SELECT p.slug,
                coalesce((SELECT max(t.ts) FROM transitions t WHERE t.plan_id=p.id AND t.to_state='done'),p.updated_at),
                (SELECT count(*) FROM transitions t WHERE t.plan_id=p.id AND t.to_state='checking')
         FROM plans p WHERE p.project=?1 AND p.state='done' ORDER BY 2 DESC,p.id DESC",
    )?;
    let done: Vec<DonePlan> = statement
        .query_map([project], |row| {
            Ok(DonePlan {
                slug: row.get(0)?,
                done_at: row.get(1)?,
                check_rounds: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    if !done.is_empty() {
        let rounds: i64 = done.iter().map(|plan| plan.check_rounds).sum();
        stats.avg_check_rounds = Some(rounds as f64 / done.len() as f64);
    }
    let offset = timefmt::utc_offset_secs(now);
    let first_day = timefmt::day_start(now, offset) - (TIMELINE_DAYS - 1) * 86_400;
    let mut days: BTreeMap<i64, i64> = BTreeMap::new();
    for plan in &done {
        let day = timefmt::day_start(plan.done_at, offset);
        if day >= first_day {
            *days.entry(day).or_default() += 1;
        }
    }
    stats.timeline = days
        .into_iter()
        .map(|(day_start, count)| DayCount { day_start, count })
        .collect();
    stats.recently_done = done.into_iter().take(5).collect();
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("state.db")).unwrap();
        (dir, store)
    }

    fn plan(store: &Store, project: &str, slug: &str, state: &str, updated: i64) -> i64 {
        store
            .connection()
            .execute(
                "INSERT INTO plans(project,slug,file,state,created_at,updated_at) VALUES(?1,?2,'f',?3,?4,?4)",
                params![project, slug, state, updated],
            )
            .unwrap();
        store.connection().last_insert_rowid()
    }

    fn transition(store: &Store, id: i64, to: &str, ts: i64) {
        store
            .connection()
            .execute(
                "INSERT INTO transitions(plan_id,ts,from_state,to_state,actor) VALUES(?1,?2,'x',?3,'yoke')",
                params![id, ts, to],
            )
            .unwrap();
    }

    #[test]
    fn plan_stats_count_states_check_rounds_and_recent_completions() {
        let (_dir, store) = store();
        let now = 1_800_000_000;
        let a = plan(&store, "/p", "a", "done", now - 100);
        transition(&store, a, "checking", now - 300);
        transition(&store, a, "checking", now - 200);
        transition(&store, a, "done", now - 100);
        let b = plan(&store, "/p", "b", "done", now - 5 * 86_400);
        transition(&store, b, "checking", now - 5 * 86_400 - 60);
        transition(&store, b, "done", now - 5 * 86_400);
        let old = plan(&store, "/p", "old", "done", now - 90 * 86_400);
        transition(&store, old, "done", now - 90 * 86_400);
        plan(&store, "/p", "d", "draft", now);
        plan(&store, "/p", "w", "building", now);
        plan(&store, "/p", "c", "cancelled", now);
        plan(&store, "/other", "x", "done", now);

        let stats = plan_stats(&store, "/p", now).unwrap();

        assert_eq!(
            (
                stats.total,
                stats.done,
                stats.in_progress,
                stats.draft,
                stats.cancelled
            ),
            (6, 3, 1, 1, 1)
        );
        // a: 2 rounds, b: 1 round, old: 0 rounds.
        assert_eq!(stats.avg_check_rounds, Some(1.0));
        assert_eq!(
            stats
                .recently_done
                .iter()
                .map(|p| p.slug.as_str())
                .collect::<Vec<_>>(),
            ["a", "b", "old"]
        );
        // The 90-day-old completion falls outside the 30-day timeline.
        assert_eq!(stats.timeline.iter().map(|d| d.count).sum::<i64>(), 2);
        assert!(stats.timeline[0].day_start < stats.timeline[1].day_start);
    }

    #[test]
    fn recent_summaries_span_projects_and_cap_the_limit() {
        let (_dir, store) = store();
        plan(&store, "/p", "a", "draft", 10);
        plan(&store, "/q", "b", "done", 20);
        plan(&store, "/p", "c", "building", 5);
        for (ts, project, pinned) in [(1, "/p", 0), (3, "/q", 0), (2, "/p", 1)] {
            store
                .connection()
                .execute(
                    "INSERT INTO memories(ts,project,source,text,pinned) VALUES(?1,?2,'remember','note',?3)",
                    params![ts, project, pinned],
                )
                .unwrap();
        }

        let plans = recent_plans(&store, 2).unwrap();
        assert_eq!(
            plans
                .iter()
                .map(|p| (p.project.as_str(), p.slug.as_str()))
                .collect::<Vec<_>>(),
            [("/q", "b"), ("/p", "a")]
        );
        let memories = recent_memories(&store, 10).unwrap();
        assert_eq!(
            memories.iter().map(|m| m.ts).collect::<Vec<_>>(),
            [2, 3, 1],
            "pinned first, then newest"
        );
    }
}
