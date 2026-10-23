//! Owner-only modes for Yoke's state: `~/.yoke`, its database and sidecars, and its logs.
//!
//! The state holds redacted tool inputs, memories, trusted configuration and usage, so no other
//! local user has a reason to read it. Permission tightening on existing files is best effort;
//! opening a link or a special file is refused instead of following it or waiting on it.

use std::fs::{DirBuilder, File, OpenOptions};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

const DIR_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;
/// Group and other permission bits.
const OTHERS: u32 = 0o077;
/// Name of the state directory; only a database inside one is allowed to tighten its parent.
pub(crate) const STATE_DIR: &str = ".yoke";
/// Files next to the database that hold diagnostics.
const STATE_FILES: [&str; 2] = ["error.log", "error.log.1"];

/// Create `path` and its missing parents as 0700, refusing a link at `path`.
pub fn create_dir_all(path: &Path) -> io::Result<()> {
    DirBuilder::new()
        .recursive(true)
        .mode(DIR_MODE)
        .create(path)?;
    if !std::fs::symlink_metadata(path)?.is_dir() {
        return Err(io::Error::other(
            "private directory must not be a symbolic link",
        ));
    }
    tighten(path);
    Ok(())
}

/// Append to a regular file without following links or blocking on a pipe; create it as 0600.
pub fn open_append(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(FILE_MODE)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other("private log must be a regular file"));
    }
    narrow(&metadata.permissions(), |permissions| {
        let _ = file.set_permissions(permissions);
    });
    Ok(file)
}

/// Remove group and other access from `path`, ignoring every failure.
pub fn tighten(path: &Path) {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    if !metadata.is_file() && !metadata.is_dir() {
        return;
    }
    let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return;
    };
    // Never open and close a database or sidecar here: closing any descriptor for an inode
    // releases this process's POSIX locks, including SQLite's shared-memory locks.
    narrow(&metadata.permissions(), |permissions| {
        // SAFETY: path is live and NUL-terminated. The flag prevents following a replaced link.
        let _ = unsafe {
            libc::fchmodat(
                libc::AT_FDCWD,
                path.as_ptr(),
                permissions.mode() as libc::mode_t,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
    });
}

fn narrow(current: &std::fs::Permissions, apply: impl FnOnce(std::fs::Permissions)) {
    let mode = current.mode();
    if mode & OTHERS != 0 {
        apply(std::fs::Permissions::from_mode(mode & !OTHERS));
    }
}

/// Make the database about to be opened private: create it as 0600 when it is new, and narrow
/// it, its sidecars and the state directory around it when they already exist.
pub(crate) fn prepare_database(path: &Path) {
    if let Some(directory) = path
        .parent()
        .filter(|directory| directory.file_name().is_some_and(|name| name == STATE_DIR))
    {
        tighten(directory);
        for name in STATE_FILES {
            tighten(&directory.join(name));
        }
    }
    // Never open an existing database behind SQLite's back: closing that descriptor could
    // release another connection's POSIX locks in this process.
    let _ = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(FILE_MODE)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path);
    tighten_database(path);
}

/// Narrow the database and the `-wal` and `-shm` sidecars that exist now.
pub(crate) fn tighten_database(path: &Path) {
    tighten(path);
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = path.as_os_str().to_owned();
        sidecar.push(suffix);
        tighten(Path::new(&sidecar));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn log_opens_refuse_links_and_special_files() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let log = dir.path().join("error.log");
        std::fs::write(&target, "unchanged").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        symlink(&target, &log).unwrap();
        assert!(open_append(&log).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"unchanged");
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o644
        );

        std::fs::remove_file(&log).unwrap();
        let fifo = std::ffi::CString::new(log.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: fifo is a live, NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        // A reader keeps even the old blocking write-open from hanging this regression.
        let _reader = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&log)
            .unwrap();
        assert!(open_append(&log).is_err());
        assert!(open_append(dir.path()).is_err());
    }

    #[test]
    fn tightening_and_directory_creation_do_not_follow_links() {
        let dir = tempfile::tempdir().unwrap();
        for directory in [false, true] {
            let target = dir
                .path()
                .join(if directory { "directory" } else { "file" });
            let link = target.with_extension("link");
            let mode = if directory { 0o755 } else { 0o644 };
            if directory {
                std::fs::create_dir(&target).unwrap();
            } else {
                std::fs::write(&target, "unchanged").unwrap();
            }
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode)).unwrap();
            symlink(&target, &link).unwrap();
            tighten(&link);
            assert_eq!(
                std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
                mode
            );
            if directory {
                assert!(create_dir_all(&link).is_err());
            }
        }
    }
}
