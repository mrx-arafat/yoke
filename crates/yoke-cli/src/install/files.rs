use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{ensure, Context, Result};

use super::FileChange;

const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

pub(super) fn check_size(content: &str) -> Result<()> {
    ensure!(
        content.len() as u64 <= MAX_FILE_BYTES,
        "settings or ownership record exceeds 8 MiB; back up the file, reduce it below 8 MiB without removing other tools' settings, then retry"
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
        "refusing non-regular file: {}; in your own terminal replace the link or special file with a regular settings file, then retry",
        path.display()
    );
    ensure!(
        metadata.len() <= MAX_FILE_BYTES,
        "settings or receipt exceeds 8 MiB: {}; back up the file and reduce it below 8 MiB, then retry",
        path.display()
    );
    let mut raw = String::new();
    let file = File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let opened = file.metadata()?;
    ensure!(
        opened.is_file() && opened.dev() == metadata.dev() && opened.ino() == metadata.ino(),
        "settings {} changed while opening; retry after the other writer finishes",
        path.display()
    );
    file.take(MAX_FILE_BYTES + 1).read_to_string(&mut raw)?;
    ensure!(
        raw.len() as u64 <= MAX_FILE_BYTES,
        "settings grew beyond 8 MiB; stop the other writer, back up and reduce the file below 8 MiB, then retry"
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
        replace(change).with_context(|| {
            format!(
                "updating {}; ownership receipts and backups remain for recovery",
                change.path.display()
            )
        })?;
    }
    Ok(())
}

fn replace(change: &FileChange) -> Result<()> {
    let parent = change
        .path
        .parent()
        .context("settings path has no parent")?;
    fs::create_dir_all(parent)?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    if let Some(after) = &change.after {
        staged.write_all(after.as_bytes())?;
    }
    if let Ok(metadata) = fs::symlink_metadata(&change.path) {
        // SAFETY: geteuid takes no pointers and has no side effects.
        ensure!(metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o222 != 0,
            "{} is read-only or belongs to another user; run yoke install as its owner after making it writable", change.path.display());
        staged.as_file().set_permissions(metadata.permissions())?;
    }
    staged.as_file().sync_all()?;
    if change.before.is_some() {
        exchange(staged.path(), &change.path)?;
        // The displaced file is inspected AFTER the atomic exchange. A save between the
        // baseline read and exchange cannot be silently discarded, as with plain rename.
        if read(staged.path()).as_ref().ok() != Some(&change.before) {
            let (_, recovery) = staged.keep()?;
            anyhow::bail!("{} changed concurrently; its displaced contents are preserved at {}; stop the other writer, compare both files and restore the saved contents before retrying", change.path.display(), recovery.display());
        }
        if change.after.is_none() {
            ensure!(
                read(&change.path)?.as_deref() == Some(""),
                "{} changed concurrently; inspect it and retry after the other writer finishes",
                change.path.display()
            );
            fs::remove_file(&change.path)?;
        }
    } else if change.after.is_some() {
        staged.persist_noclobber(&change.path).with_context(|| {
            format!(
                "{} appeared concurrently; inspect it and retry",
                change.path.display()
            )
        })?;
    }
    ensure!(
        read(&change.path)? == change.after,
        "{} changed after Yoke wrote it; stop the other writer, inspect the settings and retry",
        change.path.display()
    );
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn exchange(left: &Path, right: &Path) -> Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let left = CString::new(left.as_os_str().as_bytes())?;
    let right = CString::new(right.as_os_str().as_bytes())?;
    // SAFETY: both paths are valid NUL-terminated strings kept alive across the call.
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::renameatx_np(
            libc::AT_FDCWD,
            left.as_ptr(),
            libc::AT_FDCWD,
            right.as_ptr(),
            libc::RENAME_SWAP,
        )
    };
    // SAFETY: same as above; syscall is used so this also works with older glibc versions.
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            left.as_ptr(),
            libc::AT_FDCWD,
            right.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    anyhow::bail!("atomic settings exchange is unsupported on this OS; leave settings unchanged and install hooks manually");
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    ensure!(result == 0, "atomic settings exchange failed: {}; settings were not replaced; retry on a filesystem supporting atomic exchange", std::io::Error::last_os_error());
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
    // The index contains paths and hashes, never duplicated settings or planned contents.
    let index: Vec<_> = changes.iter().enumerate().map(|(index, change)| serde_json::json!({
        "path":change.path, "original":change.before.as_ref().map(|_| format!("{index}.original")),
        "sha256":change.before.as_deref().map(super::receipt::hash)
    })).collect();
    write(
        &backup.join("index.json"),
        Some(&serde_json::to_string_pretty(&index)?),
    )?;
    let directories = backup_directories(state_dir)?;
    for old in directories.iter().skip(5) {
        fs::remove_dir_all(old)?;
    }
    File::open(root)?.sync_all()?;
    Ok(())
}

fn backup_directories(state_dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    let root = state_dir.join("backups");
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut directories = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.split_once('-').is_some_and(|(stamp, suffix)| {
            !stamp.is_empty() && stamp.bytes().all(|b| b.is_ascii_digit()) && !suffix.is_empty()
        }) && entry.file_type()?.is_dir()
        {
            directories.push(entry.path());
        }
    }
    directories.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    Ok(directories)
}

pub(super) fn original(state_dir: &Path, hash: &str) -> Result<Option<String>> {
    for directory in backup_directories(state_dir)?.into_iter().take(5) {
        // An operation changes at most three settings files. Ignore unrecognized files.
        for index in 0..3 {
            if let Some(raw) = read(&directory.join(format!("{index}.original")))? {
                if super::receipt::hash(&raw) == hash {
                    return Ok(Some(raw));
                }
            }
        }
    }
    Ok(None)
}

pub(super) struct Lock {
    _file: File,
}

impl Lock {
    pub(super) fn acquire(state_dir: &Path) -> Result<Self> {
        yoke_core::private::create_dir_all(state_dir)?;
        let path = state_dir.join("install.lock");
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)?;
        ensure!(
            file.metadata()?.is_file(),
            "install.lock must be a regular file; inspect it before retrying"
        );
        // SAFETY: flock only operates on this live file descriptor; the OS releases it on exit.
        ensure!(
            unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
            "another install/uninstall holds {}; wait for it to finish and retry",
            path.display()
        );
        file.set_len(0)?;
        writeln!(file, "{}", std::process::id())?;
        file.sync_all()?;
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_save_after_the_baseline_is_preserved_and_reported_after_exchange() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, "concurrent save").unwrap();
        let error = replace(&FileChange {
            path: path.clone(),
            before: Some("baseline".into()),
            after: Some("installed".into()),
        })
        .unwrap_err();
        assert!(
            error.to_string().contains("changed concurrently"),
            "{error:#}"
        );
        let saved = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|candidate| candidate != &path)
            .unwrap();
        assert_eq!(fs::read_to_string(saved).unwrap(), "concurrent save");
        assert_eq!(fs::read_to_string(path).unwrap(), "installed");
    }
}
