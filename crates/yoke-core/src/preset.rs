//! Bundled, inspectable configuration additions. Adding never grants trust.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;
use toml::Value;

use crate::config::Config;

#[derive(Clone, Copy, Debug, Serialize)]
pub struct PresetInfo {
    pub name: &'static str,
    pub description: &'static str,
}

#[derive(Default, Debug)]
pub struct PresetOptions {
    pub replacement: Option<String>,
    pub wrapper: Option<String>,
    pub command_prefix: Option<String>,
    pub acknowledge_rewrite_permissions: bool,
}

#[derive(Debug, Serialize)]
pub struct AddOutcome {
    pub changed: bool,
    pub message: String,
}

pub fn list() -> &'static [PresetInfo] {
    &[
        PresetInfo {
            name: "git-safety",
            description: "Deny common destructive Git commands; not a shell sandbox",
        },
        PresetInfo {
            name: "web-redirect",
            description: "Deny built-in web tools and name your replacement",
        },
        PresetInfo {
            name: "token-saver",
            description: "Opt-in narrow command rewrite; bypasses Codex permission prompts",
        },
        PresetInfo {
            name: "ui",
            description: "Commented UI lint and screenshot check templates for plan headers",
        },
    ]
}

fn gate(name: &str, action: &str, tools: &str, message: &str) -> toml::map::Map<String, Value> {
    [
        ("name", name),
        ("on", "pre_tool_use"),
        ("action", action),
        ("match_tool", tools),
        ("message", message),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), Value::String(value.to_owned())))
    .collect()
}

fn executable_on_path(name: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|directory| {
            fs::metadata(directory.join(name)).is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
    })
}

/// Render inspectable TOML; options never become unquoted shell commands.
pub fn render(name: &str, options: &PresetOptions) -> Result<String> {
    let marker = format!("# Yoke preset: {name}\n");
    if name == "ui" {
        return Ok(format!("{marker}# Copy these into a plan's +++ TOML header and replace each placeholder.\n# [[check]]\n# name = \"ui-lint\"\n# run = \"<your UI lint command>\"\n# blocking = true\n# [[check]]\n# name = \"ui-screenshot\"\n# run = \"<your headed Playwright screenshot command>\"\n# blocking = true\n# artifacts = [] # Declare the actual screenshot paths here.\n# # Write generated artifacts into a Git-ignored output directory.\n# # Otherwise generated files invalidate the whole-tree fingerprint.\n# # Artifact declarations do not exclude paths from fingerprinting.\n"));
    }
    let mut gates = Vec::new();
    let mut warning = String::new();
    match name {
        "git-safety" => {
            // Match ordinary Git invocations and common global options. Shell
            // aliases, expansions and arbitrary programs need OS isolation.
            let git = r"\bgit(?:\s+(?:-C|-c|--git-dir|--work-tree)\s+[^\s;]+)*\s+";
            for (suffix, pattern) in [
                (
                    "force-push",
                    r"push\b[^;\n]*\s(?:--force(?:-with-lease)?(?:[=\s\x22]|$)|-[A-Za-z]*f[A-Za-z]*(?:[\s\x22]|$)|\+[^\s\x22]+)",
                ),
                ("hard-reset", r"reset\b[^;\n]*\s--hard(?:[\s\x22]|$)"),
                (
                    "force-clean",
                    r"clean\b[^;\n]*\s(?:--force(?:[\s\x22]|$)|-[A-Za-z]*f[A-Za-z]*(?:[\s\x22]|$))",
                ),
                ("checkout-discard", r"checkout\b[^;\n]*\s--(?:[\s\x22]|$)"),
            ] {
                let mut entry = gate(&format!("preset-git-{suffix}"), "deny", "Bash|exec_command|shell_command|shell", "Git safety preset blocked a destructive command. Review it and run it yourself if intended.");
                entry.insert(
                    "match_pattern".into(),
                    Value::String(format!("{git}{pattern}")),
                );
                gates.push(Value::Table(entry));
            }
        }
        "web-redirect" => {
            let replacement = options
                .replacement
                .as_deref()
                .context("web-redirect requires --replacement")?;
            ensure!(
                !replacement.trim().is_empty()
                    && replacement.len() <= 256
                    && !replacement.chars().any(char::is_control),
                "replacement must be a short, single-line instruction"
            );
            gates.push(Value::Table(gate(
                "preset-web-redirect",
                "deny",
                r"^(?:WebSearch|WebFetch|web|web\.run)$",
                &format!("Use {replacement} for web access."),
            )));
        }
        "token-saver" => {
            ensure!(options.acknowledge_rewrite_permissions, "token-saver requires --acknowledge-rewrite-permissions: Codex rewrites bypass its normal permission prompt; Claude preserves permission evaluation");
            let wrapper = options
                .wrapper
                .as_deref()
                .context("token-saver requires --wrapper")?;
            ensure!(
                !wrapper.is_empty()
                    && wrapper != "."
                    && wrapper != ".."
                    && wrapper
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b)),
                "wrapper must be an executable basename"
            );
            ensure!(
                executable_on_path(wrapper),
                "wrapper is not executable on PATH"
            );
            let prefix = options
                .command_prefix
                .as_deref()
                .context("token-saver requires --command-prefix")?;
            ensure!(
                !prefix.is_empty()
                    && prefix.len() <= 128
                    && prefix.trim() == prefix
                    && prefix.split(' ').all(|word| !word.is_empty()
                        && word
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"_./-".contains(&b))),
                "command prefix must contain literal shell-safe words"
            );
            ensure!(
                prefix.split(' ').next() != Some(wrapper),
                "wrapper cannot wrap itself"
            );
            warning = "# Permission warning: Codex rewrites answer allow and bypass its normal permission prompt.\n# Claude rewrites preserve normal permission evaluation.\n# Only the explicit prefix and simple arguments match; no shell chains or expansions.\n".into();
            for (suffix, tools, field) in [
                ("command", "Bash|shell_command", "command"),
                ("cmd", "exec_command|shell", "cmd"),
            ] {
                let mut entry = gate(
                    &format!("preset-token-saver-{suffix}"),
                    "rewrite",
                    tools,
                    "Opt-in command wrapper; Codex permission prompt is bypassed; Claude permissions are preserved.",
                );
                for (key, value) in [
                    ("rewrite_field", field.to_owned()),
                    (
                        "rewrite_pattern",
                        format!(
                            r"^{}(?:[ \t]+[A-Za-z0-9_./=:+,@% \t-]*)?$",
                            regex::escape(prefix)
                        ),
                    ),
                    ("rewrite_to", format!("{wrapper} $0")),
                    ("require_command", wrapper.to_owned()),
                ] {
                    entry.insert(key.into(), Value::String(value));
                }
                gates.push(Value::Table(entry));
            }
        }
        _ => bail!("unknown preset {name}; use yoke preset list"),
    }
    let document = Value::Table([("gate".into(), Value::Array(gates))].into_iter().collect());
    Ok(format!("{marker}{warning}{}", toml::to_string(&document)?))
}

/// Append one validated preset atomically, preserving existing bytes and trust state.
pub fn add(path: &Path, name: &str, options: &PresetOptions) -> Result<AddOutcome> {
    let block = render(name, options)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).context("create config parent")?;
    let lock_path = path.with_extension("preset.lock");
    let mut lock_file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&lock_path)
        .context("preset config is locked; ensure no other preset writer is running")?;
    ensure!(
        lock_file.metadata()?.is_file(),
        "preset lock must be a regular file"
    );
    lock_file
        .try_lock()
        .context("preset config is locked; retry after the other preset writer finishes")?;
    lock_file.set_len(0)?;
    writeln!(lock_file, "{}", std::process::id())?;
    let original = read_config(path)?;
    let text = std::str::from_utf8(&original).context("config is not UTF-8")?;
    if text.contains(&block) {
        return Ok(AddOutcome {
            changed: false,
            message: "Preset already present; run yoke trust if this config is not yet trusted."
                .into(),
        });
    }
    ensure!(
        !text.contains(&format!("# Yoke preset: {name}\n")),
        "preset already exists with different options; edit it explicitly"
    );
    Config::parse(text)?;
    let combined = format!("{text}\n{block}");
    Config::parse(&combined).context("preset would create invalid or conflicting configuration")?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    staged.write_all(combined.as_bytes())?;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        staged.as_file().set_permissions(metadata.permissions())?;
    }
    staged.as_file().sync_all()?;
    ensure!(
        read_config(path)? == original,
        "config changed while preparing preset; retry"
    );
    staged.persist(path).map_err(|error| error.error)?;
    Ok(AddOutcome { changed: true, message: "Preset added. Review the config, then run yoke trust (or yoke trust --global for global config); it is not trusted automatically.".into() })
}

fn read_config(path: &Path) -> Result<Vec<u8>> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
        Ok(metadata) => ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "config must be a regular file, not a symlink"
        ),
    }
    use std::os::unix::fs::OpenOptionsExt;
    let mut bytes = Vec::new();
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    ensure!(file.metadata()?.is_file(), "config must be a regular file");
    file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 1024 * 1024, "config exceeds 1 MiB");
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        engine::evaluate,
        event::{AgentEvent, EventKind},
    };
    use serde_json::json;

    #[test]
    fn preset_lock_records_the_owner_and_recovers_after_descriptor_exit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("yoke.toml");
        let lock_path = path.with_extension("preset.lock");
        fs::write(&lock_path, "stale process\n").unwrap();
        let options = PresetOptions::default();
        assert!(add(&path, "git-safety", &options).unwrap().changed);
        assert_eq!(
            fs::read_to_string(&lock_path).unwrap(),
            format!("{}\n", std::process::id())
        );
        let lock = OpenOptions::new().write(true).open(&lock_path).unwrap();
        lock.try_lock().unwrap();
        assert!(add(&path, "git-safety", &options).is_err());
        drop(lock);
        assert!(!add(&path, "git-safety", &options).unwrap().changed);
    }

    #[test]
    fn git_preset_blocks_destructive_invocations_and_preserves_normal_commands() {
        let cfg = Config::parse(&render("git-safety", &PresetOptions::default()).unwrap()).unwrap();
        for command in [
            "git push --force",
            "git -C /tmp push -f",
            "git push origin +main",
            "git reset --hard HEAD",
            "git clean -fd",
            "git checkout -- file",
        ] {
            let result = evaluate(
                &cfg,
                EventKind::PreToolUse,
                &AgentEvent {
                    tool_name: Some("Bash".into()),
                    tool_input: json!({"command":command}),
                    ..Default::default()
                },
            );
            assert!(result.blocked(), "{command}");
        }
        let result = evaluate(
            &cfg,
            EventKind::PreToolUse,
            &AgentEvent {
                tool_name: Some("Bash".into()),
                tool_input: json!({"command":"git status --short"}),
                ..Default::default()
            },
        );
        assert!(!result.blocked());
    }

    #[test]
    fn append_is_idempotent_preserves_bytes_and_refuses_conflicts_and_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("yoke.toml");
        fs::write(&path, "# user's comment\nmode = 'observe'\n").unwrap();
        let result = add(&path, "git-safety", &PresetOptions::default()).unwrap();
        assert!(result.changed && result.message.contains("not trusted automatically"));
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with("# user's comment\nmode = 'observe'\n"));
        assert!(
            !add(&path, "git-safety", &PresetOptions::default())
                .unwrap()
                .changed
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), content);
        let link = temp.path().join("link.toml");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(add(&link, "ui", &PresetOptions::default()).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), content);
    }

    #[test]
    fn rewrites_require_explicit_consent_and_narrow_safe_arguments() {
        let mut options = PresetOptions {
            wrapper: Some("env".into()),
            command_prefix: Some("git status".into()),
            ..Default::default()
        };
        assert!(render("token-saver", &options).is_err());
        options.acknowledge_rewrite_permissions = true;
        let text = render("token-saver", &options).unwrap();
        assert!(text.contains("Codex rewrites answer allow"));
        assert!(text.contains("Claude rewrites preserve normal permission evaluation"));
        let cfg = Config::parse(&text).unwrap();
        for (command, expected) in [
            ("git status --short", Some("env git status --short")),
            ("git status; rm file", None),
            ("git status $(id)", None),
            ("git push", None),
        ] {
            let result = evaluate(
                &cfg,
                EventKind::PreToolUse,
                &AgentEvent {
                    tool_name: Some("Bash".into()),
                    tool_input: json!({"command":command}),
                    ..Default::default()
                },
            );
            assert_eq!(
                result
                    .updated_input
                    .as_ref()
                    .and_then(|v| v["command"].as_str()),
                expected
            );
        }
        let redirect = render(
            "web-redirect",
            &PresetOptions {
                replacement: Some("my-search 'query'".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(Config::parse(&redirect).unwrap().gates[0]
            .message
            .as_ref()
            .unwrap()
            .contains("my-search"));
        assert!(Config::parse(&render("ui", &PresetOptions::default()).unwrap()).is_ok());
    }

    #[test]
    fn ui_template_uses_the_public_plan_check_schema() {
        let template = render("ui", &PresetOptions::default()).unwrap();
        let checks = template
            .lines()
            .skip(2)
            .map(|line| line.strip_prefix("# ").unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        let header: crate::plan::PlanHeader =
            toml::from_str(&format!("title = 'UI checks'\n{checks}")).unwrap();
        assert_eq!(header.checks.len(), 2);
        assert_eq!(header.checks[0].command, "<your UI lint command>");
        assert_eq!(
            header.checks[1].command,
            "<your headed Playwright screenshot command>"
        );
        assert!(header.checks.iter().all(|check| check.blocking));
        assert!(header.checks[1].artifacts.is_empty());
        assert!(template.contains("Git-ignored output directory"));
    }
}
