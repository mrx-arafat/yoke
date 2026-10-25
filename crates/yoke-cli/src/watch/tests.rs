use serde_json::json;
use yoke_core::store::DecisionRecord;

use super::render::{render, Frame};
use super::*;

/// 2026-10-16 10:00:00 UTC.
const NOW: i64 = 1_792_144_800;

fn frame(width: usize, height: Option<usize>) -> Frame {
    Frame {
        width,
        height,
        color: false,
        ascii: false,
    }
}

fn row(ts: i64, agent: &str, outcome: &str, gate: &str, command: &str) -> Event {
    Event {
        ts,
        agent: agent.into(),
        event: "pre_tool_use".into(),
        tool: Some("Bash".into()),
        outcome: outcome.into(),
        gate: gate.into(),
        detail: command.into(),
    }
}

fn busy() -> Snapshot {
    let counts = Counts {
        allowed: 412,
        would_deny: 3,
        denied: 2,
        errors: 1,
    };
    let mut snapshot = empty(NOW, 0);
    snapshot.project_name = "shop".into();
    snapshot.branch = Some("main".into());
    snapshot.changed = Some(4);
    snapshot.mode = Some("enforce".into());
    snapshot.trust = Trust::Trusted {
        gates: 3,
        stale: false,
    };
    snapshot.agents = vec![
        AgentRow {
            name: "claude",
            today: counts,
            today_partial: false,
            session: Some(SessionView {
                id: "4f1c2a9e-session".into(),
                open: true,
                counts: Counts {
                    allowed: 40,
                    ..Counts::default()
                },
                partial: false,
                since: None,
            }),
            last_event: Some(NOW - 7),
        },
        AgentRow {
            name: "codex",
            today: Counts::default(),
            today_partial: false,
            session: None,
            last_event: None,
        },
    ];
    snapshot.quota = vec![
        QuotaRow {
            agent: "claude",
            windows: vec![
                Window {
                    window_minutes: 300,
                    used_percent: 38.0,
                    resets_at: NOW + 3_600,
                },
                Window {
                    window_minutes: 10_080,
                    used_percent: 85.0,
                    resets_at: NOW + 3 * 86_400,
                },
            ],
            note: None,
        },
        QuotaRow {
            agent: "codex",
            windows: Vec::new(),
            note: Some("not recorded".into()),
        },
    ];
    snapshot.events = (0..30)
        .map(|i| {
            row(
                NOW - i,
                "claude",
                ["allow", "deny", "would_deny", "error"][i as usize % 4],
                if i % 4 == 1 { "no-force-push" } else { "" },
                "git push --force origin main && echo a very long command line that will not fit",
            )
        })
        .collect();
    snapshot.plans = Some(Ok(vec![("login-flow".into(), "executing".into())]));
    snapshot.attention = Some(Ok(Attention {
        unread: 2,
        items: vec![(
            "Observe mode would have denied 3 calls".into(),
            "yoke log --outcome would_deny --limit 20".into(),
        )],
    }));
    snapshot.jobs = Some(Ok(vec![("queued".into(), 1), ("dead".into(), 1)]));
    snapshot.hooks = vec![
        HookRow {
            agent: "claude",
            installed: Some(true),
            last_event: Some(NOW - 7),
        },
        HookRow {
            agent: "codex",
            installed: Some(false),
            last_event: None,
        },
    ];
    snapshot.gate_ms = Some((12, 340));
    snapshot.next = Some((
        Tone::Warn,
        "codex is not protected; run `yoke install --agent codex`.".into(),
    ));
    snapshot
}

#[test]
fn empty_state_says_it_is_waiting_and_shows_no_counts_or_events() {
    let mut snapshot = empty(NOW, 0);
    snapshot.project_name = "shop".into();
    snapshot.state = StateNote::Missing;
    snapshot.agents = Vec::new();
    let lines = render(&snapshot, &frame(80, None), false);
    let text = lines.join("\n");
    assert!(
        text.contains("No Yoke state yet; the first hook event creates it."),
        "{text}"
    );
    assert!(text.contains("no policy: gates are not running"), "{text}");
    assert!(text.contains("Plans     waiting for state"), "{text}");
    assert!(text.contains("No decisions yet."), "{text}");
    assert!(text.contains("next  checking yoke health"), "{text}");
}

#[test]
fn every_line_fits_the_width_and_cut_lines_end_in_an_ellipsis() {
    for width in [20, 40, 60, 80, 120] {
        for ascii in [false, true] {
            let frame = Frame {
                width,
                height: None,
                color: false,
                ascii,
            };
            let lines = render(&busy(), &frame, false);
            for line in &lines {
                assert!(line.chars().count() <= width, "{width}: {line:?}");
            }
            let more = if ascii { "..." } else { "\u{2026}" };
            let event = lines.iter().find(|l| l.starts_with("10:00:00")).unwrap();
            assert!(event.ends_with(more), "{width}: {event:?}");
        }
    }
    // Narrow lists keep time, agent and outcome and drop the event, tool and gate columns.
    let narrow = render(&busy(), &frame(50, None), false);
    let deny = narrow.iter().find(|l| l.contains("deny  ")).unwrap();
    assert!(deny.starts_with("09:59:59  claude  deny"), "{deny:?}");
    // The header drops the version before the clock, and the agent table its last-event column
    // before any count; each hook state gets its own line.
    assert_eq!(
        narrow[0],
        "yoke watch \u{b7} shop \u{b7} main 4 changed \u{b7} 10:00:00"
    );
    let today = narrow
        .iter()
        .find(|l| l.starts_with("claude  today"))
        .unwrap();
    assert_eq!(
        today.split_whitespace().collect::<Vec<_>>(),
        ["claude", "today", "412", "3", "2", "1"]
    );
    assert!(narrow
        .iter()
        .any(|l| l == "          codex not installed, no event yet"));
    let wide = render(&busy(), &frame(140, None), false);
    let deny = wide.iter().find(|l| l.contains("no-force-push ")).unwrap();
    assert!(
        deny.starts_with(
            "09:59:59  claude  pre_tool_use        Bash        deny        no-force-push     git push"
        ),
        "{deny:?}"
    );
}

#[test]
fn no_color_and_ascii_mode_print_plain_ascii_and_color_marks_outcomes() {
    let plain = render(&busy(), &frame(100, None), false).join("\n");
    assert!(!plain.contains('\x1b'));
    let ascii = Frame {
        ascii: true,
        ..frame(100, None)
    };
    let text = render(&busy(), &ascii, false).join("\n");
    assert!(text.is_ascii(), "{text}");
    // Both windows share a line when they fit, and get one line each when they do not.
    let both =
        "claude  5h  ####------   38%  resets 11:00    7d  #########-   85%  resets Mon 10:00";
    assert!(text.lines().any(|l| l == both), "{text}");
    let narrow = render(&busy(), &Frame { width: 60, ..ascii }, false).join("\n");
    assert!(
        narrow.contains("\nclaude  5h  #####-------   38%  resets 11:00\n        7d  "),
        "{narrow}"
    );
    let colored = Frame {
        color: true,
        ..frame(100, None)
    };
    let text = render(&busy(), &colored, false).join("\n");
    assert!(text.contains("\x1b[31mdeny"), "{text}");
    assert!(text.contains("\x1b[35merror"), "{text}");
    // A high quota reading is marked as such.
    assert!(text.contains("\x1b[31m   85%"), "{text}");
}

#[test]
fn the_live_frame_fills_its_height_and_quota_gives_way_to_events_first() {
    let lines = render(&busy(), &frame(80, Some(16)), true);
    assert_eq!(lines.len(), 16);
    let text = lines.join("\n");
    assert!(!text.contains("Quota"), "{text}");
    assert!(text.contains("Agents"), "{text}");
    assert!(lines[lines.len() - 1].starts_with("q quit"), "{text}");
    assert!(lines[lines.len() - 2].starts_with("next  codex is not protected"));
    let events = lines.iter().filter(|l| l.contains("git push")).count();
    assert!(events >= 3, "{text}");

    // A tall terminal shows every block and fills the rest with events.
    let tall = render(&busy(), &frame(80, Some(40)), true);
    assert_eq!(tall.len(), 40);
    assert!(tall.iter().any(|l| l.starts_with("Quota")));
    // A tiny one still ends with the keys.
    let tiny = render(&busy(), &frame(80, Some(3)), true);
    assert_eq!(tiny.len(), 3);
}

#[test]
fn rows_read_from_the_state_keep_failing_gates_and_a_one_line_target() {
    let row = |reason: Option<&str>, gates, input| DecisionRow {
        id: 1,
        record: DecisionRecord {
            policy_hash: None,
            ts: NOW,
            session_id: "s1".into(),
            agent: "codex".into(),
            project: "/p".into(),
            event: "pre_tool_use".into(),
            tool: Some("shell".into()),
            mode: "enforce".into(),
            outcome: "deny".into(),
            reason: reason.map(Into::into),
            gates,
            payload: json!({ "tool_input": input }).to_string(),
        },
    };
    let gates = json!([
        {"name": "fmt", "result": "pass", "ms": 3},
        {"name": "no-force-push", "result": "deny", "ms": 9}
    ]);
    let input = json!({"command": "git push\n  --force"});
    let denied = event(&row(Some("force push"), gates, input), "/p");
    assert_eq!(denied.gate, "no-force-push");
    assert_eq!(denied.detail, "git push --force - force push");
    // A path inside the project is shown relative to it; one outside, or a prefix, is not.
    for (path, shown) in [
        ("/p/src/a.rs", "src/a.rs"),
        ("/px/a.rs", "/px/a.rs"),
        ("/q/a.rs", "/q/a.rs"),
    ] {
        let edit = event(&row(None, json!([]), json!({"file_path": path})), "/p");
        assert_eq!((edit.gate.as_str(), edit.detail.as_str()), ("", shown));
    }
}

#[test]
fn the_tally_counts_each_agent_and_session_and_then_reads_only_new_rows() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("state.db")).unwrap();
    let record = |ts: i64, session: &str, agent: &str, project: &str, outcome: &str| {
        store
            .record_decision(
                &DecisionRecord {
                    policy_hash: None,
                    ts,
                    session_id: session.into(),
                    agent: agent.into(),
                    project: project.into(),
                    event: "pre_tool_use".into(),
                    tool: Some("Bash".into()),
                    mode: "observe".into(),
                    outcome: outcome.into(),
                    reason: None,
                    gates: json!([]),
                    payload: "{}".into(),
                },
                4096,
            )
            .unwrap();
    };
    let today = NOW - 3_600;
    // Yesterday's row counts for its session only; another project is not counted.
    record(today - 86_400, "a", "claude", "/p", "deny");
    record(today, "a", "claude", "/p", "allow");
    record(today + 1, "a", "claude", "/p", "would_deny");
    record(today + 2, "b", "codex", "/p", "error");
    record(today + 3, "c", "codex", "/other", "deny");
    let mut tally = Tally::new("/p".into(), today - 2 * 86_400, today);
    tally.advance(&store).unwrap();
    let claude = Counts {
        allowed: 1,
        would_deny: 1,
        ..Counts::default()
    };
    assert_eq!(tally.today["claude"], claude);
    assert_eq!(tally.today["codex"].errors, 1);
    assert_eq!(tally.sessions["a"].denied, 1);
    assert_eq!(tally.last_event["codex"], today + 2);
    assert_eq!(tally.events.len(), 4);

    record(today + 10, "a", "claude", "/p", "rewrite");
    tally.advance(&store).unwrap();
    assert_eq!(tally.today["claude"].allowed, 2);
    assert_eq!(tally.events.len(), 5);
    assert_eq!(tally.events[0].record.ts, today + 10);
    assert!(!tally.partial_since(today));
}
