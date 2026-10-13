//! Local memories, observed facts, and portable migration without trust transfer.

use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Component, Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    redact::redact,
    store::{DecisionRecord, Store},
};

mod digest;
mod portable;
pub use digest::{digest_due, digest_with_command};
pub use portable::{export_archive, import_archive, ExportReport, ImportReport};

pub(crate) const TEXT_LIMIT: usize = 65_536;
pub(crate) const MEMORY_COLUMNS: &str = "id,identity,ts,project,source,text,pinned,shared";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Memory {
    pub id: i64,
    pub identity: String,
    pub ts: i64,
    pub project: String,
    pub source: String,
    pub text: String,
    pub pinned: bool,
    pub shared: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Fact {
    pub id: i64,
    pub identity: String,
    pub ts: i64,
    pub session_id: String,
    pub project: String,
    pub kind: String,
    pub detail: String,
    pub digested: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemorySnapshot {
    pub memory: Memory,
    pub fact_ids: Vec<i64>,
    pub imported: bool,
}

pub fn remember(
    store: &Store,
    project: &str,
    text: &str,
    pinned: bool,
    now: i64,
) -> Result<Memory> {
    validate_project(project)?;
    validate_text(text)?;
    store.transaction(|tx| {
        tx.execute(
            "INSERT INTO memories(ts,project,source,text,pinned) VALUES (?1,?2,'remember',?3,?4)",
            params![now, project, redact(text), pinned],
        )?;
        get_on(tx, tx.last_insert_rowid())?.context("memory missing after insert")
    })
}

pub fn get(store: &Store, id: i64) -> Result<Option<Memory>> {
    get_on(store.connection(), id)
}

pub fn recall(store: &Store, project: &str, query: &str, limit: usize) -> Result<Vec<Memory>> {
    let limit = limit.min(10_000) as i64;
    if query.trim().is_empty() {
        // Exponential recency decay is strictly monotone in ts, so this has identical ordering.
        let mut statement = store.connection().prepare(&format!(
            "SELECT {MEMORY_COLUMNS} FROM memories WHERE project=?1 ORDER BY pinned DESC,ts DESC,id DESC LIMIT ?2"))?;
        return Ok(statement
            .query_map(params![project, limit], memory_row)?
            .collect::<rusqlite::Result<_>>()?);
    }
    if query.len() > 4096 {
        bail!("recall query exceeds 4096 bytes");
    }
    // Quote each literal term; user text cannot inject FTS operators or invalid syntax.
    let query = query
        .split_whitespace()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ");
    let mut statement = store.connection().prepare(
        "SELECT m.id,m.identity,m.ts,m.project,m.source,m.text,m.pinned,m.shared
         FROM memories m JOIN memories_fts ON m.id=memories_fts.rowid
         WHERE m.project=?1 AND memories_fts MATCH ?2
         ORDER BY m.pinned DESC,bm25(memories_fts),m.ts DESC,m.id DESC LIMIT ?3",
    )?;
    let rows = statement.query_map(params![project, query, limit], memory_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn pin(store: &Store, id: i64, pinned: bool) -> Result<()> {
    if store.connection().execute(
        "UPDATE memories SET pinned=?2 WHERE id=?1",
        params![id, pinned],
    )? == 0
    {
        bail!("memory {id} not found");
    }
    Ok(())
}

pub fn forget(store: &Store, id: i64) -> Result<bool> {
    Ok(forget_with_snapshot(store, id)?.is_some())
}

pub fn forget_with_snapshot(store: &Store, id: i64) -> Result<Option<MemorySnapshot>> {
    store.transaction(|tx| {
        let Some(memory) = get_on(tx, id)? else {
            return Ok(None);
        };
        let fact_ids = tx
            .prepare("SELECT fact_id FROM memory_facts WHERE memory_id=?1 ORDER BY fact_id")?
            .query_map([id], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        let imported = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM imported_identities WHERE identity=?1 AND kind='memory')",
            [&memory.identity],
            |row| row.get(0),
        )?;
        tx.execute("DELETE FROM memories WHERE id=?1", [id])?;
        Ok(Some(MemorySnapshot {
            memory,
            fact_ids,
            imported,
        }))
    })
}

/// Restore identity and provenance, allocating a new local ID if the old ID was reused.
pub fn restore_snapshot(store: &Store, snapshot: &MemorySnapshot) -> Result<()> {
    let memory = &snapshot.memory;
    validate_project(&memory.project)?;
    validate_text(&memory.text)?;
    store.transaction(|tx| {
        for id in &snapshot.fact_ids {
            let matches: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM facts WHERE id=?1 AND project=?2)", params![id,memory.project], |row| row.get(0))?;
            if !matches { bail!("cannot restore missing or mismatched fact {id}"); }
        }
        let occupied: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM memories WHERE id=?1)", [memory.id], |row| row.get(0))?;
        let preferred_id = (!occupied).then_some(memory.id);
        tx.execute("INSERT INTO memories(id,identity,ts,project,source,text,pinned,shared) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![preferred_id,memory.identity,memory.ts,memory.project,memory.source,redact(&memory.text),memory.pinned,memory.shared])?;
        let restored_id = tx.last_insert_rowid();
        for fact_id in &snapshot.fact_ids {
            tx.execute("INSERT INTO memory_facts(memory_id,fact_id) VALUES (?1,?2)", params![restored_id,fact_id])?;
        }
        if snapshot.imported {
            tx.execute("INSERT INTO imported_identities(identity,kind,local_id) VALUES (?1,'memory',?2)", params![memory.identity,restored_id])?;
        }
        Ok(())
    })
}

pub fn record_fact(
    store: &Store,
    session: &str,
    project: &str,
    kind: &str,
    detail: &str,
    now: i64,
) -> Result<i64> {
    validate_project(project)?;
    validate_text(detail)?;
    if session.len() > 4096 || kind.is_empty() || kind.len() > 128 {
        bail!("invalid fact session or kind");
    }
    store.connection().execute(
        "INSERT INTO facts(ts,session_id,project,kind,detail) VALUES (?1,?2,?3,?4,?5)",
        params![now, redact(session), project, redact(kind), redact(detail)],
    )?;
    Ok(store.connection().last_insert_rowid())
}

/// Record only explicit observed evidence; absent exit status means unknown.
pub fn record_hook_facts(store: &Store, row: &DecisionRecord, raw: &Value) -> Result<usize> {
    store.transaction(|tx| record_hook_facts_in(tx, row, raw))
}

pub(crate) fn record_hook_facts_in(
    tx: &Transaction<'_>,
    row: &DecisionRecord,
    raw: &Value,
) -> Result<usize> {
    let mut facts = Vec::new();
    if matches!(row.outcome.as_str(), "deny" | "would_deny") {
        facts.push((
            "denial",
            row.reason.clone().unwrap_or_else(|| "Action denied".into()),
        ));
    }
    if row.event == "post_tool_use" {
        if matches!(
            row.tool.as_deref(),
            Some("Edit" | "Write" | "MultiEdit" | "apply_patch")
        ) {
            facts.push((
                "file_write",
                format!(
                    "{}: {}",
                    row.tool.as_deref().unwrap_or("edit"),
                    raw.get("tool_input").unwrap_or(&Value::Null)
                ),
            ));
        }
        if raw
            .pointer("/tool_response/exit_code")
            .and_then(Value::as_i64)
            .is_some_and(|code| code != 0)
        {
            facts.push((
                "command_failure",
                raw.get("tool_response").unwrap_or(&Value::Null).to_string(),
            ));
        }
    }
    validate_project(&row.project)?;
    for (kind, detail) in &facts {
        let detail = crate::redact::redact_payload(detail, TEXT_LIMIT);
        tx.execute(
            "INSERT INTO facts(ts,session_id,project,kind,detail) VALUES (?1,?2,?3,?4,?5)",
            params![row.ts, redact(&row.session_id), row.project, kind, detail],
        )?;
    }
    Ok(facts.len())
}

pub fn context(
    store: &Store,
    project: &str,
    query: &str,
    trusted_knowledge: &str,
    token_budget: usize,
) -> Result<String> {
    let budget = token_budget.saturating_mul(4).min(1_000_000);
    if budget == 0 {
        return Ok(String::new());
    }
    let mut output = String::new();
    let mut statement = store.connection().prepare("SELECT substr(text,1,?2) FROM memories WHERE project=?1 AND pinned=1 ORDER BY ts DESC,id DESC LIMIT ?3")?;
    let mut rows = statement.query(params![project, budget as i64, budget.min(1000) as i64])?;
    while output.chars().count() < budget {
        let Some(row) = rows.next()? else { break };
        append_context(&mut output, &row.get::<_, String>(0)?, budget);
    }
    append_context(&mut output, &redact(trusted_knowledge), budget);
    let remaining = budget.saturating_sub(output.chars().count());
    if remaining > 0 {
        if query.len() > 4096 {
            bail!("recall query exceeds 4096 bytes");
        }
        let fts = query
            .split_whitespace()
            .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" AND ");
        let sql = if fts.is_empty() {
            "SELECT substr(text,1,?2) FROM memories WHERE project=?1 AND pinned=0 AND ?4='' ORDER BY ts DESC,id DESC LIMIT ?3"
        } else {
            "SELECT substr(m.text,1,?2) FROM memories m JOIN memories_fts ON m.id=memories_fts.rowid WHERE m.project=?1 AND m.pinned=0 AND memories_fts MATCH ?4 ORDER BY bm25(memories_fts),m.ts DESC LIMIT ?3"
        };
        let mut statement = store.connection().prepare(sql)?;
        let mut rows = statement.query(params![
            project,
            remaining as i64,
            remaining.min(1000) as i64,
            fts
        ])?;
        while output.chars().count() < budget {
            let Some(row) = rows.next()? else { break };
            append_context(&mut output, &row.get::<_, String>(0)?, budget);
        }
    }
    Ok(output)
}

fn append_context(output: &mut String, text: &str, budget: usize) {
    if text.is_empty() {
        return;
    }
    let used = output.chars().count();
    if used >= budget {
        return;
    }
    if !output.is_empty() {
        output.push('\n');
    }
    let remaining = budget.saturating_sub(output.chars().count());
    output.extend(text.chars().take(remaining));
}

/// Append a dated entry to this memory's project knowledge; it still needs human trust.
pub fn share(store: &Store, id: i64) -> Result<PathBuf> {
    store.transaction(|tx| {
        let memory = get_on(tx, id)?.with_context(|| format!("memory {id} not found"))?;
        validate_project(&memory.project)?;
        let root = fs::canonicalize(&memory.project)?;
        let directory = root.join(".yoke");
        if directory
            .symlink_metadata()
            .is_ok_and(|meta| meta.file_type().is_symlink())
        {
            bail!("knowledge directory must not be a symlink");
        }
        fs::create_dir_all(&directory)?;
        let path = directory.join("knowledge.md");
        if path
            .symlink_metadata()
            .is_ok_and(|meta| meta.file_type().is_symlink())
        {
            bail!("knowledge file must not be a symlink");
        }
        let existing = match fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
        {
            Ok(file) => {
                if !file.metadata()?.is_file() || file.metadata()?.len() > 4 * 1024 * 1024 {
                    bail!("knowledge must be a regular file at most 4 MiB");
                }
                let mut text = String::new();
                file.take(4 * 1024 * 1024 + 1).read_to_string(&mut text)?;
                text
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error.into()),
        };
        if existing.len() > 4 * 1024 * 1024 {
            bail!("knowledge file exceeds 4 MiB");
        }
        let marker = format!("<!-- yoke-memory:{} -->", memory.identity);
        if !existing.contains(&marker) {
            let mut temporary = tempfile::NamedTempFile::new_in(&directory)?;
            write!(
                temporary,
                "{existing}\n{marker}\n- [{}] {}\n",
                memory.ts, memory.text
            )?;
            temporary.as_file().sync_all()?;
            temporary.persist(&path).map_err(|error| error.error)?;
        }
        // The marker makes a retry idempotent if file persistence succeeds but SQLite commit fails.
        tx.execute("UPDATE memories SET shared=1 WHERE id=?1", [id])?;
        Ok(path)
    })
}

pub(crate) fn validate_project(project: &str) -> Result<()> {
    if project.len() > 4096
        || project.contains('\0')
        || !Path::new(project).is_absolute()
        || Path::new(project)
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        bail!("project must be an explicit absolute path without parent traversal");
    }
    Ok(())
}

pub(crate) fn validate_text(text: &str) -> Result<()> {
    if text.trim().is_empty() || text.len() > TEXT_LIMIT || text.contains('\0') {
        bail!("memory text must be nonempty UTF-8 without NUL, at most {TEXT_LIMIT} bytes");
    }
    Ok(())
}

pub(crate) fn get_on(connection: &Connection, id: i64) -> Result<Option<Memory>> {
    Ok(connection
        .query_row(
            &format!("SELECT {MEMORY_COLUMNS} FROM memories WHERE id=?1"),
            [id],
            memory_row,
        )
        .optional()?)
}

pub(crate) fn memory_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Memory> {
    Ok(Memory {
        id: row.get(0)?,
        identity: row.get(1)?,
        ts: row.get(2)?,
        project: row.get(3)?,
        source: row.get(4)?,
        text: row.get(5)?,
        pinned: row.get(6)?,
        shared: row.get(7)?,
    })
}

pub(crate) fn fact_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Fact> {
    Ok(Fact {
        id: row.get(0)?,
        identity: row.get(1)?,
        ts: row.get(2)?,
        session_id: row.get(3)?,
        project: row.get(4)?,
        kind: row.get(5)?,
        detail: row.get(6)?,
        digested: row.get(7)?,
    })
}

#[cfg(test)]
#[path = "memory/tests.rs"]
mod tests;
