use super::super::actions::Outcome;
use super::super::{plan_detail, Console};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use yoke_core::{plan, store::Store};

/// Git writes are confined to the temporary fixture directories.
fn git(dir: &Path, args: &[&str]) {
    assert!(dir.starts_with(std::env::temp_dir().canonicalize().unwrap()));
    let output = Command::new("git")
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgSign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

struct Fixture {
    _temp: tempfile::TempDir,
    base: PathBuf,
    c: Console,
    store: Store,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let project = base.join("repo");
    std::fs::create_dir(&project).unwrap();
    git(&project, &["init", "-q", "-b", "main"]);
    // The console's own commit runs plain `git commit`; CI homes have no global identity.
    git(&project, &["config", "user.name", "Fixture"]);
    git(
        &project,
        &["config", "user.email", "fixture@example.invalid"],
    );
    std::fs::write(project.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    git(&project, &["add", "-A"]);
    git(&project, &["commit", "-qm", "initial"]);
    let database = base.join("state.db");
    let c = Console {
        home: base.clone(),
        project,
        database: database.clone(),
        token: "token".into(),
        csrf: "csrf".into(),
        host: "127.0.0.1:1".into(),
    };
    Fixture {
        _temp: temp,
        base,
        c,
        store: Store::open(&database).unwrap(),
    }
}

fn act(f: &Fixture, name: &str, params: Value, confirmed: bool) -> anyhow::Result<Outcome> {
    super::act(&f.store, &f.c, name, &params, confirmed)
}

fn page(f: &Fixture, query: &[(&str, &str)]) -> String {
    let params: BTreeMap<String, String> = query
        .iter()
        .map(|(k, v)| ((*k).into(), (*v).into()))
        .collect();
    super::page(&f.store, &f.c, &params).unwrap()
}

#[test]
fn page_groups_changes_renders_escaped_diffs_and_actions_stage_and_commit() {
    let f = fixture();
    std::fs::write(f.c.project.join("a.txt"), "one\n<b>two</b>\nthree\n").unwrap();
    std::fs::create_dir(f.c.project.join("src")).unwrap();
    std::fs::write(f.c.project.join("src/new.rs"), "fn main() {}\n").unwrap();

    let html = page(&f, &[]);
    assert!(html.contains(r#"data-page="changes""#) && html.contains("Changes <span"));
    assert!(html.contains("Untracked") && html.contains("src/new.rs"));
    assert!(html.contains("diff-unified") && html.contains("diff-split"));
    assert!(html.contains("&lt;b&gt;two&lt;/b&gt;") && !html.contains("<b>two</b>"));
    assert!(html.contains(r#"<li class="dir"><details open><summary>src/</summary>"#));
    let untracked = page(&f, &[("file", "src/new.rs"), ("side", "untracked")]);
    assert!(untracked.contains("fn main() {}") && untracked.contains("untracked"));

    act(
        &f,
        "changes.stage",
        json!({"paths": ["a.txt", "src/new.rs"]}),
        false,
    )
    .unwrap();
    assert!(page(&f, &[]).contains("Commit 2 staged file(s)"));
    let outcome = act(
        &f,
        "changes.commit",
        json!({"subject": "Edit and add", "body": "More."}),
        false,
    )
    .unwrap();
    assert!(
        matches!(outcome, Outcome::Done(ref v) if v["message"].as_str().unwrap().contains("Edit and add"))
    );
    assert!(page(&f, &[]).contains("Working tree is clean"));
    assert!(act(&f, "changes.stage", json!({"paths": ["../outside"]}), false).is_err());
    assert!(act(&f, "changes.unknown", json!({}), false).is_err());
}

#[test]
fn push_and_stash_drop_wait_for_confirmation_and_stale_pages_are_refused() {
    let f = fixture();
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
    git(
        &f.c.project,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    let head = yoke_core::git::Repo::open(&f.c.project)
        .unwrap()
        .status()
        .unwrap()
        .head
        .unwrap();

    let stale = act(
        &f,
        "changes.push",
        json!({"branch": "main", "head": "0000000"}),
        false,
    );
    assert!(stale
        .err()
        .expect("a stale page is refused")
        .to_string()
        .contains("changed since this page loaded"));
    let params = json!({"branch": "main", "head": head});
    match act(&f, "changes.push", params.clone(), false).unwrap() {
        Outcome::Confirm(message) => {
            assert!(message.contains("origin") && message.contains("set it as the upstream"))
        }
        Outcome::Done(_) => panic!("a push must ask first"),
    }
    assert!(git_out(&origin, &["rev-parse", "--verify", "-q", "main"]).is_empty());
    assert!(matches!(
        act(&f, "changes.push", params, true).unwrap(),
        Outcome::Done(_)
    ));
    assert!(!git_out(&origin, &["rev-parse", "main"]).is_empty());

    std::fs::write(f.c.project.join("a.txt"), "changed\n").unwrap();
    act(&f, "changes.stash-save", json!({"message": "wip"}), false).unwrap();
    let id = yoke_core::git::Repo::open(&f.c.project)
        .unwrap()
        .stashes()
        .unwrap()[0]
        .id
        .clone();
    assert!(page(&f, &[]).contains("wip"));
    let drop = json!({"id": id});
    assert!(matches!(
        act(&f, "changes.stash-drop", drop.clone(), false).unwrap(),
        Outcome::Confirm(_)
    ));
    assert_eq!(
        yoke_core::git::Repo::open(&f.c.project)
            .unwrap()
            .stashes()
            .unwrap()
            .len(),
        1
    );
    act(&f, "changes.stash-drop", drop, true).unwrap();
    assert!(yoke_core::git::Repo::open(&f.c.project)
        .unwrap()
        .stashes()
        .unwrap()
        .is_empty());
    assert!(act(
        &f,
        "changes.draft-preview",
        json!({"agent": "not-a-reviewer"}),
        false
    )
    .is_err());
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[test]
fn notes_become_plan_comments_the_agent_reads_and_export_to_markdown() {
    let f = fixture();
    let record = plan::create(&f.store, &f.c.project, ".yoke/plans", "feature", "Feature").unwrap();
    std::fs::write(
        &record.file,
        "+++\ntitle='Feature'\n[[check]]\nname='unit'\nrun='true'\n+++\n\n# Goal\n\nShip it.\n\n## Risks\n\nNone.\n",
    )
    .unwrap();
    std::fs::write(f.c.project.join("a.txt"), "one\nTWO\nthree\n").unwrap();

    let html = page(&f, &[]);
    assert!(html.contains(r#"data-note-plan="feature""#));
    assert!(html.contains(r#"data-note-anchor="file:a.txt:2""#));
    assert!(
        !html.contains(r#"data-note-anchor="file:a.txt:4""#),
        "only lines in the diff offer a note"
    );

    act(
        &f,
        "note.add",
        json!({"slug": "feature", "anchor": "file:a.txt:2", "text": "Why <TWO>?"}),
        false,
    )
    .unwrap();
    act(
        &f,
        "note.add",
        json!({"slug": "feature", "anchor": "heading:risks", "text": "Quantify"}),
        false,
    )
    .unwrap();
    for bad in [
        json!({"slug": "feature", "anchor": "file:a.txt:99", "text": "x"}),
        json!({"slug": "feature", "anchor": "task:none", "text": "x"}),
        json!({"slug": "feature", "anchor": "plan", "text": "  "}),
    ] {
        assert!(act(&f, "note.add", bad, false).is_err());
    }
    let html = page(&f, &[]);
    assert!(html.contains("note-inline") && html.contains("Why &lt;TWO&gt;?"));

    let detail = plan_detail::page(&f.store, &f.c, "feature").unwrap();
    assert!(detail.contains("data-notes-root") && detail.contains("Export for review"));
    assert!(
        detail.contains(r#" data-anchor="heading:risks">Risks"#),
        "{detail}"
    );
    assert!(detail.contains(r#"data-note-anchor="plan""#));

    let Outcome::Done(data) = act(&f, "note.export", json!({"slug": "feature"}), false).unwrap()
    else {
        panic!("export is not a confirmation");
    };
    let file = &data["download"];
    assert_eq!(file["filename"], "feature-review.md");
    let markdown = file["content"].as_str().unwrap();
    assert!(markdown.contains("`file:a.txt:2` - open") && markdown.contains("> Quantify"));
    assert!(markdown.contains("# Goal") && markdown.contains("Notes: 2 open of 2."));
}
