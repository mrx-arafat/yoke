use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{ensure, Context, Result};

use super::FileChange;

const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

pub(super) fn check_size(content: &str) -> Result<()> {
    ensure!(
        content.len() as u64 <= MAX_FILE_BYTES,
        "settings or ownership record exceeds 8 MiB"
    );
    Ok(())
}

pub(super) fn read(path: &Path) -> Result<Option<String>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "refusing non-regular file: {}",
        path.display()
    );
    ensure!(
        metadata.len() <= MAX_FILE_BYTES,
        "settings or receipt exceeds 8 MiB: {}",
        path.display()
    );
    let mut raw = String::new();
    File::open(path)?
        .take(MAX_FILE_BYTES + 1)
        .read_to_string(&mut raw)?;
    ensure!(
        raw.len() as u64 <= MAX_FILE_BYTES,
        "settings grew beyond 8 MiB"
    );
    Ok(Some(raw))
}

pub(super) fn write(path: &Path, content: Option<&str>) -> Result<()> {
    if let Some(content) = content {
        check_size(content)?;
    }
    read(path)?; // Reject symlinks and non-regular targets before replacing anything.
    let parent = path.parent().context("path has no parent")?;
    if let Some(content) = content {
        fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        if let Ok(metadata) = fs::metadata(path) {
            file.as_file().set_permissions(metadata.permissions())?;
        }
        file.write_all(content.as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(path)
            .with_context(|| format!("atomically replacing {}", path.display()))?;
    } else if path.exists() {
        fs::remove_file(path).with_context(|| format!("removing owned file {}", path.display()))?;
    }
    if parent.exists() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

pub(super) fn check_unchanged(changes: &[FileChange]) -> Result<()> {
    for change in changes {
        if let Some(after) = &change.after {
            check_size(after)?;
        }
        ensure!(
            read(&change.path)? == change.before,
            "{} changed concurrently; retry without overwriting it",
            change.path.display()
        );
    }
    Ok(())
}

pub(super) fn apply(changes: &[FileChange]) -> Result<()> {
    for change in changes {
        check_unchanged(std::slice::from_ref(change))?;
        write(&change.path, change.after.as_deref()).with_context(|| {
            format!(
                "updating {}; ownership receipts and backups remain for recovery",
                change.path.display()
            )
        })?;
    }
    Ok(())
}

pub(super) fn backup(state_dir: &Path, changes: &[FileChange]) -> Result<()> {
    if changes.is_empty() {
        return Ok(());
    }
    let root = state_dir.join("backups");
    yoke_core::private::create_dir_all(&root)?;
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let backup = tempfile::Builder::new()
        .prefix(&format!("{timestamp}-"))
        .permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700))
        .tempdir_in(&root)?
        .keep();
    for (index, change) in changes.iter().enumerate() {
        if let Some(before) = &change.before {
            write(&backup.join(format!("{index}.original")), Some(before))?;
        }
    }
    // The index records absent originals too, without treating an empty file as absent.
    write(
        &backup.join("index.json"),
        Some(&serde_json::to_string_pretty(changes)?),
    )?;
    File::open(root)?.sync_all()?;
    Ok(())
}

pub(super) struct Lock {
    path: PathBuf,
}

impl Lock {
    pub(super) fn acquire(state_dir: &Path) -> Result<Self> {
        yoke_core::private::create_dir_all(state_dir)?;
        let path = state_dir.join("install.lock");
        let mut file = OpenOptions::new().write(true).create_new(true).open(&path)
            .context("another install/uninstall holds install.lock; after a crash, verify no installer runs before removing the stale lock")?;
        let lock = Self { path };
        writeln!(file, "{}", std::process::id())?;
        Ok(lock)
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_file(&self.path) {
            eprintln!("yoke: could not remove install lock: {error}");
        }
    }
}
