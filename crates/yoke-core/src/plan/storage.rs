use std::path::Path;

use anyhow::{ensure, Context, Result};
use rusqlite::{params, OptionalExtension, Row, Transaction};

use super::{now, CheckRun, PlanRecord, TransitionRecord};
use crate::redact::redact_payload;
use crate::store::Store;

const COLUMNS: &str = "id,project,slug,file,state,approved_header,revision,created_at,updated_at";

fn row(row: &Row<'_>) -> rusqlite::Result<PlanRecord> {
    Ok(PlanRecord {
        id: row.get(0)?,
        project: row.get(1)?,
        slug: row.get(2)?,
        file: row.get(3)?,
        state: row.get(4)?,
        approved_header: row.get(5)?,
        revision: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

pub(super) fn find(store: &Store, project: &Path, slug: &str) -> Result<Option<PlanRecord>> {
    Ok(store
        .connection()
        .query_row(
            &format!("SELECT {COLUMNS} FROM plans WHERE project=?1 AND slug=?2"),
            params![
                project.to_str().context("project path must be UTF-8")?,
                slug
            ],
            row,
        )
        .optional()?)
}

pub(super) fn list(store: &Store, project: &Path) -> Result<Vec<PlanRecord>> {
    let mut statement = store.connection().prepare(&format!(
        "SELECT {COLUMNS} FROM plans WHERE project=?1 ORDER BY id DESC"
    ))?;
    let records = statement.query_map(
        [project.to_str().context("project path must be UTF-8")?],
        row,
    )?;
    Ok(records.collect::<rusqlite::Result<_>>()?)
}

pub(super) fn insert(store: &Store, project: &Path, slug: &str, file: &Path) -> Result<PlanRecord> {
    let ts = now()?;
    store.transaction(|tx| {
        tx.execute("INSERT INTO plans(project,slug,file,state,created_at,updated_at) VALUES(?1,?2,?3,'draft',?4,?4)", params![project.to_str(), slug, file.to_str(), ts])?;
        let id = tx.last_insert_rowid();
        tx.execute("INSERT INTO transitions(plan_id,ts,from_state,to_state,actor,note) VALUES(?1,?2,'','draft','agent',NULL)", params![id,ts])?;
        let plan = by_id(tx, id)?;
        fact(tx, &plan, ts, "plan_transition", &serde_json::json!({"plan": slug, "from": "", "to": "draft", "actor": "agent"}).to_string())?;
        Ok(plan)
    })
}

pub(super) fn active(store: &Store, project: &Path, session: &str) -> Result<Option<PlanRecord>> {
    let id: Option<i64> = store
        .connection()
        .query_row(
            "SELECT active_plan FROM sessions WHERE id=?1 AND project=?2",
            params![session, project.to_str()],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    match id {
        Some(id) => Ok(store
            .connection()
            .query_row(
                &format!("SELECT {COLUMNS} FROM plans WHERE id=?1 AND project=?2"),
                params![id, project.to_str()],
                row,
            )
            .optional()?),
        None => Ok(None),
    }
}

fn by_id(tx: &Transaction<'_>, id: i64) -> Result<PlanRecord> {
    Ok(tx.query_row(
        &format!("SELECT {COLUMNS} FROM plans WHERE id=?1"),
        [id],
        row,
    )?)
}

pub(super) fn transition(
    store: &Store,
    plan: &PlanRecord,
    state: &str,
    actor: &str,
    note: Option<&str>,
    header: Option<&str>,
    session: Option<&str>,
) -> Result<PlanRecord> {
    store.transaction(|tx| {
        if let Some(session) = session {
            let active: Option<Option<i64>> = tx
                .query_row(
                    "SELECT active_plan FROM sessions WHERE id=?1 AND project=?2",
                    params![session, plan.project],
                    |row| row.get(0),
                )
                .optional()?;
            let active = active.context("session not found for this project; run a hook first")?;
            if let Some(id) = active.filter(|id| *id != plan.id) {
                let previous = by_id(tx, id)?;
                ensure!(
                    previous.state == "done",
                    "session already has unfinished plan {}",
                    previous.slug
                );
            }
            tx.execute(
                "UPDATE sessions SET active_plan=?1 WHERE id=?2 AND project=?3",
                params![plan.id, session, plan.project],
            )?;
        }
        update(tx, plan, state, actor, note, header)?;
        by_id(tx, plan.id)
    })
}

fn update(
    tx: &Transaction<'_>,
    plan: &PlanRecord,
    state: &str,
    actor: &str,
    note: Option<&str>,
    header: Option<&str>,
) -> Result<()> {
    let ts = now()?;
    let changed = tx.execute("UPDATE plans SET state=?1,approved_header=COALESCE(?2,approved_header),revision=revision+1,updated_at=?3 WHERE id=?4 AND revision=?5 AND state=?6", params![state,header,ts,plan.id,plan.revision,plan.state])?;
    ensure!(
        changed == 1,
        "plan changed concurrently; reload before retrying"
    );
    tx.execute("INSERT INTO transitions(plan_id,ts,from_state,to_state,actor,note) VALUES(?1,?2,?3,?4,?5,?6)", params![plan.id,ts,plan.state,state,actor,note.map(|note| redact_payload(note,4096))])?;
    fact(tx, plan, ts, "plan_transition", &serde_json::json!({"plan": plan.slug, "from": plan.state, "to": state, "actor": actor, "note": note}).to_string())?;
    Ok(())
}

pub(super) fn finish(
    store: &Store,
    plan: &PlanRecord,
    state: &str,
    runs: &mut [CheckRun],
    note: Option<&str>,
) -> Result<(PlanRecord, bool)> {
    store.transaction(|tx| {
        let current = by_id(tx, plan.id)?;
        if current.revision != plan.revision || current.state != plan.state {
            for run in runs.iter_mut() {
                run.result = "void".into();
                run.output_tail = format!(
                    "Evidence void: plan changed concurrently.\n{}",
                    run.output_tail
                );
                save_run(tx, run)?;
            }
            return Ok((current, false));
        }
        update(tx, plan, state, "yoke", note, None)?;
        for run in runs.iter_mut() {
            save_run(tx, run)?;
        }
        Ok((by_id(tx, plan.id)?, true))
    })
}

pub(super) fn save_review(store: &Store, run: &mut CheckRun) -> Result<()> {
    store.transaction(|tx| save_run(tx, run))
}

fn save_run(tx: &Transaction<'_>, run: &mut CheckRun) -> Result<()> {
    let mut findings = run.findings.clone();
    crate::redact::redact_value(&mut findings);
    run.output_tail = redact_payload(&run.output_tail, 4096);
    run.command = crate::redact::redact(&run.command);
    run.findings = findings;
    tx.execute("INSERT INTO check_runs(plan_id,ts,name,command,exit_code,output_tail,fingerprint,blocking,ms,result,findings,artifacts) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)", params![run.plan_id,run.ts,run.name,run.command,run.exit_code,run.output_tail,run.fingerprint,run.blocking,run.ms.min(i64::MAX as u64) as i64,run.result,run.findings.to_string(),serde_json::to_string(&run.artifacts)?])?;
    run.id = tx.last_insert_rowid();
    let plan = by_id(tx, run.plan_id)?;
    fact(tx, &plan, run.ts, "check_result", &serde_json::json!({"plan": plan.slug, "check": run.name, "result": run.result, "exit_code": run.exit_code, "blocking": run.blocking, "output": run.output_tail}).to_string())?;
    Ok(())
}

fn fact(tx: &Transaction<'_>, plan: &PlanRecord, ts: i64, kind: &str, detail: &str) -> Result<()> {
    let session: String = tx.query_row("SELECT id FROM sessions WHERE active_plan=?1 AND project=?2 ORDER BY started_at DESC,id LIMIT 1", params![plan.id,plan.project], |row| row.get(0)).optional()?.unwrap_or_default();
    tx.execute(
        "INSERT INTO facts(ts,session_id,project,kind,detail) VALUES(?1,?2,?3,?4,?5)",
        params![
            ts,
            session,
            plan.project,
            kind,
            redact_payload(detail, 4096)
        ],
    )?;
    Ok(())
}

pub(super) fn transitions(store: &Store, id: i64) -> Result<Vec<TransitionRecord>> {
    let mut statement = store.connection().prepare("SELECT id,plan_id,ts,from_state,to_state,actor,note FROM transitions WHERE plan_id=?1 ORDER BY id")?;
    let rows = statement.query_map([id], |row| {
        Ok(TransitionRecord {
            id: row.get(0)?,
            plan_id: row.get(1)?,
            ts: row.get(2)?,
            from_state: row.get(3)?,
            to_state: row.get(4)?,
            actor: row.get(5)?,
            note: row.get(6)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub(super) fn checks(store: &Store, id: i64) -> Result<Vec<CheckRun>> {
    let mut statement = store.connection().prepare("SELECT id,plan_id,ts,name,command,exit_code,output_tail,fingerprint,blocking,ms,result,findings,artifacts FROM check_runs WHERE plan_id=?1 ORDER BY id")?;
    let rows = statement.query_map([id], |row| {
        Ok((
            CheckRun {
                id: row.get(0)?,
                plan_id: row.get(1)?,
                ts: row.get(2)?,
                name: row.get(3)?,
                command: row.get(4)?,
                exit_code: row.get(5)?,
                output_tail: row.get(6)?,
                fingerprint: row.get(7)?,
                blocking: row.get(8)?,
                ms: row.get::<_, i64>(9)? as u64,
                result: row.get(10)?,
                findings: serde_json::Value::Null,
                artifacts: Vec::new(),
            },
            row.get::<_, String>(11)?,
            row.get::<_, String>(12)?,
        ))
    })?;
    rows.map(|row| {
        let (mut run, findings, artifacts) = row?;
        run.findings = serde_json::from_str(&findings).context("invalid saved review findings")?;
        run.artifacts =
            serde_json::from_str(&artifacts).context("invalid saved check artifacts")?;
        Ok(run)
    })
    .collect()
}
