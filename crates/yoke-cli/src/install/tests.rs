use super::*;
use serde_json::{json, Value};
use std::fs;
use tempfile::TempDir;

#[test]
fn hook_diagnostics_reports_codex_managed_only_after_install() {
    let (dir, options) = fixture();
    install(&options).unwrap();
    let installed = fs::read_to_string(&options.codex_config).unwrap();
    features::check_enabled(&installed).unwrap();
    let blocked = format!("allow_managed_hooks_only = true\n{installed}");
    put(&options.codex_config, &blocked);
    assert!(features::check_enabled(&blocked)
        .unwrap_err()
        .to_string()
        .contains("managed hooks only"));
    let codex = status(&options, dir.path())
        .into_iter()
        .find(|agent| agent.agent == "codex")
        .unwrap();
    assert!(codex.installed);
    assert!(
        codex.problems.iter().any(|problem| {
            problem.contains("managed hooks only")
                && problem.contains(&options.codex_config.display().to_string())
        }),
        "{:?}",
        codex.problems
    );
    assert_eq!(fs::read_to_string(&options.codex_config).unwrap(), blocked);
}

#[test]
fn hook_diagnostics_reports_only_missing_claude_post_tool_use_failure() {
    let (_dir, options) = fixture();
    install(&options).unwrap();
    assert!(drift::managed(&options, Agent::Claude)
        .unwrap()
        .unwrap()
        .stale
        .is_empty());
    let mut settings: Value =
        serde_json::from_str(&fs::read_to_string(&options.claude_settings).unwrap()).unwrap();
    settings["hooks"]
        .as_object_mut()
        .unwrap()
        .remove("PostToolUseFailure")
        .unwrap();
    let missing = settings.to_string();
    put(&options.claude_settings, &missing);
    let stale = drift::managed(&options, Agent::Claude)
        .unwrap()
        .unwrap()
        .stale;
    assert_eq!(stale.len(), 1, "{stale:?}");
    assert_eq!(stale[0].0, "PostToolUseFailure");
    assert!(stale[0].1.starts_with("managed hook missing or edited"));
    assert!(drift::managed(&options, Agent::Codex)
        .unwrap()
        .unwrap()
        .stale
        .is_empty());
    assert_eq!(
        fs::read_to_string(&options.claude_settings).unwrap(),
        missing
    );
}

fn fixture() -> (TempDir, InstallOptions) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut options = InstallOptions {
        claude_settings: root.join("claude/settings.json"),
        codex_hooks: root.join("codex/hooks.json"),
        codex_config: root.join("codex/config.toml"),
        claude_managed: root.join("managed"),
        state_dir: root.join("state"),
        executable: root.join("bin/yoke's executable"),
        agents: vec![Agent::Claude, Agent::Codex],
        dry_run: false,
        statusline: Statusline::Off,
        statusline_style: None,
    };
    put(&options.executable, "#!/bin/sh\n");
    fs::set_permissions(
        &options.executable,
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    options.executable = options.executable.canonicalize().unwrap();
    (dir, options)
}

fn put(path: &std::path::Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn install_requires_a_regular_executable_and_records_its_canonical_target() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let (dir, mut options) = fixture();
    fs::remove_file(&options.executable).unwrap();
    assert!(install(&options).is_err());
    put(&options.executable, "#!/bin/sh\n");
    assert!(install(&options).is_err());
    fs::set_permissions(&options.executable, fs::Permissions::from_mode(0o700)).unwrap();
    let canonical = options.executable.canonicalize().unwrap();
    let link = dir.path().join("link");
    symlink(&options.executable, &link).unwrap();
    options.executable = link;
    install(&options).unwrap();
    assert_eq!(
        read_receipt(&options, Agent::Claude)
            .unwrap()
            .unwrap()
            .executable,
        canonical
    );
}

#[test]
fn executable_ancestor_permissions_distinguish_sticky_and_group_guidance() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, options) = fixture();
    install(&options).unwrap();
    let ancestor = options.executable.parent().unwrap();
    let location = ancestor.display().to_string();
    for (mode, problem, guidance) in [
        (0o1777, false, false),
        (0o777, true, false),
        (0o775, false, true),
    ] {
        fs::set_permissions(ancestor, fs::Permissions::from_mode(mode)).unwrap();
        let claude = status(&options, dir.path()).remove(0);
        assert_eq!(
            claude.problems.iter().any(|line| line.contains(&location)),
            problem,
            "mode {mode:o}: {claude:?}"
        );
        assert_eq!(
            claude
                .notes
                .iter()
                .any(|line| line.contains(&location) && line.contains("chmod g-w")),
            guidance,
            "mode {mode:o}: {claude:?}"
        );
        if guidance {
            assert!(install(&options)
                .unwrap()
                .warnings
                .iter()
                .any(|line| { line.contains(&location) && line.contains("chmod g-w") }));
        }
    }
}

#[test]
fn json_edits_preserve_inter_entry_whitespace_and_final_newline() {
    let original = "{\n\t\"theme\" : \"dark\",\n\t\"hooks\" : {\n\t\t\"Stop\" : [ { \"hooks\": [] } ]\n\t}\n}\n";
    let mut value: Value = serde_json::from_str(original).unwrap();
    value["hooks"]["Stop"]
        .as_array_mut()
        .unwrap()
        .push(json!({"hooks":[{"command":"yoke"}]}));
    let rendered = json::render(original, &value).unwrap();
    assert!(
        rendered.starts_with(
            "{\n\t\"theme\" : \"dark\",\n\t\"hooks\" : {\n\t\t\"Stop\" : [ { \"hooks\": [] },"
        ),
        "{rendered}"
    );
    assert!(rendered.ends_with(" ]\n\t}\n}\n"), "{rendered}");
}

#[test]
fn install_lock_recovers_a_dead_owner_and_refuses_a_live_owner() {
    let (_dir, options) = fixture();
    fs::create_dir_all(&options.state_dir).unwrap();
    put(&options.state_dir.join("install.lock"), "2147483647\n");
    let lock = files::Lock::acquire(&options.state_dir).unwrap();
    assert!(files::Lock::acquire(&options.state_dir).is_err());
    drop(lock);
    assert!(files::Lock::acquire(&options.state_dir).is_ok());
}

#[test]
fn receipts_and_preview_omit_unrelated_secrets_and_backups_are_bounded() {
    let (_dir, options) = fixture();
    let original = r#"{"env":{"TOKEN":"private-settings-sentinel"},"hooks":{"Stop":[{"hooks":[{"command":"printf private-hook-sentinel"}]}],"private-event-sentinel":[]}}"#;
    put(&options.claude_settings, original);
    put(&options.codex_config, "token = 'private-config-sentinel'\n");
    let dry = install(&InstallOptions {
        dry_run: true,
        ..options.clone()
    })
    .unwrap();
    let preview = dry
        .changes
        .iter()
        .flat_map(preview_lines)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!preview.contains("private-"), "{preview}");
    assert!(preview.contains("run-hook"), "{preview}");
    install(&options).unwrap();
    for agent in [Agent::Claude, Agent::Codex] {
        let receipt = files::read(&receipt_path(&options, agent))
            .unwrap()
            .unwrap();
        assert!(!receipt.contains("private-"), "{receipt}");
        assert!(receipt.contains("hashes"));
    }
    for _ in 0..6 {
        files::backup(&options.state_dir, &dry.changes).unwrap();
    }
    assert_eq!(
        fs::read_dir(options.state_dir.join("backups"))
            .unwrap()
            .count(),
        5
    );
    for entry in fs::read_dir(options.state_dir.join("backups")).unwrap() {
        let index = fs::read_to_string(entry.unwrap().path().join("index.json")).unwrap();
        assert!(!index.contains("private-"), "{index}");
    }
    uninstall(&options).unwrap();
    assert_eq!(
        fs::read_to_string(options.claude_settings).unwrap(),
        original
    );
}

#[test]
fn settings_reads_reject_fifo_and_health_does_not_block_on_its_symlink() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let (dir, options) = fixture();
    install(&options).unwrap();
    let fifo = dir.path().join("fifo");
    let name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: a valid NUL-terminated path inside this test's temporary directory.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(files::read(&fifo).is_err());
    let project_settings = dir.path().join(".claude/settings.json");
    fs::create_dir_all(project_settings.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&fifo, &project_settings).unwrap();
    let statuses = status(&options, dir.path());
    assert!(
        statuses[0]
            .problems
            .iter()
            .any(|problem| problem.contains("regular file")),
        "{:?}",
        statuses[0].problems
    );
}

#[test]
fn expired_original_backups_do_not_make_preexisting_handlers_owned() {
    let (_dir, mut options) = fixture();
    options.agents = vec![Agent::Claude];
    let group = hook_group(&options.executable, Agent::Claude, "Stop").unwrap();
    let original = json!({"hooks":{"Stop":[{"hooks":[group["hooks"][0].clone(),{"type":"command","command":"keep original"}]}]}});
    put(&options.claude_settings, &original.to_string());
    install(&options).unwrap();
    let mut edited: Value =
        serde_json::from_str(&fs::read_to_string(&options.claude_settings).unwrap()).unwrap();
    edited["hooks"]["Stop"]
        .as_array_mut()
        .unwrap()
        .retain(|value| value != &group);
    put(&options.claude_settings, &edited.to_string());
    for _ in 0..6 {
        files::backup(
            &options.state_dir,
            &[FileChange {
                path: options.claude_settings.clone(),
                before: Some("newer backup".into()),
                after: None,
            }],
        )
        .unwrap();
    }
    uninstall(&options).unwrap();
    let restored: Value =
        serde_json::from_str(&fs::read_to_string(options.claude_settings).unwrap()).unwrap();
    assert_eq!(restored, original);
}

#[test]
fn roundtrip_preserves_existing_settings_and_is_idempotent() {
    let (_dir, options) = fixture();
    let original = "{\n  \"theme\": \"dark\", \"hooks\": {\"Stop\": [ { \"hooks\": [{\"type\":\"command\",\"command\":\"yoke custom-user-hook\"}] } ]}\n}\n";
    let config = "# Keep my comments\nmodel = 'custom'\n[features]\nhooks = false # intentional\nother = true\n";
    put(&options.claude_settings, original);
    put(&options.codex_config, config);
    let report = install(&options).unwrap();
    assert_eq!(report.changes.len(), 3);
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.starts_with("Codex requires review")));
    let installed = fs::read_to_string(&options.claude_settings).unwrap();
    assert!(installed
        .contains("{ \"hooks\": [{\"type\":\"command\",\"command\":\"yoke custom-user-hook\"}] }"));
    let claude: Value = serde_json::from_str(&installed).unwrap();
    assert!(claude["hooks"]["PostToolUseFailure"].is_array());
    let hooks: Value =
        serde_json::from_str(&fs::read_to_string(&options.codex_hooks).unwrap()).unwrap();
    assert!(hooks["hooks"].get("SessionEnd").is_some());
    assert!(hooks["hooks"].get("Notification").is_none());
    assert!(hooks["hooks"].get("PostToolUseFailure").is_none());
    assert_eq!(hooks["hooks"].as_object().unwrap().len(), 7);
    for (event, groups) in hooks["hooks"].as_object().unwrap() {
        assert_eq!(
            groups[0]["hooks"][0]["statusMessage"],
            format!("Yoke: {event}")
        );
    }
    let repeated = install(&options).unwrap();
    assert!(repeated.changes.is_empty());
    assert!(!repeated
        .warnings
        .iter()
        .any(|warning| warning.starts_with("Codex requires review")));
    let backup_root = options.state_dir.join("backups");
    let backup_count = fs::read_dir(&backup_root).unwrap().count();
    // Backups hold copies of the user's settings, which may carry tokens.
    let mode = |path: &Path| {
        std::os::unix::fs::PermissionsExt::mode(&fs::metadata(path).unwrap().permissions()) & 0o777
    };
    assert_eq!(mode(&backup_root), 0o700);
    for entry in fs::read_dir(&backup_root).unwrap() {
        assert_eq!(mode(&entry.unwrap().path()), 0o700);
    }
    let dry = InstallOptions {
        dry_run: true,
        ..options
    };
    assert_eq!(uninstall(&dry).unwrap().changes.len(), 3);
    assert_eq!(fs::read_to_string(&dry.claude_settings).unwrap(), installed);
    assert_eq!(fs::read_dir(&backup_root).unwrap().count(), backup_count);
    let options = InstallOptions {
        dry_run: false,
        ..dry
    };
    uninstall(&options).unwrap();
    assert_eq!(
        fs::read_to_string(&options.claude_settings).unwrap(),
        original
    );
    assert_eq!(fs::read_to_string(&options.codex_config).unwrap(), config);
    assert!(!options.codex_hooks.exists());
    assert!(uninstall(&options).unwrap().changes.is_empty());
}

#[test]
fn dry_run_and_codex_only_statusline_leave_the_entire_temp_root_untouched() {
    let (dir, mut options) = fixture();
    options.dry_run = true;
    assert!(!install(&options).unwrap().changes.is_empty());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    assert!(uninstall(&options).unwrap().changes.is_empty());
    options.statusline = Statusline::Wrap;
    assert!(!install(&options).unwrap().changes.is_empty());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    options.dry_run = false;
    options.agents = vec![Agent::Codex];
    assert!(install(&options)
        .unwrap_err()
        .to_string()
        .contains("statusline"));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn statusline_wraps_existing_command_and_restores_exact_settings() {
    let (_dir, mut options) = fixture();
    options.statusline = Statusline::Wrap;
    let original = "{\n  \"theme\":\"dark\",\"statusLine\": {\"type\":\"command\",\"command\":\"printf '%s' \\\"kept value\\\"\",\"padding\":2}\n}\n";
    put(&options.claude_settings, original);
    install(&options).unwrap();
    let installed: Value =
        serde_json::from_str(&fs::read_to_string(&options.claude_settings).unwrap()).unwrap();
    assert_eq!(installed["statusLine"]["padding"], 2);
    let command = installed["statusLine"]["command"].as_str().unwrap();
    assert!(command.contains(" statusline --wrap "));
    assert!(command.contains("'\\''"));
    assert!(install(&options).unwrap().changes.is_empty());
    uninstall(&options).unwrap();
    assert_eq!(
        fs::read_to_string(&options.claude_settings).unwrap(),
        original
    );
}

#[test]
fn adding_statusline_to_hook_receipt_preserves_later_edits_and_user_override() {
    let (_dir, mut options) = fixture();
    install(&options).unwrap();
    let mut value: Value =
        serde_json::from_str(&fs::read_to_string(&options.claude_settings).unwrap()).unwrap();
    value["theme"] = json!("light");
    value["statusLine"] = json!({"type":"command","command":"cat","padding":3});
    put(&options.claude_settings, &value.to_string());
    options.statusline = Statusline::Wrap;
    install(&options).unwrap();
    // Receipt persisted, settings write interrupted: retry must finish once.
    put(&options.claude_settings, &value.to_string());
    install(&options).unwrap();
    assert!(install(&options).unwrap().changes.is_empty());
    uninstall(&options).unwrap();
    let restored: Value =
        serde_json::from_str(&fs::read_to_string(&options.claude_settings).unwrap()).unwrap();
    assert_eq!(
        restored,
        json!({"theme":"light","statusLine":{"type":"command","command":"cat","padding":3}})
    );
    install(&options).unwrap();
    let mut changed: Value =
        serde_json::from_str(&fs::read_to_string(&options.claude_settings).unwrap()).unwrap();
    changed["statusLine"] = json!({"type":"command","command":"echo user replacement"});
    put(&options.claude_settings, &changed.to_string());
    assert!(install(&options).is_err());
    uninstall(&options).unwrap();
    let restored: Value =
        serde_json::from_str(&fs::read_to_string(&options.claude_settings).unwrap()).unwrap();
    assert_eq!(restored["statusLine"]["command"], "echo user replacement");
    assert_eq!(restored["theme"], "light");
}

#[test]
fn uninstall_preserves_settings_and_hooks_added_after_install() {
    let (_dir, options) = fixture();
    install(&options).unwrap();
    let mut settings: Value =
        serde_json::from_str(&fs::read_to_string(&options.claude_settings).unwrap()).unwrap();
    settings["theme"] = json!("light");
    settings["hooks"]["Stop"]
        .as_array_mut()
        .unwrap()
        .push(json!({"hooks":[{"type":"command","command":"echo keep"}]}));
    settings["hooks"]["PreToolUse"][0]["hooks"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"command","command":"echo inside"}));
    put(
        &options.claude_settings,
        &serde_json::to_string_pretty(&settings).unwrap(),
    );
    uninstall(&options).unwrap();
    let remaining: Value =
        serde_json::from_str(&fs::read_to_string(&options.claude_settings).unwrap()).unwrap();
    assert_eq!(
        remaining,
        json!({"theme":"light","hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo keep"}]}],"PreToolUse":[{"hooks":[{"type":"command","command":"echo inside"}]}]}})
    );
}

#[test]
fn preexisting_identical_hook_is_not_owned_and_legacy_flag_restores_with_later_edits() {
    let (_dir, options) = fixture();
    let planned = prepare(&options, Agent::Claude, &mut Vec::new()).unwrap();
    let group = planned
        .added
        .iter()
        .find(|entry| entry.event == "Stop")
        .unwrap()
        .group
        .clone();
    let original = json!({"hooks":{"Stop":[group]}}).to_string();
    put(&options.claude_settings, &original);
    put(
        &options.codex_config,
        "# retained\n[features]\ncodex_hooks = false # old spelling\n",
    );
    install(&options).unwrap();
    let installed = fs::read_to_string(&options.codex_config).unwrap();
    assert!(installed.contains("codex_hooks = true # old spelling"));
    put(
        &options.codex_config,
        &format!("{installed}\n[other]\nkeep = 42\n"),
    );
    uninstall(&options).unwrap();
    assert_eq!(
        fs::read_to_string(&options.claude_settings).unwrap(),
        original
    );
    let remaining = fs::read_to_string(&options.codex_config).unwrap();
    assert!(remaining.contains("codex_hooks = false # old spelling"));
    assert!(remaining.contains("keep = 42"));
}

#[test]
fn malformed_settings_and_ambiguous_ownership_fail_without_overwriting() {
    let (_dir, options) = fixture();
    put(&options.codex_hooks, "{\"hooks\":{},\"hooks\":{}}");
    assert!(install(&options).is_err());
    assert!(!options.claude_settings.exists());
    assert!(!options.state_dir.exists());
    fs::remove_file(&options.codex_hooks).unwrap();
    install(&options).unwrap();
    let mut settings: Value =
        serde_json::from_str(&fs::read_to_string(&options.claude_settings).unwrap()).unwrap();
    let stop = settings["hooks"]["Stop"][0].clone();
    settings["hooks"]["Stop"].as_array_mut().unwrap().push(stop);
    let altered = settings.to_string();
    put(&options.claude_settings, &altered);
    assert!(uninstall(&options)
        .unwrap_err()
        .to_string()
        .contains("ambiguous"));
    assert_eq!(
        fs::read_to_string(&options.claude_settings).unwrap(),
        altered
    );
    assert!(receipt_path(&options, Agent::Claude).exists());
}

#[test]
fn interrupted_install_recovers_from_receipt_and_refuses_symlink_targets() {
    let (_dir, options) = fixture();
    let receipt = prepare(&options, Agent::Claude, &mut Vec::new()).unwrap();
    put(
        &receipt_path(&options, Agent::Claude),
        &serde_json::to_string(&receipt).unwrap(),
    );
    install(&options).unwrap();
    assert!(options.claude_settings.exists());
    uninstall(&options).unwrap();
    assert!(!options.claude_settings.exists());
    let target = options.state_dir.join("not-settings");
    put(&target, "{}");
    std::os::unix::fs::symlink(&target, &options.claude_settings).unwrap();
    assert!(install(&options).is_err());
    assert_eq!(fs::read_to_string(target).unwrap(), "{}");
}

fn unlabeled_install(options: &InstallOptions) {
    install(options).unwrap();
    for agent in selected(options) {
        let path = receipt_path(options, agent);
        let mut receipt: Receipt =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let mut installed = settings(receipt.hooks.after.as_deref()).unwrap();
        for owned in &mut receipt.added {
            let old = owned.group.clone();
            owned.group["hooks"][0]
                .as_object_mut()
                .unwrap()
                .remove("statusMessage");
            let groups = installed["hooks"][&owned.event].as_array_mut().unwrap();
            *groups.iter_mut().find(|group| **group == old).unwrap() = owned.group.clone();
        }
        receipt.hooks.after = Some(installed.to_string());
        put(&receipt.hooks.path, receipt.hooks.after.as_deref().unwrap());
        put(&path, &serde_json::to_string(&receipt).unwrap());
    }
}

#[test]
fn labels_upgrade_owned_hooks_without_duplicates_and_recover_interrupted_writes() {
    let (_dir, mut options) = fixture();
    unlabeled_install(&options);
    let mut previous = Vec::new();
    for agent in selected(&options) {
        let path = hook_path(&options, agent);
        let mut value = settings(files::read(path).unwrap().as_deref()).unwrap();
        value["theme"] = json!("user edit");
        value["hooks"]["Stop"]
            .as_array_mut()
            .unwrap()
            .push(json!({"hooks":[{"type":"command","command":"echo keep"}]}));
        let raw = value.to_string();
        put(path, &raw);
        previous.push((path.to_path_buf(), raw));
    }
    options.dry_run = true;
    assert_eq!(install(&options).unwrap().changes.len(), 2);
    for (path, raw) in &previous {
        assert_eq!(fs::read_to_string(path).unwrap(), *raw);
    }
    options.dry_run = false;
    install(&options).unwrap();
    // Ownership persisted but settings replacement did not complete.
    for (path, raw) in &previous {
        put(path, raw);
    }
    install(&options).unwrap();
    for agent in selected(&options) {
        let value = settings(files::read(hook_path(&options, agent)).unwrap().as_deref()).unwrap();
        assert_eq!(value["theme"], "user edit");
        for (event, groups) in value["hooks"].as_object().unwrap() {
            assert_eq!(
                groups.as_array().unwrap().len(),
                if event == "Stop" { 2 } else { 1 }
            );
            assert_eq!(
                groups[0]["hooks"][0]["statusMessage"],
                format!("Yoke: {event}")
            );
        }
    }
    assert!(install(&options).unwrap().changes.is_empty());
    uninstall(&options).unwrap();
    for agent in selected(&options) {
        let value = settings(files::read(hook_path(&options, agent)).unwrap().as_deref()).unwrap();
        assert_eq!(
            value,
            json!({"theme":"user edit","hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo keep"}]}]}})
        );
    }
}

#[test]
fn interrupted_label_upgrade_can_uninstall_and_user_handler_edits_are_not_overwritten() {
    let (_dir, options) = fixture();
    unlabeled_install(&options);
    let previous: Vec<_> = selected(&options)
        .into_iter()
        .map(|agent| {
            let path = hook_path(&options, agent).to_path_buf();
            let raw = fs::read_to_string(&path).unwrap();
            (path, raw)
        })
        .collect();
    install(&options).unwrap();
    for (path, raw) in &previous {
        put(path, raw);
    }
    uninstall(&options).unwrap();
    assert!(!options.claude_settings.exists());
    assert!(!options.codex_hooks.exists());

    unlabeled_install(&options);
    let mut value = settings(files::read(&options.claude_settings).unwrap().as_deref()).unwrap();
    value["hooks"]["Stop"][0]["hooks"][0]["statusMessage"] = json!("My custom label");
    let edited = value.to_string();
    put(&options.claude_settings, &edited);
    let receipt = files::read(&receipt_path(&options, Agent::Claude)).unwrap();
    assert!(install(&options).is_err());
    assert_eq!(
        fs::read_to_string(&options.claude_settings).unwrap(),
        edited
    );
    assert_eq!(
        files::read(&receipt_path(&options, Agent::Claude)).unwrap(),
        receipt
    );
}

#[test]
fn preexisting_unlabeled_hooks_remain_unowned_without_duplicate_commands() {
    let (_dir, options) = fixture();
    let mut group = prepare(&options, Agent::Claude, &mut Vec::new())
        .unwrap()
        .added
        .into_iter()
        .find(|owned| owned.event == "Stop")
        .unwrap()
        .group;
    group["hooks"][0]
        .as_object_mut()
        .unwrap()
        .remove("statusMessage");
    let original = json!({"hooks":{"Stop":[group]}}).to_string();
    put(&options.claude_settings, &original);
    install(&options).unwrap();
    let value = settings(files::read(&options.claude_settings).unwrap().as_deref()).unwrap();
    assert_eq!(value["hooks"]["Stop"].as_array().unwrap().len(), 1);
    assert!(value["hooks"]["Stop"][0]["hooks"][0]
        .get("statusMessage")
        .is_none());
    uninstall(&options).unwrap();
    assert_eq!(
        fs::read_to_string(&options.claude_settings).unwrap(),
        original
    );
}

#[test]
fn install_reports_user_and_managed_hook_blockers_without_changing_them() {
    let (_dir, options) = fixture();
    let managed = options.claude_managed.join("managed-settings.json");
    put(&managed, r#"{"allowManagedHooksOnly": true}"#);
    let refused = install(&options).unwrap_err().to_string();
    assert!(refused.contains("allowManagedHooksOnly"), "{refused}");
    assert!(!options.claude_settings.exists());
    fs::remove_file(&managed).unwrap();
    install(&options).unwrap();
    let mut settings: Value =
        serde_json::from_str(&fs::read_to_string(&options.claude_settings).unwrap()).unwrap();
    settings["disableAllHooks"] = json!(true);
    put(
        &options.claude_settings,
        &serde_json::to_string_pretty(&settings).unwrap(),
    );
    put(
        &options
            .claude_managed
            .join("managed-settings.d/50-policy.json"),
        r#"{"allowManagedHooksOnly": true}"#,
    );
    let disabled = fs::read_to_string(&options.claude_settings).unwrap();
    let report = install(&options).unwrap();
    assert!(report.changes.is_empty());
    for key in ["disableAllHooks", "allowManagedHooksOnly"] {
        assert!(
            report.warnings.iter().any(|warning| warning.contains(key)),
            "{key}: {:?}",
            report.warnings
        );
    }
    assert_eq!(
        fs::read_to_string(&options.claude_settings).unwrap(),
        disabled
    );
}

#[test]
fn first_install_refuses_when_user_settings_disable_all_hooks() {
    let (_dir, options) = fixture();
    put(&options.claude_settings, r#"{"disableAllHooks": true}"#);
    let refused = install(&options).unwrap_err().to_string();
    assert!(refused.contains("disableAllHooks"), "{refused}");
    assert_eq!(
        fs::read_to_string(&options.claude_settings).unwrap(),
        r#"{"disableAllHooks": true}"#
    );
}

#[test]
fn a_moved_executable_or_settings_directory_is_refused_with_the_fix() {
    let (dir, options) = fixture();
    install(&options).unwrap();

    let mut moved = InstallOptions {
        executable: dir.path().join("elsewhere/yoke"),
        ..options.clone()
    };
    fs::create_dir_all(moved.executable.parent().unwrap()).unwrap();
    fs::copy(&options.executable, &moved.executable).unwrap();
    moved.executable = moved.executable.canonicalize().unwrap();
    let error = format!("{:#}", install(&moved).unwrap_err());
    assert!(
        error.starts_with("executable changed; uninstall before reinstalling"),
        "{error}"
    );
    for expected in [
        "the claude hooks run",
        &options.executable.display().to_string(),
        &format!("not {}", moved.executable.display()),
        &format!(
            "\"{}\" uninstall --agent claude",
            moved.executable.display()
        ),
    ] {
        assert!(error.contains(expected), "{expected}: {error}");
    }

    let relocated = InstallOptions {
        claude_settings: dir.path().join("other-claude/settings.json"),
        ..options.clone()
    };
    let error = format!("{:#}", uninstall(&relocated).unwrap_err());
    assert!(
        error.starts_with("settings path differs from installation receipt"),
        "{error}"
    );
    assert!(
        error.contains(&format!(
            "set CLAUDE_CONFIG_DIR to {}",
            options.claude_settings.parent().unwrap().display()
        )),
        "{error}"
    );
}

#[test]
fn drift_advice_is_the_repair_that_actually_works() {
    let (_dir, options) = fixture();
    install(&options).unwrap();
    let mut settings: Value =
        serde_json::from_str(&fs::read_to_string(&options.claude_settings).unwrap()).unwrap();
    settings["hooks"].as_object_mut().unwrap().remove("Stop");
    put(&options.claude_settings, &settings.to_string());
    let stale = drift::managed(&options, Agent::Claude)
        .unwrap()
        .unwrap()
        .stale;
    assert!(
        stale[0]
            .1
            .ends_with("run yoke uninstall, then yoke install"),
        "{stale:?}"
    );
    // The advice must not send anyone to a command that refuses the same state.
    assert!(install(&InstallOptions {
        dry_run: true,
        ..options
    })
    .is_err());
}

#[test]
fn if_absent_adds_the_statusline_only_when_none_is_set_and_never_replaces_one() {
    // No statusLine yet: Yoke's is added, without a wrapper.
    let (_dir, mut options) = fixture();
    options.statusline = Statusline::IfAbsent;
    let report = install(&options).unwrap();
    assert!(report
        .warnings
        .iter()
        .all(|w| !w.contains("statusline kept")));
    let settings: Value =
        serde_json::from_str(&fs::read_to_string(&options.claude_settings).unwrap()).unwrap();
    let command = settings["statusLine"]["command"].as_str().unwrap();
    assert!(command.ends_with("' statusline"), "{command}");
    assert!(install(&options).unwrap().changes.is_empty());

    // Another command is kept byte for byte, reported, and not recorded as owned.
    let (_dir, mut options) = fixture();
    options.statusline = Statusline::IfAbsent;
    let original = "{\"statusLine\": {\"type\":\"command\",\"command\":\"echo hi\"}}";
    put(&options.claude_settings, original);
    let report = install(&options).unwrap();
    let kept = report
        .warnings
        .iter()
        .find(|w| w.starts_with("statusline kept:"))
        .unwrap_or_else(|| panic!("{:?}", report.warnings));
    assert!(kept.contains("echo hi") && kept.contains("yoke install --agent claude --statusline"));
    let installed: Value =
        serde_json::from_str(&fs::read_to_string(&options.claude_settings).unwrap()).unwrap();
    assert_eq!(
        installed["statusLine"],
        json!({"type":"command","command":"echo hi"})
    );
    uninstall(&options).unwrap();
    assert_eq!(
        fs::read_to_string(&options.claude_settings).unwrap(),
        original
    );

    // A statusLine that already runs a yoke is Yoke's: nothing to add, nothing to report.
    let (_dir, mut options) = fixture();
    options.statusline = Statusline::IfAbsent;
    put(
        &options.claude_settings,
        r#"{"statusLine":{"type":"command","command":"'/old/bin/yoke' statusline --wrap 'echo hi'"}}"#,
    );
    let report = install(&options).unwrap();
    assert!(report
        .warnings
        .iter()
        .all(|w| !w.contains("statusline kept")));
    let installed: Value =
        serde_json::from_str(&fs::read_to_string(&options.claude_settings).unwrap()).unwrap();
    assert_eq!(
        installed["statusLine"]["command"],
        "'/old/bin/yoke' statusline --wrap 'echo hi'"
    );
}

#[test]
fn the_statusline_style_is_installed_changed_in_place_wrapped_and_restored() {
    use crate::statusline::Style;
    let command = |options: &InstallOptions| -> String {
        let settings: Value =
            serde_json::from_str(&fs::read_to_string(&options.claude_settings).unwrap()).unwrap();
        settings["statusLine"]["command"]
            .as_str()
            .unwrap()
            .to_owned()
    };

    // A fresh install of the detailed style, then the same request again changes nothing.
    let (_dir, mut options) = fixture();
    options.statusline = Statusline::Wrap;
    options.statusline_style = Some(Style::Detailed);
    install(&options).unwrap();
    assert!(command(&options).ends_with("' statusline --style detailed"));
    assert!(install(&options).unwrap().changes.is_empty());

    // Asking for the other style restyles Yoke's own line; asking for none leaves it as it is.
    options.statusline_style = None;
    assert!(install(&options).unwrap().changes.is_empty());
    options.statusline_style = Some(Style::Compact);
    install(&options).unwrap();
    assert!(command(&options).ends_with("' statusline"));
    options.statusline_style = Some(Style::Detailed);
    install(&options).unwrap();
    assert!(command(&options).ends_with("--style detailed"));
    // The receipt still validates, so uninstall removes the line it added.
    uninstall(&options).unwrap();
    let settings: Value =
        serde_json::from_str(&fs::read_to_string(&options.claude_settings).unwrap_or("{}".into()))
            .unwrap();
    assert!(settings.get("statusLine").is_none(), "{settings}");

    // A command the user already has is wrapped, and comes back exactly on uninstall.
    let (_dir, mut options) = fixture();
    let original = "{\"statusLine\": {\"type\":\"command\",\"command\":\"echo hi\"}}";
    put(&options.claude_settings, original);
    options.statusline = Statusline::Wrap;
    options.statusline_style = Some(Style::Detailed);
    install(&options).unwrap();
    assert!(command(&options).contains(" statusline --style detailed --wrap 'echo hi'"));
    options.statusline_style = Some(Style::Compact);
    install(&options).unwrap();
    assert!(command(&options).ends_with("' statusline --wrap 'echo hi'"));
    uninstall(&options).unwrap();
    assert_eq!(
        fs::read_to_string(&options.claude_settings).unwrap(),
        original
    );

    // A Yoke line the user wrote by hand is restyled on request and put back by uninstall.
    let (_dir, mut options) = fixture();
    let exe = options
        .executable
        .display()
        .to_string()
        .replace('\'', "'\\''");
    let by_hand =
        json!({"statusLine": {"type": "command", "command": format!("'{exe}' statusline")}})
            .to_string();
    put(&options.claude_settings, &by_hand);
    options.statusline = Statusline::Wrap;
    options.statusline_style = Some(Style::Detailed);
    install(&options).unwrap();
    assert!(command(&options).ends_with("--style detailed"));
    uninstall(&options).unwrap();
    assert_eq!(
        fs::read_to_string(&options.claude_settings).unwrap(),
        by_hand
    );
}
