//! Persistent progress-aware stop limits; this module never resumes a plan.

use std::path::Path;

use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::Serialize;
use serde_json::Value;

use crate::{
    memory, plan,
    store::{DecisionRecord, Store},
};

#[derive(Debug, Default, Serialize)]
pub struct StopDecision {
    pub block: bool,
    /// True only when this call actually changed the plan to stalled.
    pub stalled: bool,
    pub reason: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct ActivePlan {
    id: i64,
    slug: String,
    state: String,
    revision: i64,
    agent: String,
}

fn active(connection: &Connection, project: &str, session: &str) -> Result<Option<ActivePlan>> {
    Ok(connection.query_row(
        "SELECT p.id,p.slug,p.state,p.revision,s.agent FROM sessions s JOIN plans p ON p.id=s.active_plan WHERE s.id=?1 AND s.project=?2 AND p.project=?2",
        params![session, project],
        |row| Ok(ActivePlan { id: row.get(0)?, slug: row.get(1)?, state: row.get(2)?, revision: row.get(3)?, agent: row.get(4)? }),
    ).optional()?)
}

/// Evaluate a real workspace fingerprint; errors must follow the hook's fail-closed path.
/// Observe mode reports would-block without writing counters or plan state.
pub fn evaluate(
    store: &Store,
    project: &Path,
    session: &str,
    max_blocks: u32,
    now: i64,
    enforce: bool,
) -> Result<StopDecision> {
    let project = project
        .canonicalize()
        .context("resolve stop-guard project")?;
    let project_key = project
        .to_str()
        .context("stop-guard project is not UTF-8")?;
    let Some(current) = active(store.connection(), project_key, session)? else {
        return Ok(StopDecision::default());
    };
    if !matches!(current.state.as_str(), "building" | "checking") {
        return Ok(StopDecision::default());
    }
    let fingerprint = plan::fingerprint(&project).context("compute stop-guard progress")?;
    tick(
        store,
        project_key,
        session,
        &current,
        &fingerprint,
        max_blocks,
        now,
        enforce,
    )
}

#[allow(clippy::too_many_arguments)]
fn tick(
    store: &Store,
    project: &str,
    session: &str,
    expected: &ActivePlan,
    fingerprint: &str,
    max_blocks: u32,
    now: i64,
    enforce: bool,
) -> Result<StopDecision> {
    store.transaction(|transaction| {
        tick_in(
            transaction,
            project,
            session,
            expected,
            fingerprint,
            max_blocks,
            now,
            enforce,
        )
    })
}

#[allow(clippy::too_many_arguments)]
fn tick_in(
    transaction: &Transaction<'_>,
    project: &str,
    session: &str,
    expected: &ActivePlan,
    fingerprint: &str,
    max_blocks: u32,
    now: i64,
    enforce: bool,
) -> Result<StopDecision> {
    let current = active(transaction, project, session)?;
    ensure!(
        current.as_ref() == Some(expected),
        "active plan changed while computing stop progress; retry"
    );
    ensure!(
        matches!(expected.state.as_str(), "building" | "checking"),
        "stop guard only accepts active work"
    );
    ensure!(
        matches!(expected.agent.as_str(), "claude" | "codex"),
        "stop guard requires a recorded claude or codex session agent"
    );
    let transition: i64 = transaction.query_row(
        "SELECT COALESCE(MAX(id),0) FROM transitions WHERE plan_id=?1",
        [expected.id],
        |row| row.get(0),
    )?;
    let check: i64 = transaction.query_row(
        "SELECT COALESCE(MAX(id),0) FROM check_runs WHERE plan_id=?1",
        [expected.id],
        |row| row.get(0),
    )?;
    let previous: Option<(u32, String, i64, i64)> = transaction.query_row(
            "SELECT blocks,last_fingerprint,last_transition_id,last_check_run_id FROM stop_guard WHERE session_id=?1 AND plan_id=?2",
            params![session, expected.id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        ).optional()?;
    let blocks = match previous {
        Some((blocks, last_fingerprint, last_transition, last_check)) => {
            ensure!(
                transition >= last_transition && check >= last_check,
                "stop progress history moved backwards"
            );
            if fingerprint != last_fingerprint || transition > last_transition || check > last_check
            {
                0
            } else {
                blocks
            }
        }
        None => 0,
    };
    if blocks < max_blocks {
        if enforce {
            transaction.execute("INSERT INTO stop_guard(session_id,plan_id,blocks,last_fingerprint,last_block_at,last_transition_id,last_check_run_id) VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(session_id,plan_id) DO UPDATE SET blocks=excluded.blocks,last_fingerprint=excluded.last_fingerprint,last_block_at=excluded.last_block_at,last_transition_id=excluded.last_transition_id,last_check_run_id=excluded.last_check_run_id",
                    params![session, expected.id, blocks + 1, fingerprint, now, transition, check])?;
        }
        return Ok(StopDecision { block: true, stalled: false, reason: Some(format!("plan '{}' is {}; finish the work and run `yoke plan check {} --agent {}` (stop block {}/{max_blocks})", expected.slug, expected.state, expected.slug, expected.agent, blocks + 1)) });
    }
    if enforce {
        let changed = transaction.execute("UPDATE plans SET state='stalled',revision=revision+1,updated_at=?1 WHERE id=?2 AND revision=?3", params![now, expected.id, expected.revision])?;
        ensure!(changed == 1, "plan changed before stalling");
        transaction.execute("INSERT INTO transitions(plan_id,ts,from_state,to_state,actor,note) VALUES (?1,?2,?3,'stalled','yoke',?4)", params![expected.id, now, expected.state, "stop limit reached without progress"])?;
        let detail = serde_json::json!({"plan": expected.slug, "from": expected.state, "to": "stalled", "actor": "yoke", "note": "stop limit reached without progress"}).to_string();
        transaction.execute("INSERT INTO facts(ts,session_id,project,kind,detail) VALUES (?1,?2,?3,'plan_transition',?4)", params![now, session, project, crate::redact::redact_payload(&detail, 4096)])?;
    }
    Ok(StopDecision { block: false, stalled: enforce, reason: Some(format!("plan '{}' {} stalled after {max_blocks} stops without progress; a human can run `yoke plan resume {}`", expected.slug, if enforce { "is" } else { "would become" }, expected.slug)) })
}

/// Atomically persist the final Stop decision, hook facts, and guard progress.
/// A pre-existing gate denial never advances the guard; the caller row changes only on commit.
pub fn record_stop(
    store: &Store,
    project: &Path,
    max_blocks: u32,
    row: &mut DecisionRecord,
    payload_cap: usize,
    raw: &Value,
) -> Result<StopDecision> {
    ensure!(
        row.event == "stop" && !row.session_id.is_empty(),
        "record_stop requires a Stop with a session"
    );
    ensure!(
        matches!(row.mode.as_str(), "observe" | "enforce"),
        "record_stop requires a resolved mode"
    );
    ensure!(
        matches!(
            row.outcome.as_str(),
            "allow" | "deny" | "would_deny" | "error"
        ),
        "invalid base Stop outcome"
    );
    ensure!(row.gates.is_array(), "Stop gates must be an array");
    let project = project
        .canonicalize()
        .context("resolve stop-guard project")?;
    let project_key = project
        .to_str()
        .context("stop-guard project is not UTF-8")?;
    ensure!(row.project == project_key, "Stop decision project mismatch");
    let expected = if row.outcome == "allow" {
        active(store.connection(), project_key, &row.session_id)?
    } else {
        None
    };
    let fingerprint = if expected
        .as_ref()
        .is_some_and(|p| matches!(p.state.as_str(), "building" | "checking"))
    {
        plan::fingerprint(&project).context("compute stop-guard progress")?
    } else {
        String::new()
    };
    persist_stop(
        store,
        project_key,
        expected.as_ref(),
        &fingerprint,
        max_blocks,
        row,
        payload_cap,
        raw,
    )
}

#[allow(clippy::too_many_arguments)]
fn persist_stop(
    store: &Store,
    project: &str,
    expected: Option<&ActivePlan>,
    fingerprint: &str,
    max_blocks: u32,
    row: &mut DecisionRecord,
    payload_cap: usize,
    raw: &Value,
) -> Result<StopDecision> {
    let mut prepared = row.clone();
    let decision = store.transaction(|tx| {
        let mut decision = StopDecision::default();
        if prepared.outcome == "allow" {
            let current = active(tx, project, &prepared.session_id)?;
            ensure!(current.as_ref() == expected, "active plan changed while computing stop progress; retry");
            if let Some(expected) = expected.filter(|p| matches!(p.state.as_str(), "building" | "checking")) {
                decision = tick_in(tx, project, &prepared.session_id, expected, fingerprint, max_blocks, prepared.ts, prepared.mode == "enforce")?;
            }
        }
        if decision.block {
            prepared.outcome = if prepared.mode == "observe" { "would_deny" } else { "deny" }.into();
        }
        if let Some(reason) = &decision.reason {
            prepared.reason = Some(match prepared.reason.take() {
                Some(previous) if !previous.is_empty() => format!("{previous}; {reason}"),
                _ => reason.clone(),
            });
        }
        if decision.block || decision.stalled {
            prepared.gates.as_array_mut().context("Stop gates must be an array")?.push(serde_json::json!({"name":"stop-guard", "result":if decision.block {"deny"} else {"stalled"}, "ms":0,"exit_code":null}));
        }
        memory::record_hook_facts_in(tx, &prepared, raw)?;
        Store::record_decision_in(tx, &prepared, payload_cap)?;
        Ok(decision)
    })?;
    *row = prepared;
    Ok(decision)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> DecisionRecord {
        DecisionRecord {
            ts: 10,
            session_id: "session".into(),
            agent: "codex".into(),
            project: "/fixture".into(),
            event: "stop".into(),
            tool: None,
            mode: "enforce".into(),
            outcome: "allow".into(),
            reason: None,
            gates: serde_json::json!([]),
            payload: "{}".into(),
        }
    }

    #[test]
    fn atomic_stop_rolls_back_guard_facts_and_caller_row_on_each_write_failure() {
        for (table, cap) in [
            ("facts", 2),
            ("decisions", 2),
            ("facts", 0),
            ("decisions", 0),
        ] {
            let (_dir, store) = fixture();
            let expected = active(store.connection(), "/fixture", "session")
                .unwrap()
                .unwrap();
            let mut row = row();
            let original = row.clone();
            store.connection().execute_batch(&format!("CREATE TEMP TRIGGER reject_write BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT,'fixture write failure'); END;")).unwrap();
            assert!(persist_stop(
                &store,
                "/fixture",
                Some(&expected),
                "a",
                cap,
                &mut row,
                4096,
                &serde_json::json!({})
            )
            .is_err());
            assert_eq!(row, original);
            let current = active(store.connection(), "/fixture", "session")
                .unwrap()
                .unwrap();
            assert_eq!(current.state, "building");
            assert_eq!(current.revision, 0);
            for table in ["facts", "decisions", "transitions", "stop_guard"] {
                let count: i64 = store
                    .connection()
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                    .unwrap();
                assert_eq!(count, 0, "rollback {table}");
            }
            store
                .connection()
                .execute_batch("DROP TRIGGER reject_write")
                .unwrap();
            let decision = persist_stop(
                &store,
                "/fixture",
                Some(&expected),
                "a",
                cap,
                &mut row,
                4096,
                &serde_json::json!({}),
            )
            .unwrap();
            assert_eq!(decision.stalled, cap == 0);
            assert_eq!(decision.block, cap != 0);
            assert_eq!(row.outcome, if cap == 0 { "allow" } else { "deny" });
            let saved: String = store
                .connection()
                .query_row("SELECT outcome FROM decisions", [], |r| r.get(0))
                .unwrap();
            assert_eq!(saved, row.outcome);
        }
    }

    #[test]
    fn existing_denial_never_advances_guard_and_observe_only_records_hypothetical() {
        let (dir, store) = fixture();
        let project = dir.path().canonicalize().unwrap();
        let key = project.to_str().unwrap();
        store
            .connection()
            .execute("UPDATE plans SET project=?1 WHERE id=1", [key])
            .unwrap();
        store
            .connection()
            .execute("UPDATE sessions SET project=?1 WHERE id='session'", [key])
            .unwrap();
        for (mode, outcome) in [("enforce", "deny"), ("observe", "would_deny")] {
            let mut row = row();
            row.project = key.into();
            row.mode = mode.into();
            row.outcome = outcome.into();
            row.reason = Some("user gate denial".into());
            let decision =
                record_stop(&store, &project, 0, &mut row, 4096, &serde_json::json!({})).unwrap();
            assert!(!decision.stalled && !decision.block);
            assert_eq!(row.outcome, outcome);
            assert_eq!(row.reason.as_deref(), Some("user gate denial"));
        }
        let expected = active(store.connection(), key, "session").unwrap().unwrap();
        assert_eq!(expected.state, "building");
        let mut observed = row();
        observed.project = key.into();
        observed.mode = "observe".into();
        let decision = persist_stop(
            &store,
            key,
            Some(&expected),
            "a",
            2,
            &mut observed,
            4096,
            &serde_json::json!({}),
        )
        .unwrap();
        assert!(decision.block && !decision.stalled);
        assert_eq!(observed.outcome, "would_deny");
        let counters: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM stop_guard", [], |r| r.get(0))
            .unwrap();
        assert_eq!(counters, 0);
        assert_eq!(
            active(store.connection(), key, "session")
                .unwrap()
                .unwrap()
                .state,
            "building"
        );
    }

    fn fixture() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("state.db")).unwrap();
        store.connection().execute_batch("INSERT INTO plans(id,project,slug,file,state,revision,created_at,updated_at) VALUES(1,'/fixture','demo','plan.md','building',0,10,10); INSERT INTO sessions(id,agent,project,started_at,active_plan) VALUES('session','codex','/fixture',10,1);").unwrap();
        (dir, store)
    }

    fn stop(store: &Store, fingerprint: &str, enforce: bool) -> Result<StopDecision> {
        let current = active(store.connection(), "/fixture", "session")?.unwrap();
        tick(
            store,
            "/fixture",
            "session",
            &current,
            fingerprint,
            2,
            10,
            enforce,
        )
    }

    #[test]
    fn check_hint_uses_validated_recorded_agent() {
        let (_dir, store) = fixture();
        for agent in ["claude", "codex"] {
            store
                .connection()
                .execute("UPDATE sessions SET agent=?1 WHERE id='session'", [agent])
                .unwrap();
            let decision = stop(&store, "a", false).unwrap();
            assert!(decision
                .reason
                .unwrap()
                .contains(&format!("`yoke plan check demo --agent {agent}`")));
        }
        store
            .connection()
            .execute("UPDATE sessions SET agent='unknown' WHERE id='session'", [])
            .unwrap();
        assert!(stop(&store, "a", true).is_err());
        let rows: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM stop_guard", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[test]
    fn counters_persist_observe_is_readonly_and_cap_stalls_atomically() {
        let (dir, store) = fixture();
        assert!(stop(&store, "a", false).unwrap().block);
        let rows: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM stop_guard", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 0);
        assert!(stop(&store, "a", true).unwrap().block);
        drop(store);
        let store = Store::open(dir.path().join("state.db")).unwrap();
        assert!(stop(&store, "a", true).unwrap().block);
        let observe = stop(&store, "a", false).unwrap();
        assert!(!observe.block);
        assert!(!observe.stalled);
        assert_eq!(
            active(store.connection(), "/fixture", "session")
                .unwrap()
                .unwrap()
                .state,
            "building"
        );
        store
            .connection()
            .execute("UPDATE plans SET slug='ghp_fixtureproof' WHERE id=1", [])
            .unwrap();
        store.connection().execute_batch("CREATE TEMP TRIGGER stop_failure BEFORE INSERT ON facts WHEN NEW.kind='plan_transition' BEGIN SELECT RAISE(ABORT,'fixture failure'); END;").unwrap();
        assert!(stop(&store, "a", true).is_err());
        assert_eq!(
            active(store.connection(), "/fixture", "session")
                .unwrap()
                .unwrap()
                .state,
            "building"
        );
        let transitions: i64 = store
            .connection()
            .query_row(
                "SELECT count(*) FROM transitions WHERE plan_id=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(transitions, 0);
        store
            .connection()
            .execute_batch("DROP TRIGGER stop_failure")
            .unwrap();
        let decision = stop(&store, "a", true).unwrap();
        assert!(!decision.block);
        assert!(decision.stalled);
        let state = active(store.connection(), "/fixture", "session")
            .unwrap()
            .unwrap();
        assert_eq!(state.state, "stalled");
        assert_eq!(state.revision, 1);
        let fact: (i64, String, String, String, String) = store
            .connection()
            .query_row(
                "SELECT ts,session_id,project,kind,detail FROM facts",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            (fact.0, fact.1.as_str(), fact.2.as_str(), fact.3.as_str()),
            (10, "session", "/fixture", "plan_transition")
        );
        assert!(fact.4.contains("[REDACTED]"));
        assert!(!fact.4.contains("ghp_fixtureproof"));
        assert!(fact.4.contains("building"));
        assert!(fact.4.contains("stalled"));
    }

    #[test]
    fn only_fingerprint_or_real_history_ids_reset_progress_even_at_same_timestamp() {
        let (_dir, store) = fixture();
        assert!(stop(&store, "a", true).unwrap().block);
        assert!(stop(&store, "a", true).unwrap().block);
        assert!(stop(&store, "b", true).unwrap().block);
        assert!(stop(&store, "b", true).unwrap().block);
        store.connection().execute_batch("INSERT INTO transitions(plan_id,ts,from_state,to_state,actor) VALUES(1,10,'waiting','building','human');").unwrap();
        assert!(stop(&store, "b", true).unwrap().block);
        assert!(stop(&store, "b", true).unwrap().block);
        store.connection().execute_batch("INSERT INTO check_runs(plan_id,ts,name,command,output_tail,fingerprint,blocking,ms,result) VALUES(1,10,'test','true','','b',1,1,'pass');").unwrap();
        assert!(stop(&store, "b", true).unwrap().block);
        assert!(stop(&store, "b", true).unwrap().block);
        assert!(stop(&store, "b", true).unwrap().stalled);
    }

    #[test]
    fn inactive_plans_allow_and_active_fingerprint_errors_propagate() {
        let (dir, store) = fixture();
        let project = dir.path().canonicalize().unwrap();
        let key = project.to_str().unwrap();
        store
            .connection()
            .execute(
                "UPDATE plans SET project=?1,state='waiting' WHERE id=1",
                [key],
            )
            .unwrap();
        store
            .connection()
            .execute("UPDATE sessions SET project=?1 WHERE id='session'", [key])
            .unwrap();
        assert!(
            !evaluate(&store, &project, "session", 2, 10, true)
                .unwrap()
                .block
        );
        store
            .connection()
            .execute("UPDATE plans SET state='building' WHERE id=1", [])
            .unwrap();
        assert!(evaluate(&store, &project, "session", 2, 10, true).is_err());
        assert!(evaluate(&store, &project, "missing", 2, 10, true)
            .unwrap()
            .reason
            .is_none());
    }
}
