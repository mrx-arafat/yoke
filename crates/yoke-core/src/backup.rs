//! Consistent copies of the state database, made with SQLite's online backup API, and restoring
//! one into the live database.
//!
//! Backups live in `~/.yoke/state-backups` (0700), apart from the install-time settings backups
//! in `~/.yoke/backups`. Each is one private (0600) database file in rollback-journal mode, so it
//! can be downloaded, inspected and restored on its own. Restoring copies the backup into the
//! live database through the same API, so open hooks and readers keep valid files and sidecars;
//! it first writes a safety backup of the current state and refuses while a job is running.

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{bail, ensure, Context, Result};
use rusqlite::backup::{Backup, StepResult};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::store::Store;

/// Folder under `~/.yoke` that holds state backups.
pub const DIRECTORY: &str = "state-backups";
const PREFIX: &str = "state-";
const SUFFIX: &str = ".db";
/// Tables whose row counts describe a backup.
const TABLES: [&str; 5] = ["decisions", "sessions", "memories", "plans", "jobs"];

pub fn directory(home: &Path) -> PathBuf {
    home.join(".yoke").join(DIRECTORY)
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct BackupFile {
    pub name: String,
    pub path: PathBuf,
    pub bytes: u64,
    /// Modification time in Unix seconds; a backup is never written after it is created.
    pub created: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Inspection {
    pub file: BackupFile,
    /// Schema version recorded in the backup.
    pub schema: usize,
    /// Newest schema this build can open.
    pub supported: usize,
    /// `ok`, or the first problem SQLite's integrity check found.
    pub integrity: String,
    pub sha256: String,
    pub tables: Vec<(String, i64)>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Restored {
    /// The state as it was just before the restore.
    pub safety: BackupFile,
    /// Schema of the restored backup, and of the live database after upgrading it.
    pub from_schema: usize,
    pub schema: usize,
}

/// A name this module creates: `state-<digits and dashes>.db`, so it can never name a path
/// outside the backup folder.
pub fn is_backup_name(name: &str) -> bool {
    name.len() <= 64
        && name
            .strip_prefix(PREFIX)
            .and_then(|rest| rest.strip_suffix(SUFFIX))
            .is_some_and(|stamp| {
                !stamp.is_empty() && stamp.bytes().all(|b| b.is_ascii_digit() || b == b'-')
            })
}

/// `20261109-120007` for that instant in UTC.
fn stamp(now: i64) -> String {
    crate::timefmt::full_time_secs(now, 0)
        .chars()
        .filter(|c| !matches!(c, '-' | ':'))
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}

fn describe(path: &Path) -> Result<BackupFile> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("backup {} is unavailable", path.display()))?;
    ensure!(
        metadata.is_file(),
        "backup {} is not a regular file",
        path.display()
    );
    let created = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64);
    Ok(BackupFile {
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path: path.to_owned(),
        bytes: metadata.len(),
        created,
    })
}

fn finish(step: StepResult) -> Result<()> {
    match step {
        StepResult::Done => Ok(()),
        StepResult::Busy | StepResult::Locked => {
            bail!("another process held the state database; stop running agents and retry")
        }
        _ => bail!("the database copy did not finish in one step; retry"),
    }
}

/// SQLite's no-follow open refuses a link anywhere in the path (macOS temp folders sit behind
/// one), so resolve the parent; the file itself must still not be a link.
fn resolved(path: &Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .with_context(|| format!("{} has no file name", path.display()))?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(parent
        .canonicalize()
        .with_context(|| format!("resolve {}", parent.display()))?
        .join(name))
}

/// Write a consistent copy of `store` into `directory`, newest name first; the copy appears under
/// its final name only once it is complete and synced.
pub fn create(store: &Store, directory: &Path, now: i64) -> Result<BackupFile> {
    crate::private::create_dir_all(directory)
        .with_context(|| format!("create {}", directory.display()))?;
    let directory = &directory.canonicalize()?;
    // NamedTempFile creates the file 0600; SQLite then opens it as an empty database.
    let mut temporary = tempfile::Builder::new()
        .prefix(".partial-")
        .suffix(SUFFIX)
        .tempfile_in(directory)?;
    {
        let mut target = Connection::open_with_flags(
            temporary.path(),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        {
            // One step copies everything inside a single read snapshot of the live database.
            let backup = Backup::new(store.connection(), &mut target)?;
            finish(backup.step(-1)?)?;
        }
        // A single file: without a WAL the copy opens, moves and restores on its own.
        let mode: String = target.query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))?;
        ensure!(
            mode.eq_ignore_ascii_case("delete"),
            "the backup could not leave WAL mode"
        );
        target.close().map_err(|(_, error)| error)?;
    }
    temporary.as_file().sync_all()?;
    let base = format!("{PREFIX}{}", stamp(now));
    for attempt in 1..=99 {
        let name = if attempt == 1 {
            format!("{base}{SUFFIX}")
        } else {
            format!("{base}-{attempt}{SUFFIX}")
        };
        match temporary.persist_noclobber(directory.join(&name)) {
            Ok(_) => {
                File::open(directory)?.sync_all()?;
                return describe(&directory.join(name));
            }
            Err(error) if error.error.kind() == ErrorKind::AlreadyExists => {
                temporary = error.file;
            }
            Err(error) => return Err(error.error.into()),
        }
    }
    bail!("too many backups in the same second; wait a moment and retry")
}

/// Backups in `directory`, newest first; other files are ignored.
pub fn list(directory: &Path) -> Result<Vec<BackupFile>> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("read {}", directory.display())),
    };
    let mut backups = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str().filter(|name| is_backup_name(name)) else {
            continue;
        };
        if entry.file_type()?.is_file() {
            backups.push(describe(&directory.join(name))?);
        }
    }
    backups.sort_by(|a, b| b.created.cmp(&a.created).then(b.name.cmp(&a.name)));
    Ok(backups)
}

/// The backup called `name` in `directory`; any other name is refused.
pub fn find(directory: &Path, name: &str) -> Result<BackupFile> {
    ensure!(
        is_backup_name(name),
        "{name:?} is not a backup name; choose one from the backup list"
    );
    describe(&directory.join(name))
}

/// Open a backup read-only after checking that it is a regular, single-file SQLite database.
fn open(path: &Path) -> Result<Connection> {
    let path = &resolved(path)?;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .with_context(|| format!("open backup {}", path.display()))?;
    ensure!(
        file.metadata()?.is_file(),
        "backup {} is not a regular file",
        path.display()
    );
    let mut header = [0_u8; 100];
    file.read_exact(&mut header)
        .ok()
        .filter(|_| &header[..16] == b"SQLite format 3\0")
        .with_context(|| format!("{} is not a SQLite database", path.display()))?;
    ensure!(
        header[18] == 1 && header[19] == 1,
        "{} is in WAL mode, so it is not a complete single-file backup; use a backup made by Yoke",
        path.display()
    );
    let mut journal = path.as_os_str().to_owned();
    journal.push("-journal");
    ensure!(
        !Path::new(&journal).exists(),
        "{} has an unfinished journal next to it; use another backup",
        path.display()
    );
    Ok(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?)
}

fn sha256(path: &Path) -> Result<String> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Schema, integrity, checksum and row counts of a backup, read without writing it.
pub fn inspect(path: &Path) -> Result<Inspection> {
    let file = describe(path)?;
    let connection = open(path)?;
    let schema: usize = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let integrity: String =
        connection.query_row("PRAGMA integrity_check(1)", [], |row| row.get(0))?;
    let mut tables = Vec::new();
    for table in TABLES {
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [table],
            |row| row.get(0),
        )?;
        if exists {
            let count: i64 =
                connection.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })?;
            tables.push((table.to_owned(), count));
        }
    }
    Ok(Inspection {
        sha256: sha256(path)?,
        supported: Store::open(":memory:")?.schema_version()?.1,
        file,
        schema,
        integrity,
        tables,
    })
}

/// Refuse a backup that failed its integrity check or whose schema this build cannot open.
pub fn restorable(inspection: &Inspection) -> Result<()> {
    let path = inspection.file.path.display();
    ensure!(
        inspection.integrity == "ok",
        "backup {path} failed SQLite's integrity check ({}); nothing was restored",
        inspection.integrity
    );
    ensure!(
        (1..=inspection.supported).contains(&inspection.schema),
        "backup {path} has schema {} and this yoke supports 1 to {}; nothing was restored",
        inspection.schema,
        inspection.supported
    );
    Ok(())
}

/// Replace the live database's contents with `source`. Refuses a damaged, foreign or newer
/// backup and a running background job; writes a safety backup of the current state into
/// `safety` first, then upgrades the restored state to this build's schema.
pub fn restore(live: &Path, source: &Path, safety: &Path, now: i64) -> Result<Restored> {
    let inspection = inspect(source)?;
    restorable(&inspection)?;
    // This connection stays open until the end, so the copy's connection never closes the
    // database last and SQLite keeps the WAL sidecars that read-only readers need.
    let store = Store::open(live)?;
    if let Some(job) = crate::jobs::list(&store, false, 1000)?
        .into_iter()
        .find(|job| job.state == "running" && job.lease_expires.is_some_and(|at| at > now))
    {
        bail!(
            "background job {} ({}) is running; wait for it to finish (see yoke jobs), then retry; nothing was restored",
            job.id,
            job.kind
        );
    }
    let safety = create(&store, safety, now)?;
    {
        let backup_connection = open(source)?;
        let mut target = Connection::open_with_flags(
            resolved(live)?,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        target.busy_timeout(Duration::from_secs(5))?;
        let backup = Backup::new(&backup_connection, &mut target)?;
        finish(backup.step(-1)?).with_context(|| {
            format!(
                "nothing was restored; the state before this attempt is also in {}",
                safety.path.display()
            )
        })?;
    }
    let schema = Store::open(live)?.schema_version()?.0;
    drop(store);
    Ok(Restored {
        safety,
        from_schema: inspection.schema,
        schema,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn count(store: &Store, table: &str) -> i64 {
        store
            .connection()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    fn add_decisions(store: &Store, from: i64, to: i64) {
        store
            .connection()
            .execute(
                "INSERT OR IGNORE INTO sessions(id,agent,project,started_at) VALUES ('s','codex','/p',1)",
                [],
            )
            .unwrap();
        for ts in from..to {
            store
                .connection()
                .execute(
                    "INSERT INTO decisions(ts,session_id,agent,project,event,mode,outcome,gates,payload) VALUES (?1,'s','codex','/p','pre_tool_use','observe','allow','[]','{}')",
                    [ts],
                )
                .unwrap();
        }
    }

    #[test]
    fn a_backup_is_a_private_single_file_copy_that_restores_after_a_safety_backup() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join(".yoke");
        crate::private::create_dir_all(&state).unwrap();
        let live = state.join("state.db");
        let directory = directory(temp.path());
        let store = Store::open(&live).unwrap();
        add_decisions(&store, 1, 6);

        let first = create(&store, &directory, 1_794_200_000).unwrap();
        let second = create(&store, &directory, 1_794_200_000).unwrap();
        assert_eq!(first.name, "state-20261109-045320.db");
        assert_eq!(second.name, "state-20261109-045320-2.db");
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!((mode(&directory), mode(&first.path)), (0o700, 0o600));
        assert_eq!(std::fs::read(&first.path).unwrap()[18], 1, "not WAL");
        let names: Vec<String> = list(&directory)
            .unwrap()
            .into_iter()
            .map(|b| b.name)
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(
            find(&directory, "../state.db").is_err() && find(&directory, "state-1.db").is_err()
        );
        let inspection = inspect(&first.path).unwrap();
        assert_eq!(inspection.integrity, "ok");
        assert_eq!(inspection.schema, inspection.supported);
        assert!(inspection.tables.contains(&("decisions".to_owned(), 5)));
        assert_eq!(inspection.sha256.len(), 64);

        add_decisions(&store, 6, 9);
        let restored = restore(&live, &first.path, &directory, 1_794_200_100).unwrap();
        assert_eq!(count(&Store::open(&live).unwrap(), "decisions"), 5);
        assert_eq!(restored.schema, inspection.supported);
        let safety = inspect(&restored.safety.path).unwrap();
        assert!(
            safety.tables.contains(&("decisions".to_owned(), 8)),
            "{:?}",
            safety.tables
        );
        // The live database keeps its WAL sidecars, so no-write readers still open it.
        drop(store);
        assert_eq!(
            count(&Store::open_read_only(&live).unwrap(), "decisions"),
            5
        );
    }

    #[test]
    fn restore_refuses_foreign_newer_linked_files_and_a_running_job_without_changing_state() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join(".yoke");
        crate::private::create_dir_all(&state).unwrap();
        let live = state.join("state.db");
        let directory = directory(temp.path());
        let store = Store::open(&live).unwrap();
        add_decisions(&store, 1, 4);
        let good = create(&store, &directory, 1_794_200_000).unwrap();
        let newer = temp.path().join("newer.db");
        std::fs::copy(&good.path, &newer).unwrap();
        Connection::open(&newer)
            .unwrap()
            .pragma_update(None, "user_version", 999)
            .unwrap();
        let text = temp.path().join("notes.db");
        std::fs::write(&text, "not a database").unwrap();
        let link = temp.path().join("link.db");
        std::os::unix::fs::symlink(&good.path, &link).unwrap();
        add_decisions(&store, 4, 6);

        for source in [&live, &newer, &text, &link] {
            assert!(
                restore(&live, source, &directory, 1_794_200_100).is_err(),
                "{} was restored",
                source.display()
            );
        }
        crate::jobs::enqueue(&store, "retention", "retention", "", "{}", 10).unwrap();
        crate::jobs::claim(&store, 1_794_200_000, 7, 3600)
            .unwrap()
            .unwrap();
        let error = restore(&live, &good.path, &directory, 1_794_200_100).unwrap_err();
        assert!(format!("{error:#}").contains("is running"), "{error:#}");
        assert_eq!(count(&store, "decisions"), 5);
        assert_eq!(
            list(&directory).unwrap().len(),
            1,
            "no safety backup for a refused restore"
        );
    }
}
