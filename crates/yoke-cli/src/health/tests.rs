use super::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use tempfile::TempDir;

fn fixture() -> (TempDir, install::InstallOptions) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let options = install::InstallOptions {
        claude_settings: root.join("claude/settings.json"),
        codex_hooks: root.join("codex/hooks.json"),
        codex_config: root.join("codex/config.toml"),
        claude_managed: root.join("managed"),
        state_dir: root.join(".yoke"),
        executable: root.join("bin/yoke"),
        agents: vec![install::Agent::Claude, install::Agent::Codex],
        dry_run: false,
        statusline: false,
    };
    put(&options.executable, "");
    fs::set_permissions(&options.executable, fs::Permissions::from_mode(0o755)).unwrap();
    (dir, options)
}

fn put(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn inputs(root: &Path, options: install::InstallOptions) -> Inputs {
    Inputs {
        home: root.to_path_buf(),
        cwd: root.to_path_buf(),
        executable: options.executable.clone(),
        options: Ok(options),
    }
}

fn names(report: &Report) -> Vec<&'static str> {
    report
        .agents
        .iter()
        .map(|agent| agent.install.agent)
        .collect()
}

fn agent<'a>(report: &'a Report, name: &str) -> &'a AgentReport {
    report
        .agents
        .iter()
        .find(|agent| agent.install.agent == name)
        .unwrap_or_else(|| panic!("no {name} agent in {:?}", names(report)))
}

#[test]
fn receipt_problems_survive_for_an_agent_reported_as_not_installed() {
    let (dir, options) = fixture();
    let root = dir.path();
    install::install(&options).unwrap();
    put(&root.join(".yoke/install-claude.json"), "{}");
    let report = collect(inputs(root, options)).unwrap();
    assert_eq!(report.agents.len(), 2, "{:?}", names(&report));
    let claude = &agent(&report, "claude").install;
    assert!(!claude.installed, "{claude:?}");
    let codex = &agent(&report, "codex").install;
    assert!(codex.installed, "{codex:?}");
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem.starts_with("claude: installation receipt is unreadable")),
        "{:?}",
        report.problems
    );
    assert!(
        !report
            .guidance
            .iter()
            .any(|line| line.contains("yoke install --agent claude")),
        "{:?}",
        report.guidance
    );
    assert_eq!(
        report.status, "failing",
        "{:?} {:?}",
        report.problems, report.guidance
    );
}

#[test]
fn an_empty_state_database_is_reported_without_losing_agents() {
    let (dir, options) = fixture();
    let root = dir.path();
    install::install(&options).unwrap();
    put(&root.join(".yoke/state.db"), "");
    let report = collect(inputs(root, options)).unwrap();
    assert_eq!(report.agents.len(), 2, "{:?}", names(&report));
    for agent in &report.agents {
        assert!(agent.install.installed, "{:?}", agent.install);
        let last = agent
            .last_event
            .as_ref()
            .map(|last| (&last.event, &last.outcome));
        assert!(last.is_none(), "{}: {last:?}", agent.install.agent);
    }
    assert!(report.state.exists, "{:?}", report.state.path);
    assert_eq!(report.state.schema, Some(0), "{:?}", report.problems);
    let problems = &report.problems;
    assert!(
        problems
            .iter()
            .any(|p| p.contains("is empty or incomplete")),
        "{problems:?}"
    );
    assert!(
        !problems.iter().any(|p| p.contains("no such table")),
        "{problems:?}"
    );
    assert!(
        !problems
            .iter()
            .any(|p| p.contains("cannot locate agent settings")),
        "{problems:?}"
    );
    assert_eq!(
        report.status, "failing",
        "{problems:?} {:?}",
        report.guidance
    );
    assert!(
        !report
            .guidance
            .iter()
            .any(|line| line.contains("hook event is recorded yet")),
        "{:?}",
        report.guidance
    );
}
