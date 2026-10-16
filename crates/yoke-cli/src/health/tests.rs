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

#[test]
fn mode_policy_freshness_last_event_and_plan_guidance_are_reported_separately() {
    use yoke_core::{store::DecisionRecord, trust};
    let (dir, options) = fixture();
    let root = dir.path();
    install::install(&options).unwrap();
    put(&root.join("yoke.toml"), "mode='observe'\n");
    fs::create_dir_all(root.join(".yoke")).unwrap();
    let store = Store::open(root.join(".yoke/state.db")).unwrap();
    trust::prepare_project(&store, root)
        .unwrap()
        .commit(&store)
        .unwrap();
    // Editing the file after trust leaves the trusted bytes in force and marks them stale.
    put(&root.join("yoke.toml"), "mode='enforce'\n");
    store
        .record_decision(
            &DecisionRecord {
                ts: 5,
                session_id: "health".into(),
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
    drop(store);

    let report = collect(inputs(root, options)).unwrap();

    assert_eq!(report.mode, Some("observe"));
    assert!(
        report
            .policy
            .iter()
            .any(|policy| policy.trusted && policy.stale),
        "{}",
        serde_json::to_string(&report.policy).unwrap()
    );
    let last = agent(&report, "claude").last_event.as_ref().unwrap();
    assert_eq!(
        (last.event.as_str(), last.outcome.as_str()),
        ("session_start", "allow")
    );
    for expected in ["Observe mode records", "No unfinished plan"] {
        assert!(
            report.guidance.iter().any(|line| line.contains(expected)),
            "{expected}: {:?}",
            report.guidance
        );
    }
}

fn guidance_with(report: &Report, needle: &str) -> bool {
    report.guidance.iter().any(|line| line.contains(needle))
}

#[test]
fn untrusted_files_are_named_with_their_own_trust_command_and_absent_ones_are_not_listed_as_work() {
    let (dir, options) = fixture();
    let root = dir.path();
    install::install(&options).unwrap();

    let empty = collect(inputs(root, options.clone())).unwrap();
    assert!(
        empty.policy.iter().all(|policy| !policy.exists),
        "{:?}",
        serde_json::to_string(&empty.policy)
    );
    assert!(guidance_with(&empty, "yoke init"), "{:?}", empty.guidance);
    assert!(
        guidance_with(&empty, "preset add git-safety --global"),
        "{:?}",
        empty.guidance
    );

    put(&root.join("yoke.toml"), "mode='observe'\n");
    put(&root.join(".yoke/config.toml"), "mode='observe'\n");
    let report = collect(inputs(root, options)).unwrap();
    let canonical = root.canonicalize().unwrap();
    let global = format!("{} ", canonical.join(".yoke/config.toml").display());
    let project = format!("{} ", canonical.join("yoke.toml").display());
    assert!(
        report
            .guidance
            .iter()
            .any(|line| line.starts_with(&global) && line.contains("`yoke trust --global`")),
        "{:?}",
        report.guidance
    );
    assert!(
        report.guidance.iter().any(|line| {
            line.starts_with(&project)
                && line.contains("`yoke trust`")
                && !line.contains("--global")
        }),
        "{:?}",
        report.guidance
    );
    assert!(
        !guidance_with(&report, "yoke init"),
        "{:?}",
        report.guidance
    );
}

#[test]
fn every_unfinished_plan_gets_the_next_step_for_its_state() {
    let (dir, options) = fixture();
    let root = dir.path();
    install::install(&options).unwrap();
    let project = root.canonicalize().unwrap();
    drop(Store::open(root.join(".yoke/state.db")).unwrap());
    // Held open: the last connection to close removes the -wal and -shm files that
    // read-only commands need.
    let db = rusqlite::Connection::open(root.join(".yoke/state.db")).unwrap();
    for (id, slug, state) in [
        (1, "alpha", "draft"),
        (2, "beta", "approved"),
        (3, "gamma", "stalled"),
    ] {
        db.execute(
            "INSERT INTO plans(id,project,slug,file,state,created_at,updated_at) VALUES(?1,?2,?3,'p.md',?4,1,1)",
            rusqlite::params![id, project.to_str().unwrap(), slug, state],
        )
        .unwrap();
    }

    let report = collect(inputs(root, options)).unwrap();
    drop(db);

    for expected in [
        "yoke plan approve alpha",
        "yoke plan start beta --session",
        "yoke plan resume gamma",
    ] {
        assert!(
            guidance_with(&report, expected),
            "{expected}: {:?}",
            report.guidance
        );
    }
    assert!(
        !guidance_with(&report, "No unfinished plan"),
        "{:?}",
        report.guidance
    );
}

#[test]
fn a_missing_hook_executable_comes_with_the_repair_command() {
    let (dir, options) = fixture();
    let root = dir.path();
    install::install(&options).unwrap();
    fs::remove_file(&options.executable).unwrap();

    let report = collect(inputs(root, options)).unwrap();

    assert!(
        report.problems.iter().any(|problem| {
            problem.starts_with("claude: hook executable")
                && problem.contains("yoke uninstall --agent claude")
        }),
        "{:?}",
        report.problems
    );
}

#[test]
fn codex_approval_records_are_counted_and_silence_is_explained() {
    let (dir, options) = fixture();
    let root = dir.path();
    install::install(&options).unwrap();

    let none = collect(inputs(root, options.clone())).unwrap();
    assert!(
        guidance_with(&none, "no approval record for 7 of 7 Yoke hooks"),
        "{:?}",
        none.guidance
    );
    assert!(
        guidance_with(&none, "skips hooks you have not approved"),
        "{:?}",
        none.guidance
    );

    let hooks = options.codex_hooks.display().to_string();
    let mut config = fs::read_to_string(&options.codex_config).unwrap();
    for event in ["pre_tool_use", "post_tool_use", "stop"] {
        config.push_str(&format!(
            "\n[hooks.state.\"{hooks}:{event}:0:0\"]\ntrusted_hash = \"sha256:x\"\n"
        ));
    }
    put(&options.codex_config, &config);
    let some = collect(inputs(root, options.clone())).unwrap();
    assert!(
        guidance_with(&some, "no approval record for 4 of 7 Yoke hooks"),
        "{:?}",
        some.guidance
    );

    for event in [
        "pre_compact",
        "session_start",
        "session_end",
        "user_prompt_submit",
    ] {
        config.push_str(&format!(
            "\n[hooks.state.\"{hooks}:{event}:0:0\"]\ntrusted_hash = \"sha256:x\"\n"
        ));
    }
    put(&options.codex_config, &config);
    let all = collect(inputs(root, options)).unwrap();
    assert!(
        !guidance_with(&all, "no approval record"),
        "{:?}",
        all.guidance
    );
    assert!(
        guidance_with(&all, "approval records exist for all 7"),
        "{:?}",
        all.guidance
    );
}

#[test]
fn a_database_from_a_newer_yoke_is_not_blamed_on_the_disk() {
    let (dir, options) = fixture();
    let root = dir.path();
    install::install(&options).unwrap();
    drop(Store::open(root.join(".yoke/state.db")).unwrap());
    let db = rusqlite::Connection::open(root.join(".yoke/state.db")).unwrap();
    db.pragma_update(None, "user_version", 99).unwrap();

    let report = collect(inputs(root, options)).unwrap();
    drop(db);

    let problem = report
        .problems
        .iter()
        .find(|problem| problem.contains("newer than supported"))
        .unwrap_or_else(|| panic!("{:?}", report.problems));
    assert!(problem.contains("yoke uninstall"), "{problem}");
    assert!(!problem.contains("free disk space"), "{problem}");
}

#[test]
fn claude_without_the_statusline_is_pointed_at_it_once_it_is_installed() {
    let (dir, options) = fixture();
    let root = dir.path();
    install::install(&options).unwrap();

    let without = collect(inputs(root, options.clone())).unwrap();
    assert!(!agent(&without, "claude").install.statusline);
    assert!(
        guidance_with(&without, "yoke install --agent claude --statusline"),
        "{:?}",
        without.guidance
    );

    // Reinstalling with the flag adds the statusline to the same hooks.
    install::install(&install::InstallOptions {
        statusline: true,
        agents: vec![install::Agent::Claude],
        ..options.clone()
    })
    .unwrap();
    let with = collect(inputs(root, options)).unwrap();
    assert!(agent(&with, "claude").install.statusline);
    assert!(!guidance_with(&with, "--statusline"), "{:?}", with.guidance);
}
