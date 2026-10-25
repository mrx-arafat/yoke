use super::*;
use crate::config::{Config, LoadedConfig};
use crate::store::{DecisionRecord, TrustRow};
use crate::trust::SnapshotStatus;
use std::path::PathBuf;
use std::time::Duration;

#[test]
fn help_links_pin_only_a_matching_release_tag() {
    assert!(release_ref(Some("tag"), Some("v0.1.0"), "v0.1.0"));
    assert!(!release_ref(Some("branch"), Some("v0.1.0"), "v0.1.0"));
    assert!(!release_ref(Some("tag"), Some("v0.0.9"), "v0.1.0"));
    assert!(!release_ref(None, None, "v0.1.0"));
}

fn store() -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("state.db")).unwrap();
    (directory, store)
}

fn loaded(project: &str, snapshots: Vec<SnapshotStatus>) -> LoadedConfig {
    LoadedConfig {
        policy_sources: Vec::new(),
        config: Config::default(),
        origins: Config::default_origins(),
        project_root: PathBuf::from(project),
        snapshots,
        knowledge: None,
    }
}

fn decide(store: &Store, project: &str, ts: i64, outcome: &str, reason: &str) {
    let row = DecisionRecord {
        policy_hash: None,
        ts,
        session_id: "s1".into(),
        agent: "claude".into(),
        project: project.into(),
        event: "pre_tool_use".into(),
        tool: Some("Bash".into()),
        mode: if outcome == "would_deny" {
            "observe"
        } else {
            "enforce"
        }
        .into(),
        outcome: outcome.into(),
        reason: Some(reason.into()),
        gates: serde_json::json!([]),
        payload: "{}".into(),
    };
    store.record_decision(&row, 4096).unwrap();
}

fn by_kind<'a>(rows: &'a [Notification], kind: &str) -> &'a Notification {
    rows.iter().find(|row| row.kind == kind).unwrap()
}

#[test]
fn quota_alerts_deduplicate_across_projects_preserve_ack_and_escalate() {
    let (_directory, store) = store();
    let at = 1_792_144_800;
    let reading = |used, reset, time| {
        crate::quota::Quota::from_codex(&serde_json::json!({"primary":{"used_percent":used,"window_minutes":300,"resets_at":reset}}), time).unwrap()
    };
    let mut quota = reading(40, at + 7200, at);
    quota.absorb(&reading(50, at + 7200, at + 600));
    assert_eq!(
        quota_warning(&store, &quota, 85, at + 600).unwrap().len(),
        1
    );
    assert!(quota_warning(&store, &quota, 85, at + 600)
        .unwrap()
        .is_empty());
    let rows = list(&store, Path::new("/another/project"), true, 100).unwrap();
    let id = by_kind(&rows, "usage_limit").id;
    acknowledge(
        &store,
        Path::new("/another/project"),
        &Target::Ids(vec![id]),
        at + 601,
    )
    .unwrap();
    assert!(quota_warning(&store, &quota, 85, at + 601)
        .unwrap()
        .is_empty());
    quota.absorb(&reading(90, at + 7200, at + 660));
    assert_eq!(
        quota_warning(&store, &quota, 85, at + 660).unwrap().len(),
        1
    );
    quota.absorb(&reading(100, at + 7200, at + 720));
    assert_eq!(
        quota_warning(&store, &quota, 85, at + 720).unwrap().len(),
        1
    );
    assert!(quota_warning(&store, &quota, 85, at + 1021)
        .unwrap()
        .is_empty());
    assert!(list(&store, Path::new("/another/project"), true, 100)
        .unwrap()
        .is_empty());
    quota.absorb(&reading(90, at + 10000, at + 1100));
    assert_eq!(
        quota_warning(&store, &quota, 85, at + 1100).unwrap().len(),
        1
    );
}

#[test]
fn partial_usage_notifies_each_session_once_and_resolves_after_a_full_import() {
    let (_directory, store) = store();
    for session in ["first", "second"] {
        store.connection().execute("INSERT INTO sessions(id,agent,project,started_at) VALUES(?1,'claude','/work/app',0)", [session]).unwrap();
        store
            .set_kv(
                &format!("usage.partial.claude.{session}"),
                r#"{"transcript_bytes":100,"skipped_bytes":50,"limit_bytes":50}"#,
            )
            .unwrap();
    }
    let config = loaded("/work/app", Vec::new());
    sync(&store, &config, 1).unwrap();
    let rows = list(&store, Path::new("/work/app"), false, 100).unwrap();
    assert_eq!(
        rows.iter()
            .filter(|row| row.kind == "usage_partial")
            .count(),
        2
    );
    let id = by_kind(&rows, "usage_partial").id;
    acknowledge(&store, Path::new("/work/app"), &Target::Ids(vec![id]), 2).unwrap();
    sync(&store, &config, 3).unwrap();
    let rows = list(&store, Path::new("/work/app"), false, 100).unwrap();
    assert!(rows.iter().all(|row| row.count == 1));
    assert_eq!(
        rows.iter().find(|row| row.id == id).unwrap().state,
        "acknowledged"
    );
    store
        .connection()
        .execute("DELETE FROM kv WHERE key GLOB 'usage.partial.*'", [])
        .unwrap();
    sync(&store, &config, 4).unwrap();
    assert!(list(&store, Path::new("/work/app"), false, 100)
        .unwrap()
        .iter()
        .all(|row| row.resolved_at == Some(4)));
}

#[test]
fn project_retention_does_not_delete_other_projects_notifications() {
    let (_directory, store) = store();
    decide(&store, "/work/other", 1, "error", "disk full");
    sync(&store, &loaded("/work/other", Vec::new()), 2).unwrap();
    let mut config = loaded("/work/app", Vec::new());
    config.config.log.retention_days = 1;
    sync(&store, &config, 172_800).unwrap();
    assert_eq!(unread_count(&store, Path::new("/work/other")).unwrap(), 1);
}

#[test]
fn redacted_project_paths_do_not_share_notification_identity_or_legacy_authority() {
    let (_directory, store) = store();
    let alpha = "/work/ghp_alpha";
    let beta = "/work/ghp_beta";
    assert_eq!(redact(alpha), redact(beta));
    for (id, project) in [(1, alpha), (2, beta)] {
        store.connection().execute(
            "INSERT INTO plans(id,project,slug,file,state,created_at,updated_at) VALUES(?1,?2,'ship','p.md','building',1,1)",
            params![id, project],
        ).unwrap();
        store.connection().execute(
            "INSERT INTO check_runs(plan_id,ts,name,command,output_tail,fingerprint,blocking,ms,result) VALUES(?1,1,'unit','test','failed','f',1,3,'fail')",
            [id],
        ).unwrap();
    }
    let mut config = loaded(alpha, Vec::new());
    config.config.log.retention_days = 1;
    sync(&store, &config, 2).unwrap();
    let alpha_rows = list(&store, Path::new(alpha), false, 100).unwrap();
    let beta_rows = list(&store, Path::new(beta), false, 100).unwrap();
    assert_eq!(alpha_rows.len(), 1);
    assert_eq!(beta_rows.len(), 1);
    assert_eq!(alpha_rows[0].plan_id, Some(1));
    assert_eq!(beta_rows[0].plan_id, Some(2));
    assert_eq!(alpha_rows[0].project, redact(alpha));
    assert!(list(&store, Path::new(&project_identity(beta)), false, 100).is_err());
    assert_eq!(unread_count(&store, Path::new(alpha)).unwrap(), 1);
    assert!(acknowledge(
        &store,
        Path::new(alpha),
        &Target::Ids(vec![beta_rows[0].id]),
        3
    )
    .is_err());
    assert_eq!(
        acknowledge(&store, Path::new(alpha), &Target::All, 3).unwrap(),
        1
    );
    assert_eq!(unread_count(&store, Path::new(beta)).unwrap(), 1);

    store.connection().execute(
        "INSERT INTO notifications(key,kind,project,subject,title,detail,first_seen,last_seen) VALUES('legacy-ambiguous','hook_error',?1,'pre_tool_use','legacy','unknown owner',1,1)",
        [redact(alpha)],
    ).unwrap();
    let legacy = store.connection().last_insert_rowid();
    decide(&store, alpha, 4, "error", "ambiguous decision");
    decide(&store, beta, 5, "error", "ambiguous decision");
    sync(&store, &config, 172_800).unwrap();
    assert!(list(&store, Path::new(alpha), false, 100)
        .unwrap()
        .is_empty());
    assert_eq!(unread_count(&store, Path::new(alpha)).unwrap(), 0);
    assert_eq!(list(&store, Path::new(beta), false, 100).unwrap().len(), 1);
    assert_eq!(
        acknowledge(&store, Path::new(alpha), &Target::All, 172_801).unwrap(),
        0
    );
    assert!(acknowledge(&store, Path::new(beta), &Target::Ids(vec![legacy]), 172_801).is_err());
    let legacy_state: String = store
        .connection()
        .query_row(
            "SELECT state FROM notifications WHERE id=?1",
            [legacy],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        legacy_state, "unread",
        "ambiguous legacy rows are neither pruned nor acknowledged"
    );

    store.connection().execute_batch(
        "INSERT INTO notifications(key,kind,project,subject,title,detail,first_seen,last_seen) VALUES('legacy-safe','hook_error','/work/plain','pre_tool_use','legacy','known owner',1,1);"
    ).unwrap();
    assert_eq!(
        list(&store, Path::new("/work/plain"), false, 100)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        acknowledge(&store, Path::new("/work/plain"), &Target::All, 172_801).unwrap(),
        1
    );

    store.connection().execute(
        "INSERT INTO plans(id,project,slug,file,state,created_at,updated_at) VALUES(3,?1,'waiting','p.md','waiting',1,1)",
        [alpha],
    ).unwrap();
    store.connection().execute(
        "INSERT INTO notifications(key,kind,project,subject,plan_id,title,detail,count,first_seen,last_seen,state,acknowledged_at,actor) VALUES('plan:3:waiting:0','plan_waiting',?1,'waiting',3,'legacy','known plan',7,1,1,'acknowledged',6,'human')",
        [redact(alpha)],
    ).unwrap();
    sync(&store, &config, 172_802).unwrap();
    let recovered = list(&store, Path::new(alpha), false, 100).unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].plan_id, Some(3));
    assert_eq!(recovered[0].count, 7);
    assert_eq!(recovered[0].state, "acknowledged");
    assert_eq!(recovered[0].acknowledged_at, Some(6));
    assert_eq!(recovered[0].actor.as_deref(), Some("human"));
    assert_eq!(unread_count(&store, Path::new(alpha)).unwrap(), 0);
}

#[test]
fn check_output_redacts_secrets_before_truncating_the_tail() {
    let (_directory, store) = store();
    let secret = "x".repeat(350);
    let output = format!("API_KEY={secret}");
    store.connection().execute_batch(
        "INSERT INTO plans(id,project,slug,file,state,created_at,updated_at) VALUES(1,'/work/app','ship','p.md','building',1,1);"
    ).unwrap();
    store.connection().execute(
        "INSERT INTO check_runs(plan_id,ts,name,command,output_tail,fingerprint,blocking,ms,result) VALUES(1,1,'unit','test',?1,'f',1,3,'fail')",
        [&output],
    ).unwrap();
    sync(&store, &loaded("/work/app", Vec::new()), 2).unwrap();
    let rows = list(&store, Path::new("/work/app"), false, 100).unwrap();
    assert!(
        !rows[0].detail.contains(&"x".repeat(20)),
        "{}",
        rows[0].detail
    );
    assert!(rows[0].detail.contains("[REDACTED]"));
}

#[test]
fn repeated_events_share_one_row_reopen_failures_and_stay_project_scoped() {
    let (_directory, store) = store();
    let project = Path::new("/work/app");
    let config = loaded("/work/app", Vec::new());
    for ts in 1..=3 {
        decide(&store, "/work/app", ts, "error", "yoke error: disk full");
    }
    decide(&store, "/work/app", 4, "would_deny", "gate tests failed");
    decide(&store, "/work/app", 5, "would_deny", "gate lint failed");
    decide(
        &store,
        "/work/other",
        6,
        "error",
        "yoke error: other project",
    );
    sync(&store, &config, 10).unwrap();
    let rows = list(&store, project, false, 100).unwrap();
    assert_eq!(rows.len(), 2);
    let error = by_kind(&rows, "hook_error");
    assert_eq!((error.count, error.first_seen, error.last_seen), (3, 1, 3));
    assert_eq!(error.help, "hook-error");
    let observed = by_kind(&rows, "observe_would_deny");
    assert_eq!(observed.count, 2);
    assert!(observed
        .detail
        .ends_with("pre_tool_use Bash: gate lint failed"));
    assert_eq!(unread_count(&store, project).unwrap(), 2);

    let changes = store.connection().total_changes();
    sync(&store, &config, 11).unwrap();
    assert_eq!(store.connection().total_changes(), changes);
    assert_eq!(
        by_kind(&list(&store, project, false, 100).unwrap(), "hook_error").count,
        3
    );

    let other = list(&store, Path::new("/work/other"), false, 100).unwrap()[0].id;
    assert!(acknowledge(&store, project, &Target::Ids(vec![error.id, other]), 12).is_err());
    assert_eq!(
        unread_count(&store, project).unwrap(),
        2,
        "a failed batch changes nothing"
    );
    assert_eq!(acknowledge(&store, project, &Target::All, 12).unwrap(), 2);
    assert_eq!(unread_count(&store, project).unwrap(), 0);
    assert_eq!(unread_count(&store, Path::new("/work/other")).unwrap(), 1);
    assert_eq!(
        acknowledge(&store, project, &Target::Ids(vec![error.id]), 13).unwrap(),
        0
    );

    decide(&store, "/work/app", 20, "error", "yoke error: disk full");
    decide(&store, "/work/app", 21, "would_deny", "gate tests failed");
    sync(&store, &config, 22).unwrap();
    let rows = list(&store, project, true, 100).unwrap();
    assert_eq!(rows.len(), 1, "only the repeated failure reopens");
    assert_eq!((rows[0].kind.as_str(), rows[0].count), ("hook_error", 4));
    let observed = list(&store, project, false, 100).unwrap();
    let observed = by_kind(&observed, "observe_would_deny");
    assert_eq!(
        (observed.count, observed.state.as_str()),
        (3, "acknowledged")
    );
    assert_eq!(observed.actor.as_deref(), Some("human"));
}

#[test]
fn conditions_resolve_when_cleared_and_stale_trust_is_detected() {
    let (directory, store) = store();
    let root = directory.path().canonicalize().unwrap();
    let project = root.to_str().unwrap();
    store
        .connection()
        .execute_batch(&format!(
            "INSERT INTO plans(id,project,slug,file,state,created_at,updated_at) VALUES(1,'{project}','ship','p.md','stalled',1,1);
             INSERT INTO transitions(plan_id,ts,from_state,to_state,actor,note) VALUES(1,5,'building','stalled','yoke','stop limit reached without progress');
             INSERT INTO check_runs(plan_id,ts,name,command,output_tail,fingerprint,blocking,ms,result) VALUES(1,6,'unit','cargo test','1 failed','f',1,3,'fail');"
        ))
        .unwrap();
    let file = root.join("yoke.toml");
    std::fs::write(&file, "mode='enforce'").unwrap();
    let status = |trusted, stale| SnapshotStatus {
        scope: "project".into(),
        file: file.clone(),
        trusted,
        stale,
        exists: true,
        disk_error: None,
    };
    sync(&store, &loaded(project, vec![status(false, false)]), 10).unwrap();
    let rows = list(&store, &root, true, 100).unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(by_kind(&rows, "plan_stalled").subject, "ship");
    assert!(by_kind(&rows, "check_failed")
        .title
        .contains("unit failed on plan ship"));
    by_kind(&rows, "config_untrusted");

    store
        .put_trust(&TrustRow {
            scope: "project".into(),
            root: project.into(),
            file: file.to_str().unwrap().into(),
            sha256: "trusted-bytes".into(),
            content: "mode='observe'".into(),
            trusted_at: 11,
        })
        .unwrap();
    store
        .connection()
        .execute_batch(
            "UPDATE plans SET state='building' WHERE id=1;
             INSERT INTO check_runs(plan_id,ts,name,command,output_tail,fingerprint,blocking,ms,result) VALUES(1,12,'unit','cargo test','ok','f',1,3,'pass');",
        )
        .unwrap();
    sync(&store, &loaded(project, vec![status(true, true)]), 13).unwrap();
    let rows = list(&store, &root, false, 100).unwrap();
    for kind in ["plan_stalled", "check_failed", "config_untrusted"] {
        let row = by_kind(&rows, kind);
        assert_eq!(
            (row.state.as_str(), row.actor.as_deref(), row.resolved_at),
            ("acknowledged", Some("yoke"), Some(13)),
            "{kind}"
        );
    }
    assert_eq!(by_kind(&rows, "config_stale").state, "unread");
    assert_eq!(unread_count(&store, &root).unwrap(), 1);

    sync(&store, &loaded(project, vec![status(true, false)]), 14).unwrap();
    assert_eq!(unread_count(&store, &root).unwrap(), 0);
}

#[test]
fn help_search_matches_topics_and_every_topic_has_a_troubleshooting_section() {
    let ids = |query: &str| {
        search_help(query)
            .iter()
            .map(|topic| topic.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids("disableAllHooks"), ["hooks-disabled"]);
    assert_eq!(ids("STALE trusted"), ["stale-trust"]);
    assert!(ids("plan resume").contains(&"plan-recovery"));
    assert!(ids("observe would_deny").contains(&"observe-mode"));
    assert!(ids("no-such-topic").is_empty());
    assert_eq!(search_help("").len(), HELP.len());
    let docs = include_str!("../../../../docs/troubleshooting.md");
    let anchors: Vec<String> = docs
        .lines()
        .filter_map(|line| line.strip_prefix("## "))
        .map(|heading| {
            heading
                .to_lowercase()
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-')
                .collect::<String>()
                .replace(' ', "-")
        })
        .collect();
    for topic in HELP {
        // An installed machine has no docs/ directory, so the link must be openable as it is.
        let anchor = topic
            .doc
            .strip_prefix("https://github.com/mrx-arafat/yoke/blob/main/docs/troubleshooting.md#")
            .unwrap_or_else(|| panic!("{} is not a full URL", topic.doc));
        assert!(anchors.iter().any(|a| a == anchor), "missing {anchor}");
    }
    for kind in [
        "hook_error",
        "observe_would_deny",
        "plan_waiting",
        "plan_stalled",
        "check_failed",
        "config_stale",
        "config_untrusted",
    ] {
        assert!(HELP.iter().any(|topic| topic.id == help_for(kind).id));
    }
}

#[test]
fn large_backlog_drains_in_bounded_calls_with_one_shot_results_and_rowid_ranges() {
    let fill = |store: &Store| {
        store
            .transaction(|tx| {
                for i in 0..20_000_i64 {
                    let outcome = ["allow", "error", "would_deny", "deny"][(i % 4) as usize];
                    let row = DecisionRecord {
                        policy_hash: None,
                        ts: 1_000_000 + i,
                        session_id: format!("s{}", i % 7),
                        agent: "codex".into(),
                        project: format!("/work/p{}", (i / 4) % 2),
                        event: "pre_tool_use".into(),
                        tool: Some("Bash".into()),
                        mode: if outcome == "would_deny" {
                            "observe"
                        } else {
                            "enforce"
                        }
                        .into(),
                        outcome: outcome.into(),
                        reason: Some(format!("reason {}", i % 3)),
                        gates: serde_json::json!([]),
                        payload: "{}".into(),
                    };
                    Store::record_decision_in(tx, &row, 64)?;
                }
                Ok(())
            })
            .unwrap();
    };
    let snapshot = |store: &Store| {
        let mut rows: Vec<_> = ["/work/p0", "/work/p1"]
            .iter()
            .flat_map(|project| list(store, Path::new(project), false, 1000).unwrap())
            .map(|n| {
                (
                    n.kind,
                    n.project,
                    n.title,
                    n.detail,
                    n.count,
                    n.first_seen,
                    n.last_seen,
                    n.state,
                )
            })
            .collect();
        rows.sort();
        rows
    };
    let now = 1_030_000;
    let config = loaded("/work/p0", Vec::new());

    let (_one_dir, one_shot) = store();
    fill(&one_shot);
    assert!(!sync_bounded(&one_shot, &config, now, i64::MAX, Duration::MAX, PAUSE).unwrap());

    let (_dir, batched) = store();
    fill(&batched);
    let mut calls = 0;
    // A zero time budget makes every call stop after exactly one window, so the count proves
    // the bound deterministically; wall-clock assertions would flake on a loaded machine.
    loop {
        calls += 1;
        let pending = sync_bounded(&batched, &config, now, 1_000, Duration::ZERO, PAUSE).unwrap();
        if !pending {
            break;
        }
    }
    assert_eq!(calls, 20, "one bounded batch of 1,000 ids per call");
    let expected = snapshot(&one_shot);
    assert_eq!(
        expected.len(),
        8,
        "3 error reasons and 1 would-deny summary per project"
    );
    assert_eq!(snapshot(&batched), expected);

    for sql in [HOOK_ERRORS_SQL, WOULD_DENY_SQL] {
        let mut plan = batched
            .connection()
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap();
        let details: Vec<String> = plan
            .query_map([1, 2], |row| row.get(3))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let searches: Vec<_> = details
            .iter()
            .filter(|d| {
                !d.contains("B-TREE")
                    && !d.contains("SUBQUERY")
                    && !d.contains("CO-ROUTINE")
                    && !d.starts_with("SCAN")
            })
            .collect();
        assert!(
            !searches.is_empty()
                && searches
                    .iter()
                    .all(|d| d.contains("USING INTEGER PRIMARY KEY")),
            "{details:?}"
        );
    }
}

#[test]
fn a_writer_with_no_busy_timeout_gets_the_lock_between_windows() {
    let (directory, store) = store();
    store
        .transaction(|tx| {
            for i in 0..5_000_i64 {
                let row = DecisionRecord {
                    policy_hash: None,
                    ts: 1_000_000 + i,
                    session_id: "s".into(),
                    agent: "codex".into(),
                    project: "/work/p0".into(),
                    event: "pre_tool_use".into(),
                    tool: None,
                    mode: "enforce".into(),
                    outcome: "error".into(),
                    reason: Some("disk full".into()),
                    gates: serde_json::json!([]),
                    payload: "{}".into(),
                };
                Store::record_decision_in(tx, &row, 64)?;
            }
            Ok(())
        })
        .unwrap();
    let path = directory.path().join("state.db");
    let (window, windows) = std::sync::mpsc::channel::<usize>();
    let (wrote, writes) = std::sync::mpsc::channel();
    let writer = std::thread::spawn(move || {
        let db = rusqlite::Connection::open(path).unwrap();
        db.busy_timeout(Duration::ZERO).unwrap();
        for n in windows {
            wrote
                .send(db.execute(
                    "INSERT INTO kv(key,value) VALUES(?1,'hook')",
                    [format!("writer-{n}")],
                ))
                .unwrap();
        }
    });
    let config = loaded("/work/p0", Vec::new());
    let mut calls = 0;
    loop {
        let pending =
            sync_bounded(&store, &config, 1_030_000, 1_000, Duration::ZERO, PAUSE).unwrap();
        calls += 1;
        window.send(calls).unwrap();
        assert_eq!(
            writes.recv().unwrap().unwrap(),
            1,
            "writer after window {calls}"
        );
        if !pending {
            break;
        }
    }
    drop(window);
    writer.join().unwrap();
    assert_eq!(calls, 5);
}

#[test]
fn back_to_back_drains_never_starve_a_hook_waiting_for_the_lock() {
    let (directory, store) = store();
    store
        .connection()
        .execute_batch(
            "INSERT INTO sessions(id,agent,project,started_at) VALUES('s','codex','/work/p0',0);
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<80000)
             INSERT INTO decisions(ts,session_id,agent,project,event,tool,mode,outcome,reason,gates,payload)
             SELECT 1000000+i,'s','codex','/work/p0','pre_tool_use','Bash','enforce',
                    CASE WHEN i%2 THEN 'error' ELSE 'allow' END,'disk full','[]','{}' FROM n;",
        )
        .unwrap();
    let path = directory.path().join("state.db");
    let (permit, requested) = std::sync::mpsc::channel::<()>();
    let (ready, opened) = std::sync::mpsc::channel();
    let (attempting, attempts) = std::sync::mpsc::channel();
    let (written, writes) = std::sync::mpsc::channel();
    // Same opener, and so the same 2 s busy timeout, that hooks use.
    let hook = std::thread::spawn(move || {
        let hook = Store::open(path).unwrap();
        ready.send(()).unwrap();
        let mut count = 0;
        while requested.recv().is_ok() {
            let started = Instant::now();
            attempting.send(()).unwrap();
            hook.set_kv("hook.write", &count.to_string()).unwrap();
            written.send(started.elapsed()).unwrap();
            count += 1;
        }
        count
    });
    let config = loaded("/work/p0", Vec::new());
    opened.recv_timeout(Duration::from_secs(5)).unwrap();
    let mut calls = 0;
    let mut worst = Duration::ZERO;
    loop {
        // Queue a real hook writer behind the lock before starting each drain call.
        store.connection().execute_batch("BEGIN IMMEDIATE").unwrap();
        permit.send(()).unwrap();
        attempts.recv_timeout(Duration::from_secs(5)).unwrap();
        store.connection().execute_batch("COMMIT").unwrap();
        let pending = sync(&store, &config, 1_100_000).unwrap();
        worst = worst.max(writes.recv_timeout(Duration::from_secs(5)).unwrap());
        calls += 1;
        if !pending {
            break;
        }
    }
    drop(permit);
    let writes = hook
        .join()
        .expect("a hook write failed while the drain ran");
    assert!(calls > 1, "calls {calls}");
    assert_eq!(writes, calls, "every drain overlaps a queued hook write");
    // Generous: a waiting hook needs one window plus one pause, far below its 2 s timeout.
    assert!(worst < Duration::from_secs(1), "a hook waited {worst:?}");
}

#[test]
fn hook_backlog_queues_one_durable_drain_without_processing_the_whole_history() {
    let (_directory, store) = store();
    store.connection().execute_batch("INSERT INTO sessions(id,agent,project,started_at) VALUES('backlog','codex','/work/app',0);
        WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<6000)
        INSERT INTO decisions(ts,session_id,agent,project,event,mode,outcome,reason,gates,payload)
        SELECT i,'backlog','codex','/work/app','pre_tool_use','observe','would_deny','gate','[]','{}' FROM n;").unwrap();
    let config = loaded("/work/app", Vec::new());
    assert!(sync_for_hook(&store, &config, 6001).unwrap());
    assert!(sync_for_hook(&store, &config, 6002).unwrap());
    assert_eq!(
        by_kind(
            &list(&store, Path::new("/work/app"), false, 10).unwrap(),
            "observe_would_deny"
        )
        .count,
        4000
    );
    let jobs = crate::jobs::list(&store, false, 10).unwrap();
    assert_eq!(
        jobs.len(),
        1,
        "a backlog must have one durable background drain"
    );
    assert_eq!(jobs[0].kind, "notifications-drain");
    assert!(!drain_background(&store, 6003).unwrap());
    assert_eq!(
        by_kind(
            &list(&store, Path::new("/work/app"), false, 10).unwrap(),
            "observe_would_deny"
        )
        .count,
        6000
    );
    assert!(!sync_for_hook(&store, &config, 6004).unwrap());
    // Compare the unchanged bounded window with the new queue glue on identical fixture data.
    for with_queue in [false, true] {
        let mut times = Vec::new();
        for _ in 0..21 {
            store.connection().execute_batch("UPDATE kv SET value='0' WHERE key='notifications.decision_id'; DELETE FROM jobs WHERE kind='notifications-drain';").unwrap();
            let start = Instant::now();
            let pending = if with_queue {
                sync_for_hook(&store, &config, 6005).unwrap()
            } else {
                sync_bounded(
                    &store,
                    &config,
                    6005,
                    HOOK_BATCH_IDS,
                    Duration::ZERO,
                    Duration::ZERO,
                )
                .unwrap()
            };
            assert!(pending);
            times.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(f64::total_cmp);
        println!(
            "notification_window with_queue={with_queue} p50_ms={:.3} p99_ms={:.3}",
            times[10], times[20]
        );
    }
}

#[test]
fn notification_windows_skip_deleted_row_id_gaps() {
    let (_directory, store) = store();
    store.connection().execute_batch("INSERT INTO sessions(id,agent,project,started_at) VALUES('gap','claude','/work/app',0);
        INSERT INTO decisions(id,ts,session_id,agent,project,event,mode,outcome,reason,gates,payload)
        VALUES(1000000,1,'gap','claude','/work/app','pre_tool_use','observe','would_deny','gate','[]','{}');").unwrap();
    assert!(!sync_for_hook(&store, &loaded("/work/app", Vec::new()), 2).unwrap());
    assert_eq!(
        by_kind(
            &list(&store, Path::new("/work/app"), false, 10).unwrap(),
            "observe_would_deny"
        )
        .count,
        1
    );
    assert!(crate::jobs::list(&store, false, 10).unwrap().is_empty());
}
