//! Read-only inspection of what installation left in place, for `yoke health`.

use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{ensure, Result};
use serde::Serialize;
use serde_json::{json, Value};

use super::{
    features, files, hook_path, json as settings_json, matches_owned, read_receipt, settings,
    statusline, Agent, InstallOptions, Receipt,
};

const MAX_SETTINGS_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Serialize)]
pub struct AgentStatus {
    pub agent: &'static str,
    pub installed: bool,
    pub settings: PathBuf,
    pub executable: Option<PathBuf>,
    /// Whether Claude Code's statusLine runs Yoke's: owned by the installation, or already so.
    pub statusline: bool,
    /// Conditions under which installed hooks do not run as installed.
    pub problems: Vec<String>,
    /// What local files cannot show, such as agent-side hook approval.
    pub notes: Vec<String>,
}

/// Inspect both agents without writing anything; `project` adds Claude's project settings layers.
pub fn status(options: &InstallOptions, project: &Path) -> Vec<AgentStatus> {
    [Agent::Claude, Agent::Codex]
        .into_iter()
        .map(|agent| agent_status(options, agent, project))
        .collect()
}

fn agent_status(options: &InstallOptions, agent: Agent, project: &Path) -> AgentStatus {
    let mut status = AgentStatus {
        agent: agent.name(),
        installed: false,
        settings: hook_path(options, agent).to_path_buf(),
        executable: None,
        statusline: false,
        problems: Vec::new(),
        notes: Vec::new(),
    };
    let receipt = match read_receipt(options, agent) {
        Ok(Some(receipt)) => receipt,
        Ok(None) => return status,
        Err(error) => {
            status
                .problems
                .push(format!("installation receipt is unreadable: {error:#}"));
            return status;
        }
    };
    status.installed = true;
    let executable = &receipt.executable;
    let runnable = std::fs::metadata(executable)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0);
    if !runnable {
        status.problems.push(format!(
            "hook executable {} is missing or not executable; run `yoke uninstall --agent {}` with a working yoke to remove the hooks, then `yoke install --agent {}` (the install script does both)",
            executable.display(),
            agent.name(),
            agent.name()
        ));
    } else if *executable != options.executable {
        status.notes.push(format!(
            "hooks run {}, not this executable {}",
            executable.display(),
            options.executable.display()
        ));
    }
    status.executable = Some(executable.clone());
    let (problems, guidance) = super::executable::diagnostics(executable);
    status.problems.extend(problems);
    status.notes.extend(guidance);
    status.statusline = receipt.statusline.is_some();
    let mut hooks_file = None;
    match files::read(&status.settings).and_then(|raw| settings(raw.as_deref())) {
        Ok(value) => {
            let missing: Vec<&str> = receipt
                .added
                .iter()
                .filter(|owned| {
                    !value["hooks"][&owned.event]
                        .as_array()
                        .is_some_and(|groups| {
                            groups.iter().any(|group| matches_owned(group, owned))
                        })
                })
                .map(|owned| owned.event.as_str())
                .collect();
            if !missing.is_empty() {
                status.problems.push(format!(
                    "Yoke hooks for {} are missing from {}",
                    missing.join(", "),
                    status.settings.display()
                ));
            }
            hooks_file = Some(value);
        }
        Err(error) => status.problems.push(format!(
            "cannot read {}: {error:#}",
            status.settings.display()
        )),
    }
    match agent {
        Agent::Claude => {
            let local = project.join(".claude/settings.local.json");
            let shared = project.join(".claude/settings.json");
            status.problems.extend(claude_blockers(
                &options.claude_managed,
                &[&local, &shared, &options.claude_settings],
            ));
            status.notes.push(
                "Claude managed policy from MDM or the claude.ai console is not visible to Yoke"
                    .into(),
            );
            // A statusLine that runs `yoke statusline` shows Yoke's line whoever wrote it.
            status.statusline = status.statusline
                || hooks_file
                    .as_ref()
                    .and_then(|value| value.get("statusLine"))
                    .is_some_and(statusline::runs_yoke);
        }
        Agent::Codex => {
            let enabled = files::read(&options.codex_config)
                .and_then(|raw| features::check_enabled(raw.as_deref().unwrap_or("")));
            match enabled {
                Ok(()) => {}
                Err(error) => status.problems.push(format!(
                    "Codex hooks cannot run with {}: {error:#}",
                    options.codex_config.display()
                )),
            }
            status
                .notes
                .push(codex_approval_note(options, &receipt, hooks_file.as_ref()));
        }
    }
    status
}

/// Codex records each approved hook under `[hooks.state."<hooks.json>:<event>:<group>:<hook>"]`
/// in its `config.toml`. Yoke can see that a record exists, never that its hash is current.
fn codex_approval_note(
    options: &InstallOptions,
    receipt: &Receipt,
    hooks: Option<&Value>,
) -> String {
    let caveat = "Yoke only sees that a record exists, not whether it is still current (a changed hook needs approval again) or what managed requirements say.";
    let Some(doc) = files::read(&options.codex_config)
        .ok()
        .flatten()
        .and_then(|raw| raw.parse::<toml_edit::DocumentMut>().ok())
    else {
        return format!(
            "Codex runs new or changed hooks only after you approve them with /hooks; {caveat}"
        );
    };
    let states = doc
        .get("hooks")
        .and_then(toml_edit::Item::as_table_like)
        .and_then(|hooks| hooks.get("state"))
        .and_then(toml_edit::Item::as_table_like);
    let missing = receipt
        .added
        .iter()
        .filter(|owned| {
            let group = hooks
                .and_then(|value| value["hooks"][&owned.event].as_array())
                .and_then(|groups| groups.iter().position(|group| matches_owned(group, owned)))
                .unwrap_or(0);
            let key = format!(
                "{}:{}:{group}:0",
                receipt.hooks.path.display(),
                snake_case(&owned.event)
            );
            !states.is_some_and(|states| states.contains_key(&key))
        })
        .count();
    let total = receipt.added.len();
    if missing == 0 {
        format!(
            "Codex approval records exist for all {total} Yoke hooks in {}; {caveat}",
            options.codex_config.display()
        )
    } else {
        format!(
            "Codex has no approval record for {missing} of {total} Yoke hooks in {}; open codex, run /hooks and approve them. {caveat}",
            options.codex_config.display()
        )
    }
}

/// `PreToolUse` becomes `pre_tool_use`, the spelling Codex uses in its approval keys.
fn snake_case(event: &str) -> String {
    let mut out = String::new();
    for (index, character) in event.chars().enumerate() {
        if character.is_ascii_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.push(character.to_ascii_lowercase());
        } else {
            out.push(character);
        }
    }
    out
}

/// Claude settings that stop Yoke's user hooks, from the settings files Yoke can read.
/// `layers` are the non-managed files in precedence order, highest first.
pub(super) fn claude_blockers(managed: &Path, layers: &[&Path]) -> Vec<String> {
    let mut blockers = Vec::new();
    let mut managed_files = vec![managed.join("managed-settings.json")];
    if let Ok(entries) = std::fs::read_dir(managed.join("managed-settings.d")) {
        let mut drop_ins: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .collect();
        drop_ins.sort();
        managed_files.extend(drop_ins);
    }
    for path in &managed_files {
        match read_json(path) {
            Ok(Some(value)) => {
                for key in ["disableAllHooks", "allowManagedHooksOnly"] {
                    if value.get(key) == Some(&json!(true)) {
                        blockers.push(format!(
                            "Claude managed setting {key} is true in {}, so Yoke's hooks do not run",
                            path.display()
                        ));
                    }
                }
            }
            Ok(None) => {}
            Err(error) => blockers.push(format!(
                "cannot read Claude managed settings {}: {error:#}",
                path.display()
            )),
        }
    }
    // The highest-precedence file that sets disableAllHooks decides it for Yoke's hooks.
    for path in layers {
        match read_json(path) {
            Ok(Some(value)) => {
                if let Some(disabled) = value.get("disableAllHooks").and_then(Value::as_bool) {
                    if disabled {
                        blockers.push(format!(
                            "Claude disableAllHooks is true in {}, so Yoke's hooks do not run",
                            path.display()
                        ));
                    }
                    break;
                }
            }
            Ok(None) => {}
            Err(error) => {
                blockers.push(format!(
                    "cannot read Claude settings {}: {error:#}",
                    path.display()
                ));
                break;
            }
        }
    }
    blockers
}

/// Inspected settings may be links an administrator manages, so they are followed but bounded.
fn read_json(path: &Path) -> Result<Option<Value>> {
    let file = match std::fs::File::options()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    ensure!(file.metadata()?.is_file(), "settings {} must be a regular file; replace the special file before running yoke health again", path.display());
    let mut raw = String::new();
    file.take(MAX_SETTINGS_BYTES + 1).read_to_string(&mut raw)?;
    ensure!(
        raw.len() as u64 <= MAX_SETTINGS_BYTES,
        "settings exceed 8 MiB"
    );
    Ok(Some(settings_json::parse(&raw)?))
}
