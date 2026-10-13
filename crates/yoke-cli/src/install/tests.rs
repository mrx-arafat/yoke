use super::*;
use serde_json::{json, Value};
use std::fs;
use tempfile::TempDir;

fn fixture() -> (TempDir, InstallOptions) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let options = InstallOptions {
        claude_settings: root.join("claude/settings.json"),
        codex_hooks: root.join("codex/hooks.json"),
        codex_config: root.join("codex/config.toml"),
        state_dir: root.join("state"),
        executable: root.join("bin/yoke's executable"),
        agents: vec![Agent::Claude, Agent::Codex],
        dry_run: false,
        statusline: false,
    };
    (dir, options)
}

fn put(path: &std::path::Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
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
    let installed = fs::read_to_string(&options.claude_settings).unwrap();
    assert!(installed
        .contains("{ \"hooks\": [{\"type\":\"command\",\"command\":\"yoke custom-user-hook\"}] }"));
    let hooks: Value =
        serde_json::from_str(&fs::read_to_string(&options.codex_hooks).unwrap()).unwrap();
    assert!(hooks["hooks"].get("SessionEnd").is_some());
    assert!(hooks["hooks"].get("Notification").is_none());
    assert_eq!(hooks["hooks"].as_object().unwrap().len(), 7);
    assert!(install(&options).unwrap().changes.is_empty());
    let backup_root = options.state_dir.join("backups");
    let backup_count = fs::read_dir(&backup_root).unwrap().count();
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
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    assert!(uninstall(&options).unwrap().changes.is_empty());
    options.statusline = true;
    assert!(!install(&options).unwrap().changes.is_empty());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    options.dry_run = false;
    options.agents = vec![Agent::Codex];
    assert!(install(&options)
        .unwrap_err()
        .to_string()
        .contains("statusline"));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn statusline_wraps_existing_command_and_restores_exact_settings() {
    let (_dir, mut options) = fixture();
    options.statusline = true;
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
    options.statusline = true;
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
    let planned = prepare(&options, Agent::Claude).unwrap();
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
    let receipt = prepare(&options, Agent::Claude).unwrap();
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
