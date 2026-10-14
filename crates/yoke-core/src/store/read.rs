use super::*;

#[derive(Clone, Debug, Serialize)]
pub struct DecisionRow {
    pub id: i64,
    pub record: DecisionRecord,
}

#[derive(Clone, Debug, Serialize)]
pub struct SessionSummary {
    pub id: String,
    pub agent: String,
    pub project: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub active_plan: Option<i64>,
    pub decision_count: i64,
    /// Time of the newest recorded decision; None when the session has none.
    pub last_event_at: Option<i64>,
    /// Mode Yoke applied at the newest recorded decision.
    pub last_mode: Option<String>,
    pub plan_slug: Option<String>,
    pub plan_state: Option<String>,
}

/// One project recorded by sessions, decisions or plans.
#[derive(Clone, Debug, Serialize)]
pub struct ProjectSummary {
    pub project: String,
    pub sessions: i64,
    /// Sessions without a recorded end event. This is not proof that the agent is running.
    pub open_sessions: i64,
    pub last_activity: Option<i64>,
    /// Denied or would-deny decisions at or after the caller's `since` time.
    pub recent_denials: i64,
    /// Plans that are neither done nor cancelled.
    pub open_plans: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct StatusSnapshot {
    pub mode: Option<String>,
    pub plan_slug: Option<String>,
    pub plan_state: Option<String>,
    pub recent_deny: bool,
    pub latest_version: Option<String>,
}

impl Store {
    /// Cursor pagination uses insertion order; limit is capped at 10,000.
    pub fn decision_page(
        &self,
        filter: &DecisionFilter,
        before_id: Option<i64>,
    ) -> Result<Vec<DecisionRow>> {
        let mut statement = self.connection.prepare(
            "SELECT id,ts,session_id,agent,project,event,tool,mode,outcome,reason,gates,payload FROM decisions
             WHERE (?1 IS NULL OR project=?1) AND (?2 IS NULL OR session_id=?2)
             AND (?3 IS NULL OR outcome=?3) AND (?4 IS NULL OR tool=?4)
             AND (?5 IS NULL OR id<?5) ORDER BY id DESC LIMIT ?6")?;
        let rows = statement.query_map(
            params![
                filter.project.as_deref().map(redact),
                filter.session.as_deref().map(redact),
                filter.outcome,
                filter.tool.as_deref().map(redact),
                before_id,
                filter.limit.min(10_000) as i64
            ],
            |row| {
                let gates: String = row.get(10)?;
                Ok((
                    DecisionRow {
                        id: row.get(0)?,
                        record: DecisionRecord {
                            ts: row.get(1)?,
                            session_id: row.get(2)?,
                            agent: row.get(3)?,
                            project: row.get(4)?,
                            event: row.get(5)?,
                            tool: row.get(6)?,
                            mode: row.get(7)?,
                            outcome: row.get(8)?,
                            reason: row.get(9)?,
                            gates: Value::Null,
                            payload: row.get(11)?,
                        },
                    },
                    gates,
                ))
            },
        )?;
        rows.map(|row| {
            let (mut row, gates) = row?;
            row.record.gates = serde_json::from_str(&gates)?;
            Ok(row)
        })
        .collect()
    }

    pub fn sessions(&self, project: Option<&str>, limit: usize) -> Result<Vec<SessionSummary>> {
        let mut statement = self.connection.prepare(
            "SELECT s.id,s.agent,s.project,s.started_at,s.ended_at,s.active_plan,s.decision_count,
             (SELECT max(d.ts) FROM decisions d WHERE d.session_id=s.id),
             (SELECT d.mode FROM decisions d WHERE d.session_id=s.id ORDER BY d.ts DESC,d.id DESC LIMIT 1),
             p.slug,p.state
             FROM sessions s LEFT JOIN plans p ON p.id=s.active_plan
             WHERE (?1 IS NULL OR s.project=?1) ORDER BY s.started_at DESC,s.id LIMIT ?2")?;
        let rows = statement.query_map(
            params![project.map(redact), limit.min(1000) as i64],
            |row| {
                Ok(SessionSummary {
                    id: row.get(0)?,
                    agent: row.get(1)?,
                    project: row.get(2)?,
                    started_at: row.get(3)?,
                    ended_at: row.get(4)?,
                    active_plan: row.get(5)?,
                    decision_count: row.get(6)?,
                    last_event_at: row.get(7)?,
                    last_mode: row.get(8)?,
                    plan_slug: row.get(9)?,
                    plan_state: row.get(10)?,
                })
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Every recorded project, most recently active first.
    pub fn projects(&self, since: i64, limit: usize) -> Result<Vec<ProjectSummary>> {
        let mut statement = self.connection.prepare(
            "WITH known(project) AS (SELECT project FROM sessions UNION SELECT project FROM decisions UNION SELECT project FROM plans)
             SELECT k.project,
             (SELECT count(*) FROM sessions s WHERE s.project=k.project),
             (SELECT count(*) FROM sessions s WHERE s.project=k.project AND s.ended_at IS NULL),
             nullif(max(coalesce((SELECT max(ts) FROM decisions d WHERE d.project=k.project),0),
                        coalesce((SELECT max(started_at) FROM sessions s WHERE s.project=k.project),0),
                        coalesce((SELECT max(updated_at) FROM plans p WHERE p.project=k.project),0)),0) AS active,
             (SELECT count(*) FROM decisions d WHERE d.project=k.project AND d.ts>=?1 AND d.outcome IN ('deny','would_deny')),
             (SELECT count(*) FROM plans p WHERE p.project=k.project AND p.state NOT IN ('done','cancelled'))
             FROM known k ORDER BY active IS NULL,active DESC,k.project LIMIT ?2",
        )?;
        let rows = statement.query_map(params![since, limit.min(1000) as i64], |row| {
            Ok(ProjectSummary {
                project: row.get(0)?,
                sessions: row.get(1)?,
                open_sessions: row.get(2)?,
                last_activity: row.get(3)?,
                recent_denials: row.get(4)?,
                open_plans: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Compare values on the same persistent connection to notice other writers.
    pub fn revision(&self) -> Result<i64> {
        Ok(self
            .connection
            .pragma_query_value(None, "data_version", |row| row.get(0))?)
    }

    /// One query; recent denials cover the preceding hour, including observe-mode denials.
    pub fn status_snapshot(
        &self,
        session: Option<&str>,
        project: Option<&str>,
        now: i64,
    ) -> Result<StatusSnapshot> {
        let decision_scope = match (session, project) {
            (Some(_), Some(_)) => "session_id=?1 AND project=?2",
            (Some(_), None) => "session_id=?1",
            (None, Some(_)) => "project=?2",
            _ => "1=1",
        };
        let session_scope = match (session, project) {
            (Some(_), Some(_)) => "s.id=?1 AND s.project=?2",
            (Some(_), None) => "s.id=?1",
            (None, Some(_)) => "s.project=?2",
            _ => "1=1",
        };
        let sql = format!("WITH active AS (SELECT p.slug,p.state FROM sessions s JOIN plans p ON p.id=s.active_plan WHERE {session_scope} ORDER BY s.started_at DESC LIMIT 1)
            SELECT (SELECT mode FROM decisions WHERE {decision_scope} ORDER BY ts DESC,id DESC LIMIT 1),
            (SELECT slug FROM active),(SELECT state FROM active),
            EXISTS(SELECT 1 FROM decisions WHERE {decision_scope} AND ts>=?3 AND outcome IN ('deny','would_deny')),
            (SELECT value FROM kv WHERE key='update.latest_version')");
        Ok(self.connection.query_row(
            &sql,
            params![
                session.map(redact),
                project.map(redact),
                now.saturating_sub(3600)
            ],
            |row| {
                Ok(StatusSnapshot {
                    mode: row.get(0)?,
                    plan_slug: row.get(1)?,
                    plan_state: row.get(2)?,
                    recent_deny: row.get(3)?,
                    latest_version: row.get(4)?,
                })
            },
        )?)
    }
}
