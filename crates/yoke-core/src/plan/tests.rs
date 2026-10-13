use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::*;
use crate::store::DecisionRecord;

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    store: Store,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        fs::create_dir(&root).unwrap();
        git(&root, &["init", "-q"]);
        fs::write(root.join("tracked.txt"), "original\n").unwrap();
        fs::write(root.join(".gitignore"), "output/\n").unwrap();
        git(&root, &["add", "tracked.txt", ".gitignore"]);
        git(&root, &["commit", "-qm", "fixture seed"]);
        let store = Store::open(temp.path().join("state.db")).unwrap();
        store
            .record_decision(
                &DecisionRecord {
                    ts: 1,
                    session_id: "fixture-session".into(),
                    agent: "claude".into(),
                    project: root.canonicalize().unwrap().to_str().unwrap().into(),
                    event: "session_start".into(),
                    tool: None,
                    mode: "observe".into(),
                    outcome: "allow".into(),
                    reason: None,
                    gates: serde_json::json!([]),
                    payload: "{}".into(),
                },
                4096,
            )
            .unwrap();
        Self {
            _temp: temp,
            root,
            store,
        }
    }

    fn plan(&self, header: &str) -> PlanRecord {
        let plan = create(&self.store, &self.root, ".yoke/plans", "example", "Example").unwrap();
        fs::write(
            &plan.file,
            format!("+++\n{header}\n+++\n\nGoal: state remains database owned.\n"),
        )
        .unwrap();
        plan
    }

    fn building(&self, header: &str) -> PlanRecord {
        self.plan(header);
        let prepared = prepare_approval(&self.store, &self.root, "example").unwrap();
        approve(&self.store, &prepared).unwrap();
        start(&self.store, &self.root, "example", "fixture-session").unwrap()
    }
}

/// Git writes are confined to the newly created temporary fixture repository.
fn git(root: &Path, args: &[&str]) {
    assert!(root.starts_with(std::env::temp_dir()));
    let output = Command::new("git")
        .current_dir(root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .args([
            "-c",
            "user.name=Yoke Fixture",
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

#[test]
fn paths_and_headers_cannot_invent_state_or_escape_project() {
    let f = Fixture::new();
    for slug in ["../escape", "/escape", "a/b", ".", "UPPER", "-bad"] {
        assert!(create(&f.store, &f.root, ".yoke/plans", slug, "Example").is_err());
    }
    let outside = f._temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, f.root.join("escape")).unwrap();
    assert!(create(&f.store, &f.root, "escape/plans", "safe", "Example").is_err());
    assert!(!outside.join("plans").exists());
    let sample = "+++\ntitle = 'Rate-limit the login endpoint'\nreview = true\n[[check]]\nname = 'tests'\nrun = 'cargo test -p api'\n[[check]]\nname = 'ui-lint'\nrun = 'npx @axe-core/cli http://localhost:3000/login'\nblocking = false\n+++\n";
    let (_, header) = files::parse(sample).unwrap();
    assert_eq!(header.checks[0].command, "cargo test -p api");
    assert!(!header.checks[1].blocking);
    for header in [
        "title='X'\nstate='done'",
        "title='X'",
        "title='X'\n[[check]]\nname='advice'\nrun='true'\nblocking=false",
    ] {
        assert!(files::parse(&format!("+++\n{header}\n+++\n")).is_err());
    }
    let plan = f.plan("title='Example'\n[[check]]\nname='proof'\nrun='true'");
    assert_eq!(plan.state, "draft");
    assert!(check(&f.store, &f.root, "example", None).is_err());
    assert!(create(&f.store, &f.root, ".yoke/plans", "example", "overwrite").is_err());
}

#[test]
fn approval_is_bound_to_displayed_header_and_database_revision() {
    let f = Fixture::new();
    let plan = f.plan("title='Example'\n[[check]]\nname='proof'\nrun='true'");
    let prepared = prepare_approval(&f.store, &f.root, "example").unwrap();
    fs::write(
        &plan.file,
        "+++\ntitle='Changed'\n[[check]]\nname='proof'\nrun='false'\n+++\n",
    )
    .unwrap();
    assert!(approve(&f.store, &prepared).is_err());
    let prepared = prepare_approval(&f.store, &f.root, "example").unwrap();
    assert_eq!(approve(&f.store, &prepared).unwrap().state, "approved");
    assert!(approve(&f.store, &prepared).is_err());
    assert!(start(&f.store, &f.root, "example", "missing-session").is_err());
    assert_eq!(get(&f.store, &f.root, "example").unwrap().state, "approved");
    start(&f.store, &f.root, "example", "fixture-session").unwrap();
    let before = fs::read_to_string(&plan.file).unwrap();
    fs::write(&plan.file, before.replace("run='false'", "run='true'")).unwrap();
    assert!(check(&f.store, &f.root, "example", None).is_err());
    assert!(detail(&f.store, &f.root, "example")
        .unwrap()
        .checks
        .is_empty());
}

#[test]
fn real_checks_drive_state_and_preserve_advisory_failure_evidence() {
    let f = Fixture::new();
    f.building("title='Example'\n[[check]]\nname='proof'\nrun='test -f tracked.txt && printf verified'\n[[check]]\nname='advice'\nrun='printf advisory >&2; exit 7'\nblocking=false");
    assert_eq!(
        active(&f.store, &f.root, "fixture-session")
            .unwrap()
            .unwrap()
            .slug,
        "example"
    );
    let transition_facts: i64 = f
        .store
        .connection()
        .query_row(
            "SELECT count(*) FROM facts WHERE kind='plan_transition'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(transition_facts, 3);
    let start_session: String = f
        .store
        .connection()
        .query_row(
            "SELECT session_id FROM facts WHERE kind='plan_transition' ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(start_session, "fixture-session");
    f.store.connection().execute_batch("CREATE TEMP TRIGGER reject_transition_fact BEFORE INSERT ON facts WHEN NEW.kind='plan_transition' BEGIN SELECT RAISE(ABORT,'fixture fact failure'); END;").unwrap();
    assert!(ask(&f.store, &f.root, "example", "must roll back").is_err());
    assert_eq!(get(&f.store, &f.root, "example").unwrap().state, "building");
    assert_eq!(
        detail(&f.store, &f.root, "example")
            .unwrap()
            .transitions
            .len(),
        3
    );
    f.store
        .connection()
        .execute_batch("DROP TRIGGER reject_transition_fact")
        .unwrap();
    assert_eq!(
        ask(
            &f.store,
            &f.root,
            "example",
            "Choose behavior password=fixture-secret"
        )
        .unwrap()
        .state,
        "waiting"
    );
    assert!(check(&f.store, &f.root, "example", None).is_err());
    assert_eq!(
        resume(&f.store, &f.root, "example").unwrap().state,
        "building"
    );
    let summary = check(&f.store, &f.root, "example", None).unwrap();
    assert!(summary.passed);
    assert_eq!(summary.plan.state, "done");
    assert_eq!(summary.checks[0].exit_code, Some(0));
    assert_eq!(summary.checks[0].output_tail, "verified");
    assert_eq!(summary.checks[1].exit_code, Some(7));
    assert_eq!(summary.fingerprint_before, summary.fingerprint_after);
    let detail = detail(&f.store, &f.root, "example").unwrap();
    assert_eq!(detail.checks.len(), 2);
    assert_eq!(detail.transitions.last().unwrap().actor, "yoke");
    assert!(detail
        .transitions
        .iter()
        .any(|transition| transition.from_state == "waiting"
            && transition.to_state == "building"
            && transition.actor == "human"));
    let facts: Vec<(String, String, String)> = f
        .store
        .connection()
        .prepare("SELECT kind,session_id,detail FROM facts ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        facts
            .iter()
            .filter(|(kind, _, _)| kind == "check_result")
            .count(),
        2
    );
    assert!(facts
        .iter()
        .filter(|(kind, _, _)| kind == "check_result")
        .all(|(_, session, _)| session == "fixture-session"));
    assert!(!facts
        .iter()
        .any(|(_, _, detail)| detail.contains("fixture-secret")));
    assert!(facts[0].1.is_empty());
}

#[test]
fn mid_run_edits_and_missing_review_never_mark_done() {
    let f = Fixture::new();
    f.building("title='Example'\n[[check]]\nname='mutation'\nrun='printf modified >> tracked.txt'");
    let summary = check(&f.store, &f.root, "example", None).unwrap();
    assert!(!summary.passed);
    assert_eq!(summary.plan.state, "building");
    assert_ne!(summary.fingerprint_before, summary.fingerprint_after);
    assert!(summary
        .checks
        .last()
        .unwrap()
        .output_tail
        .contains("working tree changed"));
    let f = Fixture::new();
    f.building("title='Example'\nreview=true\n[[check]]\nname='proof'\nrun='true'");
    let summary = check(&f.store, &f.root, "example", None).unwrap();
    assert!(!summary.passed);
    assert_eq!(summary.plan.state, "building");
    assert_eq!(summary.checks.last().unwrap().name, "review");
    assert!(summary
        .checks
        .last()
        .unwrap()
        .output_tail
        .contains("independent review is required"));
    let report = crate::review::ReviewReport {
        findings: vec![],
        passed: false,
        error: Some("malformed reviewer response".into()),
        exit_code: Some(2),
        output_tail: "invalid output".into(),
        ms: 4,
    };
    let before = fingerprint(&f.root).unwrap();
    assert!(
        !record_review_with_fingerprint(&f.store, &f.root, "example", &report, &before).unwrap()
    );
    let detail = detail(&f.store, &f.root, "example").unwrap();
    assert!(detail
        .checks
        .last()
        .unwrap()
        .output_tail
        .contains("malformed reviewer response"));
    assert_eq!(detail.checks.last().unwrap().result, "fail");
    let passing = crate::review::ReviewReport {
        passed: true,
        error: None,
        exit_code: Some(0),
        findings: vec![],
        output_tail: "invalid output".into(),
        ms: 4,
    };
    assert!(
        record_review_with_fingerprint(&f.store, &f.root, "example", &passing, &before).unwrap()
    );
    fs::write(f.root.join("tracked.txt"), "edited during review").unwrap();
    assert!(
        !record_review_with_fingerprint(&f.store, &f.root, "example", &passing, &before).unwrap()
    );
    let evidence = super::detail(&f.store, &f.root, "example").unwrap();
    let run = evidence.checks.last().unwrap();
    assert_eq!(run.result, "void");
    assert_eq!(run.fingerprint, before);
    assert!(run
        .output_tail
        .contains("working tree changed during review"));
    record_review(&f.store, &f.root, "example", &passing).unwrap();
    assert_eq!(
        super::detail(&f.store, &f.root, "example")
            .unwrap()
            .checks
            .last()
            .unwrap()
            .result,
        "void"
    );
    fs::rename(f.root.join(".git"), f._temp.path().join("removed-git")).unwrap();
    assert!(
        !record_review_with_fingerprint(&f.store, &f.root, "example", &passing, &before).unwrap()
    );
    let detail = super::detail(&f.store, &f.root, "example").unwrap();
    assert!(detail
        .checks
        .last()
        .unwrap()
        .output_tail
        .contains("invalid output"));
    assert!(detail
        .checks
        .last()
        .unwrap()
        .output_tail
        .contains("fingerprint failed"));
    assert_eq!(detail.checks.last().unwrap().result, "error");
}

#[test]
fn fingerprint_covers_head_binary_diff_and_untracked_but_not_ignored_files() {
    let f = Fixture::new();
    let initial = fingerprint(&f.root).unwrap();
    fs::create_dir(f.root.join("output")).unwrap();
    fs::write(f.root.join("output/ignored"), "ignored").unwrap();
    assert_eq!(fingerprint(&f.root).unwrap(), initial);
    fs::write(f.root.join("new file.bin"), [0u8, 1, 2, 255]).unwrap();
    let added = fingerprint(&f.root).unwrap();
    assert_ne!(added, initial);
    fs::write(f.root.join("new file.bin"), [0u8, 1, 3, 255]).unwrap();
    assert_ne!(fingerprint(&f.root).unwrap(), added);
    fs::remove_file(f.root.join("new file.bin")).unwrap();
    fs::write(f.root.join("tracked.txt"), [0u8, 3, 0, 9]).unwrap();
    assert_ne!(fingerprint(&f.root).unwrap(), initial);
    fs::write(f.root.join("tracked.txt"), "original\n").unwrap();
    git(&f.root, &["commit", "--allow-empty", "-qm", "new head"]);
    assert_ne!(fingerprint(&f.root).unwrap(), initial);
    let before_link = fingerprint(&f.root).unwrap();
    std::os::unix::fs::symlink("tracked.txt", f.root.join("link")).unwrap();
    assert_ne!(fingerprint(&f.root).unwrap(), before_link);
}

#[test]
fn timeout_and_declared_ignored_artifacts_have_real_evidence() {
    let f = Fixture::new();
    f.building("title='Example'\n[[check]]\nname='bounded'\nrun='sleep 3'\ntimeout=1");
    let summary = check(&f.store, &f.root, "example", None).unwrap();
    assert!(!summary.passed);
    assert_eq!(summary.checks[0].result, "timeout");
    let f = Fixture::new();
    f.building("title='Example'\n[[check]]\nname='image'\nrun='mkdir -p output; printf fixture > output/result.png'\nartifacts=['output/result.png']");
    let summary = check(&f.store, &f.root, "example", None).unwrap();
    assert!(summary.passed);
    assert_eq!(
        summary.checks[0].artifacts,
        vec![f
            .root
            .join("output/result.png")
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()]
    );
}

#[test]
fn concurrent_state_change_preserves_void_check_evidence_without_overwriting_state() {
    let f = Fixture::new();
    let building = f.building("title='Example'\n[[check]]\nname='proof'\nrun='true'");
    let checking =
        storage::transition(&f.store, &building, "checking", "agent", None, None, None).unwrap();
    storage::transition(
        &f.store,
        &checking,
        "stalled",
        "yoke",
        Some("stop cap"),
        None,
        None,
    )
    .unwrap();
    let mut runs = vec![CheckRun {
        id: 0,
        plan_id: checking.id,
        ts: 1,
        name: "proof".into(),
        command: "true".into(),
        exit_code: Some(0),
        output_tail: "actual execution evidence".into(),
        fingerprint: "fixture-fingerprint".into(),
        blocking: true,
        ms: 1,
        result: "pass".into(),
        findings: serde_json::json!([]),
        artifacts: vec![],
    }];
    f.store.connection().execute_batch("CREATE TEMP TRIGGER reject_check_fact BEFORE INSERT ON facts WHEN NEW.kind='check_result' BEGIN SELECT RAISE(ABORT,'fixture fact failure'); END;").unwrap();
    assert!(storage::finish(&f.store, &checking, "done", &mut runs, None).is_err());
    assert!(detail(&f.store, &f.root, "example")
        .unwrap()
        .checks
        .is_empty());
    f.store
        .connection()
        .execute_batch("DROP TRIGGER reject_check_fact")
        .unwrap();
    let (plan, applied) = storage::finish(&f.store, &checking, "done", &mut runs, None).unwrap();
    assert!(!applied);
    assert_eq!(plan.state, "stalled");
    let detail = detail(&f.store, &f.root, "example").unwrap();
    assert_eq!(detail.checks[0].result, "void");
    assert!(detail.checks[0]
        .output_tail
        .contains("actual execution evidence"));
}
