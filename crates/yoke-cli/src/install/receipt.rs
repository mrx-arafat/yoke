//! Durable ownership records contain owned values and hashes, never whole agent settings.

use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{files, settings, FileChange, Receipt};

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Hashes {
    hooks_before: Option<String>,
    hooks_after: Option<String>,
    config_before: Option<String>,
    config_after: Option<String>,
}

pub(super) fn hash(raw: &str) -> String {
    format!("{:x}", Sha256::digest(raw.as_bytes()))
}

fn hooks(raw: Option<&str>, after: bool, receipt: &Receipt) -> Result<Option<String>> {
    let Some(raw) = raw else { return Ok(None) };
    let value = settings(Some(raw))?;
    let mut minimal = json!({});
    if let Some(events) = value.get("hooks").and_then(Value::as_object) {
        minimal["hooks"] = json!({});
        for event in events
            .keys()
            .filter(|event| receipt.added.iter().any(|owned| &owned.event == *event))
        {
            // Empty arrays record structural presence, not another tool's settings.
            minimal["hooks"][event] = json!([]);
        }
    }
    if after {
        for owned in &receipt.added {
            minimal["hooks"][&owned.event] = json!([owned.group]);
        }
    }
    // A wrapped command is an owned setting needed to reverse the wrapper.
    if let Some(owned) = &receipt.statusline {
        if after {
            minimal["statusLine"] = owned.after.clone();
        } else if let Some(before) = &owned.before {
            minimal["statusLine"] = before.clone();
        }
    }
    Ok(Some(minimal.to_string()))
}

fn config(raw: Option<&str>) -> Result<Option<String>> {
    let Some(raw) = raw else { return Ok(None) };
    let doc = raw.parse::<toml_edit::DocumentMut>()?;
    let mut minimal = toml_edit::DocumentMut::new();
    if let Some(features) = doc.get("features").and_then(toml_edit::Item::as_table_like) {
        minimal["features"] = toml_edit::Item::Table(toml_edit::Table::new());
        for key in ["hooks", "codex_hooks"] {
            if let Some(value) = features.get(key).and_then(toml_edit::Item::as_bool) {
                minimal["features"][key] = toml_edit::value(value);
            }
        }
    }
    Ok(Some(minimal.to_string()))
}

pub(super) fn serialize(receipt: &Receipt, _changes: &[FileChange]) -> Result<String> {
    let mut value = serde_json::to_value(receipt)?;
    value["version"] = json!(2);
    if receipt.version == 1 {
        let original = settings(receipt.hooks.before.as_deref())?;
        for (index, owned) in receipt.added.iter().enumerate() {
            let hashes: Vec<_> = original["hooks"][&owned.event]
                .as_array()
                .into_iter()
                .flatten()
                .map(|group| hash(&group.to_string()))
                .collect();
            value["added"][index]["preexisting"] = serde_json::to_value(hashes)?;
        }
    }
    let old = receipt.hashes.as_ref();
    let hashes = Hashes {
        hooks_before: old
            .and_then(|h| h.hooks_before.clone())
            .or_else(|| receipt.hooks.before.as_deref().map(hash)),
        hooks_after: old
            .and_then(|h| h.hooks_after.clone())
            .or_else(|| receipt.hooks.after.as_deref().map(hash)),
        config_before: old.and_then(|h| h.config_before.clone()).or_else(|| {
            receipt
                .config
                .as_ref()
                .and_then(|file| file.before.as_deref().map(hash))
        }),
        config_after: old.and_then(|h| h.config_after.clone()).or_else(|| {
            receipt
                .config
                .as_ref()
                .and_then(|file| file.after.as_deref().map(hash))
        }),
    };
    value["hashes"] = serde_json::to_value(hashes)?;
    value["hooks"]["before"] =
        serde_json::to_value(hooks(receipt.hooks.before.as_deref(), false, receipt)?)?;
    value["hooks"]["after"] =
        serde_json::to_value(hooks(receipt.hooks.after.as_deref(), true, receipt)?)?;
    value["previous_hooks_after"] = Value::Null;
    if let Some(file) = &receipt.config {
        value["config"]["before"] = serde_json::to_value(config(file.before.as_deref())?)?;
        value["config"]["after"] = serde_json::to_value(config(file.after.as_deref())?)?;
    }
    Ok(serde_json::to_string_pretty(&value)?)
}

/// Recover exact formatting only from bounded private backups; hashes remain sufficient for
/// ownership-based removal if an old backup has expired.
pub(super) fn hydrate(receipt: &mut Receipt, state_dir: &Path) -> Result<()> {
    let Some(hashes) = &receipt.hashes else {
        return Ok(());
    };
    for (file, before_hash, after_hash) in std::iter::once((
        &mut receipt.hooks,
        &hashes.hooks_before,
        &hashes.hooks_after,
    ))
    .chain(
        receipt
            .config
            .iter_mut()
            .map(|file| (file, &hashes.config_before, &hashes.config_after)),
    ) {
        let mut original_available = before_hash.is_none();
        if let Some(expected) = before_hash {
            if let Some(original) = files::original(state_dir, expected)? {
                file.before = Some(original);
                original_available = true;
            }
        }
        if let Some(current) = files::read(&file.path)? {
            if original_available && after_hash.as_deref() == Some(hash(&current).as_str()) {
                file.after = Some(current);
            }
        }
    }
    Ok(())
}
