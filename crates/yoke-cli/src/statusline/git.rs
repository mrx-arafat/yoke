//! Git facts for the statusline. The branch is read straight from `HEAD` files, which is
//! instant. The count of changed files needs `git status`, which can take seconds in a large
//! repository, so a paint only reads a cached count and, when it is stale, starts a detached
//! refresh. A repository that is too slow is skipped for a while rather than waited for.
//!
//! Git runs only in the session's own directory, never with a repository path from elsewhere.

use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use serde_json::{json, Value};
use yoke_core::engine::execute_bounded;

use super::cache::{self, Entry};
use super::line::{Branch, Dirty};

const FILE: &str = "statusline-git.json";
/// A count this young is not refreshed; an older one is shown while a refresh runs.
const FRESH: i64 = 5;
/// An older count than this is not shown at all.
const SHOWN: i64 = 600;
const RETRY_SLOW: i64 = 60;
const RETRY_UNAVAILABLE: i64 = 600;
/// Directory levels searched upward for a repository.
const LEVELS: usize = 64;
const GIT_TIMEOUT: Duration = Duration::from_secs(2);
const OUTPUT_CAP: usize = 256 * 1024;

struct Repo {
    git_dir: PathBuf,
}

fn read_small(path: &Path, limit: u64) -> Option<String> {
    use std::io::Read;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut text = String::new();
    file.take(limit).read_to_string(&mut text).ok()?;
    Some(text)
}

/// The repository containing `start`: a `.git` directory, or a `.git` file that points at one,
/// as in worktrees and submodules.
fn find(start: &Path) -> Option<Repo> {
    for directory in start.ancestors().take(LEVELS) {
        let dot = directory.join(".git");
        let Ok(meta) = std::fs::symlink_metadata(&dot) else {
            continue;
        };
        if meta.is_dir() {
            return Some(Repo { git_dir: dot });
        }
        if !meta.is_file() {
            return None;
        }
        let text = read_small(&dot, 4096)?;
        let target = text.lines().next()?.strip_prefix("gitdir:")?.trim();
        let git_dir = directory.join(target);
        return git_dir.is_dir().then_some(Repo { git_dir });
    }
    None
}

/// The branch name, or a short commit id with `true` when HEAD is detached.
fn head(git_dir: &Path) -> Option<(String, bool)> {
    let text = read_small(&git_dir.join("HEAD"), 256)?;
    let text = text.trim();
    if let Some(reference) = text.strip_prefix("ref:") {
        let reference = reference.trim();
        let name = reference.strip_prefix("refs/heads/").unwrap_or(reference);
        return (!name.is_empty()).then(|| (name.to_owned(), false));
    }
    (text.len() >= 7 && text.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| (text[..7].to_owned(), true))
}

/// What a cache entry means now: the count to show, and whether to start a refresh.
fn lookup(entry: Option<&Entry>, now: i64) -> (Option<Dirty>, bool) {
    let Some(entry) = entry else {
        return (None, true);
    };
    let Some(age) = entry.age(now) else {
        return (None, true);
    };
    let idle = !entry.in_flight(now);
    match entry.value["state"].as_str() {
        Some("ok") => {
            let dirty = (age <= SHOWN).then(|| Dirty {
                files: entry.value["files"]
                    .as_u64()
                    .map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX)),
                capped: entry.value["capped"].as_bool().unwrap_or(false),
            });
            (dirty, age > FRESH && idle)
        }
        Some("slow") => (None, age > RETRY_SLOW && idle),
        _ => (None, age > RETRY_UNAVAILABLE && idle),
    }
}

/// The branch of the repository containing `dir`, with the last known count of changed files.
/// Returns at once: a stale or missing count starts a detached refresh and is left out.
pub(super) fn status(home: &Path, dir: &Path, now: i64) -> Option<Branch> {
    let repo = find(dir)?;
    let (name, detached) = head(&repo.git_dir)?;
    let key = dir.to_string_lossy();
    let entry = cache::load(home, FILE, &key);
    let (dirty, stale) = lookup(entry.as_ref(), now);
    if stale {
        // A failure to start only means the count stays unknown.
        let _ = cache::refresh(home, FILE, &key, entry, now, "--refresh-git", dir);
    }
    Some(Branch {
        name,
        detached,
        dirty,
    })
}

fn measure(dir: &Path) -> Value {
    let Ok(mut git) = yoke_core::process::git() else {
        return json!({"state": "unavailable"});
    };
    git.args([
        "status",
        "--porcelain=v1",
        "--untracked-files=normal",
        "--",
        ".",
    ])
    .current_dir(dir)
    .env("GIT_TERMINAL_PROMPT", "0")
    .env("LC_ALL", "C");
    match execute_bounded(&mut git, b"", GIT_TIMEOUT, OUTPUT_CAP) {
        Ok(output) if output.timed_out => json!({"state": "slow"}),
        Ok(output) if output.success => {
            let files = output
                .stdout
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
                .count();
            json!({"state": "ok", "files": files, "capped": output.stdout_truncated})
        }
        _ => json!({"state": "unavailable"}),
    }
}

/// The detached refresh: count the changed files under `dir` and record the result.
pub(super) fn refresh(home: &Path, dir: &Path, now: i64) -> Result<()> {
    let value = measure(dir);
    cache::save(
        home,
        FILE,
        &dir.to_string_lossy(),
        Entry {
            at: now,
            started: 0,
            value,
        },
    )
    .map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A value computed `age` seconds before the test's clock reads 1,000; `started` is the
    /// time a refresh began, or 0 for none.
    fn entry(age: i64, started: i64, value: Value) -> Entry {
        Entry {
            at: 1_000 - age,
            started,
            value,
        }
    }

    #[test]
    fn git_metadata_links_are_not_read() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        let metadata = root.path().join("metadata");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir(&metadata).unwrap();
        std::fs::write(metadata.join("HEAD"), "ref: refs/heads/forged\n").unwrap();
        symlink(metadata.join("HEAD"), repo.join(".git/HEAD")).unwrap();
        assert_eq!(head(&repo.join(".git")), None);
        std::fs::remove_file(repo.join(".git/HEAD")).unwrap();
        std::fs::remove_dir(repo.join(".git")).unwrap();
        symlink(&metadata, repo.join(".git")).unwrap();
        assert!(find(&repo).is_none());
        std::fs::remove_file(repo.join(".git")).unwrap();
        let pointer = root.path().join("pointer");
        std::fs::write(&pointer, format!("gitdir: {}\n", metadata.display())).unwrap();
        symlink(&pointer, repo.join(".git")).unwrap();
        assert!(find(&repo).is_none());
        assert_eq!(read_small(Path::new("/dev/null"), 32), None);
    }

    #[test]
    fn head_reads_branches_detached_commits_and_worktree_pointers() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/feature/x\n").unwrap();
        let deep = repo.join("a/b");
        std::fs::create_dir_all(&deep).unwrap();
        let found = find(&deep).unwrap();
        assert_eq!(head(&found.git_dir), Some(("feature/x".into(), false)));

        // A linked worktree has a `.git` file pointing at its own metadata directory.
        let linked = root.path().join("linked");
        let metadata = repo.join(".git/worktrees/linked");
        std::fs::create_dir_all(&linked).unwrap();
        std::fs::create_dir_all(&metadata).unwrap();
        std::fs::write(
            linked.join(".git"),
            format!("gitdir: {}\n", metadata.display()),
        )
        .unwrap();
        std::fs::write(
            metadata.join("HEAD"),
            "0123456789abcdef0123456789abcdef01234567\n",
        )
        .unwrap();
        let found = find(&linked).unwrap();
        assert_eq!(head(&found.git_dir), Some(("0123456".into(), true)));

        // A relative pointer, as submodules use, resolves against the directory holding it.
        let module = root.path().join("module");
        std::fs::create_dir_all(root.path().join("store/m")).unwrap();
        std::fs::create_dir_all(&module).unwrap();
        std::fs::write(module.join(".git"), "gitdir: ../store/m\n").unwrap();
        std::fs::write(root.path().join("store/m/HEAD"), "ref: refs/heads/main\n").unwrap();
        assert_eq!(
            head(&find(&module).unwrap().git_dir),
            Some(("main".into(), false))
        );

        // A broken pointer and a garbage HEAD all yield nothing.
        std::fs::write(module.join(".git"), "gitdir: ../missing\n").unwrap();
        assert!(find(&module).is_none());
        std::fs::write(repo.join(".git/HEAD"), "garbage").unwrap();
        assert_eq!(head(&repo.join(".git")), None);
    }

    #[test]
    fn a_cached_count_is_shown_while_it_is_refreshed_and_slow_repositories_back_off() {
        let ok = json!({"state": "ok", "files": 3, "capped": true});
        let shown = Some(Dirty {
            files: 3,
            capped: true,
        });
        // Missing, fresh, stale, in flight, too old to show, and from a clock gone backwards.
        assert_eq!(lookup(None, 1_000), (None, true));
        assert_eq!(
            lookup(Some(&entry(2, 0, ok.clone())), 1_000),
            (shown, false)
        );
        assert_eq!(
            lookup(Some(&entry(30, 0, ok.clone())), 1_000),
            (shown, true)
        );
        assert_eq!(
            lookup(Some(&entry(30, 997, ok.clone())), 1_000),
            (shown, false)
        );
        assert_eq!(
            lookup(Some(&entry(700, 0, ok.clone())), 1_000),
            (None, true)
        );
        assert_eq!(lookup(Some(&entry(-500, 0, ok)), 1_000), (None, true));
        // A repository that timed out is left alone for a minute; no git is retried quickly.
        let slow = json!({"state": "slow"});
        assert_eq!(
            lookup(Some(&entry(30, 0, slow.clone())), 1_000),
            (None, false)
        );
        assert_eq!(lookup(Some(&entry(90, 0, slow)), 1_000), (None, true));
        let none = json!({"state": "unavailable"});
        assert_eq!(
            lookup(Some(&entry(90, 0, none.clone())), 1_000),
            (None, false)
        );
        assert_eq!(lookup(Some(&entry(700, 0, none)), 1_000), (None, true));
    }

    #[test]
    fn the_detached_refresh_counts_changed_and_untracked_files_with_real_git() {
        if std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".yoke")).unwrap();
        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(repo.path())
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .output()
                .unwrap();
            assert!(status.status.success(), "{args:?}");
        };
        git(&["init", "-q"]);
        std::fs::write(repo.path().join("a"), "1").unwrap();
        git(&["add", "a"]);
        git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "-m",
            "one",
        ]);
        std::fs::write(repo.path().join("a"), "2").unwrap();
        std::fs::write(repo.path().join("b"), "new").unwrap();

        refresh(home.path(), repo.path(), 5_000).unwrap();

        let saved = cache::load(home.path(), FILE, &repo.path().to_string_lossy()).unwrap();
        assert_eq!(
            saved.value,
            json!({"state": "ok", "files": 2, "capped": false})
        );
        assert_eq!(saved.at, 5_000);
    }
}
