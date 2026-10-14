//! Read-only trial evidence: activity, outcomes, denials and recovery incidents per agent and mode.

use anyhow::{ensure, Context, Result};
use regex::Regex;
use rusqlite::{params, Connection, Transaction, TransactionBehavior};
use serde::Serialize;
use serde_json::Value;

use crate::redact::redact;
use crate::store::Store;

/// A working day is one calendar day with decisions in at least this many distinct clock hours.
pub const WORKING_DAY_HOURS: i64 = 6;
/// Release acceptance (#22): one working day in observe mode and five in enforce mode, per agent.
pub const OBSERVE_WORKING_DAYS: i64 = 1;
pub const ENFORCE_WORKING_DAYS: i64 = 5;
pub const AGENTS: [&str; 2] = ["claude", "codex"];
const WINDOW: &str =
    "(?1 IS NULL OR ts >= ?1) AND (?2 IS NULL OR ts < ?2) AND (?3 IS NULL OR agent = ?3)";
const FLAGGED: &str = "outcome IN ('deny','would_deny','error')";

#[derive(Clone, Debug)]
pub struct TrialQuery {
    /// Inclusive lower bound, Unix seconds.
    pub since: Option<i64>,
    /// Exclusive upper bound, Unix seconds.
    pub until: Option<i64>,
    pub agent: Option<String>,
    /// Page size for flagged decisions.
    pub limit: usize,
    /// Return flagged decisions with an id below this cursor.
    pub before: Option<i64>,
    /// Bucket days and hours in the local time zone; false buckets in UTC.
    pub local_time: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct TrialReport {
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub agent: Option<String>,
    pub time_zone: &'static str,
    pub cohorts: Vec<Cohort>,
    /// Every deny, would_deny and error decision in the window.
    pub flagged_total: i64,
    pub flagged: Vec<Flagged>,
    /// Pass as `before` to read the next page of flagged decisions.
    pub next_before: Option<i64>,
    /// Plan recovery and error.log incidents; plans are not agent-scoped.
    pub incidents: Vec<Incident>,
    pub acceptance: Vec<Criterion>,
}

/// Activity of one agent in one mode.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Cohort {
    pub agent: String,
    pub mode: String,
    pub first_ts: i64,
    pub last_ts: i64,
    pub first_time: String,
    pub last_time: String,
    pub decisions: i64,
    pub sessions: i64,
    pub active_days: i64,
    /// Distinct clock hours with at least one decision.
    pub active_hours: i64,
    pub working_days: i64,
    pub events: Vec<EventOutcomes>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct EventOutcomes {
    pub event: String,
    pub allow: i64,
    pub deny: i64,
    pub would_deny: i64,
    pub rewrite: i64,
    pub error: i64,
}

/// A decision a human must classify as expected or unexpected.
#[derive(Clone, Debug, Serialize)]
pub struct Flagged {
    pub id: i64,
    pub ts: i64,
    pub time: String,
    pub agent: String,
    pub mode: String,
    pub event: String,
    pub tool: Option<String>,
    pub outcome: String,
    /// Gates that did not pass.
    pub gates: Vec<String>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Incident {
    /// None for error.log lines written without a timestamp.
    pub ts: Option<i64>,
    pub time: String,
    pub kind: String,
    pub project: Option<String>,
    pub plan: Option<String>,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Criterion {
    pub criterion: String,
    pub agent: Option<String>,
    /// `met`, `not_met`, or `needs_human`.
    pub status: &'static str,
    pub detail: String,
}

fn modifiers(local_time: bool) -> (&'static str, &'static str) {
    // (bucket a UTC instant, read a local wall-clock date as UTC); "+0 seconds" is SQLite's no-op.
    if local_time {
        ("localtime", "utc")
    } else {
        ("+0 seconds", "+0 seconds")
    }
}

/// Parse `YYYY-MM-DD` (midnight in the report time zone) or an RFC 3339 timestamp with offset.
pub fn parse_time(store: &Store, text: &str, local_time: bool) -> Result<i64> {
    let shape = Regex::new(
        r"\A\d{4}-\d{2}-\d{2}(?:T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?(?:Z|[+-]\d{2}:\d{2}))?\z",
    )
    .expect("constant time regex");
    ensure!(
        shape.is_match(text),
        "expected YYYY-MM-DD or an RFC 3339 timestamp such as 2026-10-13T09:00:00+06:00, got {text:?}"
    );
    let date_only = text.len() == 10;
    let modifier = if date_only {
        modifiers(local_time).1
    } else {
        "+0 seconds"
    };
    // SQLite normalizes 2026-02-30 to March; a date that does not round-trip is invalid.
    let (valid, seconds): (bool, Option<i64>) = store.connection().query_row(
        "SELECT date(substr(?1,1,10)) IS substr(?1,1,10), CAST(strftime('%s',?1,?2) AS INTEGER)",
        params![text, modifier],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    match seconds {
        Some(seconds) if valid => Ok(seconds),
        _ => anyhow::bail!("invalid date or time {text:?}"),
    }
}

/// Summarize trial evidence without writing; `error_log` is the text of `~/.yoke/error.log`.
pub fn report(store: &Store, query: &TrialQuery, error_log: Option<&str>) -> Result<TrialReport> {
    let connection = store.connection();
    // One snapshot for every query; a deferred transaction never takes a write lock.
    let snapshot = Transaction::new_unchecked(connection, TransactionBehavior::Deferred)?;
    let (bucket, _) = modifiers(query.local_time);
    let cohorts = cohorts(&snapshot, query, bucket)?;
    let flagged_total = snapshot.query_row(
        &format!("SELECT count(*) FROM decisions WHERE {WINDOW} AND {FLAGGED}"),
        params![query.since, query.until, query.agent],
        |row| row.get(0),
    )?;
    let (flagged, next_before) = flagged(&snapshot, query, bucket)?;
    let version: i64 = snapshot.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let mut incidents = if version >= 3 {
        plan_incidents(&snapshot, query, bucket)?
    } else {
        Vec::new()
    };
    for (ts, kind, detail) in error_log.map(error_log_entries).unwrap_or_default() {
        if ts.is_some_and(|ts| {
            query.since.is_some_and(|since| ts < since)
                || query.until.is_some_and(|until| ts >= until)
        }) {
            continue;
        }
        incidents.push(Incident {
            ts,
            time: match ts {
                Some(ts) => format_time(&snapshot, ts, bucket)?,
                None => "unknown".into(),
            },
            kind: kind.into(),
            project: None,
            plan: None,
            detail,
        });
    }
    snapshot.commit()?;
    // Stable: same-second incidents keep their kind order; undated log lines go last.
    incidents.sort_by_key(|incident| (incident.ts.is_none(), incident.ts));
    let acceptance = acceptance(query.agent.as_deref(), &cohorts, flagged_total, &incidents);
    Ok(TrialReport {
        since: query.since,
        until: query.until,
        agent: query.agent.clone(),
        time_zone: if query.local_time { "local" } else { "utc" },
        cohorts,
        flagged_total,
        flagged,
        next_before,
        incidents,
        acceptance,
    })
}

fn cohorts(connection: &Connection, query: &TrialQuery, bucket: &str) -> Result<Vec<Cohort>> {
    let window = params![
        query.since,
        query.until,
        query.agent,
        bucket,
        WORKING_DAY_HOURS
    ];
    let mut cohorts = connection
        .prepare(&format!(
            "WITH w AS (SELECT agent,mode,ts,session_id FROM decisions WHERE {WINDOW}),
             days AS (SELECT agent,mode,strftime('%Y-%m-%d',ts,'unixepoch',?4) AS day,
                count(DISTINCT strftime('%H',ts,'unixepoch',?4)) AS hours FROM w GROUP BY agent,mode,day)
             SELECT w.agent,w.mode,min(ts),max(ts),count(*),count(DISTINCT session_id),
                (SELECT count(*) FROM days d WHERE d.agent=w.agent AND d.mode=w.mode),
                (SELECT coalesce(sum(hours),0) FROM days d WHERE d.agent=w.agent AND d.mode=w.mode),
                (SELECT count(*) FROM days d WHERE d.agent=w.agent AND d.mode=w.mode AND hours>=?5),
                strftime('%Y-%m-%d %H:%M:%S',min(ts),'unixepoch',?4),strftime('%Y-%m-%d %H:%M:%S',max(ts),'unixepoch',?4)
             FROM w GROUP BY w.agent,w.mode ORDER BY w.agent,w.mode"
        ))?
        .query_map(window, |row| {
            Ok(Cohort {
                agent: row.get(0)?,
                mode: row.get(1)?,
                first_ts: row.get(2)?,
                last_ts: row.get(3)?,
                decisions: row.get(4)?,
                sessions: row.get(5)?,
                active_days: row.get(6)?,
                active_hours: row.get(7)?,
                working_days: row.get(8)?,
                first_time: row.get(9)?,
                last_time: row.get(10)?,
                events: Vec::new(),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut statement = connection.prepare(&format!(
        "SELECT agent,mode,event,sum(outcome='allow'),sum(outcome='deny'),sum(outcome='would_deny'),
            sum(outcome='rewrite'),sum(outcome='error')
         FROM decisions WHERE {WINDOW} GROUP BY agent,mode,event ORDER BY agent,mode,event"
    ))?;
    let rows = statement.query_map(params![query.since, query.until, query.agent], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            EventOutcomes {
                event: row.get(2)?,
                allow: row.get(3)?,
                deny: row.get(4)?,
                would_deny: row.get(5)?,
                rewrite: row.get(6)?,
                error: row.get(7)?,
            },
        ))
    })?;
    for row in rows {
        let (agent, mode, outcomes) = row?;
        if let Some(cohort) = cohorts
            .iter_mut()
            .find(|cohort| cohort.agent == agent && cohort.mode == mode)
        {
            cohort.events.push(outcomes);
        }
    }
    Ok(cohorts)
}

fn flagged(
    connection: &Connection,
    query: &TrialQuery,
    bucket: &str,
) -> Result<(Vec<Flagged>, Option<i64>)> {
    let limit = query.limit.clamp(1, 1000);
    let mut statement = connection.prepare(&format!(
        "SELECT id,ts,strftime('%Y-%m-%d %H:%M:%S',ts,'unixepoch',?4),agent,mode,event,tool,outcome,reason,gates
         FROM decisions WHERE {WINDOW} AND {FLAGGED} AND (?5 IS NULL OR id < ?5) ORDER BY id DESC LIMIT ?6"
    ))?;
    let rows = statement.query_map(
        params![
            query.since,
            query.until,
            query.agent,
            bucket,
            query.before,
            limit as i64 + 1
        ],
        |row| {
            let gates: String = row.get(9)?;
            Ok(Flagged {
                id: row.get(0)?,
                ts: row.get(1)?,
                time: row.get(2)?,
                agent: row.get(3)?,
                mode: row.get(4)?,
                event: row.get(5)?,
                tool: row.get(6)?,
                outcome: row.get(7)?,
                gates: failed_gates(&gates),
                reason: row
                    .get::<_, Option<String>>(8)?
                    .map(|reason| redact(&reason)),
            })
        },
    )?;
    let mut page = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    let next = (page.len() > limit).then(|| {
        page.truncate(limit);
        page[limit - 1].id
    });
    Ok((page, next))
}

fn failed_gates(gates: &str) -> Vec<String> {
    let Ok(Value::Array(gates)) = serde_json::from_str::<Value>(gates) else {
        return Vec::new();
    };
    gates
        .iter()
        .filter(|gate| {
            !matches!(
                gate.get("result").and_then(Value::as_str),
                Some("pass" | "rewrite")
            )
        })
        .filter_map(|gate| gate.get("name").and_then(Value::as_str).map(redact))
        .collect()
}

fn plan_incidents(
    connection: &Connection,
    query: &TrialQuery,
    bucket: &str,
) -> Result<Vec<Incident>> {
    // Plans are shared by every agent in a project, so the agent filter does not apply here.
    let mut statement = connection.prepare(
        "SELECT ts,strftime('%Y-%m-%d %H:%M:%S',ts,'unixepoch',?3),kind,project,slug,detail FROM (
            SELECT t.ts,t.id,p.project,p.slug,
                CASE WHEN t.from_state=t.to_state THEN 'reassign'
                     WHEN t.to_state='building' THEN 'resume'
                     WHEN t.to_state='stalled' THEN 'stall'
                     ELSE 'cancel' END AS kind,
                t.from_state||' -> '||t.to_state||coalesce(': '||t.note,'') AS detail
            FROM transitions t JOIN plans p ON p.id=t.plan_id
            WHERE (t.to_state='building' AND t.from_state IN ('waiting','stalled','checking') AND t.actor='human')
               OR (t.to_state IN ('stalled','cancelled') AND t.from_state!=t.to_state)
               OR (t.from_state=t.to_state AND t.note LIKE 'reassigned from %')
            UNION ALL
            SELECT c.ts,c.id,p.project,p.slug,'interrupted_check',c.command
            FROM check_runs c JOIN plans p ON p.id=c.plan_id WHERE c.result='interrupted'
         ) WHERE (?1 IS NULL OR ts >= ?1) AND (?2 IS NULL OR ts < ?2) ORDER BY ts,kind,id",
    )?;
    let rows = statement.query_map(params![query.since, query.until, bucket], |row| {
        Ok(Incident {
            ts: Some(row.get(0)?),
            time: row.get(1)?,
            kind: row.get(2)?,
            project: Some(row.get(3)?),
            plan: Some(row.get(4)?),
            detail: redact(&row.get::<_, String>(5)?),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Classify error.log lines; the leading Unix timestamp is absent on lines a child process wrote.
fn error_log_entries(text: &str) -> Vec<(Option<i64>, &'static str, String)> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (ts, message) = match line.split_once(' ') {
                Some((head, rest)) if head.parse::<i64>().is_ok() => (head.parse().ok(), rest),
                _ => (None, line),
            };
            let kind = if message.contains("was not saved") {
                "state_write_failed"
            } else if message.contains("yoke error:") {
                "hook_error"
            } else if message.contains("usage ingestion failed") {
                "usage_ingestion_failed"
            } else {
                "error_log"
            };
            let mut detail = redact(message.trim());
            if detail.len() > 512 {
                let mut end = 512;
                while !detail.is_char_boundary(end) {
                    end -= 1;
                }
                detail.truncate(end);
                detail.push_str("...");
            }
            (ts, kind, detail)
        })
        .collect()
}

fn format_time(connection: &Connection, ts: i64, bucket: &str) -> Result<String> {
    connection
        .query_row(
            "SELECT strftime('%Y-%m-%d %H:%M:%S',?1,'unixepoch',?2)",
            params![ts, bucket],
            |row| row.get(0),
        )
        .context("format timestamp")
}

fn acceptance(
    agent: Option<&str>,
    cohorts: &[Cohort],
    flagged_total: i64,
    incidents: &[Incident],
) -> Vec<Criterion> {
    let mut criteria = Vec::new();
    for name in AGENTS
        .iter()
        .filter(|name| agent.is_none_or(|agent| agent == **name))
    {
        for (mode, needed, label) in [
            (
                "observe",
                OBSERVE_WORKING_DAYS,
                "observe trial covers a full working day",
            ),
            (
                "enforce",
                ENFORCE_WORKING_DAYS,
                "enforce trial covers a working week",
            ),
        ] {
            let days = cohorts
                .iter()
                .find(|cohort| cohort.agent == *name && cohort.mode == mode)
                .map_or(0, |cohort| cohort.working_days);
            criteria.push(Criterion {
                criterion: label.into(),
                agent: Some((*name).into()),
                status: if days >= needed { "met" } else { "not_met" },
                detail: format!(
                    "{days} of {needed} working day(s) in {mode} mode; a working day has decisions in at least {WORKING_DAY_HOURS} distinct clock hours"
                ),
            });
        }
    }
    let logged = incidents
        .iter()
        .filter(|incident| incident.plan.is_none())
        .count();
    criteria.push(Criterion {
        criterion: "every denial and error is classified and unexpected ones are resolved".into(),
        agent: None,
        status: if flagged_total == 0 && logged == 0 { "met" } else { "needs_human" },
        detail: format!("{flagged_total} deny/would_deny/error decision(s) and {logged} error.log entr(ies) to mark expected or unexpected in docs/trials.md"),
    });
    let recoveries = incidents.len() - logged;
    criteria.push(Criterion {
        criterion: "recovery incidents are documented".into(),
        agent: None,
        status: if recoveries == 0 {
            "met"
        } else {
            "needs_human"
        },
        detail: format!("{recoveries} plan recovery incident(s) to describe in docs/trials.md"),
    });
    criteria
}

#[cfg(test)]
#[path = "trial/tests.rs"]
mod tests;
