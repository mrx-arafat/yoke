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
fn start_pins_the_head_commit_as_the_review_base() {
    let f = Fixture::new();
    let head = |root: &Path| {
        let out = Command::new("git")
            .current_dir(root)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    let at_start = head(&f.root);
    let started = f.building("title='Example'\n[[check]]\nname='proof'\nrun='true'");
    assert_eq!(started.base_ref.as_deref(), Some(at_start.as_str()));
    fs::write(f.root.join("later.txt"), "later\n").unwrap();
    git(&f.root, &["add", "later.txt"]);
    git(&f.root, &["commit", "-qm", "work during plan"]);
    assert_ne!(head(&f.root), at_start);
    let stored = get(&f.store, &f.root, "example").unwrap();
    assert_eq!(stored.base_ref.as_deref(), Some(at_start.as_str()));
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
        agent: Some("codex".into()),
        model: Some("o3".into()),
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
    assert_eq!(
        detail.checks.last().unwrap().command,
        "independent review by codex (requested model: o3; reported model: unknown)"
    );
    let passing = crate::review::ReviewReport {
        passed: true,
        error: None,
        exit_code: Some(0),
        findings: vec![],
        output_tail: "invalid output".into(),
        ms: 4,
        agent: None,
        model: None,
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
    let building = f.building("title='Example'\n[[check]]\nname='proof'\nrun='true'\n[[requirement]]\nid='proof-passes'\ntext='The proof check passes'\nchecks=['proof']");
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
        artifact_hashes: Default::default(),
    }];
    let (_, findings, _) = acceptance::attempt(
        &f.store,
        &checking,
        &files::approved(&checking).unwrap(),
        &runs,
        true,
    )
    .unwrap()
    .unwrap();
    runs.push(CheckRun {
        name: "acceptance".into(),
        findings,
        ..runs[0].clone()
    });
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
    assert_eq!(
        detail.progress.requirements[0].status,
        RequirementStatus::Unsatisfied
    );
}

fn record_session(f: &Fixture, id: &str) {
    f.store
        .record_decision(
            &DecisionRecord {
                ts: 2,
                session_id: id.into(),
                agent: "codex".into(),
                project: f.root.canonicalize().unwrap().to_str().unwrap().into(),
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
}

#[test]
fn interrupted_check_recovers_only_after_its_owner_is_gone() {
    let f = Fixture::new();
    let building = f.building("title='Example'\n[[check]]\nname='proof'\nrun='true'");
    storage::transition(&f.store, &building, "checking", "agent", None, None, None).unwrap();
    let owner = files::claim_check(&get(&f.store, &f.root, "example").unwrap())
        .unwrap()
        .unwrap();
    let refused = resume(&f.store, &f.root, "example").unwrap_err();
    assert!(refused.to_string().contains("live process"), "{refused:#}");
    assert_eq!(get(&f.store, &f.root, "example").unwrap().state, "checking");
    drop(owner);
    let resumed = after_lock_release(|| resume(&f.store, &f.root, "example"));
    assert_eq!(resumed.state, "building");
    let detail = detail(&f.store, &f.root, "example").unwrap();
    let last = detail.transitions.last().unwrap();
    assert_eq!(
        (
            last.from_state.as_str(),
            last.to_state.as_str(),
            last.actor.as_str()
        ),
        ("checking", "building", "human")
    );
    assert!(last.note.as_deref().unwrap().contains("interrupted"));
    assert_eq!(detail.checks.last().unwrap().result, "interrupted");
    assert!(after_lock_release(|| check(&f.store, &f.root, "example", None)).passed);
}

/// Children forked by concurrent test threads hold a duplicate of a just-released
/// flock descriptor until they exec, so a free lock can look owned for a moment.
/// Single-threaded `yoke` processes never see this; a lock held past the deadline still fails.
fn after_lock_release<T>(mut attempt: impl FnMut() -> anyhow::Result<T>) -> T {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match attempt() {
            Err(error)
                if (error.to_string().contains("live process")
                    || error.to_string().contains("already checking"))
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(std::time::Duration::from_millis(10))
            }
            result => return result.unwrap(),
        }
    }
}

#[test]
fn running_check_owns_its_plan_so_resume_cannot_take_it() {
    let f = Fixture::new();
    f.building("title='Example'\n[[check]]\nname='slow'\nrun='sleep 2'\ntimeout=10");
    let database = f.root.parent().unwrap().join("state.db");
    let root = f.root.clone();
    let runner = std::thread::spawn(move || {
        let store = Store::open(database).unwrap();
        check(&store, &root, "example", None).unwrap()
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while get(&f.store, &f.root, "example").unwrap().state != "checking" {
        assert!(std::time::Instant::now() < deadline, "check never started");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let refused = resume(&f.store, &f.root, "example").unwrap_err();
    assert!(refused.to_string().contains("live process"), "{refused:#}");
    assert!(runner.join().unwrap().passed);
    assert_eq!(get(&f.store, &f.root, "example").unwrap().state, "done");
}

#[test]
fn reassign_moves_ownership_and_cancel_releases_the_session() {
    let f = Fixture::new();
    let started = f.building("title='Example'\n[[check]]\nname='proof'\nrun='true'");
    record_session(&f, "replacement");
    let same = reassign(&f.store, &f.root, "example", "fixture-session").unwrap_err();
    assert!(same.to_string().contains("already assigned"), "{same:#}");
    let moved = reassign(&f.store, &f.root, "example", "replacement").unwrap();
    assert_eq!(moved.state, "building");
    assert_eq!(moved.approved_header, started.approved_header);
    assert_eq!(moved.base_ref, started.base_ref);
    assert!(active(&f.store, &f.root, "fixture-session")
        .unwrap()
        .is_none());
    assert_eq!(
        active(&f.store, &f.root, "replacement")
            .unwrap()
            .unwrap()
            .slug,
        "example"
    );
    let note = detail(&f.store, &f.root, "example")
        .unwrap()
        .transitions
        .pop()
        .unwrap()
        .note
        .unwrap();
    assert_eq!(note, "reassigned from fixture-session to replacement");

    let cancelled = cancel(&f.store, &f.root, "example", "scope was dropped").unwrap();
    assert_eq!(cancelled.state, "cancelled");
    assert!(active(&f.store, &f.root, "replacement").unwrap().is_none());
    let last = detail(&f.store, &f.root, "example")
        .unwrap()
        .transitions
        .pop()
        .unwrap();
    assert_eq!(
        (
            last.to_state.as_str(),
            last.actor.as_str(),
            last.note.as_deref()
        ),
        ("cancelled", "human", Some("scope was dropped"))
    );
    assert!(cancel(&f.store, &f.root, "example", "again").is_err());
    assert!(prepare_approval(&f.store, &f.root, "example").is_err());
    assert!(resume(&f.store, &f.root, "example").is_err());
    assert!(reassign(&f.store, &f.root, "example", "fixture-session").is_err());

    create(&f.store, &f.root, ".yoke/plans", "second", "Second").unwrap();
    let prepared = prepare_approval(&f.store, &f.root, "second").unwrap();
    approve(&f.store, &prepared).unwrap();
    assert_eq!(
        start(&f.store, &f.root, "second", "replacement")
            .unwrap()
            .state,
        "building"
    );
}

#[test]
fn artifacts_are_hashed_at_capture_and_later_changes_are_reported() {
    let f = Fixture::new();
    f.building("title='Example'\n[[check]]\nname='image'\nrun='mkdir -p output; printf fixture > output/result.png'\nartifacts=['output/result.png']");
    let summary = check(&f.store, &f.root, "example", None).unwrap();
    assert!(summary.passed);
    let path = f.root.join("output/result.png").canonicalize().unwrap();
    assert_eq!(
        summary.checks[0].artifact_hashes[path.to_str().unwrap()],
        "f16d05ec6b29248d2c61adb1e9263f78e4f7bace1b955014a2d17872cfe4064d"
    );
    let statuses = |f: &Fixture| {
        verify_evidence(&f.store, &f.root, "example")
            .unwrap()
            .into_iter()
            .map(|report| report.status)
            .collect::<Vec<_>>()
    };
    assert_eq!(statuses(&f), [EvidenceStatus::Verified]);
    fs::write(&path, "replaced").unwrap();
    assert_eq!(statuses(&f), [EvidenceStatus::Tampered]);
    assert_eq!(
        detail(&f.store, &f.root, "example").unwrap().evidence[0].status,
        EvidenceStatus::Tampered
    );
    fs::remove_file(&path).unwrap();
    assert_eq!(statuses(&f), [EvidenceStatus::Missing]);
    f.store
        .connection()
        .execute("DELETE FROM evidence", [])
        .unwrap();
    assert_eq!(statuses(&f), [EvidenceStatus::Unrecorded]);
}

const TRACKED: &str = "title='Example'\n[[check]]\nname='proof'\nrun='true'\n[[requirement]]\nid='proof-passes'\ntext='The proof check passes'\nchecks=['proof']\n[[requirement]]\nid='copy'\ntext='A human approves the copy'\nhuman=true\n[[task]]\nid='build'\ntitle='Build it'\nrequirements=['proof-passes']\n[[task]]\nid='signoff'\ntitle='Sign off the copy'\nrequirements=['copy']\ndepends_on=['build']\nowner='human'";

#[test]
fn requirement_and_task_headers_reject_bad_ids_links_and_cycles() {
    let (_, header) = files::parse(&format!("+++\n{TRACKED}\n+++\n")).unwrap();
    assert_eq!(header.tasks[1].owner, Actor::Human);
    for (from, to) in [
        ("id='build'", "id='Build'"),
        ("id='signoff'", "id='build'"),
        ("requirements=['copy']", "requirements=['missing']"),
        ("depends_on=['build']", "depends_on=['signoff']"),
        ("checks=['proof']", "checks=['other']"),
        ("human=true", "human=true\nchecks=['proof']"),
        ("name='proof'", "name='acceptance'"),
    ] {
        let header = TRACKED.replacen(from, to, 1);
        assert_ne!(header, TRACKED, "{from}");
        assert!(
            files::parse(&format!("+++\n{header}\n+++\n")).is_err(),
            "{to}"
        );
    }
    let cycle = TRACKED.replace(
        "requirements=['proof-passes']",
        "requirements=['proof-passes']\ndepends_on=['signoff']",
    );
    let error = files::parse(&format!("+++\n{cycle}\n+++\n")).unwrap_err();
    assert!(format!("{error:#}").contains("cycle"), "{error:#}");
    let advisory = TRACKED.replace(
        "run='true'",
        "run='true'\nblocking=false\n[[check]]\nname='gate'\nrun='true'",
    );
    assert!(files::parse(&format!("+++\n{advisory}\n+++\n")).is_err());
}

#[test]
fn tasks_and_human_requirements_gate_done_with_dependencies_and_ownership() {
    let f = Fixture::new();
    f.building(TRACKED);
    let blocked = complete_task(&f.store, &f.root, "example", "signoff", None, Actor::Human);
    assert!(format!("{:#}", blocked.unwrap_err()).contains("unfinished tasks: build"));
    let built = complete_task(
        &f.store,
        &f.root,
        "example",
        "build",
        Some("added it"),
        Actor::Agent,
    )
    .unwrap();
    assert_eq!(built.state, TaskState::Done);
    complete_task(&f.store, &f.root, "example", "build", None, Actor::Agent).unwrap();
    let owned = complete_task(&f.store, &f.root, "example", "signoff", None, Actor::Agent);
    assert!(format!("{:#}", owned.unwrap_err()).contains("owned by a human"));
    assert!(accept_requirement(&f.store, &f.root, "example", "proof-passes", "ok").is_err());

    let summary = check(&f.store, &f.root, "example", None).unwrap();
    assert!(!summary.passed);
    assert_eq!(summary.plan.state, "building");
    let acceptance = summary.checks.last().unwrap();
    assert_eq!(
        (acceptance.name.as_str(), acceptance.result.as_str()),
        ("acceptance", "fail")
    );
    assert!(acceptance
        .output_tail
        .contains("yoke plan accept example copy"));
    let progress = detail(&f.store, &f.root, "example").unwrap().progress;
    let statuses: Vec<_> = progress
        .requirements
        .iter()
        .map(|item| item.status)
        .collect();
    assert_eq!(
        statuses,
        [RequirementStatus::Check, RequirementStatus::Unsatisfied]
    );
    assert_eq!(progress.tasks[1].state, TaskState::Ready);

    accept_requirement(&f.store, &f.root, "example", "copy", "reviewed in staging").unwrap();
    complete_task(&f.store, &f.root, "example", "signoff", None, Actor::Human).unwrap();
    let summary = check(&f.store, &f.root, "example", None).unwrap();
    assert!(summary.passed, "{:?}", summary.checks.last());
    assert_eq!(summary.plan.state, "done");
    let progress = detail(&f.store, &f.root, "example").unwrap().progress;
    let statuses: Vec<_> = progress
        .requirements
        .iter()
        .map(|item| item.status)
        .collect();
    assert_eq!(
        statuses,
        [RequirementStatus::Check, RequirementStatus::Human]
    );
    assert_eq!(
        progress.requirements[1]
            .completion
            .as_ref()
            .unwrap()
            .evidence
            .as_deref(),
        Some("reviewed in staging")
    );
}

fn reviewer(output: &str) -> Command {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(format!("cat >/dev/null; printf '%s' '{output}'"));
    command
}

#[test]
fn blocking_validation_findings_hold_approval_until_a_human_closes_them() {
    let f = Fixture::new();
    let plan = create(&f.store, &f.root, ".yoke/plans", "example", "Example").unwrap();
    fs::write(
        &plan.file,
        format!("+++\n{TRACKED}\n+++\n\n## Rollout plan\n\nShip it.\n"),
    )
    .unwrap();
    let timeout = std::time::Duration::from_secs(10);
    let malformed = reviewer(r#"[{"severity":"advice","anchor":"task:nope","summary":"x"}]"#);
    assert!(validate(&f.store, &f.root, "example", &mut { malformed }, timeout).is_err());
    assert!(detail(&f.store, &f.root, "example")
        .unwrap()
        .feedback
        .is_empty());
    let findings = r#"[{"severity":"blocking","anchor":"requirement:copy","summary":"No one owns copy review"},{"severity":"advice","anchor":"heading:rollout-plan","summary":"Name a rollback"}]"#;
    let result = validate(
        &f.store,
        &f.root,
        "example",
        &mut reviewer(findings),
        timeout,
    )
    .unwrap();
    assert_eq!(result.blocking, 1);
    let blocking = result.findings[0].id;
    // Editing the plan cannot hide a finding written against an earlier revision.
    let original = fs::read_to_string(&plan.file).unwrap();
    fs::write(
        &plan.file,
        original.replace("Ship it.", "Ship it carefully."),
    )
    .unwrap();
    let refused = prepare_approval(&f.store, &f.root, "example").unwrap_err();
    assert!(format!("{refused:#}").contains("No one owns copy review"));
    assert!(comment(
        &f.store,
        &f.root,
        "example",
        "heading:missing",
        "x",
        Actor::Human
    )
    .is_err());
    let note = comment(
        &f.store,
        &f.root,
        "example",
        "task:signoff",
        "Who signs?",
        Actor::Human,
    )
    .unwrap();
    assert!(note.current && note.anchored);
    reply(
        &f.store,
        &f.root,
        "example",
        note.id,
        "Design lead",
        Actor::Agent,
    )
    .unwrap();
    close(
        &f.store,
        &f.root,
        "example",
        blocking,
        Closure::Acknowledged,
        Some("accepted risk"),
    )
    .unwrap();
    let item = close(
        &f.store,
        &f.root,
        "example",
        blocking,
        Closure::Acknowledged,
        None,
    )
    .unwrap();
    assert_eq!(item.state.as_deref(), Some("acknowledged"));
    assert!(!item.current);
    assert!(close(
        &f.store,
        &f.root,
        "example",
        blocking,
        Closure::Resolved,
        None
    )
    .is_err());
    let prepared = prepare_approval(&f.store, &f.root, "example").unwrap();
    let open: Vec<_> = prepared
        .open_feedback
        .iter()
        .map(|item| item.anchor.as_str())
        .collect();
    assert_eq!(open, ["heading:rollout-plan", "task:signoff"]);
    assert_eq!(approve(&f.store, &prepared).unwrap().state, "approved");
}

#[test]
fn progress_and_feedback_never_bypass_reapproval_of_edited_checks() {
    let f = Fixture::new();
    let plan = f.building(TRACKED);
    let item = comment(
        &f.store,
        &f.root,
        "example",
        "check:proof",
        "Is true enough?",
        Actor::Human,
    )
    .unwrap();
    close(
        &f.store,
        &f.root,
        "example",
        item.id,
        Closure::Resolved,
        None,
    )
    .unwrap();
    complete_task(&f.store, &f.root, "example", "build", None, Actor::Agent).unwrap();
    let frozen = get(&f.store, &f.root, "example").unwrap().approved_header;
    let content = fs::read_to_string(&plan.file).unwrap();
    fs::write(&plan.file, content.replace("run='true'", "run='false'")).unwrap();
    assert!(complete_task(&f.store, &f.root, "example", "signoff", None, Actor::Human).is_err());
    assert!(accept_requirement(&f.store, &f.root, "example", "copy", "ok").is_err());
    assert!(check(&f.store, &f.root, "example", None).is_err());
    let after = get(&f.store, &f.root, "example").unwrap();
    assert_eq!(
        (after.state.as_str(), &after.approved_header),
        ("building", &frozen)
    );
}

#[test]
fn interrupted_check_recovers_even_after_the_plan_header_was_edited() {
    let f = Fixture::new();
    let building = f.building("title='Example'\n[[check]]\nname='proof'\nrun='true'");
    storage::transition(&f.store, &building, "checking", "agent", None, None, None).unwrap();
    let plan = get(&f.store, &f.root, "example").unwrap();
    let content = fs::read_to_string(&plan.file).unwrap();
    fs::write(
        &plan.file,
        content.replace("title='Example'", "title='Edited'"),
    )
    .unwrap();
    // From `checking` neither approval nor reassignment is possible, so recovery must not
    // depend on the edited header; the frozen approval survives for the next check to compare.
    let resumed = after_lock_release(|| resume(&f.store, &f.root, "example"));
    assert_eq!(resumed.state, "building");
    assert_eq!(resumed.approved_header, plan.approved_header);
    let detail = detail(&f.store, &f.root, "example").unwrap();
    assert_eq!(detail.checks.last().unwrap().result, "interrupted");
    let note = detail.transitions.last().unwrap().note.as_deref().unwrap();
    assert!(
        note.contains("interrupted") && note.contains("re-approval required"),
        "{note}"
    );
    let refused = check(&f.store, &f.root, "example", None).unwrap_err();
    assert!(
        refused.to_string().contains("header changed"),
        "{refused:#}"
    );
    let prepared = prepare_approval(&f.store, &f.root, "example").unwrap();
    assert_eq!(approve(&f.store, &prepared).unwrap().state, "approved");
}

#[test]
fn plan_binds_a_session_whose_latest_event_is_in_another_project() {
    let f = Fixture::new();
    let other = f._temp.path().join("other");
    fs::create_dir(&other).unwrap();
    git(&other, &["init", "-q"]);
    git(&other, &["commit", "-q", "--allow-empty", "-m", "seed"]);
    // The session first spoke in the fixture project, then in `other`.
    f.store
        .record_decision(
            &DecisionRecord {
                ts: 3,
                session_id: "fixture-session".into(),
                agent: "claude".into(),
                project: other.canonicalize().unwrap().to_str().unwrap().into(),
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
    let plan = create(&f.store, &other, ".yoke/plans", "elsewhere", "Elsewhere").unwrap();
    fs::write(
        &plan.file,
        "+++\ntitle='Elsewhere'\n[[check]]\nname='proof'\nrun='true'\n+++\n\nGoal.\n",
    )
    .unwrap();
    let prepared = prepare_approval(&f.store, &other, "elsewhere").unwrap();
    approve(&f.store, &prepared).unwrap();

    start(&f.store, &other, "elsewhere", "fixture-session").unwrap();

    let bound = active(&f.store, &other, "fixture-session")
        .unwrap()
        .unwrap();
    assert_eq!(bound.slug, "elsewhere");
}
