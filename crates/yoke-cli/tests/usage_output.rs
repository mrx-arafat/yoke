use std::process::{Command, Output};

fn usage(home: &std::path::Path, width: &str, json: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_yoke"));
    command
        .arg("usage")
        .env("HOME", home)
        .env("COLUMNS", width)
        .env("TERM", "xterm-256color")
        .env("TZ", "UTC")
        .env_remove("NO_COLOR")
        .current_dir(home);
    if json {
        command.arg("--json");
    }
    let output = command.output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    output
}

#[test]
fn redirected_usage_wraps_without_color_and_json_ignores_presentation_settings() {
    let home = tempfile::tempdir().unwrap();
    let human = String::from_utf8(usage(home.path(), "40", false).stdout).unwrap();
    assert!(!human.contains('\x1b'));
    assert!(
        human.lines().all(|line| line.chars().count() <= 40),
        "{human}"
    );
    for heading in [
        "Quota / account limits",
        "Token history",
        "Recent sessions",
        "Notes",
    ] {
        assert!(human.contains(heading), "{human}");
    }
    let wide = usage(home.path(), "120", true);
    let narrow = usage(home.path(), "1", true);
    let stable = |output: Output| {
        let raw = String::from_utf8(output.stdout).unwrap();
        assert!(!raw.contains('\x1b'));
        let report: serde_json::Value = serde_json::from_str(&raw).unwrap();
        // The wall clock can tick between child processes; everything else must match bytewise.
        raw.replacen(&format!("\"now\": {}", report["now"]), "\"now\": 0", 1)
    };
    assert_eq!(stable(wide), stable(narrow));
    assert!(!home.path().join(".yoke").exists());
}
