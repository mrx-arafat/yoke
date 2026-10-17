//! Claude's documented statusLine command field, with exact-value ownership.
use std::path::Path;

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::Statusline;
use crate::statusline::Style;

#[derive(Serialize, Deserialize, PartialEq)]
pub(super) struct Owned {
    pub before: Option<Value>,
    pub after: Value,
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// `'<exe>' statusline`, with `--style detailed` when that style is asked for; the compact
/// default adds no flag, so a line installed before styles existed stays byte for byte equal.
fn base(executable: &str, style: Style) -> String {
    match style {
        Style::Compact => format!("{} statusline", quote(executable)),
        Style::Detailed => format!("{} statusline --style detailed", quote(executable)),
    }
}

/// The style a command asks for: detailed only when it carries the flag right after `statusline`.
pub(super) fn style_of(command: &str) -> Style {
    let detailed = command
        .split_once(" statusline --style detailed")
        .is_some_and(|(_, rest)| rest.is_empty() || rest.starts_with(" --wrap "));
    if detailed {
        Style::Detailed
    } else {
        Style::Compact
    }
}

/// `style` is what the user asked for. Without it, a line that already runs Yoke keeps its own
/// style and a new one is compact.
pub(super) fn build(
    executable: &Path,
    before: Option<Value>,
    style: Option<Style>,
) -> Result<Owned> {
    let executable = executable
        .to_str()
        .context("executable path must be UTF-8")?;
    let (compact, detailed) = (
        base(executable, Style::Compact),
        base(executable, Style::Detailed),
    );
    let mut after = before.clone().unwrap_or_else(|| json!({"type":"command"}));
    ensure!(
        after.is_object() && after["type"] == "command",
        "existing statusLine must be a command object; no settings changed"
    );
    let command = match before.as_ref() {
        None => base(executable, style.unwrap_or_default()),
        Some(value) => {
            let existing = value["command"]
                .as_str()
                .context("existing statusLine command must be a string")?;
            ensure!(
                !existing.trim().is_empty() && existing.len() <= 4096 && !existing.contains('\0'),
                "existing statusLine command is empty or exceeds wrapper limits"
            );
            // A pre-existing identical entry belongs to the user; asking for the other style
            // restyles it, and uninstall puts it back as it was rather than wrapping it.
            if existing == compact || existing == detailed {
                base(executable, style.unwrap_or_else(|| style_of(existing)))
            } else {
                ensure!(
                    !existing.starts_with(&format!("{compact} --wrap "))
                        && !existing.starts_with(&format!("{detailed} --wrap ")),
                    "existing Yoke statusline lacks matching receipt; refusing recursive wrapping"
                );
                format!(
                    "{} --wrap {}",
                    base(executable, style.unwrap_or_default()),
                    quote(existing)
                )
            }
        }
    };
    after["command"] = Value::String(command);
    Ok(Owned { before, after })
}

pub(super) fn add(executable: &Path, value: &mut Value, style: Option<Style>) -> Result<Owned> {
    let owned = build(executable, value.get("statusLine").cloned(), style)?;
    value["statusLine"] = owned.after.clone();
    Ok(owned)
}

/// The ownership record for a line Yoke already owns when the user now asks for a different
/// style, or `None` when it already has that style. Wrapped commands stay wrapped.
pub(super) fn restyle(executable: &Path, owned: &Owned, style: Style) -> Result<Option<Owned>> {
    let current = owned.after["command"].as_str().map(style_of);
    if current == Some(style) {
        return Ok(None);
    }
    build(executable, owned.before.clone(), Some(style)).map(Some)
}

/// Apply `mode` to `value`. Returns the ownership record when Yoke's statusline was added.
pub(super) fn apply(
    mode: Statusline,
    style: Option<Style>,
    executable: &Path,
    value: &mut Value,
    warnings: &mut Vec<String>,
) -> Result<Option<Owned>> {
    match mode {
        Statusline::Off => Ok(None),
        Statusline::Wrap => add(executable, value, style).map(Some),
        Statusline::IfAbsent => match value.get("statusLine") {
            None => add(executable, value, style).map(Some),
            Some(existing) if runs_yoke(existing) => Ok(None),
            Some(existing) => {
                warnings.push(kept_warning(existing));
                Ok(None)
            }
        },
    }
}

/// Whether a statusLine already runs some `yoke statusline`, which is Yoke's whoever wrote it.
pub(super) fn runs_yoke(existing: &Value) -> bool {
    let Some(command) = existing["command"].as_str() else {
        return false;
    };
    // Yoke writes `'<path>/yoke' statusline` with the program path in single quotes.
    let command = command.trim_start();
    let (program, rest) = match command.strip_prefix('\'') {
        Some(quoted) => match quoted.split_once('\'') {
            Some(parts) => parts,
            None => return false,
        },
        None => command.split_once(' ').unwrap_or((command, "")),
    };
    Path::new(program)
        .file_name()
        .is_some_and(|name| name == "yoke")
        && (rest == " statusline" || rest.starts_with(" statusline "))
}

/// The line the installer script looks for; keep the `statusline kept:` prefix stable.
fn kept_warning(existing: &Value) -> String {
    let command = existing["command"].as_str().map_or_else(
        || "not a command".to_owned(),
        |command| {
            let mut shown: String = command.chars().take(80).collect();
            if shown.len() < command.len() {
                shown.push_str("...");
            }
            format!("`{shown}`")
        },
    );
    format!(
        "statusline kept: Claude already has a statusLine ({command}); Yoke left it unchanged. `yoke install --agent claude --statusline` puts Yoke's line first and that command's output after it, with its colors removed and its lines joined."
    )
}

pub(super) fn restore(value: &mut Value, owned: &Owned) {
    // A later edited or removed statusLine is user-owned and must survive.
    if value.get("statusLine") == Some(&owned.after) {
        match &owned.before {
            Some(before) => value["statusLine"] = before.clone(),
            None => {
                value.as_object_mut().unwrap().remove("statusLine");
            }
        }
    }
}
