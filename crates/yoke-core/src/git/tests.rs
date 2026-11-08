use super::*;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Every git call here runs inside a fixture directory under the system temp dir.
fn git(dir: &Path, args: &[&str]) -> String {
    assert!(dir.starts_with(std::env::temp_dir().canonicalize().unwrap()));
    let mut command = base_command(dir, false).unwrap();
    let output = command.args(args).output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn configure(dir: &Path) {
    for (key, value) in [
        ("user.name", "Fixture"),
        ("user.email", "fixture@example.invalid"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", "/dev/null"),
    ] {
        git(dir, &["config", key, value]);
    }
}

struct Fixture {
    _temp: TempDir,
    root: PathBuf,
    base: PathBuf,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let root = base.join("work");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    configure(&root);
    fs::write(root.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    fs::write(root.join("b.txt"), "bee\n").unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "initial"]);
    Fixture {
        _temp: temp,
        root,
        base,
    }
}

fn entry<'a>(status: &'a Status, path: &str) -> &'a Entry {
    status
        .entries
        .iter()
        .find(|entry| entry.path == path)
        .unwrap_or_else(|| panic!("no entry for {path}: {status:?}"))
}

#[test]
fn status_and_diffs_describe_each_side_of_a_change() {
    let f = fixture();
    let repo = Repo::open(&f.root).unwrap();
    fs::write(f.root.join("a.txt"), "one\nTWO\nthree\nfour\n").unwrap();
    git(&f.root, &["mv", "b.txt", "d.txt"]);
    fs::write(f.root.join("c.txt"), "cee\n").unwrap();
    git(&f.root, &["add", "c.txt"]);
    fs::write(f.root.join("e.txt"), "eee\n").unwrap();
    fs::write(f.root.join("blob.bin"), [0_u8, 1, 2, 255, 0]).unwrap();

    let status = repo.status().unwrap();
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert_eq!(status.operation, None);
    assert_eq!(entry(&status, "a.txt").unstaged, Some(Kind::Modified));
    assert_eq!(entry(&status, "a.txt").staged, None);
    let renamed = entry(&status, "d.txt");
    assert_eq!(renamed.staged, Some(Kind::Renamed));
    assert_eq!(renamed.orig_path.as_deref(), Some("b.txt"));
    assert_eq!(entry(&status, "c.txt").staged, Some(Kind::Added));
    assert!(entry(&status, "e.txt").untracked);

    let diff = repo.file_diff("a.txt", Side::Unstaged).unwrap();
    assert_eq!((diff.additions, diff.deletions), (2, 1));
    let lines: Vec<_> = diff.hunks[0]
        .lines
        .iter()
        .map(|line| (line.kind, line.old, line.new, line.text.as_str()))
        .collect();
    assert_eq!(
        lines,
        [
            (LineKind::Context, Some(1), Some(1), "one"),
            (LineKind::Remove, Some(2), None, "two"),
            (LineKind::Add, None, Some(2), "TWO"),
            (LineKind::Context, Some(3), Some(3), "three"),
            (LineKind::Add, None, Some(4), "four"),
        ]
    );
    let untracked = repo.file_diff("e.txt", Side::Untracked).unwrap();
    assert_eq!(untracked.hunks[0].lines[0].new, Some(1));
    assert!(repo.file_diff("blob.bin", Side::Untracked).unwrap().binary);
    let moved = repo.file_diff("d.txt", Side::Staged).unwrap();
    assert_eq!(moved.old_path.as_deref(), Some("b.txt"));

    assert!(repo.file_diff("../x", Side::Unstaged).is_err());
    assert!(repo.file_diff("nope.txt", Side::Unstaged).is_err());
    let wrong_side = repo.file_diff("a.txt", Side::Staged).unwrap_err();
    assert!(
        wrong_side.to_string().contains("no staged changes"),
        "{wrong_side:#}"
    );
}

#[test]
fn staging_and_committing_only_touch_listed_paths_and_never_commit_nothing() {
    let f = fixture();
    let repo = Repo::open(&f.root).unwrap();
    fs::write(f.root.join("a.txt"), "changed\n").unwrap();
    git(&f.root, &["mv", "b.txt", "d.txt"]);

    repo.stage(&["a.txt".into()]).unwrap();
    assert_eq!(
        entry(&repo.status().unwrap(), "a.txt").staged,
        Some(Kind::Modified)
    );
    // Unstaging a staged rename must undo both sides, not leave a staged delete behind.
    repo.unstage(&["d.txt".into()]).unwrap();
    let status = repo.status().unwrap();
    assert_eq!(entry(&status, "b.txt").unstaged, Some(Kind::Deleted));
    assert_eq!(entry(&status, "b.txt").staged, None);
    assert!(entry(&status, "d.txt").untracked);

    for bad in ["../outside", "/etc/passwd", "unknown.txt", "a.txt"] {
        assert!(repo.stage(&[bad.into()]).is_err(), "{bad} must be refused");
    }
    assert!(repo.commit("   ", "").is_err());

    repo.stage_all().unwrap();
    let done = repo
        .commit("Rework files", "Because the names were wrong.")
        .unwrap();
    assert!(!done.hash.is_empty());
    assert_eq!(
        git(&f.root, &["log", "-1", "--format=%B"]).trim(),
        "Rework files\n\nBecause the names were wrong."
    );
    let again = repo.commit("Empty", "").unwrap_err();
    assert!(again.to_string().contains("nothing is staged"), "{again:#}");
    assert!(repo.status().unwrap().entries.is_empty());
}

#[test]
fn branches_list_switch_and_create_refuse_unsafe_names() {
    let f = fixture();
    let repo = Repo::open(&f.root).unwrap();
    repo.create_branch("feature/x", None).unwrap();
    assert_eq!(repo.status().unwrap().branch.as_deref(), Some("feature/x"));
    let names: Vec<_> = repo
        .branches()
        .unwrap()
        .into_iter()
        .map(|b| (b.name, b.current))
        .collect();
    assert_eq!(
        names,
        [("feature/x".to_owned(), true), ("main".to_owned(), false)]
    );
    repo.switch("main").unwrap();
    assert_eq!(repo.status().unwrap().branch.as_deref(), Some("main"));
    for bad in ["-x", "has space", "a..b", "main", ""] {
        assert!(
            repo.create_branch(bad, None).is_err(),
            "{bad:?} must be refused"
        );
    }
    assert!(repo
        .create_branch("from-bad-start", Some("no-such-ref"))
        .is_err());
    assert!(repo.switch("nope").is_err());
    assert!(repo.switch("-c").is_err());
}

#[test]
fn push_pull_and_fetch_work_against_a_remote_and_never_force() {
    let f = fixture();
    let repo = Repo::open(&f.root).unwrap();
    let origin = f.base.join("origin.git");
    git(
        &f.base,
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            origin.to_str().unwrap(),
        ],
    );
    assert!(repo.fetch().is_err(), "no remote yet");
    assert!(repo.push_plan().is_err());
    git(
        &f.root,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );

    let plan = repo.push_plan().unwrap();
    assert_eq!(
        (plan.remote.as_str(), plan.set_upstream, plan.ahead),
        ("origin", true, 1)
    );
    assert_eq!(plan.commits.len(), 1);
    repo.push(&plan).unwrap();
    assert_eq!(
        repo.status().unwrap().upstream.as_deref(),
        Some("origin/main")
    );
    assert!(repo
        .push_plan()
        .unwrap_err()
        .to_string()
        .contains("nothing to push"));

    // A second clone advances the remote; fetch shows it as behind, a fast-forward pull catches up.
    let other = f.base.join("other");
    git(
        &f.base,
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    configure(&other);
    fs::write(other.join("remote.txt"), "r\n").unwrap();
    git(&other, &["add", "-A"]);
    git(&other, &["commit", "-qm", "remote work"]);
    git(&other, &["push", "-q"]);
    repo.fetch().unwrap();
    assert_eq!(repo.status().unwrap().behind, 1);
    repo.pull(PullStrategy::FastForwardOnly).unwrap();
    assert_eq!(repo.status().unwrap().behind, 0);

    // Diverge: ff-only refuses; merge refuses on a dirty tree and succeeds on a clean one.
    fs::write(f.root.join("local.txt"), "l\n").unwrap();
    git(&f.root, &["add", "-A"]);
    git(&f.root, &["commit", "-qm", "local work"]);
    fs::write(other.join("remote2.txt"), "r2\n").unwrap();
    git(&other, &["add", "-A"]);
    git(&other, &["commit", "-qm", "more remote work"]);
    git(&other, &["push", "-q"]);
    repo.fetch().unwrap();
    assert!(repo.pull(PullStrategy::FastForwardOnly).is_err());
    fs::write(f.root.join("a.txt"), "dirty\n").unwrap();
    let dirty = repo.pull(PullStrategy::Merge).unwrap_err();
    assert!(
        dirty.to_string().contains("clean working tree"),
        "{dirty:#}"
    );
    fs::write(f.root.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    repo.pull(PullStrategy::Merge).unwrap();
    assert_eq!(repo.status().unwrap().operation, None);

    // The reviewed plan is bound to HEAD: a newer commit invalidates it.
    let reviewed = repo.push_plan().unwrap();
    fs::write(f.root.join("late.txt"), "x\n").unwrap();
    git(&f.root, &["add", "-A"]);
    git(&f.root, &["commit", "-qm", "late"]);
    assert!(repo
        .push(&reviewed)
        .unwrap_err()
        .to_string()
        .contains("changed since"));
    repo.push(&repo.push_plan().unwrap()).unwrap();

    git(
        &f.root,
        &[
            "remote",
            "set-url",
            "origin",
            "https://user:token@example.invalid/x.git",
        ],
    );
    fs::write(f.root.join("more.txt"), "x\n").unwrap();
    git(&f.root, &["add", "-A"]);
    git(&f.root, &["commit", "-qm", "more"]);
    assert_eq!(
        repo.push_plan().unwrap().url,
        "https://example.invalid/x.git"
    );
}

#[test]
fn stashes_are_addressed_by_commit_so_a_shifted_list_cannot_drop_the_wrong_one() {
    let f = fixture();
    let repo = Repo::open(&f.root).unwrap();
    assert_eq!(
        repo.stash_save("", false).unwrap(),
        "There was nothing to stash."
    );
    for (file, message) in [("a.txt", "first"), ("b.txt", "second")] {
        fs::write(f.root.join(file), format!("{message}\n")).unwrap();
        repo.stash_save(message, false).unwrap();
    }
    let stale: Vec<_> = repo.stashes().unwrap();
    assert_eq!(stale.len(), 2);
    assert!(stale[0].message.contains("second") && stale[1].message.contains("first"));
    let first = stale[1].id.clone();

    fs::write(f.root.join("c.txt"), "third\n").unwrap();
    repo.stash_save("third", true).unwrap();
    repo.stash_drop(&first).unwrap();
    let left: Vec<_> = repo
        .stashes()
        .unwrap()
        .into_iter()
        .map(|s| s.message)
        .collect();
    assert!(
        left.len() == 2 && left[0].contains("third") && left[1].contains("second"),
        "{left:?}"
    );

    let second = repo.stashes().unwrap()[1].id.clone();
    repo.stash_apply(&second).unwrap();
    assert_eq!(
        fs::read_to_string(f.root.join("b.txt")).unwrap(),
        "second\n"
    );
    assert_eq!(repo.stashes().unwrap().len(), 2, "apply keeps the stash");
    let third = repo.stashes().unwrap()[0].id.clone();
    repo.stash_pop(&third).unwrap();
    assert_eq!(repo.stashes().unwrap().len(), 1, "pop removes it");
    assert!(repo.stash_drop(&first).is_err(), "already gone");
    assert!(repo.stash_drop("not-a-hash").is_err());
}

#[test]
fn commit_message_drafts_send_only_redacted_non_secret_staged_text_and_parse_strictly() {
    let f = fixture();
    let repo = Repo::open(&f.root).unwrap();
    assert!(repo.staged_for_message().is_err(), "nothing staged");
    fs::write(f.root.join("ok.txt"), "plain content\n").unwrap();
    fs::write(f.root.join(".env"), "TOKEN=hunter2-private-value\n").unwrap();
    repo.stage_all().unwrap();
    let staged = repo.staged_for_message().unwrap();
    assert_eq!(
        (staged.files.as_slice(), staged.skipped.as_slice()),
        (&["ok.txt".to_owned()][..], &[".env".to_owned()][..])
    );
    assert_eq!(staged.digest, repo.staged_for_message().unwrap().digest);

    let capture = f.base.join("sent.txt");
    let ask = |reply: &str| {
        let script = format!("cat > \"$1\"; printf '%s' '{reply}'");
        let mut command = Command::new("sh");
        command.args(["-c", &script, "sh", capture.to_str().unwrap()]);
        repo.draft_commit_message_with(&mut command, &staged, Duration::from_secs(20))
    };
    let draft = ask(r#"{"subject":"Add plain file","body":"It is needed."}"#).unwrap();
    assert_eq!(
        (draft.subject.as_str(), draft.body.as_str()),
        ("Add plain file", "It is needed.")
    );
    let sent = fs::read_to_string(&capture).unwrap();
    assert!(
        sent.contains("ok.txt") && !sent.contains("hunter2") && !sent.contains(".env"),
        "{sent}"
    );
    assert_eq!(
        ask("```json\n{\"subject\":\"Fenced\"}\n```")
            .unwrap()
            .subject,
        "Fenced"
    );
    assert!(ask("here is a message").is_err());
    assert!(ask(r#"{"subject":"x","extra":1}"#).is_err());
    assert!(ask(r#"{"subject":"two\nlines"}"#).is_err());

    fs::write(f.root.join("more.txt"), "more\n").unwrap();
    repo.stage_all().unwrap();
    assert_ne!(staged.digest, repo.staged_for_message().unwrap().digest);
}
