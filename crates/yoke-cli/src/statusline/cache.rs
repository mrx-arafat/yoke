//! Small JSON caches beside the state database for facts that are slow to compute, such as a
//! git status or a health check. A paint reads them and, when one is stale, starts a detached
//! refresh instead of waiting, so the statusline never blocks on them.
//!
//! Nothing here creates the state directory: without it there is no cache and nothing refreshes.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const MAX_BYTES: u64 = 16 * 1024;
const MAX_ENTRIES: usize = 16;
/// A started refresh is given this long before another is started.
pub(super) const REFRESH_WAIT: i64 = 10;
/// A clock that ran backwards must not freeze a cache entry as fresh.
const SKEW: i64 = 60;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(super) struct Entry {
    /// When the value was computed.
    pub at: i64,
    /// When a refresh was last started; it is awaited before another one starts.
    #[serde(default)]
    pub started: i64,
    #[serde(default)]
    pub value: Value,
}

impl Entry {
    /// Seconds since the value was computed, or `None` when the clock disagrees with it.
    pub fn age(&self, now: i64) -> Option<i64> {
        (self.at <= now + SKEW).then(|| (now - self.at).max(0))
    }

    /// Whether a refresh started recently is still expected to deliver.
    pub fn in_flight(&self, now: i64) -> bool {
        self.started > self.at && (0..=REFRESH_WAIT).contains(&(now - self.started))
    }
}

type Table = BTreeMap<String, Entry>;

fn path(home: &Path, file: &str) -> PathBuf {
    home.join(".yoke").join(file)
}

/// A symlink, a non-regular file, an oversized or a malformed one reads as empty.
fn read(path: &Path) -> Table {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Table::new();
    };
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return Table::new();
    }
    let mut text = String::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(MAX_BYTES).read_to_string(&mut text))
        .ok()
        .and_then(|_| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub(super) fn load(home: &Path, file: &str, key: &str) -> Option<Entry> {
    read(&path(home, file)).remove(key)
}

/// Record `entry` with one atomic rename. Returns whether it wrote: without a real state
/// directory there is no cache. Best effort: the caller paints without it on failure.
pub(super) fn save(home: &Path, file: &str, key: &str, entry: Entry) -> Result<bool> {
    let target = path(home, file);
    let directory = target.parent().context("cache file has no directory")?;
    if !std::fs::symlink_metadata(directory).is_ok_and(|meta| meta.is_dir()) {
        return Ok(false);
    }
    let mut table = read(&target);
    table.insert(key.to_owned(), entry);
    while table.len() > MAX_ENTRIES {
        let oldest = table
            .iter()
            .min_by_key(|(_, entry)| entry.at.max(entry.started))
            .map(|(key, _)| key.clone());
        match oldest {
            Some(oldest) => table.remove(&oldest),
            None => break,
        };
    }
    let mut staged = tempfile::Builder::new()
        .prefix(".statusline-")
        .tempfile_in(directory)
        .context("stage statusline cache")?;
    serde_json::to_writer(&mut staged, &table)?;
    staged.persist(&target).map_err(|error| error.error)?;
    Ok(true)
}

/// Note that a refresh is starting, then start `yoke statusline <flag> <dir>` detached with its
/// working directory set to `dir`. It never waits for the child.
pub(super) fn refresh(
    home: &Path,
    file: &str,
    key: &str,
    current: Option<Entry>,
    now: i64,
    flag: &str,
    dir: &Path,
) -> Result<()> {
    let mut entry = current.unwrap_or_default();
    entry.started = now;
    // No cache to hold the result means a refresh would be wasted on every paint.
    if !save(home, file, key, entry)? {
        return Ok(());
    }
    let mut child = Command::new(std::env::current_exe()?);
    child
        .arg("statusline")
        .arg(flag)
        .arg(dir)
        .current_dir(dir)
        // Hooks of the refresh itself must not run, and its stdout is nobody's.
        .env("YOKE_OFF", "1")
        .env(crate::hook::BACKGROUND, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    {
        use std::os::unix::process::CommandExt;
        child.process_group(0);
    }
    child.spawn().context("start statusline refresh")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_round_trip_only_inside_an_existing_state_directory_and_stay_bounded() {
        let home = tempfile::tempdir().unwrap();
        let entry = |at| Entry {
            at,
            started: 0,
            value: serde_json::json!({"files": at}),
        };
        save(home.path(), "c.json", "a", entry(1)).unwrap();
        assert!(
            !home.path().join(".yoke").exists(),
            "never creates the directory"
        );
        assert!(load(home.path(), "c.json", "a").is_none());

        std::fs::create_dir(home.path().join(".yoke")).unwrap();
        for at in 1..=(MAX_ENTRIES as i64 + 4) {
            save(home.path(), "c.json", &format!("k{at}"), entry(at)).unwrap();
        }
        assert!(
            load(home.path(), "c.json", "k1").is_none(),
            "oldest evicted"
        );
        assert_eq!(
            load(home.path(), "c.json", "k20").unwrap().value["files"],
            20
        );
        assert!(
            std::fs::metadata(home.path().join(".yoke/c.json"))
                .unwrap()
                .len()
                <= MAX_BYTES
        );
    }

    #[test]
    fn damaged_oversized_or_linked_files_read_as_empty_and_are_replaced() {
        let home = tempfile::tempdir().unwrap();
        let directory = home.path().join(".yoke");
        std::fs::create_dir(&directory).unwrap();
        let file = directory.join("c.json");
        std::fs::write(&file, "not json").unwrap();
        assert!(load(home.path(), "c.json", "a").is_none());
        std::fs::write(&file, vec![b' '; MAX_BYTES as usize + 1]).unwrap();
        assert!(load(home.path(), "c.json", "a").is_none());
        std::fs::remove_file(&file).unwrap();
        let secret = home.path().join("elsewhere.json");
        std::fs::write(&secret, r#"{"a":{"at":1}}"#).unwrap();
        std::os::unix::fs::symlink(&secret, &file).unwrap();
        assert!(
            load(home.path(), "c.json", "a").is_none(),
            "symlinks are not followed"
        );
        // Saving replaces the link itself and leaves its target alone.
        save(home.path(), "c.json", "a", Entry::default()).unwrap();
        assert!(std::fs::symlink_metadata(&file).unwrap().is_file());
        assert_eq!(
            std::fs::read_to_string(&secret).unwrap(),
            r#"{"a":{"at":1}}"#
        );
    }

    #[test]
    fn age_and_in_flight_ignore_a_clock_that_went_backwards() {
        let entry = Entry {
            at: 100,
            started: 105,
            value: Value::Null,
        };
        assert_eq!(entry.age(110), Some(10));
        assert!(entry.in_flight(110));
        assert!(!entry.in_flight(116));
        assert_eq!(
            Entry {
                at: 1_000,
                ..Entry::default()
            }
            .age(100),
            None
        );
    }
}
