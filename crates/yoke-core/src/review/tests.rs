use super::*;
use std::fs;

/// Opt-in acceptance only: never inherits credentials or the user's agent settings.
#[test]
#[ignore = "requires an explicitly authorized sequential real reviewer slot"]
fn isolated_real_codex_acceptance() {
    real_codex_acceptance(true);
}

/// Uses the CLI's existing authentication without inspecting or copying it.
#[test]
#[ignore = "requires an explicitly authorized sequential authenticated reviewer slot"]
fn authenticated_real_codex_acceptance() {
    real_codex_acceptance(false);
}

fn real_codex_acceptance(isolate_auth: bool) {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    let home = root.path().join("home");
    let codex_home = root.path().join("codex");
    for directory in [&project, &home, &codex_home] {
        fs::create_dir(directory).unwrap();
    }
    // Empty disposable Git metadata, without Git write commands or copied config.
    fs::create_dir_all(project.join(".git/objects")).unwrap();
    fs::create_dir_all(project.join(".git/refs/heads")).unwrap();
    fs::write(project.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(project.join("answer.rs"), "pub fn answer() -> u32 { 42 }\n").unwrap();
    let mut child = command(Reviewer::Codex);
    if isolate_auth {
        child
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &home)
            .env("CODEX_HOME", &codex_home)
            .env("YOKE_OFF", "1");
    }
    let report = review_diff(
        &project,
        &mut child,
        Duration::from_secs(120),
        "diff --git a/answer.rs b/answer.rs\nnew file mode 100644\n--- /dev/null\n+++ b/answer.rs\n@@ -0,0 +1 @@\n+pub fn answer() -> u32 { 42 }\n",
    ).unwrap();
    println!("{}", serde_json::to_string(&report).unwrap());
    assert!(
        report.error.is_none(),
        "isolated real reviewer did not complete"
    );
}

fn fixture(root: &Path, response: &str, exit: i32) -> Command {
    fs::write(root.join("response"), response).unwrap();
    let script = root.join("review-fixture.sh");
    fs::write(&script, "test \"$YOKE_OFF\" = 1 || exit 41\ncat > \"$FIXTURE_INPUT\"\ncat \"$FIXTURE_RESPONSE\"\nexit \"$FIXTURE_EXIT\"\n").unwrap();
    let mut command = Command::new("sh");
    command
        .arg(script)
        .env("FIXTURE_INPUT", root.join("received"))
        .env("FIXTURE_RESPONSE", root.join("response"))
        .env("FIXTURE_EXIT", exit.to_string());
    command
}

#[test]
fn fixtures_preserve_findings_and_fail_closed_on_invalid_results() {
    let root = tempfile::tempdir().unwrap();
    for (json, exit, passed, count, error) in [
        ("[]", 0, true, 0, false),
        (
            r#"[{"severity":"advisory","file":"src/a.rs","line":2,"summary":"Consider clearer wording"}]"#,
            0,
            true,
            1,
            false,
        ),
        (
            r#"[{"severity":"blocking","file":"src/a.rs","line":2,"summary":"Drops a pending write"}]"#,
            0,
            false,
            1,
            false,
        ),
        ("[]", 2, false, 0, true),
        ("```json\n[]\n```", 0, false, 0, true),
        (
            r#"[{"severity":"blocking","file":"a.rs","line":1,"summary":"x","extra":true}]"#,
            0,
            false,
            0,
            true,
        ),
        (
            r#"[{"severity":"advisory","file":"../secret","line":0,"summary":""}]"#,
            0,
            false,
            0,
            true,
        ),
    ] {
        let report = review_diff(
            root.path(),
            &mut fixture(root.path(), json, exit),
            Duration::from_secs(2),
            "diff fixture",
        )
        .unwrap();
        assert_eq!(report.passed, passed, "{json}");
        assert_eq!(report.findings.len(), count, "{json}");
        assert_eq!(report.error.is_some(), error, "{json}");
        assert_eq!(report.exit_code, Some(exit));
        assert!(fs::read_to_string(root.path().join("received"))
            .unwrap()
            .contains("diff fixture"));
    }
}

#[test]
fn fixture_timeout_and_truncated_valid_tail_cannot_pass_review() {
    let root = tempfile::tempdir().unwrap();
    let mut slow = Command::new("sh");
    slow.args(["-c", "cat >/dev/null; sleep 30"]);
    let report = review_diff(root.path(), &mut slow, Duration::from_millis(100), "diff").unwrap();
    assert!(!report.passed);
    assert!(report.error.unwrap().contains("timed out"));
    let oversized = format!("{}[]", " ".repeat(RESPONSE_CAP));
    let report = review_diff(
        root.path(),
        &mut fixture(root.path(), &oversized, 0),
        Duration::from_secs(2),
        "diff",
    )
    .unwrap();
    assert!(!report.passed);
    assert!(report.error.unwrap().contains("exceeded"));
    assert!(report.output_tail.len() <= 4096);
}

#[test]
fn real_git_diff_feeds_fixture_and_missing_executable_returns_failed_evidence() {
    let root = tempfile::tempdir().unwrap();
    let project = Path::new(env!("CARGO_MANIFEST_DIR"));
    let report = run_with_command(
        project,
        &mut fixture(root.path(), "[]", 0),
        Duration::from_secs(2),
    )
    .unwrap();
    assert!(report.passed, "{:?}", report.error);
    assert!(fs::read_to_string(root.path().join("received"))
        .unwrap()
        .contains("DIFF DATA:"));
    let report = run_with_command(
        project,
        &mut Command::new(root.path().join("missing-reviewer")),
        Duration::from_secs(2),
    )
    .unwrap();
    assert!(!report.passed);
    assert!(report.error.unwrap().contains("execute reviewer"));
}

#[test]
fn untracked_source_and_symlink_metadata_reach_reviewer_without_following_links() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("new.rs"),
        "fn untracked_only_feature() { panic!(\"broken\"); }",
    )
    .unwrap();
    std::os::unix::fs::symlink("/outside/not-read", root.path().join("link")).unwrap();
    let changes = input::append_untracked(root.path(), String::new(), b"new.rs\0link\0").unwrap();
    let report = review_diff(
        root.path(),
        &mut fixture(root.path(), "[]", 0),
        Duration::from_secs(2),
        &changes,
    )
    .unwrap();
    assert!(report.passed);
    let received = fs::read_to_string(root.path().join("received")).unwrap();
    assert!(received.contains("untracked_only_feature"));
    assert!(received.contains("/outside/not-read"));
    assert!(received.contains("symlink"));
    fs::write(root.path().join("binary"), [0, 1, 2]).unwrap();
    assert!(input::append_untracked(root.path(), String::new(), b"binary\0").is_err());
    assert!(input::append_untracked(root.path(), String::new(), b"../outside\0").is_err());
}

#[test]
fn reviewer_selection_and_commands_keep_the_public_readonly_contract() {
    assert_eq!(Reviewer::select("auto", "claude").unwrap(), Reviewer::Codex);
    assert_eq!(Reviewer::select("auto", "codex").unwrap(), Reviewer::Claude);
    assert!(Reviewer::select("auto", "unknown").is_err());
    assert!(Reviewer::select("unknown", "claude").is_err());
    for reviewer in [Reviewer::Claude, Reviewer::Codex] {
        let command = command(reviewer);
        assert!(command
            .get_envs()
            .any(|(key, value)| key == "YOKE_OFF" && value == Some(std::ffi::OsStr::new("1"))));
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect();
        match reviewer {
            Reviewer::Claude => {
                assert!(args.windows(2).any(|args| args == ["--tools", ""]));
                assert!(args
                    .windows(2)
                    .any(|args| args == ["--output-format", "text"]));
            }
            Reviewer::Codex => {
                assert!(args
                    .windows(2)
                    .any(|args| args == ["--sandbox", "read-only"]));
                assert!(args
                    .windows(2)
                    .any(|args| args == ["--disable", "shell_tool"]));
                assert!(args
                    .windows(2)
                    .any(|args| args == ["--disable", "unified_exec"]));
            }
        }
    }
}
