//! Durable background jobs: idempotent enqueue, leased execution and bounded retries.
//! There is no daemon; hooks recover expired leases and start a short-lived worker.

use anyhow::{bail, ensure, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::{redact::redact_payload, store::Store};

pub const MAX_ATTEMPTS: i64 = 3;
const BACKOFF_SECS: i64 = 60;
const BACKOFF_CAP_SECS: i64 = 3600;
const ERROR_CAP: usize = 2048;
const ACTIVE: &str = "('queued','running','failed')";
const COLUMNS: &str = "id,kind,dedupe_key,project,payload,state,attempts,max_attempts,run_after,owner_pid,lease_expires,last_error,created_at,updated_at";

/// `failed` means a retry is scheduled at `run_after`; `dead` waits for a human retry.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Job {
    pub id: i64,
    pub kind: String,
    pub dedupe_key: String,
    pub project: String,
    pub payload: String,
    pub state: String,
    pub attempts: i64,
    pub max_attempts: i64,
    pub run_after: i64,
    pub owner_pid: Option<i64>,
    pub lease_expires: Option<i64>,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Seconds to wait after the given failed attempt: 60, 120, 240, ... capped at one hour.
pub fn backoff(attempts: i64) -> i64 {
    (BACKOFF_SECS << attempts.saturating_sub(1).clamp(0, 16)).min(BACKOFF_CAP_SECS)
}

/// Return the active job for this key, or queue a new one; `true` means it was created.
pub fn enqueue(
    store: &Store,
    kind: &str,
    dedupe_key: &str,
    project: &str,
    payload: &str,
    now: i64,
) -> Result<(Job, bool)> {
    store.transaction(|tx| enqueue_in(tx, kind, dedupe_key, project, payload, now))
}

fn enqueue_in(
    connection: &Connection,
    kind: &str,
    dedupe_key: &str,
    project: &str,
    payload: &str,
    now: i64,
) -> Result<(Job, bool)> {
    ensure!(
        (1..=64).contains(&kind.len()) && (1..=512).contains(&dedupe_key.len()),
        "job kind and dedupe key must be nonempty and bounded"
    );
    ensure!(payload.len() <= 65_536, "job payload exceeds 64 KiB; repair the hook's oversized session/transcript metadata and start a new agent session, then inspect yoke jobs --all; recorded facts remain available and no job was queued");
    let active = connection
        .query_row(
            &format!("SELECT id FROM jobs WHERE dedupe_key=?1 AND state IN {ACTIVE}"),
            [dedupe_key],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    if let Some(id) = active {
        return Ok((get_on(connection, id)?, false));
    }
    connection.execute(
        "INSERT INTO jobs(kind,dedupe_key,project,payload,state,max_attempts,run_after,created_at,updated_at)
         VALUES (?1,?2,?3,?4,'queued',?5,?6,?6,?6)",
        params![kind, dedupe_key, project, payload, MAX_ATTEMPTS, now],
    )?;
    Ok((get_on(connection, connection.last_insert_rowid())?, true))
}

/// Requeue running jobs whose owner let the lease expire, or mark them dead when no attempts remain.
pub fn recover(store: &Store, now: i64) -> Result<usize> {
    store.transaction(|tx| {
        let expired = tx
            .prepare("SELECT id,attempts,max_attempts,owner_pid FROM jobs WHERE state='running' AND lease_expires < ?1")?
            .query_map([now], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?, row.get::<_, Option<i64>>(3)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, attempts, max_attempts, owner) in &expired {
            let (state, run_after) = if attempts >= max_attempts {
                ("dead", now)
            } else {
                ("queued", now.saturating_add(backoff(*attempts)))
            };
            let error = format!(
                "worker process {} stopped before finishing; lease expired",
                owner.unwrap_or_default()
            );
            tx.execute(
                "UPDATE jobs SET state=?2,run_after=?3,owner_pid=NULL,lease_expires=NULL,last_error=?4,updated_at=?5 WHERE id=?1",
                params![id, state, run_after, error, now],
            )?;
        }
        Ok(expired.len())
    })
}

/// Atomically lease the oldest due job to `owner`, counting the attempt.
pub fn claim(store: &Store, now: i64, owner: u32, lease_secs: i64) -> Result<Option<Job>> {
    ensure!(lease_secs > 0, "job lease must be positive");
    store.transaction(|tx| {
        let due = tx
            .query_row(
                "SELECT id FROM jobs WHERE state IN ('queued','failed') AND run_after <= ?1 ORDER BY run_after,id LIMIT 1",
                [now],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        let Some(id) = due else { return Ok(None) };
        tx.execute(
            "UPDATE jobs SET state='running',attempts=attempts+1,owner_pid=?2,lease_expires=?3,updated_at=?4 WHERE id=?1",
            params![id, owner, now.saturating_add(lease_secs), now],
        )?;
        Ok(Some(get_on(tx, id)?))
    })
}

/// Extend this worker's lease; fails when recovery handed the job to someone else.
pub fn heartbeat(store: &Store, job: &Job, now: i64, lease_secs: i64) -> Result<()> {
    ensure!(lease_secs > 0, "job lease must be positive");
    owned_update(
        store.connection(),
        job,
        "lease_expires=?4,updated_at=?5",
        params![
            job.id,
            job.owner_pid,
            job.attempts,
            now.saturating_add(lease_secs),
            now
        ],
    )
}

/// Mark the job succeeded. With `follow_up`, a fresh job for the same work (kind, key, project
/// and payload) is queued in the same transaction, so a crash can never leave the work
/// finished with its remainder unqueued; the follow-up is returned.
pub fn succeed(store: &Store, job: &Job, now: i64, follow_up: bool) -> Result<Option<Job>> {
    store.transaction(|tx| {
        owned_update(
            tx,
            job,
            "state='succeeded',owner_pid=NULL,lease_expires=NULL,last_error=NULL,updated_at=?4",
            params![job.id, job.owner_pid, job.attempts, now],
        )?;
        if !follow_up {
            return Ok(None);
        }
        Ok(Some(
            enqueue_in(
                tx,
                &job.kind,
                &job.dedupe_key,
                &job.project,
                &job.payload,
                now,
            )?
            .0,
        ))
    })
}

/// Record a redacted, capped error. Retryable failures back off until attempts run out.
pub fn fail(
    store: &Store,
    job: &Job,
    now: i64,
    error: &str,
    retryable: bool,
) -> Result<&'static str> {
    let (state, run_after) = if retryable && job.attempts < job.max_attempts {
        ("failed", now.saturating_add(backoff(job.attempts)))
    } else {
        ("dead", now)
    };
    owned_update(
        store.connection(),
        job,
        "state=?4,run_after=?5,owner_pid=NULL,lease_expires=NULL,last_error=?6,updated_at=?7",
        params![
            job.id,
            job.owner_pid,
            job.attempts,
            state,
            run_after,
            redact_payload(error, ERROR_CAP),
            now
        ],
    )?;
    Ok(state)
}

fn owned_update(
    connection: &Connection,
    job: &Job,
    set: &str,
    values: impl rusqlite::Params,
) -> Result<()> {
    let changed = connection.execute(
        &format!("UPDATE jobs SET {set} WHERE id=?1 AND state='running' AND owner_pid IS ?2 AND attempts=?3"),
        values,
    )?;
    if changed != 1 {
        bail!(
            "job {} lease was lost; recovery may have handed it to another worker; inspect yoke jobs --all and wait for the current worker rather than starting duplicate work",
            job.id
        );
    }
    Ok(())
}

/// Give a dead job a fresh set of attempts. Callers must ensure a human asked for this.
pub fn retry(store: &Store, id: i64, now: i64) -> Result<Job> {
    store.transaction(|tx| {
        let Some(job) = get_optional(tx, id)? else {
            bail!("job {id} not found; run yoke jobs --all to choose an existing dead job ID before retrying");
        };
        ensure!(
            job.state == "dead",
            "only dead jobs can be retried; job {id} is {}; run yoke jobs --all to inspect its state and wait for active work to finish before retrying a dead job",
            job.state
        );
        let active = tx
            .query_row(
                &format!("SELECT id,state FROM jobs WHERE dedupe_key=?1 AND state IN {ACTIVE}"),
                [&job.dedupe_key],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        if let Some((other, state)) = active {
            bail!("job {other} for the same work is already {state}; run yoke jobs --all and wait for that work to finish instead of retrying a duplicate");
        }
        tx.execute(
            "UPDATE jobs SET state='queued',attempts=0,run_after=?2,updated_at=?2 WHERE id=?1",
            params![id, now],
        )?;
        get_on(tx, id)
    })
}

/// Newest jobs first; succeeded jobs only when `all` is set.
pub fn list(store: &Store, all: bool, limit: usize) -> Result<Vec<Job>> {
    let mut statement = store.connection().prepare(&format!(
        "SELECT {COLUMNS} FROM jobs WHERE ?1 OR state!='succeeded' ORDER BY id DESC LIMIT ?2"
    ))?;
    let rows = statement.query_map(params![all, limit.min(10_000) as i64], job_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn due(store: &Store, now: i64) -> Result<bool> {
    Ok(store.connection().query_row(
        "SELECT EXISTS(SELECT 1 FROM jobs WHERE state IN ('queued','failed') AND run_after <= ?1)",
        [now],
        |row| row.get(0),
    )?)
}

/// The newest job for this key that has not succeeded: queued, running, failed or dead.
pub fn unfinished(store: &Store, dedupe_key: &str) -> Result<Option<Job>> {
    Ok(store
        .connection()
        .query_row(
            &format!("SELECT {COLUMNS} FROM jobs WHERE dedupe_key=?1 AND state!='succeeded' ORDER BY id DESC LIMIT 1"),
            [dedupe_key],
            job_row,
        )
        .optional()?)
}

/// Remove finished jobs strictly older than the retention boundary.
pub fn prune(store: &Store, now: i64, retention_days: u32) -> Result<usize> {
    Ok(store.connection().execute(
        "DELETE FROM jobs WHERE state IN ('succeeded','dead') AND updated_at < ?1",
        [now.saturating_sub(i64::from(retention_days) * 86_400)],
    )?)
}

fn get_on(connection: &Connection, id: i64) -> Result<Job> {
    get_optional(connection, id)?.ok_or_else(|| anyhow::anyhow!("job {id} missing"))
}

fn get_optional(connection: &Connection, id: i64) -> Result<Option<Job>> {
    Ok(connection
        .query_row(
            &format!("SELECT {COLUMNS} FROM jobs WHERE id=?1"),
            [id],
            job_row,
        )
        .optional()?)
}

fn job_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Job> {
    Ok(Job {
        id: row.get(0)?,
        kind: row.get(1)?,
        dedupe_key: row.get(2)?,
        project: row.get(3)?,
        payload: row.get(4)?,
        state: row.get(5)?,
        attempts: row.get(6)?,
        max_attempts: row.get(7)?,
        run_after: row.get(8)?,
        owner_pid: row.get(9)?,
        lease_expires: row.get(10)?,
        last_error: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

#[cfg(test)]
#[path = "jobs/tests.rs"]
mod tests;
