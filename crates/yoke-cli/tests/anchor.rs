//! A session keeps the project it started in: the directory an event reports is chosen by the
//! agent, so it must not decide whether the trusted policy applies or where gates run.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};
use tempfile::TempDir;
use yoke_core::{
    store::{DecisionFilter, Store},
    trust,
};

struct Fixture {
    _temp: TempDir,
    home: PathBuf,
    project: PathBuf,
    outside: PathBuf,
}

impl Fixture {
    /// A trusted enforce policy at `project` with one deny gate and one check gate.
    fn new(policy: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let (home, project, outside) = (base.join("home"), base.join("project"), base.join("out"));
        for dir in [home.join(".yoke"), project.join("sub"), outside.clone()] {
            fs::create_dir_all(dir).unwrap();
        }
        let f = Self {
            _temp: temp,
            home,
            project,
            outside,
        };
        fs::write(f.project.join("yoke.toml"), policy).unwrap();
        let store = f.store();
        trust::prepare_project(&store, &f.project)
            .unwrap()
            .commit(&store)
            .unwrap();
        f
    }

    fn store(&self) -> Store {
        Store::open(self.home.join(".yoke/state.db")).unwrap()
    }

    fn hook(&self, agent: &str, session: &str, command: &str, cwd: &Path) -> Output {
        self.hook_with(agent, session, command, cwd, None)
    }

    fn hook_with(
        &self,
        agent: &str,
        session: &str,
        command: &str,
        cwd: &Path,
        project_dir: Option<&Path>,
    ) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_yoke"));
        child
            .args(["run-hook", "--agent", agent, "PreToolUse"])
            // The hook runs in the directory the tool call reports.
            .current_dir(cwd)
            .env("HOME", &self.home)
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME")
            .env_remove("YOKE_OFF")
            .env_remove("CLAUDE_PROJECT_DIR")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = project_dir {
            child.env("CLAUDE_PROJECT_DIR", dir);
        }
        let mut child = child.spawn().unwrap();
        let event = json!({
            "session_id": session,
            "tool_name": "Bash",
            "tool_input": {"command": command},
            "cwd": cwd,
        });
        child
            .stdin
            .take()
            .unwrap()
            .write_all(event.to_string().as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    /// (outcome, project) of every decision, oldest first.
    fn decisions(&self) -> Vec<(String, String)> {
        let mut rows = self.store().decisions(&DecisionFilter::default()).unwrap();
        rows.reverse();
        rows.into_iter().map(|r| (r.outcome, r.project)).collect()
    }
}

fn stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
}

const DENY_RM: &str = "mode='enforce'\n[[gate]]\nname='no-rm'\non='pre_tool_use'\naction='deny'\nmatch_tool='Bash'\nmatch_contains='rm -rf'\nmessage='no rm -rf allowed'\n";

#[test]
fn no_directory_an_event_reports_turns_the_project_policy_off() {
    for agent in ["claude", "codex"] {
        let f = Fixture::new(DENY_RM);
        let link = f.project.join("link");
        std::os::unix::fs::symlink(&f.outside, &link).unwrap();
        // The first event starts the session at the project root; the rest report a directory
        // with an untrusted yoke.toml, one outside the project, and a symlink that leaves it.
        fs::write(f.project.join("sub/yoke.toml"), "").unwrap();
        let cwds = [
            f.project.clone(),
            f.project.join("sub"),
            f.outside.clone(),
            link,
        ];
        for cwd in &cwds {
            let output = f.hook(agent, "s", "rm -rf /tmp/x", cwd);
            assert!(
                String::from_utf8_lossy(&output.stdout).contains("no rm -rf allowed"),
                "{agent} cwd {}: {output:?}",
                cwd.display()
            );
        }
        let project = f.project.to_str().unwrap();
        assert_eq!(
            f.decisions(),
            vec![("deny".to_string(), project.to_string()); cwds.len()],
            "{agent}: every event belongs to the one project"
        );
        // The file nobody trusted is reported when it is first seen, not at the next session.
        let planted = f.project.join("sub/yoke.toml");
        let reported: i64 = rusqlite::Connection::open(f.home.join(".yoke/state.db"))
            .unwrap()
            .query_row(
                "SELECT count(*) FROM notifications WHERE kind='config_untrusted' AND subject=?1",
                [planted.to_str().unwrap()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(reported, 1, "{agent}: the ignored yoke.toml is reported");
    }
}

#[test]
fn gates_run_in_the_project_even_when_the_agent_works_elsewhere() {
    let f = Fixture::new(
        "mode='enforce'\n[[gate]]\nname='lint'\non='pre_tool_use'\nmatch_tool='Bash'\nrun='sh check.sh'\nmessage='lint failed'\n",
    );
    fs::write(f.project.join("check.sh"), "exit 1").unwrap();
    // A passing look-alike the agent put where it now works.
    fs::write(f.project.join("sub/check.sh"), "exit 0").unwrap();
    for cwd in [f.project.clone(), f.project.join("sub")] {
        let output = f.hook("claude", "s", "git commit", &cwd);
        assert_eq!(
            stdout(&output)["hookSpecificOutput"]["permissionDecision"],
            "deny",
            "cwd {}: {output:?}",
            cwd.display()
        );
    }
}

#[test]
fn a_session_whose_project_directory_was_moved_is_governed_by_the_directory_it_reports() {
    let f = Fixture::new(DENY_RM);
    f.hook("claude", "s", "ls", &f.project);
    let moved = f.outside.join("moved");
    fs::rename(&f.project, &moved).unwrap();
    // The trusted policy belongs to the old path, so nothing is denied and nothing errors.
    let output = f.hook("claude", "s", "rm -rf /tmp/x", &moved);
    assert!(output.stdout.is_empty(), "{output:?}");
    assert_eq!(f.decisions().last().unwrap().0, "allow");
}

#[test]
fn the_project_dir_claude_names_anchors_a_session_that_first_reports_another_directory() {
    let f = Fixture::new(DENY_RM);
    let output = f.hook_with("claude", "s", "rm -rf /tmp/x", &f.outside, Some(&f.project));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("no rm -rf allowed"),
        "{output:?}"
    );
    // Without the variable the same first event would have started the session outside.
    let output = f.hook("claude", "other", "rm -rf /tmp/x", &f.outside);
    assert!(output.stdout.is_empty(), "{output:?}");
}
