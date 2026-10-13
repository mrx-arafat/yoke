//! Reversible hook registration using explicit paths and durable ownership receipts.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[path = "install/features.rs"]
mod features;
#[path = "install/files.rs"]
mod files;
#[path = "install/json.rs"]
mod json;
#[path = "install/statusline.rs"]
mod statusline;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Agent {
    Claude,
    Codex,
}

pub struct InstallOptions {
    pub claude_settings: PathBuf,
    pub codex_hooks: PathBuf,
    pub codex_config: PathBuf,
    pub state_dir: PathBuf,
    pub executable: PathBuf,
    pub agents: Vec<Agent>,
    pub dry_run: bool,
    pub statusline: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileChange {
    pub path: PathBuf,
    pub before: Option<String>,
    pub after: Option<String>,
}

#[derive(Debug, Default)]
pub struct InstallReport {
    pub changes: Vec<FileChange>,
    pub warnings: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct OwnedHook {
    event: String,
    group: Value,
}

#[derive(Serialize, Deserialize)]
struct Receipt {
    version: u8,
    agent: Agent,
    executable: PathBuf,
    hooks: FileChange,
    added: Vec<OwnedHook>,
    config: Option<FileChange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    statusline: Option<statusline::Owned>,
}

impl Agent {
    fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

const COMMON_EVENTS: &[&str] = &[
    "PreToolUse",
    "PostToolUse",
    "UserPromptSubmit",
    "Stop",
    "SessionStart",
    "PreCompact",
    "SessionEnd",
];

fn validate(options: &InstallOptions) -> Result<()> {
    ensure!(
        !options.statusline || options.agents.contains(&Agent::Claude),
        "statusline requires selecting Claude; no settings changed"
    );
    ensure!(!options.agents.is_empty(), "select at least one agent");
    let mut paths = HashSet::new();
    for path in [
        &options.claude_settings,
        &options.codex_hooks,
        &options.codex_config,
        &options.state_dir,
    ] {
        ensure!(
            path.is_absolute(),
            "installation paths must be absolute: {}",
            path.display()
        );
        ensure!(
            !path
                .components()
                .any(|c| c == std::path::Component::ParentDir),
            "parent path components are not allowed"
        );
        ensure!(paths.insert(path), "installation paths must be distinct");
    }
    ensure!(
        options.executable.is_absolute(),
        "executable path must be absolute"
    );
    for path in [
        &options.claude_settings,
        &options.codex_hooks,
        &options.codex_config,
    ] {
        ensure!(
            !path.starts_with(&options.state_dir) && **path != options.executable,
            "settings paths must be outside Yoke state and executable paths"
        );
    }
    let executable = options
        .executable
        .to_str()
        .context("executable path must be UTF-8")?;
    ensure!(
        !executable.contains(['\0', '\n', '\r']),
        "invalid executable path"
    );
    Ok(())
}

fn hook_path(options: &InstallOptions, agent: Agent) -> &Path {
    match agent {
        Agent::Claude => &options.claude_settings,
        Agent::Codex => &options.codex_hooks,
    }
}

fn receipt_path(options: &InstallOptions, agent: Agent) -> PathBuf {
    options
        .state_dir
        .join(format!("install-{}.json", agent.name()))
}

fn read_receipt(options: &InstallOptions, agent: Agent) -> Result<Option<Receipt>> {
    let Some(raw) = files::read(&receipt_path(options, agent))? else {
        return Ok(None);
    };
    let receipt: Receipt =
        serde_json::from_str(&raw).context("invalid installation ownership receipt")?;
    ensure!(
        receipt.version == 1 && receipt.agent == agent,
        "unsupported installation receipt"
    );
    ensure!(
        receipt.hooks.path == hook_path(options, agent),
        "settings path differs from installation receipt"
    );
    if let Some(config) = &receipt.config {
        ensure!(
            agent == Agent::Codex && config.path == options.codex_config,
            "config path differs from installation receipt"
        );
    }
    if let Some(owned) = &receipt.statusline {
        ensure!(agent == Agent::Claude, "invalid statusline receipt agent");
        ensure!(
            statusline::build(&receipt.executable, owned.before.clone())? == *owned,
            "invalid owned statusline in installation receipt"
        );
    }
    let mut events = HashSet::new();
    for owned in &receipt.added {
        ensure!(
            COMMON_EVENTS.contains(&owned.event.as_str())
                || (agent == Agent::Claude && owned.event == "Notification"),
            "invalid owned event in installation receipt"
        );
        ensure!(
            events.insert(&owned.event),
            "duplicate owned event in installation receipt"
        );
        ensure!(
            owned.group == hook_group(&receipt.executable, agent, &owned.event)?,
            "invalid owned command in installation receipt"
        );
    }
    Ok(Some(receipt))
}

fn hook_group(executable: &Path, agent: Agent, event: &str) -> Result<Value> {
    let executable = executable
        .to_str()
        .context("executable path must be UTF-8")?;
    let quoted = format!("'{}'", executable.replace('\'', "'\\''"));
    let mut handler = json!({"type":"command", "command":format!("{quoted} run-hook --agent {} {event}", agent.name())});
    if agent == Agent::Codex && event == "SessionEnd" {
        handler["timeout"] = json!(3);
    }
    Ok(json!({"hooks":[handler]}))
}

fn settings(raw: Option<&str>) -> Result<Value> {
    let value = json::parse(raw.unwrap_or("{}"))?;
    if let Some(hooks) = value.get("hooks") {
        let hooks = hooks.as_object().context("hooks must be an object")?;
        for (event, entries) in hooks {
            ensure!(entries.is_array(), "hooks.{event} must be an array");
        }
    }
    Ok(value)
}

fn prepare(options: &InstallOptions, agent: Agent) -> Result<Receipt> {
    let path = hook_path(options, agent).to_path_buf();
    let before = files::read(&path)?;
    let mut value = settings(before.as_deref())?;
    if agent == Agent::Claude {
        ensure!(
            value.get("disableAllHooks") != Some(&json!(true)),
            "Claude disableAllHooks is true; enable hooks explicitly before installing"
        );
        ensure!(
            value.get("allowManagedHooksOnly") != Some(&json!(true)),
            "Claude allows managed hooks only"
        );
    }
    let hooks = value
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .unwrap();
    let mut events = COMMON_EVENTS.to_vec();
    if agent == Agent::Claude {
        events.push("Notification");
    }
    let mut added = Vec::new();
    for event in events {
        let group = hook_group(&options.executable, agent, event)?;
        let groups = hooks
            .entry(event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .unwrap();
        if !groups.contains(&group) {
            groups.push(group.clone());
            added.push(OwnedHook {
                event: event.to_owned(),
                group,
            });
        }
    }
    let statusline = if agent == Agent::Claude && options.statusline {
        Some(statusline::add(&options.executable, &mut value)?)
    } else {
        None
    };
    let after = Some(json::render(before.as_deref().unwrap_or("{}"), &value)?);
    let config = if agent == Agent::Codex {
        features::prepare(&options.codex_config)?
    } else {
        None
    };
    Ok(Receipt {
        version: 1,
        agent,
        executable: options.executable.clone(),
        hooks: FileChange {
            path,
            before,
            after,
        },
        added,
        config,
        statusline,
    })
}

fn selected(options: &InstallOptions) -> Vec<Agent> {
    let mut result = Vec::new();
    for agent in &options.agents {
        if !result.contains(agent) {
            result.push(*agent);
        }
    }
    result
}

fn change(report: &mut InstallReport, file: FileChange) {
    if file.before != file.after {
        report.changes.push(file);
    }
}

/// Plan or install only documented local hooks; all filesystem locations are explicit.
pub fn install(options: &InstallOptions) -> Result<InstallReport> {
    validate(options)?;
    let mut report = InstallReport::default();
    let mut receipts = Vec::new();
    for agent in selected(options) {
        if let Some(mut receipt) = read_receipt(options, agent)? {
            ensure!(
                receipt.executable == options.executable,
                "executable changed; uninstall before reinstalling"
            );
            for file in std::iter::once(&receipt.hooks).chain(receipt.config.iter()) {
                let current = files::read(&file.path)?;
                if current == file.before {
                    change(
                        &mut report,
                        FileChange {
                            path: file.path.clone(),
                            before: current,
                            after: file.after.clone(),
                        },
                    );
                } else if current != file.after {
                    if file.path == receipt.hooks.path {
                        let value = settings(current.as_deref())?;
                        for owned in &receipt.added {
                            let count =
                                value["hooks"][&owned.event].as_array().map_or(0, |groups| {
                                    groups.iter().filter(|g| **g == owned.group).count()
                                });
                            ensure!(
                                count == 1,
                                "owned {} hook changed; uninstall before reinstalling",
                                owned.event
                            );
                        }
                    } else {
                        features::check_enabled(current.as_deref().unwrap_or(""))?;
                    }
                }
            }
            if agent == Agent::Claude {
                let current = files::read(&receipt.hooks.path)?;
                let planned = report
                    .changes
                    .iter()
                    .find(|file| file.path == receipt.hooks.path)
                    .map(|file| file.after.clone())
                    .unwrap_or_else(|| current.clone());
                let mut value = settings(planned.as_deref())?;
                if let Some(owned) = &receipt.statusline {
                    if value.get("statusLine") != Some(&owned.after) {
                        ensure!(
                            value.get("statusLine") == owned.before.as_ref(),
                            "owned statusline changed; uninstall before reinstalling"
                        );
                        // The receipt is durable before settings are replaced.
                        // Recover an interrupted addition without nesting wrappers.
                        value["statusLine"] = owned.after.clone();
                        let after = Some(json::render(planned.as_deref().unwrap_or("{}"), &value)?);
                        report
                            .changes
                            .retain(|file| file.path != receipt.hooks.path);
                        change(
                            &mut report,
                            FileChange {
                                path: receipt.hooks.path.clone(),
                                before: current,
                                after,
                            },
                        );
                    }
                } else if options.statusline {
                    let owned = statusline::add(&options.executable, &mut value)?;
                    let after = Some(json::render(planned.as_deref().unwrap_or("{}"), &value)?);
                    report
                        .changes
                        .retain(|file| file.path != receipt.hooks.path);
                    change(
                        &mut report,
                        FileChange {
                            path: receipt.hooks.path.clone(),
                            before: current,
                            after,
                        },
                    );
                    // Keep the original hook baseline; never absorb unrelated later
                    // settings into the whole-file restoration fast path.
                    let mut baseline = settings(receipt.hooks.after.as_deref())?;
                    baseline["statusLine"] = owned.after.clone();
                    receipt.hooks.after = Some(json::render(
                        receipt.hooks.after.as_deref().unwrap_or("{}"),
                        &baseline,
                    )?);
                    receipt.statusline = Some(owned);
                    let path = receipt_path(options, agent);
                    let serialized = serde_json::to_string_pretty(&receipt)?;
                    files::check_size(&serialized)?;
                    receipts.push(FileChange {
                        before: files::read(&path)?,
                        path,
                        after: Some(serialized),
                    });
                }
            }
        } else {
            let receipt = prepare(options, agent)?;
            change(&mut report, receipt.hooks.clone());
            if let Some(config) = &receipt.config {
                change(&mut report, config.clone());
            }
            let serialized = serde_json::to_string_pretty(&receipt)?;
            files::check_size(&serialized)?;
            receipts.push(FileChange {
                path: receipt_path(options, agent),
                before: None,
                after: Some(serialized),
            });
        }
        if agent == Agent::Codex {
            report.warnings.push("Codex requires review and trust of new hooks via /hooks; installation does not grant hook trust. Local hooks do not cover hosted tools or cloud orchestration.".into());
        }
    }
    if !options.dry_run && (!receipts.is_empty() || !report.changes.is_empty()) {
        let _lock = files::Lock::acquire(&options.state_dir)?;
        files::check_unchanged(&report.changes)?;
        files::check_unchanged(&receipts)?;
        files::backup(&options.state_dir, &report.changes)?;
        // Persist ownership before touching settings: an interrupted install can be retried or uninstalled.
        files::apply(&receipts)?;
        files::apply(&report.changes)?;
    }
    Ok(report)
}

fn remove_hooks(receipt: &Receipt, current: Option<String>) -> Result<FileChange> {
    let original = settings(receipt.hooks.before.as_deref())?;
    let exact_restore = receipt
        .statusline
        .as_ref()
        .is_none_or(|owned| original.get("statusLine") == owned.before.as_ref());
    if exact_restore && (current == receipt.hooks.after || current == receipt.hooks.before) {
        return Ok(FileChange {
            path: receipt.hooks.path.clone(),
            before: current,
            after: receipt.hooks.before.clone(),
        });
    }
    // A user removed the file; uninstall must not resurrect it.
    let Some(raw) = current.as_deref() else {
        return Ok(FileChange {
            path: receipt.hooks.path.clone(),
            before: None,
            after: None,
        });
    };
    let mut value = settings(Some(raw))?;
    if let Some(hooks) = value.get_mut("hooks").and_then(Value::as_object_mut) {
        for owned in &receipt.added {
            if let Some(groups) = hooks.get_mut(&owned.event).and_then(Value::as_array_mut) {
                let matches: Vec<usize> = groups
                    .iter()
                    .enumerate()
                    .filter_map(|(i, group)| (group == &owned.group).then_some(i))
                    .collect();
                ensure!(
                    matches.len() <= 1,
                    "ambiguous duplicated owned {} hook; leaving settings untouched",
                    owned.event
                );
                if let Some(index) = matches.first() {
                    groups.remove(*index);
                } else {
                    remove_handler(groups, owned, &original)?;
                }
                if groups.is_empty() && original["hooks"].get(&owned.event).is_none() {
                    hooks.remove(&owned.event);
                }
            }
        }
        if hooks.is_empty() && original.get("hooks").is_none() {
            value.as_object_mut().unwrap().remove("hooks");
        }
    }
    if let Some(owned) = &receipt.statusline {
        statusline::restore(&mut value, owned);
    }
    let after = Some(json::render(raw, &value)?);
    Ok(FileChange {
        path: receipt.hooks.path.clone(),
        before: current,
        after,
    })
}

fn remove_handler(groups: &mut [Value], owned: &OwnedHook, original: &Value) -> Result<()> {
    let handler = &owned.group["hooks"][0];
    let command = handler["command"].as_str().unwrap();
    let mut candidates = Vec::new();
    for (index, group) in groups.iter().enumerate() {
        // Never claim a pre-existing, unchanged group, even when its command matches ours.
        if original["hooks"][&owned.event]
            .as_array()
            .is_some_and(|old| old.contains(group))
        {
            continue;
        }
        if let Some(handlers) = group["hooks"].as_array() {
            if handlers
                .iter()
                .any(|h| h["command"].as_str() == Some(command))
            {
                ensure!(
                    group.as_object().is_some_and(|g| g.len() == 1),
                    "owned {} matcher group changed; restore its recorded entry before uninstall",
                    owned.event
                );
                let indices: Vec<usize> = handlers
                    .iter()
                    .enumerate()
                    .filter_map(|(i, h)| (h == handler).then_some(i))
                    .collect();
                ensure!(
                    indices.len() == 1,
                    "owned {} handler changed or duplicated; leaving settings untouched",
                    owned.event
                );
                candidates.push((index, indices[0]));
            }
        }
    }
    ensure!(
        candidates.len() <= 1,
        "ambiguous owned {} handler; leaving settings untouched",
        owned.event
    );
    if let Some((group, handler)) = candidates.first() {
        groups[*group]["hooks"]
            .as_array_mut()
            .unwrap()
            .remove(*handler);
    }
    Ok(())
}

/// Remove only receipted entries, retaining unrelated edits made since installation.
pub fn uninstall(options: &InstallOptions) -> Result<InstallReport> {
    validate(options)?;
    let mut report = InstallReport::default();
    let mut receipts = Vec::new();
    for agent in selected(options) {
        let Some(receipt) = read_receipt(options, agent)? else {
            continue;
        };
        change(
            &mut report,
            remove_hooks(&receipt, files::read(&receipt.hooks.path)?)?,
        );
        if let Some(config) = &receipt.config {
            change(&mut report, features::restore(config)?);
        }
        let path = receipt_path(options, agent);
        let before = files::read(&path)?;
        receipts.push(FileChange {
            path,
            before,
            after: None,
        });
    }
    if !options.dry_run && !receipts.is_empty() {
        let _lock = files::Lock::acquire(&options.state_dir)?;
        files::check_unchanged(&report.changes)?;
        files::check_unchanged(&receipts)?;
        files::backup(&options.state_dir, &report.changes)?;
        files::apply(&report.changes)?;
        files::apply(&receipts)?;
    }
    Ok(report)
}

#[cfg(test)]
#[path = "install/tests.rs"]
mod tests;
