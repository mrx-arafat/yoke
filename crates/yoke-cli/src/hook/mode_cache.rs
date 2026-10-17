//! The last mode (observe or enforce) each project's hooks ran under, kept in `~/.yoke/mode-cache`.
//!
//! When Yoke cannot read its state database it cannot know the mode. Observe mode promises to
//! block nothing, so a project that was last seen in observe mode is allowed through with a loud
//! warning; anything else is denied, as before. The file is written only after a hook has read
//! its policy successfully, and only when the mode of a project changed.
//!
//! The keyed hash detects a stray edit or a file copied from another home; it is not a secret.
//! What keeps an agent from rewriting the file is the guard, which protects all of `~/.yoke`.

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use yoke_core::config::Mode;

const FILE: &str = ".yoke/mode-cache";
const TAG: &str = "yoke-mode-cache v1";
/// Projects remembered; the oldest entry goes first.
const MAX_ENTRIES: usize = 256;
const MAX_BYTES: u64 = 64 * 1024;

/// What the cache knows about one project.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Recall {
    /// Observe when the hooks last ran, recorded at this Unix time.
    Observe(i64),
    Enforce,
    /// No record for this project (or no file at all).
    Missing,
    /// A file that cannot be relied on, and why.
    Unusable(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    key: String,
    mode: Mode,
    stamp: i64,
}

/// Identity of the project a hook governs, from the files on disk alone so that it can be
/// computed while the state database is unreadable.
pub(super) fn project_key(cwd: &Path) -> Option<String> {
    let root = yoke_core::trust::config_home(cwd).ok()?;
    Some(hex(Sha256::digest(root.as_os_str().as_encoded_bytes())))
}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn mac(home: &Path, key: &str, mode: Mode, stamp: i64) -> String {
    let mut hash = Sha256::new();
    for part in [
        TAG.as_bytes(),
        home.as_os_str().as_encoded_bytes(),
        key.as_bytes(),
        mode.as_str().as_bytes(),
        stamp.to_string().as_bytes(),
    ] {
        hash.update(part);
        hash.update([0]);
    }
    hex(hash.finalize())
}

fn path(home: &Path) -> PathBuf {
    home.join(FILE)
}

/// Every entry of a well-formed, owner-only cache file. `Ok(None)` is no file at all.
fn load(home: &Path) -> Result<Option<Vec<Entry>>, &'static str> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let mut file = match options.open(path(home)) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
            return Err("it is a symbolic link")
        }
        Err(_) => return Err("it cannot be opened"),
    };
    let metadata = file.metadata().map_err(|_| "it cannot be read")?;
    // SAFETY: geteuid has no preconditions.
    if !metadata.is_file() || metadata.uid() != unsafe { libc::geteuid() } {
        return Err("it is not a regular file of this user");
    }
    if metadata.mode() & 0o077 != 0 {
        return Err("other users can access it");
    }
    if metadata.len() > MAX_BYTES {
        return Err("it is too large");
    }
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|_| "it is not valid text")?;
    let mut entries = Vec::new();
    for line in text.lines() {
        let rest = line.strip_prefix(TAG).ok_or("a line is malformed")?;
        let [key, mode, stamp, check] = rest.split_whitespace().collect::<Vec<_>>()[..] else {
            return Err("a line is malformed");
        };
        let mode = match mode {
            "observe" => Mode::Observe,
            "enforce" => Mode::Enforce,
            _ => return Err("a line is malformed"),
        };
        let stamp: i64 = stamp.parse().map_err(|_| "a line is malformed")?;
        if check != mac(home, key, mode, stamp) {
            return Err("it was edited or came from another home");
        }
        entries.push(Entry {
            key: key.into(),
            mode,
            stamp,
        });
    }
    Ok(Some(entries))
}

pub(super) fn recall(home: &Path, key: &str) -> Recall {
    match load(home) {
        Err(reason) => Recall::Unusable(reason),
        Ok(None) => Recall::Missing,
        Ok(Some(entries)) => {
            entries
                .iter()
                .find(|entry| entry.key == key)
                .map_or(Recall::Missing, |entry| match entry.mode {
                    Mode::Observe => Recall::Observe(entry.stamp),
                    Mode::Enforce => Recall::Enforce,
                })
        }
    }
}

/// Record the mode a project's hooks just ran under. Does nothing when it is already on record,
/// so an ordinary hook only reads a few lines. A failure is ignored: the cache is a courtesy
/// for the day the state is unreadable, and a missing record only means a denial then.
pub(super) fn remember(home: &Path, key: &str, mode: Mode, now: i64) {
    let mut entries = match load(home) {
        Ok(Some(entries)) => entries,
        // A file that cannot be relied on is replaced rather than extended.
        Ok(None) | Err(_) => Vec::new(),
    };
    if entries
        .iter()
        .any(|entry| entry.key == key && entry.mode == mode)
    {
        return;
    }
    entries.retain(|entry| entry.key != key);
    entries.push(Entry {
        key: key.into(),
        mode,
        stamp: now,
    });
    if entries.len() > MAX_ENTRIES {
        entries.sort_by_key(|entry| entry.stamp);
        entries.drain(..entries.len() - MAX_ENTRIES);
    }
    let _ = write(home, &entries);
}

fn write(home: &Path, entries: &[Entry]) -> std::io::Result<()> {
    let text: String = entries
        .iter()
        .map(|entry| {
            format!(
                "{TAG} {} {} {} {}\n",
                entry.key,
                entry.mode.as_str(),
                entry.stamp,
                mac(home, &entry.key, entry.mode, entry.stamp)
            )
        })
        .collect();
    let temporary = home.join(format!(".yoke/.mode-cache.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, path(home))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".yoke")).unwrap();
        home
    }

    #[test]
    fn a_remembered_mode_is_recalled_for_its_project_only() {
        let home = home();
        remember(home.path(), "aaa", Mode::Observe, 100);
        remember(home.path(), "bbb", Mode::Enforce, 200);
        assert_eq!(recall(home.path(), "aaa"), Recall::Observe(100));
        assert_eq!(recall(home.path(), "bbb"), Recall::Enforce);
        assert_eq!(recall(home.path(), "ccc"), Recall::Missing);
        let mode = std::fs::metadata(path(home.path())).unwrap().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_missing_or_unreliable_file_is_never_read_as_observe() {
        let home = home();
        assert_eq!(recall(home.path(), "aaa"), Recall::Missing);
        remember(home.path(), "aaa", Mode::Observe, 100);
        let original = std::fs::read_to_string(path(home.path())).unwrap();
        for (label, text) in [
            ("edited mode", original.replace(" observe ", " enforce ")),
            ("edited stamp", original.replace(" 100 ", " 101 ")),
            ("truncated", original[..original.len() / 2].to_owned()),
            ("garbage", "observe\n".to_owned()),
        ] {
            std::fs::write(path(home.path()), text).unwrap();
            assert!(
                matches!(recall(home.path(), "aaa"), Recall::Unusable(_)),
                "{label}"
            );
        }
        // A copy from another home fails the keyed hash, and a file others can write fails the mode.
        std::fs::write(path(home.path()), &original).unwrap();
        let other = tempfile::tempdir().unwrap();
        std::fs::create_dir(other.path().join(".yoke")).unwrap();
        std::fs::copy(path(home.path()), path(other.path())).unwrap();
        assert!(matches!(recall(other.path(), "aaa"), Recall::Unusable(_)));
        std::fs::set_permissions(
            path(home.path()),
            std::os::unix::fs::PermissionsExt::from_mode(0o666),
        )
        .unwrap();
        assert!(matches!(recall(home.path(), "aaa"), Recall::Unusable(_)));
    }

    #[test]
    fn an_unchanged_mode_is_not_rewritten_and_the_file_stays_bounded() {
        let home = home();
        remember(home.path(), "aaa", Mode::Observe, 100);
        let first = std::fs::metadata(path(home.path())).unwrap().ino();
        remember(home.path(), "aaa", Mode::Observe, 999);
        assert_eq!(std::fs::metadata(path(home.path())).unwrap().ino(), first);
        assert_eq!(recall(home.path(), "aaa"), Recall::Observe(100));
        remember(home.path(), "aaa", Mode::Enforce, 300);
        assert_eq!(recall(home.path(), "aaa"), Recall::Enforce);
        for index in 0..MAX_ENTRIES + 10 {
            remember(
                home.path(),
                &format!("p{index}"),
                Mode::Observe,
                1000 + index as i64,
            );
        }
        let lines = std::fs::read_to_string(path(home.path()))
            .unwrap()
            .lines()
            .count();
        assert_eq!(lines, MAX_ENTRIES);
        assert_eq!(recall(home.path(), "aaa"), Recall::Missing);
    }
}
