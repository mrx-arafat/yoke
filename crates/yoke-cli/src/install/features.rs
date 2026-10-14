use std::path::Path;

use anyhow::{ensure, Context, Result};
use toml_edit::{value, DocumentMut, Item, Table};

use super::{files, FileChange};

fn parse(raw: &str) -> Result<DocumentMut> {
    let doc: DocumentMut = raw.parse().context("parsing Codex config.toml")?;
    if let Some(features) = doc.get("features") {
        let table = features
            .as_table_like()
            .context("Codex features must be a table")?;
        for key in ["hooks", "codex_hooks"] {
            if let Some(flag) = table.get(key) {
                ensure!(flag.as_bool().is_some(), "features.{key} must be boolean");
            }
        }
        if let (Some(current), Some(alias)) = (table.get("hooks"), table.get("codex_hooks")) {
            ensure!(
                current.as_bool() == alias.as_bool(),
                "conflicting hooks and codex_hooks feature flags; resolve before installing"
            );
        }
    }
    Ok(doc)
}

/// Codex enables hooks by default; only an explicit `false` flag turns them off.
pub(super) fn hooks_disabled(raw: &str) -> Result<bool> {
    let doc = parse(raw)?;
    Ok(doc
        .get("features")
        .and_then(Item::as_table_like)
        .is_some_and(|table| {
            ["hooks", "codex_hooks"]
                .iter()
                .any(|key| table.get(key).and_then(Item::as_bool) == Some(false))
        }))
}

pub(super) fn check_enabled(raw: &str) -> Result<()> {
    let doc = parse(raw)?;
    ensure!(
        doc.get("allow_managed_hooks_only").and_then(Item::as_bool) != Some(true),
        "Codex allows managed hooks only"
    );
    if let Some(table) = doc.get("features").and_then(Item::as_table_like) {
        for key in ["hooks", "codex_hooks"] {
            ensure!(
                table.get(key).and_then(Item::as_bool) != Some(false),
                "Codex hooks disabled after install; uninstall before reinstalling"
            );
        }
    }
    Ok(())
}

pub(super) fn prepare(path: &Path) -> Result<Option<FileChange>> {
    let before = files::read(path)?;
    let mut doc = parse(before.as_deref().unwrap_or(""))?;
    ensure!(
        doc.get("allow_managed_hooks_only").and_then(Item::as_bool) != Some(true),
        "Codex allows managed hooks only"
    );
    if !doc.contains_key("features") {
        doc["features"] = Item::Table(Table::new());
    }
    let features = doc["features"].as_table_like_mut().unwrap();
    // Keep the documented alias when already present, avoiding undocumented alias precedence.
    let keys: Vec<&str> = if features.contains_key("codex_hooks") {
        if features.contains_key("hooks") {
            vec!["hooks", "codex_hooks"]
        } else {
            vec!["codex_hooks"]
        }
    } else {
        vec!["hooks"]
    };
    for key in keys {
        if features.get(key).and_then(Item::as_bool) != Some(true) {
            let mut enabled = value(true);
            if let Some(previous) = features.get(key).and_then(Item::as_value) {
                *enabled.as_value_mut().unwrap().decor_mut() = previous.decor().clone();
            }
            features.insert(key, enabled);
        }
    }
    let after = Some(doc.to_string());
    Ok((before != after).then(|| FileChange {
        path: path.to_path_buf(),
        before,
        after,
    }))
}

pub(super) fn restore(file: &FileChange) -> Result<FileChange> {
    let current = files::read(&file.path)?;
    if current == file.after || current == file.before {
        return Ok(FileChange {
            path: file.path.clone(),
            before: current,
            after: file.before.clone(),
        });
    }
    let Some(raw) = current.as_deref() else {
        return Ok(FileChange {
            path: file.path.clone(),
            before: None,
            after: None,
        });
    };
    let mut doc = parse(raw)?;
    let original = parse(file.before.as_deref().unwrap_or(""))?;
    let installed = parse(file.after.as_deref().unwrap_or(""))?;
    if let Some(features) = doc.get_mut("features").and_then(Item::as_table_like_mut) {
        for key in ["hooks", "codex_hooks"] {
            let old = original
                .get("features")
                .and_then(Item::as_table_like)
                .and_then(|t| t.get(key));
            let new = installed
                .get("features")
                .and_then(Item::as_table_like)
                .and_then(|t| t.get(key));
            if old.and_then(Item::as_bool) != new.and_then(Item::as_bool)
                && features.get(key).and_then(Item::as_bool) == Some(true)
            {
                if let Some(old) = old {
                    features.insert(key, old.clone());
                } else {
                    features.remove(key);
                }
            }
        }
        if features.is_empty() && original.get("features").is_none() {
            doc.remove("features");
        }
    }
    Ok(FileChange {
        path: file.path.clone(),
        before: current,
        after: Some(doc.to_string()),
    })
}
