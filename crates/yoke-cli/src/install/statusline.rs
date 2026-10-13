//! Claude's documented statusLine command field, with exact-value ownership.
use std::path::Path;

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Serialize, Deserialize, PartialEq)]
pub(super) struct Owned {
    pub before: Option<Value>,
    pub after: Value,
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

pub(super) fn build(executable: &Path, before: Option<Value>) -> Result<Owned> {
    let executable = executable
        .to_str()
        .context("executable path must be UTF-8")?;
    let base = format!("{} statusline", quote(executable));
    let mut after = before.clone().unwrap_or_else(|| json!({"type":"command"}));
    ensure!(
        after.is_object() && after["type"] == "command",
        "existing statusLine must be a command object; no settings changed"
    );
    let command = match before.as_ref() {
        None => base,
        Some(value) => {
            let existing = value["command"]
                .as_str()
                .context("existing statusLine command must be a string")?;
            ensure!(
                !existing.trim().is_empty() && existing.len() <= 4096 && !existing.contains('\0'),
                "existing statusLine command is empty or exceeds wrapper limits"
            );
            // A pre-existing identical entry belongs to the user. It stays
            // identical on uninstall rather than being wrapped recursively.
            if existing == base {
                base
            } else {
                ensure!(
                    !existing.starts_with(&format!("{base} --wrap ")),
                    "existing Yoke statusline lacks matching receipt; refusing recursive wrapping"
                );
                format!("{base} --wrap {}", quote(existing))
            }
        }
    };
    after["command"] = Value::String(command);
    Ok(Owned { before, after })
}

pub(super) fn add(executable: &Path, value: &mut Value) -> Result<Owned> {
    let owned = build(executable, value.get("statusLine").cloned())?;
    value["statusLine"] = owned.after.clone();
    Ok(owned)
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
