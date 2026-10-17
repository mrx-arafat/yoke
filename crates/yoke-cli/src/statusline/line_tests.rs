use super::*;

/// 2026-10-16 10:00 UTC, a Friday.
const NOW: i64 = 1_792_144_800;

fn look(width: usize) -> Look {
    Look {
        color: false,
        ascii: false,
        detailed: false,
        width,
    }
}

fn window(minutes: u32, used: f64, resets_in: i64) -> Window {
    Window {
        window_minutes: minutes,
        used_percent: used,
        resets_at: NOW + resets_in,
    }
}

fn busy() -> Facts {
    Facts {
        mode: Some("enforce".into()),
        trusted: Some(true),
        branch: Some(Branch {
            name: "main".into(),
            detached: false,
            dirty: Some(Dirty {
                files: 3,
                capped: false,
            }),
        }),
        counts: Some(SessionCounts {
            allowed: 42,
            blocked: 2,
            last: Some((NOW - 7, "pre_tool_use".into(), Some("Bash".into()))),
            ..SessionCounts::default()
        }),
        plan: Some(("add-auth".into(), "building".into())),
        quota: vec![window(300, 18.0, 3_600), window(10_080, 65.0, 3 * 86_400)],
        cost_usd: Some(0.2858),
        duration_ms: Some(257_506),
        lines: Some((12, 3)),
        context: Some(42.0),
        model: Some("Opus".into()),
        session: Some("0123456789abcdef".into()),
        unread: 2,
        ..Facts::default()
    }
}

fn first(facts: &Facts, look: &Look) -> String {
    render(facts, look, NOW, 0).remove(0)
}

#[test]
fn a_busy_session_shows_every_segment_in_order() {
    assert_eq!(
        first(&busy(), &look(0)),
        "yoke enforce | main (3 changed) | allowed 42 \u{b7} blocked 2 | plan add-auth building \u{b7} Stop held | 5h 18% \u{b7} 7d 65% (resets Mon 10:00) | $0.29 \u{b7} 4m \u{b7} +12/-3 | ctx 42% \u{2588}\u{2588}\u{2588}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591} | ! 2 unread"
    );
}

#[test]
fn ascii_mode_uses_only_seven_bit_characters() {
    let mut facts = busy();
    facts.branch.as_mut().unwrap().name = "caf\u{e9}".into();
    let line = first(
        &facts,
        &Look {
            ascii: true,
            ..look(0)
        },
    );
    assert!(line.is_ascii(), "{line}");
    assert!(line.contains("allowed 42, blocked 2"), "{line}");
    assert!(line.contains("ctx 42% [###-----]"), "{line}");
    assert!(line.contains("caf?"), "{line}");
}

#[test]
fn colors_follow_the_level_and_nothing_is_colored_without_them() {
    let mut facts = busy();
    facts.quota = vec![window(300, 18.0, 3_600), window(10_080, 90.0, 3 * 86_400)];
    facts.context = Some(70.0);
    let colored = first(
        &facts,
        &Look {
            color: true,
            ..look(0)
        },
    );
    assert!(colored.contains("\x1b[32m5h 18%\x1b[0m"), "{colored:?}");
    assert!(colored.contains("\x1b[31m7d 90%\x1b[0m"), "{colored:?}");
    assert!(colored.contains("\x1b[33m70% "), "{colored:?}");
    assert!(colored.contains("\x1b[31mblocked 2\x1b[0m"), "{colored:?}");
    assert!(!first(&facts, &look(0)).contains('\x1b'));
}

#[test]
fn nothing_known_is_one_plain_line() {
    assert_eq!(first(&Facts::default(), &look(80)), "yoke (no data yet)");
}

#[test]
fn zero_blocked_is_shown_in_enforce_and_zero_would_deny_in_observe() {
    let counts = Some(SessionCounts {
        allowed: 5,
        ..SessionCounts::default()
    });
    let enforce = Facts {
        mode: Some("enforce".into()),
        counts: counts.clone(),
        ..Facts::default()
    };
    assert!(first(&enforce, &look(0)).ends_with("allowed 5 \u{b7} blocked 0"));
    let observe = Facts {
        mode: Some("observe".into()),
        counts,
        ..Facts::default()
    };
    assert!(first(&observe, &look(0)).ends_with("allowed 5 \u{b7} would-deny 0"));
    // Errors appear only when there are some.
    let mut failing = observe;
    failing.counts.as_mut().unwrap().errors = 1;
    assert!(first(&failing, &look(0)).ends_with("would-deny 0 \u{b7} errors 1"));
}

#[test]
fn an_untrusted_setup_says_no_gate_runs_and_stop_is_held_only_in_enforce() {
    let facts = Facts {
        mode: Some("observe".into()),
        trusted: Some(false),
        plan: Some(("p".into(), "building".into())),
        ..Facts::default()
    };
    let line = first(&facts, &look(0));
    assert_eq!(line, "yoke observe (no trusted policy) | plan p building");
    let held = Facts {
        mode: Some("enforce".into()),
        plan: Some(("p".into(), "waiting".into())),
        ..Facts::default()
    };
    // A waiting plan releases Stop until a human resumes it.
    assert!(!first(&held, &look(0)).contains("Stop held"));
}

#[test]
fn a_narrow_terminal_drops_the_least_important_segments_first() {
    let facts = busy();
    let full = first(&facts, &look(0));
    let mut previous = full.chars().count();
    let mut seen_dropped: Vec<&str> = Vec::new();
    for width in (20..=previous).rev() {
        let line = first(&facts, &look(width));
        let size = line.chars().count();
        assert!(size <= width, "{width}: {size}: {line}");
        assert!(line.starts_with("yoke enforce"), "{line}");
        // Segments leave in a fixed order and never come back as the width shrinks.
        for (marker, label) in [
            ("$0.29", "cost"),
            ("ctx 42%", "context"),
            ("5h 18%", "quota"),
            ("plan add-auth", "plan"),
            ("allowed 42", "decisions"),
            ("main", "git"),
        ] {
            if !line.contains(marker) && !seen_dropped.contains(&label) {
                seen_dropped.push(label);
            }
        }
        assert!(size <= previous, "{width}");
        previous = size;
    }
    assert_eq!(
        seen_dropped,
        ["cost", "context", "quota", "plan", "decisions", "git"]
    );
    // Alerts outrank everything but the mode.
    let narrow = first(&facts, &look(30));
    assert_eq!(narrow, "yoke enforce | ! 2 unread");
}

#[test]
fn a_mode_wider_than_the_terminal_is_cut_with_a_mark() {
    let line = first(&busy(), &look(8));
    assert_eq!(line.chars().count(), 8);
    assert!(line.ends_with('\u{2026}'), "{line}");
}

#[test]
fn text_from_outside_is_redacted_cleaned_and_clipped() {
    let facts = Facts {
        plan: Some(("fix\x1b[31m-it".into(), "building".into())),
        branch: Some(Branch {
            name: format!("topic/{}", "x".repeat(80)),
            ..Branch::default()
        }),
        problem: Some("token ghp_abcdefghijklmnopqrstuvwxyz0123456789 leaked".into()),
        ..Facts::default()
    };
    let line = first(&facts, &look(0));
    assert!(line.contains("plan fix[31m-it"), "{line:?}");
    assert!(!line.contains('\x1b'), "{line:?}");
    assert!(!line.contains("ghp_abcdefghijklmnopqrstuvwxyz"), "{line}");
    assert!(
        line.contains(&format!("topic/{}\u{2026}", "x".repeat(25))),
        "{line}"
    );
}

#[test]
fn expired_quota_windows_and_empty_cost_are_left_out() {
    let facts = Facts {
        quota: vec![window(300, 90.0, -5)],
        cost_usd: Some(0.0),
        duration_ms: None,
        lines: Some((0, 0)),
        ..Facts::default()
    };
    assert_eq!(first(&facts, &look(0)), "yoke (no data yet) | $0.00");
    let tiny = Facts {
        cost_usd: Some(0.004),
        ..Facts::default()
    };
    assert!(first(&tiny, &look(0)).ends_with("<$0.01"));
}

#[test]
fn the_detailed_style_adds_a_second_line_and_the_compact_style_does_not() {
    let mut facts = busy();
    facts.problem = Some("Yoke hooks are not installed for any agent.".into());
    let compact = render(&facts, &look(0), NOW, 0);
    assert_eq!(compact.len(), 1);
    let detailed = render(
        &facts,
        &Look {
            detailed: true,
            ..look(0)
        },
        NOW,
        0,
    );
    assert_eq!(detailed.len(), 2);
    assert_eq!(
        detailed[1],
        "last Bash 7s ago | model Opus | 5h 18% resets 11:00 \u{b7} 7d 65% resets Mon 10:00 | session 01234567 | ! Yoke hooks are not installed for any agent."
    );
    // The problem is written out on the second line only, not twice.
    assert!(!detailed[0].contains("Yoke hooks"), "{}", detailed[0]);
    // With nothing extra to say there is no second line, and none when the room is too small.
    let squeezed = render(
        &facts,
        &Look {
            detailed: true,
            width: 1,
            ..look(0)
        },
        NOW,
        0,
    );
    assert_eq!(squeezed.len(), 1, "{squeezed:?}");
    let bare = render(
        &Facts::default(),
        &Look {
            detailed: true,
            ..look(0)
        },
        NOW,
        0,
    );
    assert_eq!(bare.len(), 1);
}

#[test]
fn a_health_problem_outlasts_everything_but_the_mode_and_the_alerts() {
    let mut facts = busy();
    facts.problem = Some("Yoke hooks are not installed for any agent.".into());
    // Room for the mode, the alerts and the problem, but for no other segment.
    let line = first(&facts, &look(12 + 3 + 10 + 3 + 45));
    assert_eq!(
        line,
        "yoke enforce | ! 2 unread | ! Yoke hooks are not installed for any agent."
    );
    // With less room the problem goes before the alerts do.
    assert_eq!(first(&facts, &look(30)), "yoke enforce | ! 2 unread");
}
