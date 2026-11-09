//! One recorded agent session in full, and session search, for the console. Read-only.
//! Everything comes from what the hooks already stored: decisions with their redacted, capped
//! payloads, facts and the memories made from them, and imported usage samples.
use anyhow::Result;
use rusqlite::{params, OptionalExtension};
use serde_json::Value;

use crate::memory::{memory_row, Memory};
use crate::redact::redact;
use crate::store::{SessionSummary, Store};
use crate::touched::{files_in_payload, Touched};
use crate::usage::{self, UsageSummary, UsageTotals};

/// Timeline entries shown, the newest ones; the decisions ledger keeps the rest.
pub const TIMELINE_LIMIT: usize = 300;
/// Tool calls read for touched files, newest first.
pub const FILE_SCAN_LIMIT: usize = 5000;
/// Newest prompts searched by text.
pub const PROMPT_SEARCH_LIMIT: usize = 5000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimelineEvent {
    pub id: i64,
    pub ts: i64,
    pub event: String,
    pub tool: Option<String>,
    pub mode: String,
    pub outcome: String,
    pub reason: Option<String>,
    /// The prompt, command or path the event carried, when the stored payload names one.
    pub summary: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCount {
    pub tool: String,
    pub calls: i64,
    /// Calls denied, or that an enforcing Yoke would have denied.
    pub denied: i64,
}

pub struct SessionDetail {
    pub session: SessionSummary,
    pub first_prompt: Option<String>,
    /// Oldest first: the newest `TIMELINE_LIMIT` events, without routine allowed post-tool records.
    pub timeline: Vec<TimelineEvent>,
    /// Events that qualify for the timeline, shown or not.
    pub timeline_total: i64,
    pub tools: Vec<ToolCount>,
    pub files: Touched,
    /// More tool calls exist than `FILE_SCAN_LIMIT`, so older files may be missing.
    pub files_partial: bool,
    pub memories: Vec<Memory>,
    /// Imported token usage by model, and its total with the estimated cost.
    pub usage: Vec<UsageSummary>,
    pub usage_total: UsageTotals,
}

const SESSION_COLUMNS: &str =
    "s.id,s.agent,s.project,s.started_at,s.ended_at,s.active_plan,s.decision_count,
     (SELECT max(d.ts) FROM decisions d WHERE d.session_id=s.id),
     (SELECT d.mode FROM decisions d WHERE d.session_id=s.id ORDER BY d.ts DESC,d.id DESC LIMIT 1),
     p.slug,p.state";

fn session_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionSummary> {
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
}

/// Rows the timeline leaves out: each tool call already shows as its `pre_tool_use` decision.
const ROUTINE: &str = "NOT (event IN ('post_tool_use','PostToolUse') AND outcome='allow')";

/// The session with this exact id, or `None` when none was recorded.
pub fn detail(store: &Store, id: &str) -> Result<Option<SessionDetail>> {
    let connection = store.connection();
    let id = redact(id);
    let Some(session) = connection
        .query_row(
            &format!("SELECT {SESSION_COLUMNS} FROM sessions s LEFT JOIN plans p ON p.id=s.active_plan WHERE s.id=?1"),
            [&id],
            session_row,
        )
        .optional()?
    else {
        return Ok(None);
    };
    let timeline_total = connection.query_row(
        &format!("SELECT count(*) FROM decisions WHERE session_id=?1 AND {ROUTINE}"),
        [&id],
        |row| row.get(0),
    )?;
    let mut timeline = connection
        .prepare(&format!(
            "SELECT id,ts,event,tool,mode,outcome,reason,payload FROM decisions WHERE session_id=?1 AND {ROUTINE}
             ORDER BY ts DESC,id DESC LIMIT ?2"
        ))?
        .query_map(params![id, TIMELINE_LIMIT as i64], |row| {
            let (event, tool, payload): (String, Option<String>, String) =
                (row.get(2)?, row.get(3)?, row.get(7)?);
            Ok(TimelineEvent {
                id: row.get(0)?,
                ts: row.get(1)?,
                summary: summary(&event, &payload),
                event,
                tool,
                mode: row.get(4)?,
                outcome: row.get(5)?,
                reason: row.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    timeline.reverse();
    let tools = connection
        .prepare(
            "SELECT coalesce(tool,'(unnamed)'),count(*),coalesce(sum(outcome IN ('deny','would_deny')),0)
             FROM decisions WHERE session_id=?1 AND event='pre_tool_use'
             GROUP BY 1 ORDER BY 2 DESC,1",
        )?
        .query_map([&id], |row| {
            Ok(ToolCount {
                tool: row.get(0)?,
                calls: row.get(1)?,
                denied: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut files = Touched::default();
    let mut scanned = 0;
    let mut calls = connection.prepare(
        "SELECT tool,payload FROM decisions WHERE session_id=?1 AND event='pre_tool_use'
         ORDER BY ts DESC,id DESC LIMIT ?2",
    )?;
    let rows = calls.query_map(params![id, FILE_SCAN_LIMIT as i64 + 1], |row| {
        Ok((row.get::<_, Option<String>>(0)?, row.get::<_, String>(1)?))
    })?;
    // Newest first, so reverse each list afterwards to read in the order the session went.
    for row in rows {
        let (tool, payload) = row?;
        scanned += 1;
        if scanned > FILE_SCAN_LIMIT {
            break;
        }
        files.merge(files_in_payload(tool.as_deref(), &payload));
    }
    files.modified.reverse();
    files.read.reverse();
    let memories = connection
        .prepare(
            "SELECT m.id,m.identity,m.ts,m.project,m.source,m.text,m.pinned,m.shared,m.memory_type
             FROM memories m WHERE m.project=?2 AND m.id IN
             (SELECT mf.memory_id FROM memory_facts mf JOIN facts f ON f.id=mf.fact_id WHERE f.session_id=?1)
             ORDER BY m.ts,m.id LIMIT 200",
        )?
        .query_map(params![id, session.project], memory_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let report = usage::report(
        store,
        None,
        Some(&id),
        &usage::UsageRange::default(),
        usage::Period::Day,
    )?;
    Ok(Some(SessionDetail {
        first_prompt: first_prompt(store, &id)?,
        usage: report.sessions,
        usage_total: report.totals,
        files_partial: scanned > FILE_SCAN_LIMIT,
        session,
        timeline,
        timeline_total,
        tools,
        files,
        memories,
    }))
}

/// The first prompt recorded for a session.
pub fn first_prompt(store: &Store, id: &str) -> Result<Option<String>> {
    Ok(store
        .connection()
        .query_row(
            "SELECT payload FROM decisions WHERE session_id=?1 AND event='user_prompt_submit' ORDER BY ts,id LIMIT 1",
            [redact(id)],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .and_then(|payload| prompt(&payload)))
}

/// Sessions whose id or project contains `query`, or whose prompt does, among the newest
/// `PROMPT_SEARCH_LIMIT` prompts. Case-insensitive for ASCII; `project` narrows to one project.
pub fn search(
    store: &Store,
    project: Option<&str>,
    query: &str,
    limit: usize,
) -> Result<Vec<SessionSummary>> {
    let query = query.trim().to_lowercase();
    let mut statement = store.connection().prepare(&format!(
        "SELECT {SESSION_COLUMNS} FROM sessions s LEFT JOIN plans p ON p.id=s.active_plan
         WHERE (?1 IS NULL OR s.project=?1)
         AND (instr(lower(s.id),?2)>0 OR instr(lower(s.project),?2)>0 OR s.id IN (
             SELECT session_id FROM (SELECT session_id,payload FROM decisions
                 WHERE event='user_prompt_submit' ORDER BY id DESC LIMIT ?4)
             WHERE instr(lower(CASE WHEN json_valid(payload) THEN coalesce(json_extract(payload,'$.prompt'),'')
                 WHEN instr(payload,'\"prompt\"')>0 THEN substr(payload,instr(payload,'\"prompt\"')+8) ELSE '' END),?2)>0))
         ORDER BY s.started_at DESC,s.id LIMIT ?3"
    ))?;
    let rows = statement.query_map(
        params![
            project.map(redact),
            query,
            limit.min(1000) as i64,
            PROMPT_SEARCH_LIMIT as i64
        ],
        session_row,
    )?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The prompt, command or path a stored payload carries.
fn summary(event: &str, payload: &str) -> Option<String> {
    if event == "user_prompt_submit" {
        return prompt(payload);
    }
    let value: Value = serde_json::from_str(payload).ok()?;
    let input = value.get("tool_input")?;
    [
        "command",
        "cmd",
        "file_path",
        "notebook_path",
        "path",
        "pattern",
        "url",
    ]
    .iter()
    .find_map(|key| {
        let text = match input.get(key)? {
            Value::String(text) => text.clone(),
            Value::Array(parts) => parts
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
            _ => return None,
        };
        (!text.trim().is_empty()).then_some(text)
    })
}

/// The prompt text of a `user_prompt_submit` payload. A payload cut by the size cap is no longer
/// JSON; then the prompt is decoded up to the cut, which is better than showing nothing.
fn prompt(payload: &str) -> Option<String> {
    if let Ok(value) = serde_json::from_str::<Value>(payload) {
        return value
            .get("prompt")?
            .as_str()
            .filter(|text| !text.trim().is_empty())
            .map(str::to_owned);
    }
    let at = payload.find("\"prompt\"")?;
    let body = payload[at + "\"prompt\"".len()..]
        .trim_start()
        .strip_prefix(':')?
        .trim_start()
        .strip_prefix('"')?;
    let mut escaped = false;
    let end = body
        .char_indices()
        .find_map(|(index, ch)| {
            let closes = ch == '"' && !escaped;
            escaped = ch == '\\' && !escaped;
            closes.then_some(index)
        })
        .unwrap_or(body.len());
    let mut text = &body[..end];
    // A cut can end inside an escape such as `\u00`; drop that partial escape.
    for _ in 0..6 {
        if let Ok(decoded) = serde_json::from_str::<String>(&format!("\"{text}\"")) {
            return (!decoded.trim().is_empty()).then_some(decoded);
        }
        let Some((cut, _)) = text.char_indices().next_back() else {
            break;
        };
        text = &text[..cut];
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::DecisionRecord;

    /// (time, session, project, event, tool, outcome, payload)
    type Row<'a> = (
        i64,
        &'a str,
        &'a str,
        &'a str,
        Option<&'a str>,
        &'a str,
        &'a str,
    );

    #[test]
    fn detail_reads_the_session_story_and_search_matches_prompts_ids_and_projects() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path().join("state.db")).unwrap();
        let (a, b) = ("/work/alpha", "/work/beta");
        let rows: [Row; 6] = [
            (
                10,
                "s-alpha-1",
                a,
                "session_start",
                None,
                "allow",
                r#"{"cwd":"/work/alpha"}"#,
            ),
            (
                11,
                "s-alpha-1",
                a,
                "user_prompt_submit",
                None,
                "allow",
                r#"{"cwd":"/work/alpha","prompt":"Fix the flaky login test"}"#,
            ),
            (
                12,
                "s-alpha-1",
                a,
                "pre_tool_use",
                Some("Edit"),
                "allow",
                r#"{"tool_name":"Edit","tool_input":{"file_path":"/work/alpha/src/login.rs","old_string":"a"}}"#,
            ),
            (
                13,
                "s-alpha-1",
                a,
                "post_tool_use",
                Some("Edit"),
                "allow",
                r#"{"tool_name":"Edit"}"#,
            ),
            (
                14,
                "s-alpha-1",
                a,
                "pre_tool_use",
                Some("Bash"),
                "would_deny",
                r#"{"tool_name":"Bash","tool_input":{"command":"rm -rf target"}}"#,
            ),
            // A prompt cut by the payload cap inside an escape is still read up to the cut.
            (
                15,
                "s-beta-2",
                b,
                "user_prompt_submit",
                None,
                "allow",
                r#"{"cwd":"/work/beta","prompt":"Write the release notes \u00"#,
            ),
        ];
        for (ts, session, project, event, tool, outcome, payload) in rows {
            let row = DecisionRecord {
                policy_hash: None,
                ts,
                session_id: session.into(),
                agent: "claude".into(),
                project: project.into(),
                event: event.into(),
                tool: tool.map(Into::into),
                mode: "observe".into(),
                outcome: outcome.into(),
                reason: (outcome != "allow").then(|| "gate said no".into()),
                gates: serde_json::json!([]),
                payload: payload.into(),
            };
            store.record_decision(&row, 4096).unwrap();
        }
        let fact =
            crate::memory::record_fact(&store, "s-alpha-1", a, "file_write", "Edit: login.rs", 12)
                .unwrap();
        let memory = crate::memory::remember(
            &store,
            a,
            "Login test waits for the session cookie",
            false,
            16,
        )
        .unwrap();
        store
            .connection()
            .execute(
                "INSERT INTO memory_facts(memory_id,fact_id) VALUES (?1,?2)",
                params![memory.id, fact],
            )
            .unwrap();

        let detail = detail(&store, "s-alpha-1").unwrap().unwrap();
        assert_eq!(
            detail.first_prompt.as_deref(),
            Some("Fix the flaky login test")
        );
        let events: Vec<(&str, Option<&str>)> = detail
            .timeline
            .iter()
            .map(|e| (e.event.as_str(), e.summary.as_deref()))
            .collect();
        assert_eq!(
            events,
            [
                ("session_start", None),
                ("user_prompt_submit", Some("Fix the flaky login test")),
                ("pre_tool_use", Some("/work/alpha/src/login.rs")),
                ("pre_tool_use", Some("rm -rf target")),
            ],
            "the allowed post-tool record is left out"
        );
        assert_eq!(detail.timeline_total, 4);
        assert_eq!(
            detail.tools,
            [
                ToolCount {
                    tool: "Bash".into(),
                    calls: 1,
                    denied: 1
                },
                ToolCount {
                    tool: "Edit".into(),
                    calls: 1,
                    denied: 0
                },
            ]
        );
        assert_eq!(detail.files.modified, ["/work/alpha/src/login.rs"]);
        assert!(!detail.files_partial);
        assert_eq!(
            detail.memories.iter().map(|m| m.id).collect::<Vec<_>>(),
            [memory.id]
        );
        assert!(detail.usage.is_empty());
        assert_eq!(detail.usage_total.samples, 0);
        assert!(super::detail(&store, "missing").unwrap().is_none());
        assert_eq!(
            first_prompt(&store, "s-beta-2").unwrap().as_deref(),
            Some("Write the release notes ")
        );

        let ids = |project: Option<&str>, query: &str| {
            search(&store, project, query, 50)
                .unwrap()
                .into_iter()
                .map(|s| s.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(None, "FLAKY login"), ["s-alpha-1"]);
        assert_eq!(ids(None, "release notes"), ["s-beta-2"]);
        assert_eq!(ids(None, "beta-2"), ["s-beta-2"]);
        assert_eq!(ids(None, "/work/"), ["s-beta-2", "s-alpha-1"]);
        assert_eq!(ids(Some(a), "/work/"), ["s-alpha-1"]);
        assert!(
            ids(None, "cwd").is_empty(),
            "JSON keys of a prompt payload never match"
        );
    }
}
