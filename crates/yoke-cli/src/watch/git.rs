//! Branch and changed-file count of the project's repository, read-only and bounded in time.
//! The branch comes from `HEAD` on disk; only the count runs git, limited to the project tree.

use std::fs;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use yoke_core::engine::execute_bounded;

/// Bytes of git status output read at most; a larger listing is a huge change set, so the count
/// is skipped rather than parsed.
const STATUS_CAP: usize = 4 * 1024 * 1024;

fn read_capped(path: &Path, cap: u64) -> Option<String> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut text = String::new();
    file.take(cap).read_to_string(&mut text).ok()?;
    Some(text)
}

/// The git directory of the repository that contains `root`, following the `gitdir:` file a
/// linked worktree or a submodule has in place of a `.git` directory.
fn git_dir(root: &Path) -> Option<PathBuf> {
    for directory in root.ancestors() {
        let dot = directory.join(".git");
        match fs::symlink_metadata(&dot) {
            Ok(meta) if meta.is_dir() => return Some(dot),
            Ok(meta) if meta.is_file() => {
                let text = read_capped(&dot, 4096)?;
                let target = Path::new(text.lines().next()?.strip_prefix("gitdir:")?.trim());
                return Some(if target.is_absolute() {
                    target.to_path_buf()
                } else {
                    directory.join(target)
                });
            }
            _ => {}
        }
    }
    None
}

/// `main`, or `detached 1a2b3c4` for a detached HEAD; `None` outside a repository.
pub(crate) fn branch(root: &Path) -> Option<String> {
    let head = read_capped(&git_dir(root)?.join("HEAD"), 1024)?;
    let head = head.trim();
    let name = if let Some(reference) = head.strip_prefix("ref:") {
        let reference = reference.trim();
        reference
            .strip_prefix("refs/heads/")
            .unwrap_or(reference)
            .to_owned()
    } else if head.len() >= 7 && head.bytes().all(|b| b.is_ascii_hexdigit()) {
        format!("detached {}", &head[..7])
    } else {
        return None;
    };
    let name: String = name.chars().filter(|c| !c.is_control()).take(64).collect();
    (!name.is_empty()).then_some(name)
}

/// Files with uncommitted changes, untracked ones included, under `root` only. `None` when
/// `root` is not in a repository, or git is missing, fails, or runs past `timeout`.
pub(crate) fn changed_files(root: &Path, timeout: Duration) -> Option<usize> {
    git_dir(root)?;
    let output = execute_bounded(
        yoke_core::process::git()
            .ok()?
            .arg("-C")
            .arg(root)
            // Never take the index lock: a watcher must not race the user's own git commands.
            .args(["--no-optional-locks", "status", "--porcelain=v1", "-z"])
            .args(["--untracked-files=normal", "--", "."])
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE"),
        b"",
        timeout,
        STATUS_CAP,
    )
    .ok()?;
    if !output.success || output.timed_out || output.stdout_truncated {
        return None;
    }
    Some(count_entries(&output.stdout))
}

/// Entries of `git status --porcelain=v1 -z`; a rename or copy carries its source path as an
/// extra field that is not a separate entry.
fn count_entries(stdout: &[u8]) -> usize {
    let mut fields = stdout.split(|b| *b == 0).filter(|f| !f.is_empty());
    let mut count = 0;
    while let Some(field) = fields.next() {
        count += 1;
        if matches!(field.first(), Some(b'R' | b'C')) {
            fields.next();
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_metadata_links_are_not_read() {
        let repo = tempfile::tempdir().unwrap();
        fs::create_dir(repo.path().join(".git")).unwrap();
        let target = repo.path().join("target");
        fs::write(&target, "ref: refs/heads/forged\n").unwrap();
        std::os::unix::fs::symlink(&target, repo.path().join(".git/HEAD")).unwrap();
        assert_eq!(branch(repo.path()), None);
        assert_eq!(read_capped(Path::new("/dev/null"), 32), None);
    }

    #[test]
    fn reads_branch_detached_head_and_worktree_gitdir_files() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::create_dir_all(repo.join("sub/dir")).unwrap();
        fs::write(repo.join(".git/HEAD"), "ref: refs/heads/feature/x\n").unwrap();
        assert_eq!(branch(&repo.join("sub/dir")).as_deref(), Some("feature/x"));

        fs::write(
            repo.join(".git/HEAD"),
            "0123456789abcdef0123456789abcdef01234567\n",
        )
        .unwrap();
        assert_eq!(branch(&repo).as_deref(), Some("detached 0123456"));

        // A linked worktree has a `.git` file that points at its own git directory.
        let linked = root.path().join("linked");
        let worktree_git = repo.join(".git/worktrees/linked");
        fs::create_dir_all(&worktree_git).unwrap();
        fs::create_dir_all(&linked).unwrap();
        fs::write(worktree_git.join("HEAD"), "ref: refs/heads/wt\n").unwrap();
        fs::write(
            linked.join(".git"),
            format!("gitdir: {}\n", worktree_git.display()),
        )
        .unwrap();
        assert_eq!(branch(&linked).as_deref(), Some("wt"));

        assert_eq!(branch(&root.path().join("elsewhere")), None);
    }

    #[test]
    fn counts_status_entries_with_rename_sources_once() {
        assert_eq!(count_entries(b""), 0);
        assert_eq!(count_entries(b" M a.rs\0?? new.txt\0"), 2);
        assert_eq!(count_entries(b"R  new.rs\0old.rs\0 M b.rs\0"), 2);
    }
}
