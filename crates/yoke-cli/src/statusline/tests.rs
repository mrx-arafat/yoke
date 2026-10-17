use super::*;
use serde_json::json;
use yoke_core::store::DecisionRecord;
use yoke_core::trust;

/// 2026-10-16 10:00 UTC, a Friday; the weekly window below resets the following Monday.
const NOW: i64 = 1_792_144_800;

fn plain() -> Settings {
    Settings::plain()
}

/// The wrapper has a fixed 250 ms budget that a loaded host can miss once; a wrapper that
/// really cannot finish fails every attempt.
fn refresh_wrapped(home: &Path, input: &[u8], wrap: &str) -> String {
    let mut last = None;
    for _ in 0..10 {
        match refresh(home, input, Some(wrap), &plain()) {
            Ok(rendered) => return rendered.line,
            Err(error) if error.to_string().contains("exceeded 250 ms") => last = Some(error),
            Err(error) => panic!("{error:#}"),
        }
    }
    panic!("the wrapper missed its 250 ms budget 10 times: {last:?}");
}

fn line_at(home: &Path, input: &Value, settings: &Settings) -> String {
    refresh_at(home, input.to_string().as_bytes(), None, NOW, 0, settings)
        .unwrap()
        .line
}

fn state(home: &Path) -> Store {
    std::fs::create_dir_all(home.join(".yoke")).unwrap();
    Store::open(home.join(".yoke/state.db")).unwrap()
}

fn decision(
    session: &str,
    project: &str,
    event: &str,
    mode: &str,
    outcome: &str,
) -> DecisionRecord {
    DecisionRecord {
        ts: NOW - 5,
        session_id: session.into(),
        agent: "claude".into(),
        project: project.into(),
        event: event.into(),
        tool: Some("Bash".into()),
        mode: mode.into(),
        outcome: outcome.into(),
        reason: None,
        gates: json!([]),
        payload: "{}".into(),
    }
}

#[test]
fn absent_state_does_not_create_database_and_wrapper_receives_identical_stdin() {
    let home = tempfile::tempdir().unwrap();
    let input = br#"{ "context_window": {"used_percentage": 42} }"#;
    let line = refresh_wrapped(home.path(), input, "cat");
    assert_eq!(
        line,
        format!(
            "yoke (no data yet) | ctx 42% \u{2588}\u{2588}\u{2588}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591} | {}",
            String::from_utf8_lossy(input)
        )
    );
    assert!(!home.path().join(".yoke").exists());
    assert!(refresh(home.path(), b"{}", Some("sleep 2"), &plain()).is_err());
    assert!(refresh(home.path(), &[b' '; INPUT_CAP + 1], None, &plain()).is_err());
}

#[test]
fn unreadable_json_still_paints_the_recorded_state_line_and_warns_separately() {
    let home = tempfile::tempdir().unwrap();
    let rendered = refresh(home.path(), b"not json", None, &plain()).unwrap();
    assert_eq!(rendered.line, "yoke (no data yet)");
    let warning = rendered.warning.unwrap();
    assert!(
        warning.starts_with("invalid statusline JSON: "),
        "{warning}"
    );
    assert!(!home.path().join(".yoke").exists());
}

#[test]
fn existing_wal_state_is_read_without_changes_and_missing_shm_is_not_created() {
    fn files(directory: &Path) -> std::collections::BTreeMap<std::ffi::OsString, Vec<u8>> {
        std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.file_name(), std::fs::read(entry.path()).unwrap())
            })
            .collect()
    }
    let home = tempfile::tempdir().unwrap();
    let directory = home.path().join(".yoke");
    std::fs::create_dir(&directory).unwrap();
    // Hooks truncate the WAL when the last connection closes, so build the state in a
    // scratch directory, copy the live main file with its sidecars while a second
    // connection still holds the WAL open, and read that copy with no connection open.
    let scratch = home.path().join("scratch");
    std::fs::create_dir(&scratch).unwrap();
    let store = Store::open(scratch.join("state.db")).unwrap();
    store
        .set_kv(crate::update::LATEST_VERSION_KEY, "v999.0.0")
        .unwrap();
    let holder = rusqlite::Connection::open(scratch.join("state.db")).unwrap();
    let rows: i64 = holder
        .query_row("SELECT count(*) FROM kv", [], |row| row.get(0))
        .unwrap();
    assert!(rows > 0);
    for name in ["state.db", "state.db-wal", "state.db-shm"] {
        std::fs::copy(scratch.join(name), directory.join(name)).unwrap();
    }
    drop(store);
    drop(holder);
    let before = files(&directory);
    assert!(before.contains_key(std::ffi::OsStr::new("state.db-shm")));
    assert!(!before[std::ffi::OsStr::new("state.db-wal")].is_empty());
    assert_eq!(
        refresh(home.path(), b"{}", None, &plain()).unwrap().line,
        // No trusted policy is recorded in this copy, which the line says outright.
        "yoke (no data yet) (no trusted policy) | update v999.0.0"
    );
    assert_eq!(files(&directory), before);

    // Only this disposable fixture's sidecar is removed. A callback must
    // surface unavailable state rather than recreate the WAL index.
    std::fs::remove_file(directory.join("state.db-shm")).unwrap();
    let before = files(&directory);
    assert!(refresh(home.path(), b"{}", None, &plain()).is_err());
    assert_eq!(files(&directory), before);
}

#[test]
fn quota_renders_from_the_payload_and_is_recorded_only_inside_existing_state() {
    let input = json!({
        "context_window": {"used_percentage": 6},
        "rate_limits": {
            "five_hour": {"used_percentage": 8, "resets_at": NOW + 3_600},
            "seven_day": {"used_percentage": 63, "resets_at": NOW + 3 * 86_400}
        }
    })
    .to_string();
    let line = "yoke (no data yet) | 5h 8% \u{b7} 7d 63% (resets Mon 10:00) | ctx 6% \u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}";
    let home = tempfile::tempdir().unwrap();
    let at = |bytes: &[u8], now| {
        refresh_at(home.path(), bytes, None, now, 0, &plain())
            .unwrap()
            .line
    };
    assert_eq!(at(input.as_bytes(), NOW), line);
    assert!(!home.path().join(".yoke").exists());

    std::fs::create_dir(home.path().join(".yoke")).unwrap();
    assert_eq!(at(input.as_bytes(), NOW), line);
    let saved = quota::read_file(&quota::claude_file(home.path()), "claude")
        .unwrap()
        .unwrap();
    assert_eq!((saved.observed_at, saved.windows.len()), (NOW, 2));
    assert!(!home.path().join(".yoke/state.db").exists());

    // A payload without limits renders nothing extra and leaves the record alone.
    assert_eq!(at(b"{}", NOW + 1), "yoke (no data yet)");
}

#[test]
fn only_a_newer_valid_release_is_announced_and_context_must_be_in_range() {
    assert_eq!(newer_release(Some("v999.0.0")).as_deref(), Some("v999.0.0"));
    assert_eq!(newer_release(Some("../bad")), None);
    assert_eq!(newer_release(Some("v0.0.1")), None);
    assert_eq!(newer_release(None), None);
    assert!(context_percentage(&json!({"context_window":{"used_percentage":101}})).is_none());
}

#[test]
fn recorded_decisions_and_trust_become_the_mode_and_count_segments() {
    let home = tempfile::tempdir().unwrap();
    let store = state(home.path());
    let trusted = home.path().join("trusted");
    let untrusted = home.path().join("untrusted");
    for project in [&trusted, &untrusted] {
        std::fs::create_dir(project).unwrap();
        std::fs::write(project.join("yoke.toml"), "mode = 'observe'\n").unwrap();
    }
    trust::prepare_project(&store, &trusted)
        .unwrap()
        .commit(&store)
        .unwrap();
    let (trusted, untrusted) = (
        trusted.canonicalize().unwrap(),
        untrusted.canonicalize().unwrap(),
    );
    let (trusted, untrusted) = (trusted.to_str().unwrap(), untrusted.to_str().unwrap());
    for (event, outcome) in [
        ("session_start", "allow"),
        ("pre_tool_use", "allow"),
        ("pre_tool_use", "allow"),
        ("pre_tool_use", "deny"),
    ] {
        store
            .record_decision(
                &decision("enforced", trusted, event, "enforce", outcome),
                4096,
            )
            .unwrap();
    }
    for outcome in ["allow", "would_deny"] {
        store
            .record_decision(
                &decision("watched", untrusted, "pre_tool_use", "observe", outcome),
                4096,
            )
            .unwrap();
    }
    let line = |session: &str| line_at(home.path(), &json!({ "session_id": session }), &plain());

    // Session start is not an action: two actions passed and one was blocked. A refusal in the
    // last hour also raises the recent-deny alert, whichever mode recorded it.
    assert_eq!(
        line("enforced"),
        "yoke enforce | allowed 2 \u{b7} blocked 1 | ! recent deny"
    );
    assert_eq!(
        line("watched"),
        "yoke observe (no trusted policy) | allowed 1 \u{b7} would-deny 1 | ! recent deny"
    );
    // A session no hook has recorded yet has no counts to show.
    assert_eq!(line("brand-new"), "yoke (no data yet) (no trusted policy)");
}

#[test]
fn the_callback_supplies_model_cost_duration_lines_and_context() {
    let home = tempfile::tempdir().unwrap();
    let input = json!({
        "model": {"id": "m-1", "display_name": "Opus"},
        "cost": {
            "total_cost_usd": 0.2858,
            "total_duration_ms": 257_506,
            "total_lines_added": 12,
            "total_lines_removed": 3
        },
        "context_window": {"used_percentage": 42}
    });
    assert_eq!(
        line_at(home.path(), &input, &plain()),
        "yoke (no data yet) | $0.29 \u{b7} 4m \u{b7} +12/-3 | ctx 42% \u{2588}\u{2588}\u{2588}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}"
    );
    let mut detailed = plain();
    detailed.look.detailed = true;
    let both = line_at(home.path(), &input, &detailed);
    assert_eq!(both.lines().count(), 2, "{both}");
    assert_eq!(both.lines().nth(1), Some("model Opus"));
    // Fields the callback lacks are left out, not zero.
    assert_eq!(
        line_at(home.path(), &json!({"cost": {}}), &plain()),
        "yoke (no data yet)"
    );
}

#[test]
fn colors_ascii_and_width_come_from_the_environment() {
    let vars = |pairs: &'static [(&'static str, &'static str)]| {
        move |name: &str| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    };
    let settings = |style, pairs| Settings::from_vars(style, &vars(pairs), || Some(77));
    let default = settings(Style::Compact, &[]);
    assert!(default.look.color && !default.look.ascii && !default.look.detailed);
    // The terminal is asked only when nothing else decides the width.
    assert_eq!(default.look.width, 77);
    // An empty NO_COLOR counts as unset; any other value turns colors off.
    assert!(settings(Style::Compact, &[("NO_COLOR", "")]).look.color);
    assert!(!settings(Style::Compact, &[("NO_COLOR", "1")]).look.color);
    assert!(settings(Style::Compact, &[("YOKE_ASCII", "1")]).look.ascii);
    assert!(!settings(Style::Compact, &[("YOKE_ASCII", "0")]).look.ascii);
    assert!(settings(Style::Detailed, &[]).look.detailed);
    let width = |pairs| settings(Style::Compact, pairs).look.width;
    assert_eq!(width(&[("COLUMNS", "64")]), 64);
    assert_eq!(
        width(&[("COLUMNS", "64"), ("YOKE_STATUSLINE_WIDTH", "50")]),
        50
    );
    assert_eq!(width(&[("COLUMNS", "wide")]), 77);
    assert_eq!(width(&[("YOKE_STATUSLINE_WIDTH", "0")]), 77);
    let blind = Settings::from_vars(Style::Compact, &|_: &str| None, || None);
    assert_eq!(blind.look.width, DEFAULT_WIDTH);
}

#[test]
fn the_branch_and_a_cached_count_show_without_waiting_for_git() {
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    std::fs::create_dir(repo.path().join(".git")).unwrap();
    std::fs::write(repo.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    let input = json!({"workspace": {"current_dir": repo.path()}});
    let mut settings = plain();
    settings.git = true;

    // Without a state directory there is no cache and nothing is started: branch only.
    assert_eq!(
        line_at(home.path(), &input, &settings),
        "yoke (no data yet) | main"
    );

    // A fresh cached count is shown as it is and starts nothing.
    std::fs::create_dir(home.path().join(".yoke")).unwrap();
    cache::save(
        home.path(),
        "statusline-git.json",
        &repo.path().to_string_lossy(),
        cache::Entry {
            at: NOW - 2,
            started: 0,
            value: json!({"state": "ok", "files": 3, "capped": false}),
        },
    )
    .unwrap();
    assert_eq!(
        line_at(home.path(), &input, &settings),
        "yoke (no data yet) | main (3 changed)"
    );
}

#[test]
fn a_cached_health_problem_is_shown_and_a_fresh_one_starts_nothing() {
    let home = tempfile::tempdir().unwrap();
    drop(state(home.path()));
    let dir = home.path().join("work");
    std::fs::create_dir(&dir).unwrap();
    cache::save(
        home.path(),
        HEALTH_FILE,
        &dir.to_string_lossy(),
        cache::Entry {
            at: NOW - 5,
            started: 0,
            value: json!({"problem": "Yoke hooks are not installed for any agent."}),
        },
    )
    .unwrap();
    let mut settings = plain();
    settings.health = true;
    let input = json!({"cwd": dir});
    assert_eq!(
        cached_problem(home.path(), &dir, NOW).as_deref(),
        Some("Yoke hooks are not installed for any agent.")
    );
    assert!(line_at(home.path(), &input, &settings)
        .ends_with("! Yoke hooks are not installed for any agent."));
}

#[test]
fn a_wrapped_command_keeps_its_room_when_the_line_is_narrow() {
    let home = tempfile::tempdir().unwrap();
    let input = json!({
        "cost": {"total_cost_usd": 1.5, "total_duration_ms": 600_000},
        "context_window": {"used_percentage": 50}
    });
    let mut settings = plain();
    settings.look.width = 40;
    let rendered = refresh_at(
        home.path(),
        input.to_string().as_bytes(),
        Some("echo wrapped-output"),
        NOW,
        0,
        &settings,
    )
    .unwrap();
    assert!(rendered.line.chars().count() <= 40, "{}", rendered.line);
    assert!(
        rendered.line.ends_with(" | wrapped-output"),
        "{}",
        rendered.line
    );
    assert!(rendered.line.starts_with("yoke (no data yet)"));
}

#[test]
fn a_wrapper_may_close_stdin_without_reading_the_callback() {
    // Exceed the pipe capacity on both macOS and Linux so early close is deterministic.
    let input = vec![b' '; 256 * 1024];
    assert_eq!(
        run_wrapper("exec 0<&-; printf wrapped-output", &input).unwrap(),
        "wrapped-output"
    );
    assert!(run_wrapper("exec 0<&-; exit 7", &input).is_err());
}
