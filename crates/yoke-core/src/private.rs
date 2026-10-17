//! Owner-only modes for Yoke's state: `~/.yoke`, its database and sidecars, and its logs.
//!
//! The state holds redacted tool inputs, memories, trusted configuration and usage, so no other
//! local user has a reason to read it. Every call here is best effort on existing files: an
//! owner that cannot be changed, or a read-only file system, must never stop a hook.

use std::fs::{DirBuilder, File, OpenOptions};
use std::io;
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

/// Create `path` and its missing parents as 0700, and tighten an existing `path`.
pub fn create_dir_all(path: &Path) -> io::Result<()> {
    DirBuilder::new()
        .recursive(true)
        .mode(DIR_MODE)
        .create(path)?;
    tighten(path);
    Ok(())
}

/// Open `path` for appending, creating it as 0600 and tightening an existing file.
pub fn open_append(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(FILE_MODE)
        .open(path)?;
    if let Ok(metadata) = file.metadata() {
        narrow(&metadata.permissions(), |permissions| {
            let _ = file.set_permissions(permissions);
        });
    }
    Ok(file)
}

/// Remove group and other access from `path`, ignoring every failure.
pub fn tighten(path: &Path) {
    if let Ok(metadata) = std::fs::metadata(path) {
        narrow(&metadata.permissions(), |permissions| {
            let _ = std::fs::set_permissions(path, permissions);
        });
    }
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
    // A file that already exists is left as it is by `create`; SQLite then gives the sidecars
    // the mode of this file.
    let _ = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(FILE_MODE)
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
