//! Installation previews never include neighboring agent settings or unrelated hook groups.

use serde_json::Value;

use super::FileChange;

pub(super) fn lines(change: &FileChange) -> Vec<String> {
    let values = [change.before.as_deref(), change.after.as_deref()]
        .map(|raw| serde_json::from_str::<Value>(raw.unwrap_or("{}")).ok());
    let mut lines = Vec::new();
    if let [Some(before), Some(after)] = values {
        for (sign, own, other) in [("-", &before, &after), ("+", &after, &before)] {
            if let Some(events) = own.get("hooks").and_then(Value::as_object) {
                for (event, groups) in events {
                    for group in groups.as_array().into_iter().flatten() {
                        for handler in group["hooks"].as_array().into_iter().flatten() {
                            let command = handler["command"].as_str().unwrap_or("");
                            if !command.contains(" run-hook --agent ") {
                                continue;
                            }
                            let unchanged =
                                other["hooks"][event].as_array().into_iter().flatten().any(
                                    |group| {
                                        group["hooks"]
                                            .as_array()
                                            .is_some_and(|handlers| handlers.contains(handler))
                                    },
                                );
                            if !unchanged {
                                lines.push(format!("{sign} hooks.{event}: {handler}"));
                            }
                        }
                    }
                }
            }
        }
        if before.get("statusLine") != after.get("statusLine") {
            lines.push("- statusLine: previous command (contents hidden)".into());
            lines.push("+ statusLine: replacement command (contents hidden)".into());
        }
    } else {
        let docs = [change.before.as_deref(), change.after.as_deref()]
            .map(|raw| raw.unwrap_or("").parse::<toml_edit::DocumentMut>().ok());
        if let [Some(before), Some(after)] = docs {
            for key in ["hooks", "codex_hooks"] {
                let flag = |doc: &toml_edit::DocumentMut| {
                    doc.get("features")
                        .and_then(toml_edit::Item::as_table_like)
                        .and_then(|t| t.get(key))
                        .and_then(toml_edit::Item::as_bool)
                };
                if flag(&before) != flag(&after) {
                    lines.push(format!("- features.{key}: {:?}", flag(&before)));
                    lines.push(format!("+ features.{key}: {:?}", flag(&after)));
                }
            }
        }
    }
    // Reuse the common inert-text renderer after removing all unrelated settings.
    let text = lines
        .into_iter()
        .map(|line| yoke_core::redact::redact(&line))
        .collect::<Vec<_>>()
        .join("\n");
    crate::preview::change_lines(None, Some(&text))
        .into_iter()
        .map(|line| line.strip_prefix("+ ").unwrap_or(&line).to_owned())
        .collect()
}
