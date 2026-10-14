use super::*;
use crate::store::DecisionRecord;

const DAY: i64 = 86_400;
// 2026-10-12T00:00:00Z
const MONDAY: i64 = 1_791_763_200;

fn record(store: &Store, ts: i64, agent: &str, mode: &str, event: &str, outcome: &str) {
    let gates = if outcome == "allow" {
        serde_json::json!([{"name":"lint","result":"pass","ms":1,"exit_code":0}])
    } else {
        serde_json::json!([{"name":"lint","result":"pass","ms":1,"exit_code":0},
            {"name":"self-protection","result":"deny","ms":0,"exit_code":null}])
    };
    store
        .record_decision(
            &DecisionRecord {
                ts,
                session_id: format!("{agent}-{}", (ts - MONDAY) / DAY),
                agent: agent.into(),
                project: "/trial/project".into(),
                event: event.into(),
                tool: Some("Bash".into()),
                mode: mode.into(),
                outcome: outcome.into(),
                reason: (outcome != "allow").then(|| "blocked password=hunter2".into()),
                gates,
                payload: "{}".into(),
            },
            4096,
        )
        .unwrap();
}

fn query() -> TrialQuery {
    TrialQuery {
        since: None,
        until: None,
        agent: None,
        limit: 100,
        before: None,
        local_time: false,
    }
}

#[test]
fn report_rejects_empty_and_reversed_time_windows() {
    let store = Store::open(":memory:").unwrap();
    for until in [MONDAY, MONDAY - DAY] {
        let error = report(
            &store,
            &TrialQuery {
                since: Some(MONDAY),
                until: Some(until),
                ..query()
            },
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("until must be after since"));
    }
}

#[test]
fn report_counts_working_days_outcomes_and_acceptance_thresholds() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("state.db")).unwrap();
    // Codex observe: six distinct hours on Monday, two decisions in the 09:00 hour.
    for hour in 9..15 {
        record(
            &store,
            MONDAY + hour * 3600,
            "codex",
            "observe",
            "pre_tool_use",
            "allow",
        );
    }
    record(
        &store,
        MONDAY + 9 * 3600 + 1800,
        "codex",
        "observe",
        "pre_tool_use",
        "would_deny",
    );
    // Claude observe: five hours on Monday is not a working day.
    for hour in 9..14 {
        record(
            &store,
            MONDAY + hour * 3600,
            "claude",
            "observe",
            "stop",
            "allow",
        );
    }
    // Codex enforce: five six-hour days, plus a short sixth day; Claude enforce: four days.
    for day in 1..7 {
        let hours = if day == 6 { 2 } else { 6 };
        for hour in 0..hours {
            record(
                &store,
                MONDAY + day * DAY + (8 + hour) * 3600,
                "codex",
                "enforce",
                "post_tool_use",
                "allow",
            );
            if day < 5 {
                record(
                    &store,
                    MONDAY + day * DAY + (8 + hour) * 3600,
                    "claude",
                    "enforce",
                    "pre_tool_use",
                    "allow",
                );
            }
        }
    }
    record(
        &store,
        MONDAY + 2 * DAY + 3600,
        "codex",
        "enforce",
        "pre_tool_use",
        "deny",
    );
    record(
        &store,
        MONDAY + 3 * DAY + 3600,
        "codex",
        "enforce",
        "stop",
        "error",
    );

    let report = report(&store, &query(), None).unwrap();
    let codex_observe = &report.cohorts[3];
    assert_eq!(
        (codex_observe.agent.as_str(), codex_observe.mode.as_str()),
        ("codex", "observe")
    );
    assert_eq!(codex_observe.first_ts, MONDAY + 9 * 3600);
    assert_eq!(codex_observe.last_ts, MONDAY + 14 * 3600);
    assert_eq!(
        (
            codex_observe.decisions,
            codex_observe.sessions,
            codex_observe.active_days,
            codex_observe.active_hours,
            codex_observe.working_days
        ),
        (7, 1, 1, 6, 1)
    );
    assert_eq!(
        codex_observe.events,
        [EventOutcomes {
            event: "pre_tool_use".into(),
            allow: 6,
            deny: 0,
            would_deny: 1,
            rewrite: 0,
            error: 0
        }]
    );
    let codex_enforce = &report.cohorts[2];
    assert_eq!(codex_enforce.mode, "enforce");
    // Hour 01:00 on days two and three adds active hours, not working days.
    assert_eq!(
        (
            codex_enforce.active_days,
            codex_enforce.active_hours,
            codex_enforce.working_days
        ),
        (6, 34, 5)
    );
    assert_eq!(codex_enforce.events.len(), 3);
    let statuses: Vec<_> = report
        .acceptance
        .iter()
        .map(|criterion| (criterion.agent.as_deref(), criterion.status))
        .collect();
    assert_eq!(
        statuses,
        [
            (Some("claude"), "not_met"),
            (Some("claude"), "not_met"),
            (Some("codex"), "met"),
            (Some("codex"), "met"),
            (None, "needs_human"),
            (None, "met"),
        ]
    );
    assert_eq!(report.flagged_total, 3);
    assert_eq!(report.flagged[0].outcome, "error");
    assert_eq!(report.flagged[0].gates, ["self-protection"]);
    assert!(!serde_json::to_string(&report).unwrap().contains("hunter2"));

    // Paging and window filters read the same evidence in bounded slices.
    let first = super::report(
        &store,
        &TrialQuery {
            limit: 2,
            ..query()
        },
        None,
    )
    .unwrap();
    assert_eq!(first.flagged.len(), 2);
    let rest = super::report(
        &store,
        &TrialQuery {
            limit: 2,
            before: first.next_before,
            ..query()
        },
        None,
    )
    .unwrap();
    assert_eq!((rest.flagged.len(), rest.next_before), (1, None));
    assert_eq!(rest.flagged[0].outcome, "would_deny");
    let window = TrialQuery {
        since: Some(MONDAY + DAY),
        until: Some(MONDAY + 2 * DAY),
        agent: Some("claude".into()),
        ..query()
    };
    let window = super::report(&store, &window, None).unwrap();
    assert_eq!(window.cohorts.len(), 1);
    assert_eq!(
        (
            window.cohorts[0].active_days,
            window.cohorts[0].working_days
        ),
        (1, 1)
    );
    assert_eq!(window.acceptance.len(), 4);
}

#[test]
fn incidents_include_plan_recoveries_and_windowed_error_log_lines() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("state.db")).unwrap();
    store
        .connection()
        .execute_batch(&format!(
            "INSERT INTO plans(id,project,slug,file,state,created_at,updated_at) VALUES(1,'/trial/project','demo','plan.md','building',1,1);
             INSERT INTO transitions(plan_id,ts,from_state,to_state,actor,note) VALUES
               (1,{t1},'building','checking','agent',NULL),
               (1,{t2},'checking','building','human','recovered an interrupted check; its results were never recorded'),
               (1,{t3},'building','stalled','yoke','stop guard gave up'),
               (1,{t4},'stalled','building','human',NULL),
               (1,{t5},'building','building','human','reassigned from one to two'),
               (1,{t6},'building','cancelled','human','abandoned token=abc123');
             INSERT INTO check_runs(plan_id,ts,name,command,exit_code,output_tail,fingerprint,blocking,ms,result)
               VALUES(1,{t2},'check','yoke plan check demo',NULL,'','fp',1,0,'interrupted');",
            t1 = MONDAY + 60,
            t2 = MONDAY + 120,
            t3 = MONDAY + 180,
            t4 = MONDAY + 240,
            t5 = MONDAY + 300,
            t6 = MONDAY + 2 * DAY,
        ))
        .unwrap();
    let log = format!(
        "{} yoke: audit decision was not saved: readonly; trusted gate decision preserved\n\
         {} yoke error: parse hook JSON object\n\
         yoke: update download failed (exit status: 56)\n\
         {} yoke: too late\n",
        MONDAY + 30,
        MONDAY + 90,
        MONDAY + 3 * DAY
    );
    let window = TrialQuery {
        until: Some(MONDAY + DAY),
        ..query()
    };
    let report = report(&store, &window, Some(&log)).unwrap();
    let kinds: Vec<_> = report
        .incidents
        .iter()
        .map(|incident| incident.kind.as_str())
        .collect();
    assert_eq!(
        kinds,
        [
            "state_write_failed",
            "hook_error",
            "interrupted_check",
            "resume",
            "stall",
            "resume",
            "reassign",
            "error_log"
        ]
    );
    assert_eq!(
        report.incidents[3].detail,
        "checking -> building: recovered an interrupted check; its results were never recorded"
    );
    assert_eq!(report.incidents[2].time, "2026-10-12 00:02:00");
    assert_eq!(report.incidents[7].time, "unknown");
    let full = super::report(&store, &query(), Some(&log)).unwrap();
    let cancel = full
        .incidents
        .iter()
        .find(|incident| incident.kind == "cancel")
        .unwrap();
    assert!(!cancel.detail.contains("abc123"), "{}", cancel.detail);
    assert_eq!(full.acceptance.last().unwrap().status, "needs_human");

    assert_eq!(parse_time(&store, "2026-10-12", false).unwrap(), MONDAY);
    assert_eq!(
        parse_time(&store, "2026-10-12T06:00:00+06:00", true).unwrap(),
        MONDAY
    );
    assert_eq!(
        parse_time(&store, "2026-10-12T00:00:00.5Z", false).unwrap(),
        MONDAY
    );
    for invalid in [
        "2026-02-30",
        "2026-10-12T00:00:00",
        "yesterday",
        "2026-10-12 00:00:00Z",
    ] {
        assert!(parse_time(&store, invalid, false).is_err(), "{invalid}");
    }
}
