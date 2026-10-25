use super::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use tempfile::TempDir;

#[test]
fn health_reports_group_and_other_access_without_changing_permissions() {
    let (dir, options) = fixture();
    let state = dir.path().join(".yoke");
    fs::create_dir_all(&state).unwrap();
    let db = state.join("state.db");
    drop(Store::open(&db).unwrap());
    fs::set_permissions(&state, fs::Permissions::from_mode(0o750)).unwrap();
    fs::set_permissions(&db, fs::Permissions::from_mode(0o604)).unwrap();
    let report = collect(inputs(dir.path(), options)).unwrap();
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.contains(".yoke") && p.contains("chmod 700")),
        "{:?}",
        report.problems
    );
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.contains("state.db") && p.contains("chmod 600")),
        "{:?}",
        report.problems
    );
    assert_eq!(
        fs::metadata(state).unwrap().permissions().mode() & 0o777,
        0o750
    );
    assert_eq!(
        fs::metadata(db).unwrap().permissions().mode() & 0o777,
        0o604
    );
}

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
        statusline: install::Statusline::Off,
        statusline_style: None,
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
                policy_hash: None,
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
fn stale_policy_guidance_uses_its_scope_and_restores_missing_files_before_trust() {
    use yoke_core::trust;
    let (dir, options) = fixture();
    let root = dir.path();
    let global = root.join(".yoke/config.toml");
    let project = root.join("yoke.toml");
    let knowledge = root.join(".yoke/knowledge.md");
    put(&global, "mode='observe'\n");
    put(&project, "mode='observe'\n");
    put(&knowledge, "# Notes\n");
    let store = Store::open(root.join(".yoke/state.db")).unwrap();
    trust::prepare_global(&store, root)
        .unwrap()
        .commit(&store)
        .unwrap();
    trust::prepare_project(&store, root)
        .unwrap()
        .commit(&store)
        .unwrap();
    for path in [&global, &project, &knowledge] {
        fs::write(path, "# changed\n").unwrap();
    }
    for missing in [false, true] {
        if missing {
            for path in [&global, &project, &knowledge] {
                fs::remove_file(path).unwrap();
            }
        }
        let report = collect(inputs(root, options.clone())).unwrap();
        for (path, command) in [
            (&global, "`yoke trust --global`"),
            (&project, "`yoke trust`"),
            (&knowledge, "`yoke trust`"),
        ] {
            let line = report
                .guidance
                .iter()
                .find(|line| {
                    line.starts_with(&path.display().to_string()) && line.contains("trusted bytes")
                })
                .unwrap();
            assert!(line.contains(command), "{line}");
            assert_eq!(line.contains("restore"), missing, "{line}");
        }
    }
}

#[test]
fn observe_guidance_names_the_effective_mode_source_or_an_existing_policy() {
    use yoke_core::trust;
    for (global, project, scope) in [
        ("mode='observe'", Some("[meta]\nname='demo'"), "global"),
        ("mode='enforce'", Some("mode='observe'"), "project"),
        ("[meta]\nname='demo'", None, "global"),
        (
            "[meta]\nname='demo'",
            Some("[meta]\nname='demo'"),
            "project",
        ),
    ] {
        let (dir, options) = fixture();
        let root = dir.path();
        let global_path = root.join(".yoke/config.toml");
        let project_path = root.join("yoke.toml");
        put(&global_path, global);
        let store = Store::open(root.join(".yoke/state.db")).unwrap();
        trust::prepare_global(&store, root)
            .unwrap()
            .commit(&store)
            .unwrap();
        if let Some(project) = project {
            put(&project_path, project);
            trust::prepare_project(&store, root)
                .unwrap()
                .commit(&store)
                .unwrap();
        }
        let report = collect(inputs(root, options)).unwrap();
        let line = report
            .guidance
            .iter()
            .find(|line| line.starts_with("Observe mode records"))
            .unwrap();
        let (path, command) = if scope == "global" {
            (&global_path, "`yoke trust --global`")
        } else {
            (&project_path, "`yoke trust`")
        };
        assert!(line.contains(&path.display().to_string()), "{line}");
        assert!(line.contains(command), "{line}");
    }
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
fn a_failing_retention_job_is_reported_until_a_later_one_succeeds() {
    let (dir, options) = fixture();
    let root = dir.path();
    install::install(&options).unwrap();
    drop(Store::open(root.join(".yoke/state.db")).unwrap());
    // Held open: the last connection to close removes the -wal and -shm files that
    // read-only commands need.
    let db = rusqlite::Connection::open(root.join(".yoke/state.db")).unwrap();
    let job = |id: i64, state: &str| {
        db.execute(
            "INSERT INTO jobs(id,kind,dedupe_key,project,payload,state,attempts,max_attempts,run_after,last_error,created_at,updated_at)
             VALUES(?1,'retention','retention','','{}',?2,3,3,1,'disk full',1,1)",
            rusqlite::params![id, state],
        )
        .unwrap();
    };
    job(1, "dead");
    let failing = collect(inputs(root, options.clone())).unwrap();
    assert!(
        guidance_with(&failing, "Retention cleanup is failing (job 1: disk full)"),
        "{:?}",
        failing.guidance
    );
    job(2, "succeeded");
    let recovered = collect(inputs(root, options)).unwrap();
    drop(db);
    assert!(
        !guidance_with(&recovered, "Retention cleanup"),
        "{:?}",
        recovered.guidance
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
    for (name, start) in [("claude", "`claude`"), ("codex", "`codex`")] {
        let line = none
            .guidance
            .iter()
            .find(|line| line.starts_with(&format!("No {name} hook event")))
            .unwrap();
        assert!(
            line.contains(start)
                && line.contains("`yoke log --limit 10`")
                && line.contains("`yoke health`"),
            "{line}"
        );
    }

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
        statusline: install::Statusline::Wrap,
        agents: vec![install::Agent::Claude],
        ..options.clone()
    })
    .unwrap();
    let with = collect(inputs(root, options)).unwrap();
    assert!(agent(&with, "claude").install.statusline);
    assert!(!guidance_with(&with, "--statusline"), "{:?}", with.guidance);
}

#[test]
fn a_statusline_someone_else_pointed_at_yoke_counts_as_installed() {
    let (dir, options) = fixture();
    let root = dir.path();
    install::install(&options).unwrap();
    let mut settings: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&options.claude_settings).unwrap()).unwrap();
    settings["statusLine"] =
        serde_json::json!({"type":"command","command":"'/elsewhere/bin/yoke' statusline"});
    put(&options.claude_settings, &settings.to_string());

    let report = collect(inputs(root, options.clone())).unwrap();
    assert!(agent(&report, "claude").install.statusline);
    assert!(
        !guidance_with(&report, "--statusline"),
        "{:?}",
        report.guidance
    );

    settings["statusLine"] = serde_json::json!({"type":"command","command":"echo hi"});
    put(&options.claude_settings, &settings.to_string());
    let other = collect(inputs(root, options)).unwrap();
    assert!(!agent(&other, "claude").install.statusline);
}

#[test]
fn attention_is_explained_and_pending_memory_facts_are_counted() {
    let (dir, options) = fixture();
    let root = dir.path();
    fs::set_permissions(
        options.executable.parent().unwrap(),
        fs::Permissions::from_mode(0o775),
    )
    .unwrap();
    install::install(&options).unwrap();
    fs::create_dir_all(root.join(".yoke")).unwrap();
    let store = Store::open(root.join(".yoke/state.db")).unwrap();
    let canonical = root.canonicalize().unwrap();
    let project = canonical.to_str().unwrap();
    let fact = |session: &str, project: &str, now: i64| {
        yoke_core::memory::record_fact(&store, session, project, "file_write", "Edit: x", now)
            .unwrap()
    };
    fact("a", project, 1);
    fact("a", project, 2);
    fact("b", project, 3);
    // A digested fact and a fact of another project are not this project's backlog.
    let digested = fact("c", project, 4);
    fact("d", "/elsewhere", 5);
    // Yoke's own connection must close last: it leaves the WAL files a read-only open needs.
    rusqlite::Connection::open(root.join(".yoke/state.db"))
        .unwrap()
        .execute("UPDATE facts SET digested=1 WHERE id=?1", [digested])
        .unwrap();
    drop(store);

    let report = collect(inputs(root, options)).unwrap();

    let memory = report.memory.as_ref().unwrap();
    assert_eq!((memory.pending_facts, memory.digest), (3, false));
    // Observe mode is on by default, so this is the state of a correct fresh install.
    assert_eq!(report.status, "attention");
    assert!(
        guidance_with(&report, "group-writable"),
        "{:?}",
        report.guidance
    );
    assert!(guidance_with(&report, "chmod g-w"), "{:?}", report.guidance);
    assert!(
        report.meaning.contains("nothing is broken"),
        "{}",
        report.meaning
    );
}

#[test]
fn a_database_without_wal_files_names_the_way_back_and_is_not_blamed_on_the_disk() {
    let (dir, options) = fixture();
    let root = dir.path();
    install::install(&options).unwrap();
    drop(Store::open(root.join(".yoke/state.db")).unwrap());
    // Another SQLite client's last close deletes both sidecars.
    for sidecar in ["state.db-wal", "state.db-shm"] {
        fs::remove_file(root.join(".yoke").join(sidecar)).unwrap();
    }

    let report = collect(inputs(root, options)).unwrap();

    let problem = report
        .problems
        .iter()
        .find(|problem| problem.starts_with("cannot read "))
        .unwrap_or_else(|| panic!("{:?}", report.problems));
    assert!(problem.contains("yoke log --limit 1"), "{problem}");
    assert!(!problem.contains("free disk space"), "{problem}");
    assert!(
        !root.join(".yoke/state.db-wal").exists(),
        "a read-only health check must not create files"
    );
}
