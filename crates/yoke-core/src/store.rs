//! Core SQLite persistence. Log records are sanitized before any SQL write.

use std::{os::unix::ffi::OsStrExt, path::Path, time::Duration};

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::Serialize;
use serde_json::Value;

use crate::redact::{hook_payload, redact, redact_value};

const MIGRATIONS: &[&str] = &[
    include_str!("store/001-core.sql"),
    include_str!("store/002-memory.sql"),
    include_str!("store/003-workflow.sql"),
    include_str!("store/004-guard-usage.sql"),
    include_str!("store/005-read-models.sql"),
    include_str!("store/006-session-counts.sql"),
    include_str!("store/007-plan-base-ref.sql"),
    include_str!("store/008-plan-lifecycle.sql"),
    include_str!("store/009-plan-acceptance.sql"),
    include_str!("store/010-jobs-recall.sql"),
    include_str!("store/011-notifications.sql"),
    include_str!("store/012-usage-time.sql"),
    include_str!("store/013-scale-indexes.sql"),
    include_str!("store/014-session-anchors.sql"),
];

mod read;
pub use read::{DecisionRow, ProjectSummary, SessionCounts, SessionSummary, StatusSnapshot};

pub struct Store {
    connection: Connection,
}

/// A database in WAL mode that is read without writing needs its `-wal` and `-shm` files.
/// Another SQLite client's last close deletes them. SQLite would then fail with error 14 and
/// leave an empty `-wal` behind, so `Store::open_read_only` checks first and creates nothing.
#[derive(Debug)]
pub struct MissingSidecars {
    pub missing: Vec<String>,
}

impl std::fmt::Display for MissingSidecars {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the state database is missing {}; another SQLite client deletes these files when it closes the database last. Run `yoke log --limit 1` once to recreate them, then retry",
            self.missing.join(" and ")
        )
    }
}

impl std::error::Error for MissingSidecars {}

fn check_sidecars(path: &Path) -> Result<()> {
    use std::io::Read;
    let mut header = [0u8; 20];
    // A file too short for a header, or unreadable, is not a WAL database; SQLite reports it.
    let Ok(mut file) = std::fs::File::open(path) else {
        return Ok(());
    };
    // Byte 19 is the read format version: 2 means WAL.
    if file.read_exact(&mut header).is_err() || header[19] != 2 {
        return Ok(());
    }
    let mut missing = Vec::new();
    for suffix in ["-wal", "-shm"] {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        let sidecar = std::path::PathBuf::from(name);
        if !sidecar.exists() {
            missing.extend(
                sidecar
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned()),
            );
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(MissingSidecars { missing }.into())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustRow {
    pub scope: String,
    pub root: String,
    pub file: String,
    pub sha256: String,
    pub content: String,
    pub trusted_at: i64,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct DecisionRecord {
    pub ts: i64,
    pub session_id: String,
    pub agent: String,
    pub project: String,
    pub event: String,
    pub tool: Option<String>,
    pub mode: String,
    pub outcome: String,
    pub reason: Option<String>,
    pub gates: Value,
    pub payload: String,
}

#[derive(Clone, Debug)]
pub struct DecisionFilter {
    pub project: Option<String>,
    pub session: Option<String>,
    pub outcome: Option<String>,
    pub tool: Option<String>,
    pub limit: usize,
}

impl Default for DecisionFilter {
    fn default() -> Self {
        Self {
            project: None,
            session: None,
            outcome: None,
            tool: None,
            limit: 100,
        }
    }
}

impl Store {
    /// In-crate domain queries must bind inputs and leave migrations to this module.
    pub(crate) fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Commit a complete domain operation or roll back all its writes on error.
    pub(crate) fn transaction<T>(
        &self,
        f: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let result = f(&transaction)?;
        transaction.commit()?;
        Ok(result)
    }

    /// Open existing state without creating files, running migrations, or writing pragmas.
    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self> {
        let path = std::fs::canonicalize(path)?;
        ensure_regular_file(&path)?;
        check_sidecars(&path)?;
        let encoded = path
            .as_os_str()
            .as_bytes()
            .iter()
            .map(|byte| {
                if byte.is_ascii_alphanumeric() || b"/-._~".contains(byte) {
                    (*byte as char).to_string()
                } else {
                    format!("%{byte:02X}")
                }
            })
            .collect::<String>();
        // The bundled SQLite unix VFS honors readonly_shm: never create or write its WAL index.
        // Missing sidecars fail rather than returning an immutable snapshot that ignores live WAL.
        let connection = Connection::open_with_flags(
            format!("file:{encoded}?mode=ro&readonly_shm=1"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )?;
        connection.busy_timeout(Duration::from_millis(5))?;
        let version: usize =
            connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > MIGRATIONS.len() {
            bail!(
                "state database schema {version} is newer than supported {}",
                MIGRATIONS.len()
            );
        }
        Ok(Self { connection })
    }

    /// Open an existing parent directory's database and apply numbered migrations.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if path != Path::new(":memory:") {
            ensure_regular_file(path).context("open state database")?;
            if !path.as_os_str().is_empty() {
                crate::private::prepare_database(path);
            }
        }
        let mut connection = Connection::open(path).context("open state database")?;
        connection.busy_timeout(Duration::from_secs(2))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        // The sidecars persist (below), so without a size limit the last connection to close
        // never truncates the WAL: each hook process rebuilds the WAL index from the whole
        // file on open and appends after it, so the file and the open cost grow until an
        // automatic checkpoint. With a limit the closing checkpoint leaves an empty WAL.
        connection.pragma_update(None, "journal_size_limit", WAL_SIZE_LIMIT)?;
        // Table rebuilds in migrations must not cascade into child rows; enforcement is
        // restored once the migrated schema passes a foreign key check.
        connection.pragma_update(None, "foreign_keys", false)?;
        if path != Path::new(":memory:") && !path.as_os_str().is_empty() {
            let mut persist: std::ffi::c_int = 1;
            // SAFETY: the live connection owns the handle; SQLite reads this int synchronously.
            let code = unsafe {
                rusqlite::ffi::sqlite3_file_control(
                    connection.handle(),
                    c"main".as_ptr(),
                    rusqlite::ffi::SQLITE_FCNTL_PERSIST_WAL,
                    (&mut persist as *mut std::ffi::c_int).cast(),
                )
            };
            if code != rusqlite::ffi::SQLITE_OK {
                bail!("cannot preserve WAL sidecars for no-write readers (SQLite {code})");
            }
        }
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: usize =
            transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > MIGRATIONS.len() {
            bail!(
                "state database schema {version} is newer than supported {}",
                MIGRATIONS.len()
            );
        }
        let pending = version < MIGRATIONS.len();
        // Only violations a migration introduced refuse the upgrade, so older damage can
        // never lock every hook out of a database it could still use.
        let before = if pending {
            foreign_key_violations(&transaction)?
        } else {
            0
        };
        for (index, sql) in MIGRATIONS.iter().enumerate().skip(version) {
            transaction.execute_batch(sql)?;
            transaction.pragma_update(None, "user_version", index + 1)?;
        }
        if pending && foreign_key_violations(&transaction)? > before {
            bail!("migration left foreign key violations; state database was not changed");
        }
        transaction.commit()?;
        connection.pragma_update(None, "foreign_keys", true)?;
        if path != Path::new(":memory:") && !path.as_os_str().is_empty() {
            // The sidecars exist now; SQLite gives them the mode of the database.
            crate::private::tighten_database(path);
        }
        Ok(Self { connection })
    }

    /// Store an exact approved snapshot; executable trust content is never redacted.
    pub fn put_trust(&self, row: &TrustRow) -> Result<()> {
        self.put_trust_batch(std::slice::from_ref(row))
    }

    /// Commit config and knowledge snapshots atomically after approval.
    pub fn put_trust_batch(&self, rows: &[TrustRow]) -> Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        for row in rows {
            transaction.execute(
                "INSERT INTO trust(scope,root,file,sha256,content,trusted_at) VALUES (?1,?2,?3,?4,?5,?6)
                 ON CONFLICT(scope,root,file) DO UPDATE SET sha256=excluded.sha256,content=excluded.content,trusted_at=excluded.trusted_at",
                params![row.scope, row.root, row.file, row.sha256, row.content, row.trusted_at],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn get_trust(&self, scope: &str, root: &str, file: &str) -> Result<Option<TrustRow>> {
        Ok(self.connection.query_row(
            "SELECT scope,root,file,sha256,content,trusted_at FROM trust WHERE scope=?1 AND root=?2 AND file=?3",
            params![scope, root, file], trust_row,
        ).optional()?)
    }

    pub fn list_trust(&self, scope: &str, root: &str) -> Result<Vec<TrustRow>> {
        let mut statement = self.connection.prepare(
            "SELECT scope,root,file,sha256,content,trusted_at FROM trust WHERE scope=?1 AND root=?2 ORDER BY file",
        )?;
        let rows = statement.query_map(params![scope, root], trust_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Atomically persist a redacted decision and create or end its session.
    pub fn record_decision(&self, row: &DecisionRecord, payload_cap: usize) -> Result<()> {
        self.transaction(|tx| Self::record_decision_in(tx, row, payload_cap))
    }

    pub(crate) fn record_decision_in(
        transaction: &Transaction<'_>,
        row: &DecisionRecord,
        payload_cap: usize,
    ) -> Result<()> {
        let session = redact(&row.session_id);
        let agent = redact(&row.agent);
        let project = redact(&row.project);
        let mut gates = row.gates.clone();
        redact_value(&mut gates);
        transaction.execute(
            "INSERT INTO sessions(id,agent,project,started_at) VALUES (?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET project=excluded.project",
            params![session, agent, project, row.ts],
        )?;
        transaction.execute(
            "INSERT INTO decisions(ts,session_id,agent,project,event,tool,mode,outcome,reason,gates,payload)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![row.ts, session, agent, project, redact(&row.event), row.tool.as_deref().map(redact),
                row.mode, row.outcome, row.reason.as_deref().map(redact), gates.to_string(), hook_payload(&row.payload, payload_cap)],
        )?;
        if row.event == "session_end" {
            transaction.execute(
                "UPDATE sessions SET ended_at=?2 WHERE id=?1",
                params![session, row.ts],
            )?;
        }
        Ok(())
    }

    pub fn decisions(&self, filter: &DecisionFilter) -> Result<Vec<DecisionRecord>> {
        // Only the filters that were given reach SQL. A term like `(?1 IS NULL OR project=?1)`
        // hides the column from the planner, so every filtered read scanned the whole table.
        let mut terms = Vec::new();
        let mut values: Vec<rusqlite::types::Value> = Vec::new();
        for (column, value) in [
            ("project", filter.project.as_deref().map(redact)),
            ("session_id", filter.session.as_deref().map(redact)),
            ("outcome", filter.outcome.clone()),
            ("tool", filter.tool.as_deref().map(redact)),
        ] {
            if let Some(value) = value {
                values.push(value.into());
                terms.push(format!("{column}=?{}", values.len()));
            }
        }
        let scope = if terms.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", terms.join(" AND "))
        };
        values.push((filter.limit.min(10_000) as i64).into());
        let mut statement = self.connection.prepare(&format!(
            "SELECT ts,session_id,agent,project,event,tool,mode,outcome,reason,gates,payload FROM decisions{scope}
             ORDER BY ts DESC,id DESC LIMIT ?{}",
            values.len()
        ))?;
        let rows = statement.query_map(rusqlite::params_from_iter(values), |row| {
            let gates: String = row.get(9)?;
            Ok((
                DecisionRecord {
                    ts: row.get(0)?,
                    session_id: row.get(1)?,
                    agent: row.get(2)?,
                    project: row.get(3)?,
                    event: row.get(4)?,
                    tool: row.get(5)?,
                    mode: row.get(6)?,
                    outcome: row.get(7)?,
                    reason: row.get(8)?,
                    gates: Value::Null,
                    payload: row.get(10)?,
                },
                gates,
            ))
        })?;
        rows.map(|row| {
            let (mut record, gates) = row?;
            record.gates = serde_json::from_str(&gates).context("invalid decision gates JSON")?;
            Ok(record)
        })
        .collect()
    }

    /// One bounded retention pass. Decisions, facts and the check runs of done or cancelled
    /// plans older than the boundary are deleted in short transactions until the budget is
    /// spent, so a large backlog drains over several passes without holding the write lock.
    /// Trusted policy, memories and the check runs of unfinished plans are never deleted here.
    pub fn prune_history(
        &self,
        now: i64,
        retention_days: u32,
        budget: Duration,
    ) -> Result<PruneReport> {
        self.prune_history_paced(now, retention_days, budget, Duration::ZERO)
    }

    /// `prune_history` for a background worker: sleeps `pause` between batches. SQLite's busy
    /// handler retries at most every 100 ms, so a longer pause lets a waiting hook take the write
    /// lock instead of losing every race to back-to-back batches.
    pub fn prune_history_paced(
        &self,
        now: i64,
        retention_days: u32,
        budget: Duration,
        pause: Duration,
    ) -> Result<PruneReport> {
        let cutoff = retention_cutoff(now, retention_days);
        let deadline = std::time::Instant::now() + budget;
        let prunes: [fn(&Self, i64) -> Result<usize>; 3] = [
            Self::prune_decisions,
            Self::prune_facts,
            Self::prune_check_runs,
        ];
        let mut deleted = [0; 3];
        let mut complete = true;
        'histories: for (index, prune) in prunes.iter().enumerate() {
            loop {
                let batch = prune(self, cutoff)?;
                deleted[index] += batch;
                if batch < PRUNE_BATCH {
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    complete = false;
                    break 'histories;
                }
                std::thread::sleep(pause);
            }
        }
        Ok(PruneReport {
            decisions: deleted[0],
            facts: deleted[1],
            check_runs: deleted[2],
            complete,
        })
    }

    fn prune_decisions(&self, cutoff: i64) -> Result<usize> {
        Ok(self.connection.execute(
            "DELETE FROM decisions WHERE id IN (SELECT id FROM decisions WHERE ts < ?1 ORDER BY ts LIMIT ?2)",
            params![cutoff, PRUNE_BATCH],
        )?)
    }

    /// Memories digested from a pruned fact stay; only their link to it goes.
    fn prune_facts(&self, cutoff: i64) -> Result<usize> {
        // Both statements select the same batch: the order is total and the transaction is exclusive.
        const BATCH: &str = "SELECT id FROM facts WHERE ts < ?1 ORDER BY ts,id LIMIT ?2";
        self.transaction(|tx| {
            // A stale mapping would make re-importing the same fact fail on a local id conflict.
            tx.execute(
                &format!(
                    "DELETE FROM imported_identities WHERE kind='fact' AND local_id IN ({BATCH})"
                ),
                params![cutoff, PRUNE_BATCH],
            )?;
            Ok(tx.execute(
                &format!("DELETE FROM facts WHERE id IN ({BATCH})"),
                params![cutoff, PRUNE_BATCH],
            )?)
        })
    }

    /// Evidence rows follow their check run; unfinished plans keep their whole history.
    fn prune_check_runs(&self, cutoff: i64) -> Result<usize> {
        Ok(self.connection.execute(
            "DELETE FROM check_runs WHERE id IN (SELECT id FROM check_runs WHERE ts < ?1 AND plan_id IN (SELECT id FROM plans WHERE state IN ('done','cancelled')) LIMIT ?2)",
            params![cutoff, PRUNE_BATCH],
        )?)
    }

    /// The schema version this database is at and the newest one this binary knows.
    pub fn schema_version(&self) -> Result<(usize, usize)> {
        let version: usize = self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        Ok((version, MIGRATIONS.len()))
    }

    /// Time, event, mode and outcome of the newest decision an agent's hooks recorded.
    pub fn last_decision(&self, agent: &str) -> Result<Option<(i64, String, String, String)>> {
        Ok(self
            .connection
            .query_row(
                "SELECT ts,event,mode,outcome FROM decisions WHERE agent=?1 ORDER BY ts DESC,id DESC LIMIT 1",
                [agent],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?)
    }

    pub fn end_session(&self, id: &str, ended_at: i64) -> Result<()> {
        self.connection.execute(
            "UPDATE sessions SET ended_at=?2 WHERE id=?1",
            params![redact(id), ended_at],
        )?;
        Ok(())
    }

    /// Read internal metadata; this is not a log API.
    pub fn get_kv(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .connection
            .query_row("SELECT value FROM kv WHERE key=?1", [key], |row| row.get(0))
            .optional()?)
    }

    pub fn set_kv(&self, key: &str, value: &str) -> Result<()> {
        self.connection.execute("INSERT INTO kv(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key, value])?;
        Ok(())
    }

    /// Atomically reserve a periodic task; malformed timestamps are explicit errors.
    pub fn claim_interval(&self, key: &str, now: i64, interval_secs: i64) -> Result<bool> {
        if interval_secs <= 0 {
            bail!("interval must be positive");
        }
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        let previous: Option<String> = transaction
            .query_row("SELECT value FROM kv WHERE key=?1", [key], |row| row.get(0))
            .optional()?;
        if let Some(previous) = previous {
            let previous: i64 = previous.parse().context("invalid interval timestamp")?;
            // A stamp more than one interval ahead cannot be a claim made on this clock (it was
            // set back); counting it as expired keeps the task from waiting for the clock to
            // reach it. A stamp slightly ahead is two processes with skewed clocks.
            let ahead = previous.saturating_sub(now);
            if ahead <= interval_secs && now.saturating_sub(previous) < interval_secs {
                return Ok(false);
            }
        }
        transaction.execute("INSERT INTO kv(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key, now.to_string()])?;
        transaction.commit()?;
        Ok(true)
    }
}

/// A pipe, device or directory where the database should be would make a hook wait on it, or
/// fail with an error that names nothing. A missing file is fine: opening creates it.
fn ensure_regular_file(path: &Path) -> Result<()> {
    match std::fs::metadata(path) {
        Ok(metadata) if !metadata.is_file() => bail!(
            "{} is not a regular file; move it aside in your own terminal so that Yoke can create its state database",
            path.display()
        ),
        _ => Ok(()),
    }
}

/// Rows one retention statement may delete, which bounds each write transaction.
const PRUNE_BATCH: usize = 1000;

/// Bytes a restarted WAL is truncated to inside a long-lived connection; the value only has to
/// be non-negative for the closing checkpoint of a last connection to truncate the WAL to zero.
const WAL_SIZE_LIMIT: i64 = 4 * 1024 * 1024;

/// Rows one retention pass deleted; `complete` is false when the time budget ran out first.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct PruneReport {
    pub decisions: usize,
    pub facts: usize,
    pub check_runs: usize,
    pub complete: bool,
}

fn retention_cutoff(now: i64, retention_days: u32) -> i64 {
    now.saturating_sub(i64::from(retention_days) * 86_400)
}

fn foreign_key_violations(connection: &Connection) -> Result<usize> {
    let mut statement = connection.prepare("PRAGMA foreign_key_check")?;
    let mut rows = statement.query([])?;
    let mut count = 0;
    while rows.next()?.is_some() {
        count += 1;
    }
    Ok(count)
}

fn trust_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TrustRow> {
    Ok(TrustRow {
        scope: row.get(0)?,
        root: row.get(1)?,
        file: row.get(2)?,
        sha256: row.get(3)?,
        content: row.get(4)?,
        trusted_at: row.get(5)?,
    })
}

#[cfg(test)]
#[path = "store/tests.rs"]
mod tests;
