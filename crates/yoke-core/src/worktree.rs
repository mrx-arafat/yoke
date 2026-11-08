//! Plan worktrees: status, diff, squash-merge sync and guarded removal of linked Git worktrees.
//!
//! Every operation fails closed: an unknown or dirty state refuses, nothing is forced, and the
//! caller names the worktree by a path that must appear in `git worktree list` for the repository.
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;

use crate::gitcmd::{run, run_ok};

/// Ignored top-level entries that `git worktree remove` may delete with the worktree. Anything
/// else ignored (an `.env`, local notes) blocks discarding, because Git would delete it silently.
const DISPOSABLE: [&str; 7] = [
    "target",
    "node_modules",
    ".codegraph",
    ".playwright-cli",
    "test-results",
    "__pycache__",
    ".pytest_cache",
];
const LISTED_CHANGES: usize = 200;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub head: String,
    /// None when HEAD is detached.
    pub branch: Option<String>,
    pub locked: bool,
    pub is_main: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Change {
    /// Two-letter `git status --porcelain` code.
    pub code: String,
    pub path: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub target: Worktree,
    pub base: Worktree,
    pub changes: Vec<Change>,
    pub changes_total: usize,
    /// Commits on the branch that the base branch lacks, and the reverse.
    pub ahead: u64,
    pub behind: u64,
    /// The base branch already holds everything the branch changes; None when it cannot be told.
    pub merged: Option<bool>,
    pub conflicts: Option<bool>,
    pub sync_blockers: Vec<String>,
    pub discard_blockers: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiffText {
    /// Branch changes the base branch lacks (`base...HEAD`).
    pub committed: String,
    /// Tracked changes not yet committed in the worktree.
    pub uncommitted: String,
    pub untracked: Vec<String>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Synced {
    pub commit: String,
    pub branch: String,
    pub base_branch: String,
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Every worktree of the repository containing `repo`, main checkout first.
pub fn list(repo: &Path) -> Result<Vec<Worktree>> {
    let out = run_ok(repo, &["worktree", "list", "--porcelain", "-z"])
        .context("listing worktrees needs Git 2.36 or newer")?;
    let mut result: Vec<Worktree> = Vec::new();
    let mut current: Option<Worktree> = None;
    for field in out.split('\0') {
        if field.is_empty() {
            result.extend(current.take());
        } else if let Some(path) = field.strip_prefix("worktree ") {
            result.extend(current.take());
            current = Some(Worktree {
                path: canonical(Path::new(path)),
                head: String::new(),
                branch: None,
                locked: false,
                is_main: result.is_empty(),
            });
        } else if let Some(item) = current.as_mut() {
            if let Some(head) = field.strip_prefix("HEAD ") {
                item.head = head.to_owned();
            } else if let Some(branch) = field.strip_prefix("branch ") {
                item.branch = branch.strip_prefix("refs/heads/").map(str::to_owned);
            } else if field == "locked" || field.starts_with("locked ") {
                item.locked = true;
            } else if field == "bare" {
                bail!("a bare repository has no main checkout to sync into");
            }
        }
    }
    result.extend(current.take());
    ensure!(
        !result.is_empty(),
        "Git listed no worktrees for this repository"
    );
    Ok(result)
}

fn find(repo: &Path, path: &Path) -> Result<(Worktree, Worktree)> {
    let all = list(repo)?;
    let wanted = canonical(path);
    let target = all
        .iter()
        .find(|w| w.path == wanted)
        .cloned()
        .context("that worktree is not part of this repository; reload the page")?;
    Ok((all[0].clone(), target))
}

fn in_progress(dir: &Path) -> Option<&'static str> {
    [
        ("MERGE_HEAD", "merge"),
        ("rebase-merge", "rebase"),
        ("rebase-apply", "rebase"),
        ("CHERRY_PICK_HEAD", "cherry-pick"),
        ("REVERT_HEAD", "revert"),
        ("BISECT_LOG", "bisect"),
    ]
    .into_iter()
    .find(|(name, _)| {
        run_ok(dir, &["rev-parse", "--git-path", name])
            .map(|out| dir.join(out.trim()).exists())
            // Unknown counts as in progress: refuse rather than guess.
            .unwrap_or(true)
    })
    .map(|(_, label)| label)
}

/// Porcelain entries as (code, path); `ignored` selects `!!` entries instead of everything else.
fn status(dir: &Path, ignored: bool) -> Result<Vec<Change>> {
    let mut args = vec!["status", "--porcelain=v1", "-z", "--untracked-files=all"];
    if ignored {
        args.push("--ignored");
    }
    let out = run_ok(dir, &args)?;
    let mut fields = out.split('\0').filter(|f| !f.is_empty());
    let mut changes = Vec::new();
    while let Some(field) = fields.next() {
        let (code, path) = field.split_at(field.len().min(2));
        // One separating space; later spaces belong to the file name.
        let path = path.strip_prefix(' ').unwrap_or(path);
        if code.starts_with('R') || code.starts_with('C') || code.ends_with('R') {
            fields.next(); // the original path of a rename or copy
        }
        if (code == "!!") == ignored {
            changes.push(Change {
                code: code.to_owned(),
                path: path.to_owned(),
            });
        }
    }
    Ok(changes)
}

struct Probe {
    tree: Option<String>,
    conflicts: bool,
}

/// Merge the branch into the base in memory; nothing in either checkout is touched.
fn probe(dir: &Path, base: &str, branch: &str) -> Result<Probe> {
    let out = run(
        dir,
        &[
            "merge-tree",
            "--write-tree",
            "--no-messages",
            &format!("refs/heads/{base}"),
            &format!("refs/heads/{branch}"),
        ],
    )?;
    if out.success {
        return Ok(Probe {
            tree: out.stdout.lines().next().map(str::to_owned),
            conflicts: false,
        });
    }
    // Exit status 1 is a conflicted merge; anything else is a real failure.
    ensure!(
        out.stdout.lines().next().is_some() || out.stderr.is_empty(),
        "git merge-tree failed: {}",
        out.stderr
    );
    Ok(Probe {
        tree: None,
        conflicts: true,
    })
}

fn count(dir: &Path, range: &str) -> Result<u64> {
    run_ok(dir, &["rev-list", "--count", range])?
        .trim()
        .parse()
        .context("counting commits")
}

/// Ignored files that Git would delete with the worktree and that are not disposable build output.
/// `allowed` are worktree-relative paths the caller knows are safe to lose (a plan's own file).
fn ignored_kept(dir: &Path, allowed: &[String]) -> Result<Vec<String>> {
    let mut kept = Vec::new();
    for entry in status(dir, true)? {
        let path = entry.path;
        let top = path.trim_end_matches('/').split('/').next().unwrap_or("");
        if DISPOSABLE.contains(&top) {
            continue;
        }
        if path.ends_with('/') {
            // A collapsed ignored folder: look at the files inside it one by one.
            let listing = run_ok(
                dir,
                &[
                    "ls-files",
                    "-z",
                    "--others",
                    "--ignored",
                    "--exclude-standard",
                    "--",
                    &path,
                ],
            )?;
            let files: Vec<&str> = listing.split('\0').filter(|f| !f.is_empty()).collect();
            if files.len() > 2000 {
                kept.push(path);
            } else {
                kept.extend(
                    files
                        .into_iter()
                        .filter(|f| !allowed.iter().any(|a| a == f))
                        .map(str::to_owned),
                );
            }
        } else if !allowed.contains(&path) {
            kept.push(path);
        }
        if kept.len() >= 5 {
            break;
        }
    }
    kept.truncate(5);
    Ok(kept)
}

/// Everything the console shows and every action re-checks, from one place.
pub fn report(
    repo: &Path,
    path: &Path,
    current: &Path,
    allowed_ignored: &[String],
) -> Result<Report> {
    let (base, target) = find(repo, path)?;
    let all = status(&target.path, false)?;
    let changes_total = all.len();
    let main_changes = status(&base.path, false)?.len();
    let (mut ahead, mut behind, mut merged, mut conflicts) = (0, 0, None, None);
    if let (Some(b), Some(t)) = (&base.branch, &target.branch) {
        ahead = count(&base.path, &format!("refs/heads/{b}..refs/heads/{t}"))?;
        behind = count(&base.path, &format!("refs/heads/{t}..refs/heads/{b}"))?;
        let probed = probe(&base.path, b, t)?;
        conflicts = Some(probed.conflicts);
        merged = Some(match &probed.tree {
            Some(tree) => {
                tree.trim()
                    == run_ok(
                        &base.path,
                        &["rev-parse", &format!("refs/heads/{b}^{{tree}}")],
                    )?
                    .trim()
            }
            None => false,
        });
    }
    let mut sync_blockers = Vec::new();
    let mut discard_blockers = Vec::new();
    if target.is_main {
        let text = "This is the main checkout, not a plan worktree.".to_owned();
        sync_blockers.push(text.clone());
        discard_blockers.push(text);
    }
    match (&base.branch, &target.branch) {
        (Some(b), Some(t)) => {
            if conflicts == Some(true) {
                sync_blockers.push(format!(
                    "Merging {t} into {b} would conflict; bring {b} into {t} first."
                ));
            } else if merged == Some(true) {
                sync_blockers.push(format!(
                    "{b} already holds every change on {t}; there is nothing to sync."
                ));
            }
            if merged != Some(true) {
                discard_blockers.push(format!(
                    "{t} has changes that {b} does not have yet; sync them first."
                ));
            }
        }
        (None, _) => {
            let text =
                "The main checkout is on a detached HEAD; check out its branch first.".to_owned();
            sync_blockers.push(text.clone());
            discard_blockers.push(text);
        }
        (_, None) => {
            sync_blockers.push(
                "The worktree is on a detached HEAD; check out a branch in it first.".to_owned(),
            );
            let held = base.branch.as_ref().is_some_and(|b| {
                run(
                    &base.path,
                    &[
                        "merge-base",
                        "--is-ancestor",
                        &target.head,
                        &format!("refs/heads/{b}"),
                    ],
                )
                .is_ok_and(|out| out.success)
            });
            if !held {
                discard_blockers.push(
                    "The worktree's detached HEAD has commits that no branch holds.".to_owned(),
                );
            }
        }
    }
    if changes_total > 0 {
        let text = format!(
            "The worktree has {changes_total} uncommitted or untracked change(s); commit or remove them first."
        );
        sync_blockers.push(text.clone());
        discard_blockers.push(text);
    }
    if main_changes > 0 {
        sync_blockers.push(format!(
            "The main checkout has {main_changes} uncommitted or untracked change(s); commit or stash them first."
        ));
    }
    for (label, dir, discards) in [
        ("The worktree", &target.path, true),
        ("The main checkout", &base.path, false),
    ] {
        if let Some(operation) = in_progress(dir) {
            let text =
                format!("{label} is in the middle of a {operation}; finish or abort it first.");
            sync_blockers.push(text.clone());
            if discards {
                discard_blockers.push(text);
            }
        }
    }
    if target.locked {
        discard_blockers
            .push("The worktree is locked; unlock it with git worktree unlock first.".to_owned());
    }
    if target.path == canonical(current) {
        discard_blockers.push(
            "The console is running from this worktree; open the console from the main checkout to discard it.".to_owned(),
        );
    }
    let kept = ignored_kept(&target.path, allowed_ignored)?;
    if !kept.is_empty() {
        discard_blockers.push(format!(
            "Git would delete ignored files with the worktree: {}. Move what you need out, or remove it, first.",
            kept.join(", ")
        ));
    }
    Ok(Report {
        target,
        base,
        changes: all.into_iter().take(LISTED_CHANGES).collect(),
        changes_total,
        ahead,
        behind,
        merged,
        conflicts,
        sync_blockers,
        discard_blockers,
    })
}

/// What the branch changes against its base, plus the worktree's own uncommitted work.
pub fn diff(repo: &Path, path: &Path) -> Result<DiffText> {
    let (base, target) = find(repo, path)?;
    let flags = ["diff", "--no-color", "--no-ext-diff", "--no-textconv"];
    let mut truncated = false;
    let committed = match (&base.branch, &target.branch) {
        (Some(b), Some(t)) => {
            let range = format!("refs/heads/{b}...refs/heads/{t}");
            let mut args = flags.to_vec();
            args.push(&range);
            let out = run(&target.path, &args)?;
            ensure!(out.success, "git diff failed: {}", out.stderr);
            truncated |= out.truncated;
            out.stdout
        }
        _ => String::new(),
    };
    let mut args = flags.to_vec();
    args.push("HEAD");
    let out = run(&target.path, &args)?;
    ensure!(out.success, "git diff failed: {}", out.stderr);
    truncated |= out.truncated;
    let untracked = status(&target.path, false)?
        .into_iter()
        .filter(|c| c.code == "??")
        .map(|c| c.path)
        .take(LISTED_CHANGES)
        .collect();
    Ok(DiffText {
        committed,
        uncommitted: out.stdout,
        untracked,
        truncated,
    })
}

fn refuse(blockers: &[String]) -> Result<()> {
    ensure!(blockers.is_empty(), "{}", blockers.join(" "));
    Ok(())
}

/// Squash-merge the worktree's branch into the main checkout's branch and commit it there.
/// `expected_head` is the worktree HEAD the human reviewed. Repository hooks never run.
pub fn sync(
    repo: &Path,
    path: &Path,
    expected_head: &str,
    message: &str,
    current: &Path,
) -> Result<Synced> {
    ensure!(
        !message.trim().is_empty() && message.len() <= 4000 && !message.contains('\0'),
        "give a commit message of up to 4000 characters"
    );
    let report = report(repo, path, current, &[])?;
    ensure!(
        report.target.head == expected_head,
        "the worktree changed since you opened it; reload the page and review it again"
    );
    refuse(&report.sync_blockers)?;
    let (Some(branch), Some(base_branch)) =
        (report.target.branch.clone(), report.base.branch.clone())
    else {
        bail!("both checkouts need a branch");
    };
    let squash = run(
        &report.base.path,
        &["merge", "--squash", &format!("refs/heads/{branch}")],
    )?;
    ensure!(
        squash.success,
        "the squash merge failed ({}); inspect git status in {} before doing anything else",
        squash.stderr,
        report.base.path.display()
    );
    let commit = run(&report.base.path, &["commit", "-m", message])?;
    ensure!(
        commit.success,
        "the squashed changes are staged in {} but were not committed ({}); commit them there yourself, or run git reset --merge to drop them",
        report.base.path.display(),
        commit.stderr
    );
    let commit = run_ok(&report.base.path, &["rev-parse", "HEAD"])?
        .trim()
        .to_owned();
    Ok(Synced {
        commit,
        branch,
        base_branch,
    })
}

/// Remove a worktree that holds no unique work, with Git's own non-forcing removal. The branch is kept.
pub fn discard(
    repo: &Path,
    path: &Path,
    expected_head: &str,
    current: &Path,
    allowed_ignored: &[String],
) -> Result<()> {
    let report = report(repo, path, current, allowed_ignored)?;
    ensure!(
        report.target.head == expected_head,
        "the worktree changed since you opened it; reload the page and review it again"
    );
    refuse(&report.discard_blockers)?;
    let target = report.target.path.to_string_lossy().into_owned();
    run_ok(&report.base.path, &["worktree", "remove", "--", &target])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Repo {
        _temp: tempfile::TempDir,
        main: PathBuf,
        linked: PathBuf,
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = run(dir, args).unwrap();
        assert!(out.success, "git {args:?}: {}", out.stderr);
        out.stdout
    }

    fn commit(dir: &Path, file: &str, text: &str) {
        std::fs::write(dir.join(file), text).unwrap();
        git(dir, &["add", file]);
        git(dir, &["commit", "-qm", file]);
    }

    /// A repository on `main` with a linked worktree on `feature`, one commit ahead.
    fn repo() -> Repo {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let main = root.join("main");
        std::fs::create_dir(&main).unwrap();
        git(&main, &["init", "-q", "-b", "main"]);
        git(&main, &["config", "user.name", "Fixture"]);
        git(&main, &["config", "user.email", "fixture@example.invalid"]);
        commit(&main, "base.txt", "base\n");
        let linked = root.join("feature");
        git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature",
                linked.to_str().unwrap(),
            ],
        );
        commit(&linked, "feature.txt", "feature\n");
        Repo {
            _temp: temp,
            main,
            linked,
        }
    }

    #[test]
    fn sync_squashes_the_branch_into_main_and_refuses_unsafe_states() {
        let r = repo();
        let head = git(&r.linked, &["rev-parse", "HEAD"]).trim().to_owned();
        let seen = report(&r.main, &r.linked, &r.main, &[]).unwrap();
        assert_eq!((seen.ahead, seen.behind, seen.merged), (1, 0, Some(false)));
        assert!(seen.sync_blockers.is_empty(), "{:?}", seen.sync_blockers);
        assert!(diff(&r.main, &r.linked)
            .unwrap()
            .committed
            .contains("+feature"));

        // Unsafe states refuse and leave both checkouts alone.
        std::fs::write(r.main.join("loose.txt"), "x").unwrap();
        let err = sync(&r.main, &r.linked, &head, "msg", &r.main).unwrap_err();
        assert!(err.to_string().contains("main checkout has 1"), "{err:#}");
        std::fs::remove_file(r.main.join("loose.txt")).unwrap();
        let err = sync(&r.main, &r.linked, "0000", "msg", &r.main).unwrap_err();
        assert!(err.to_string().contains("changed since"), "{err:#}");
        assert!(!r.main.join("feature.txt").exists());

        let done = sync(&r.main, &r.linked, &head, "Squash feature", &r.main).unwrap();
        assert_eq!(
            (done.branch.as_str(), done.base_branch.as_str()),
            ("feature", "main")
        );
        assert_eq!(
            std::fs::read_to_string(r.main.join("feature.txt")).unwrap(),
            "feature\n"
        );
        assert_eq!(
            git(&r.main, &["log", "-1", "--format=%s"]).trim(),
            "Squash feature"
        );
        assert_eq!(
            git(&r.main, &["rev-list", "--count", "HEAD"]).trim(),
            "2",
            "one squash commit"
        );
        // The branch is now contained, so syncing again has nothing to do.
        let again = report(&r.main, &r.linked, &r.main, &[]).unwrap();
        assert_eq!(again.merged, Some(true));
        assert!(again
            .sync_blockers
            .iter()
            .any(|b| b.contains("nothing to sync")));
    }

    #[test]
    fn conflicting_branches_do_not_sync_and_discard_never_loses_work() {
        let r = repo();
        let head = git(&r.linked, &["rev-parse", "HEAD"]).trim().to_owned();
        // Unmerged commits block discarding.
        let err = discard(&r.main, &r.linked, &head, &r.main, &[]).unwrap_err();
        assert!(err.to_string().contains("does not have yet"), "{err:#}");
        // Both sides change the same line: sync refuses before touching main.
        commit(&r.main, "feature.txt", "main version\n");
        let err = sync(&r.main, &r.linked, &head, "msg", &r.main).unwrap_err();
        assert!(err.to_string().contains("would conflict"), "{err:#}");
        assert_eq!(git(&r.main, &["status", "--porcelain"]), "");
        // Remove the conflict by taking main's change into the branch's history-free equivalent.
        git(&r.main, &["reset", "-q", "--hard", "HEAD~1"]);
        let done = sync(&r.main, &r.linked, &head, "Squash", &r.main).unwrap();
        assert!(!done.commit.is_empty());

        // Dirty, untracked, ignored and self-hosting states each refuse.
        std::fs::write(r.linked.join("scratch.txt"), "work").unwrap();
        let err = discard(&r.main, &r.linked, &head, &r.main, &[]).unwrap_err();
        assert!(err.to_string().contains("untracked change"), "{err:#}");
        std::fs::remove_file(r.linked.join("scratch.txt")).unwrap();
        std::fs::write(r.linked.join(".gitignore"), ".env\ntarget/\n").unwrap();
        git(&r.linked, &["add", ".gitignore"]);
        git(&r.linked, &["commit", "-qm", "ignore"]);
        let head = git(&r.linked, &["rev-parse", "HEAD"]).trim().to_owned();
        // The ignore-file commit is itself unmerged work.
        assert!(discard(&r.main, &r.linked, &head, &r.main, &[]).is_err());
        sync(&r.main, &r.linked, &head, "Squash ignore", &r.main).unwrap();
        std::fs::write(r.linked.join(".env"), "SECRET=1").unwrap();
        let err = discard(&r.main, &r.linked, &head, &r.main, &[]).unwrap_err();
        assert!(err.to_string().contains(".env"), "{err:#}");
        std::fs::remove_file(r.linked.join(".env")).unwrap();
        let err = discard(&r.main, &r.linked, &head, &r.linked, &[]).unwrap_err();
        assert!(
            err.to_string().contains("running from this worktree"),
            "{err:#}"
        );

        // Disposable build output does not block; Git removes the clean, merged worktree.
        std::fs::create_dir(r.linked.join("target")).unwrap();
        std::fs::write(r.linked.join("target/out"), "x").unwrap();
        discard(&r.main, &r.linked, &head, &r.main, &[]).unwrap();
        assert!(!r.linked.exists());
        assert!(
            git(&r.main, &["branch", "--list", "feature"]).contains("feature"),
            "branch kept"
        );
    }
}
