//! Counts over recorded history for the console's Memory and Usage pages: memories by type
//! per day, memories by type in total, and what each project did in a period. Read-only.

use std::collections::BTreeMap;

use anyhow::Result;
use rusqlite::params;

use crate::store::Store;

/// Memories created on one day, by type. Days with none are not listed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeDay {
    /// `YYYY-MM-DD` in the requested UTC offset.
    pub day: String,
    pub counts: BTreeMap<String, i64>,
}

/// Memories created at or after `since` (Unix seconds), by calendar day and type, oldest day
/// first. `project` limits it to one project; `None` counts every project.
pub fn memories_by_type(
    store: &Store,
    project: Option<&str>,
    since: i64,
    offset_minutes: i32,
) -> Result<Vec<TypeDay>> {
    let mut statement = store.connection().prepare(
        "SELECT strftime('%Y-%m-%d',ts+?3,'unixepoch') AS day,memory_type,count(*) FROM memories
         WHERE (?1 IS NULL OR project=?1) AND ts>=?2 GROUP BY day,memory_type ORDER BY day,memory_type",
    )?;
    let rows = statement.query_map(
        params![project, since, i64::from(offset_minutes) * 60],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        },
    )?;
    let mut days: Vec<TypeDay> = Vec::new();
    for row in rows {
        let (day, memory_type, count) = row?;
        match days.last_mut() {
            Some(last) if last.day == day => {
                last.counts.insert(memory_type, count);
            }
            _ => days.push(TypeDay {
                day,
                counts: BTreeMap::from([(memory_type, count)]),
            }),
        }
    }
    Ok(days)
}

/// Every memory of the project (or of all projects) by type, all time.
pub fn memory_type_totals(store: &Store, project: Option<&str>) -> Result<BTreeMap<String, i64>> {
    let mut statement = store.connection().prepare(
        "SELECT memory_type,count(*) FROM memories WHERE (?1 IS NULL OR project=?1) GROUP BY memory_type",
    )?;
    let rows = statement.query_map([project], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// What one project recorded in a period.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectActivity {
    pub project: String,
    pub sessions: i64,
    pub decisions: i64,
    pub memories: i64,
    /// Input plus output tokens of dated usage samples; cache tokens are left out.
    pub tokens: i64,
}

/// Activity at or after `since` (Unix seconds) for each project that had any, busiest first
/// (tokens, then decisions, then sessions).
pub fn project_activity(store: &Store, since: i64, limit: usize) -> Result<Vec<ProjectActivity>> {
    let mut by_project: BTreeMap<String, ProjectActivity> = BTreeMap::new();
    let mut add = |sql: &str, set: fn(&mut ProjectActivity, i64)| -> Result<()> {
        let mut statement = store.connection().prepare(sql)?;
        let rows = statement.query_map([since], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (project, count) = row?;
            let entry = by_project
                .entry(project.clone())
                .or_insert_with(|| ProjectActivity {
                    project,
                    ..ProjectActivity::default()
                });
            set(entry, count);
        }
        Ok(())
    };
    add(
        "SELECT project,count(*) FROM sessions WHERE started_at>=?1 GROUP BY project",
        |a, n| a.sessions = n,
    )?;
    add(
        "SELECT project,count(*) FROM decisions WHERE ts>=?1 GROUP BY project",
        |a, n| a.decisions = n,
    )?;
    add(
        "SELECT project,count(*) FROM memories WHERE ts>=?1 GROUP BY project",
        |a, n| a.memories = n,
    )?;
    add(
        "SELECT project,sum(input_tokens+output_tokens) FROM usage_samples WHERE ts>=?1 GROUP BY project",
        |a, n| a.tokens = n,
    )?;
    let mut rows: Vec<ProjectActivity> = by_project.into_values().collect();
    rows.sort_by(|a, b| {
        (b.tokens, b.decisions, b.sessions, &a.project).cmp(&(
            a.tokens,
            a.decisions,
            a.sessions,
            &b.project,
        ))
    });
    rows.truncate(limit.min(1000));
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{remember, remember_typed};

    #[test]
    fn memories_group_by_local_day_and_type_and_projects_rank_by_tokens() {
        let store = Store::open(":memory:").unwrap();
        // 86_400 * 3 - 60 is 23:59 UTC on day 3; at UTC+01:00 it is already day 4.
        let late = 86_400 * 3 - 60;
        remember(&store, "/a", "plain", false, late).unwrap();
        remember_typed(&store, "/a", "chose WAL", "decision", false, late + 1).unwrap();
        remember_typed(&store, "/b", "other project", "decision", false, 86_400 * 5).unwrap();
        remember(&store, "/a", "too old", false, 10).unwrap();
        let days = memories_by_type(&store, Some("/a"), 86_400, 60).unwrap();
        assert_eq!(days.len(), 1);
        assert_eq!(days[0].day, "1970-01-04");
        assert_eq!(
            days[0].counts,
            BTreeMap::from([("decision".into(), 1), ("note".into(), 1)])
        );
        assert_eq!(memories_by_type(&store, None, 86_400, 0).unwrap().len(), 2);
        assert_eq!(memory_type_totals(&store, Some("/a")).unwrap()["note"], 2);

        store.connection().execute_batch(
            "INSERT INTO sessions(id,agent,project,started_at) VALUES ('s1','claude','/a',100),('s2','codex','/b',100),('old','codex','/b',1);
             INSERT INTO decisions(ts,session_id,agent,project,event,mode,outcome,gates,payload) VALUES (100,'s1','claude','/a','stop','observe','allow','[]','{}'),(101,'s1','claude','/a','stop','observe','allow','[]','{}'),(100,'s2','codex','/b','stop','observe','allow','[]','{}');
             INSERT INTO usage_samples(agent,session_id,project,record_key,model,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,ts) VALUES ('codex','s2','/b','1','m',7,3,1000,0,100),('claude','s1','/a','1','m',1,1,0,0,100);",
        ).unwrap();
        let rows = project_activity(&store, 50, 10).unwrap();
        let summary = rows
            .iter()
            .map(|r| (r.project.as_str(), r.sessions, r.decisions, r.tokens))
            .collect::<Vec<_>>();
        assert_eq!(summary, vec![("/b", 1, 1, 10), ("/a", 1, 2, 2)]);
    }
}
