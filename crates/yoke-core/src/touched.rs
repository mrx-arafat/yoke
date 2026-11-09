//! Files an agent tool call named, read from the redacted record Yoke keeps of it.
//!
//! The stored hook payload is capped, so it may be cut in the middle of a JSON value. These
//! readers therefore scan the text for the few keys and patch markers that carry a path
//! instead of parsing the whole document, and they never invent a path that is not there.

use crate::redact::redact;

/// Paths one tool call modified or only read, in first-seen order and without repeats.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Touched {
    pub modified: Vec<String>,
    pub read: Vec<String>,
}

impl Touched {
    /// Fold `other` in, keeping first-seen order and dropping repeats.
    pub fn merge(&mut self, other: Touched) {
        for (into, from) in [
            (&mut self.modified, other.modified),
            (&mut self.read, other.read),
        ] {
            for path in from {
                if !into.contains(&path) {
                    into.push(path);
                }
            }
        }
    }
}

const MODIFYING: [&str; 4] = ["Edit", "Write", "MultiEdit", "NotebookEdit"];
const PATCH_MARKERS: [&str; 4] = [
    "*** Update File: ",
    "*** Add File: ",
    "*** Delete File: ",
    "*** Move to: ",
];

/// Paths named by one tool call. `tool` is the tool's name, `raw` its recorded payload or tool
/// input. A tool this does not know yields nothing, so a search path is never shown as a file.
pub fn files_in_payload(tool: Option<&str>, raw: &str) -> Touched {
    let mut touched = Touched::default();
    let push = |into: &mut Vec<String>, path: String| {
        let path = redact(&path);
        if !path.is_empty() && !into.contains(&path) {
            into.push(path);
        }
    };
    match tool {
        Some("Read") => {
            for path in string_values(raw, "file_path") {
                push(&mut touched.read, path);
            }
        }
        Some(name) if MODIFYING.contains(&name) => {
            for key in ["file_path", "notebook_path"] {
                for path in string_values(raw, key) {
                    push(&mut touched.modified, path);
                }
            }
        }
        Some("apply_patch") => {
            for path in patch_paths(raw) {
                push(&mut touched.modified, path);
            }
        }
        _ => {}
    }
    touched
}

/// The tool name in a stored file-write fact, which reads `Edit: {"file_path":...}`.
pub fn fact_tool(detail: &str) -> Option<&str> {
    let (tool, _) = detail.split_once(": ")?;
    (!tool.is_empty() && tool.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        .then_some(tool)
}

/// Every value of `"key":"..."` in the text, decoded; a value cut off by the cap is skipped.
fn string_values(raw: &str, key: &str) -> Vec<String> {
    let needle = format!("\"{key}\"");
    let mut found = Vec::new();
    let mut rest = raw;
    while let Some(at) = rest.find(&needle) {
        rest = &rest[at + needle.len()..];
        let after = rest.trim_start();
        let Some(after) = after.strip_prefix(':') else {
            continue;
        };
        let Some(body) = after.trim_start().strip_prefix('"') else {
            continue;
        };
        let mut escaped = false;
        let end = body.char_indices().find_map(|(index, ch)| {
            let closes = ch == '"' && !escaped;
            escaped = ch == '\\' && !escaped;
            closes.then_some(index)
        });
        if let Some(end) = end {
            if let Ok(value) = serde_json::from_str::<String>(&format!("\"{}\"", &body[..end])) {
                found.push(value);
            }
            rest = &body[end..];
        }
    }
    found
}

/// Paths after `*** Update File: ` and the other patch headers, up to the end of the line.
fn patch_paths(raw: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for marker in PATCH_MARKERS {
        let mut rest = raw;
        while let Some(at) = rest.find(marker) {
            rest = &rest[at + marker.len()..];
            // In stored JSON a line break is the two characters backslash and n.
            let end = rest.find(['\\', '"', '\n', '\r']).unwrap_or(rest.len());
            let path = rest[..end].trim();
            if !path.is_empty() {
                paths.push(path.to_owned());
            }
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_come_from_the_keys_and_markers_that_carry_them_even_when_the_payload_is_cut() {
        let edit = r#"{"tool_name":"Edit","tool_input":{"file_path":"/p/src/a \"q\".rs","old_string":"x"},"tool_resp"#;
        assert_eq!(
            files_in_payload(Some("Edit"), edit).modified,
            vec![r#"/p/src/a "q".rs"#]
        );
        let read = r#"{"tool_input":{"file_path":"/p/Cargo.toml"}}"#;
        let touched = files_in_payload(Some("Read"), read);
        assert_eq!(
            (touched.read, touched.modified),
            (vec!["/p/Cargo.toml".into()], vec![])
        );
        let patch = r#"{"tool_input":{"command":"*** Begin Patch\n*** Update File: src/x.rs\n@@\n*** Add File: docs/n.md\n+hi\n*** End Patch"}}"#;
        assert_eq!(
            files_in_payload(Some("apply_patch"), patch).modified,
            vec!["src/x.rs", "docs/n.md"]
        );
        // A search directory is not a touched file, and a value cut by the cap is not a path.
        assert_eq!(
            files_in_payload(Some("Grep"), r#"{"path":"/p","file_path":"/q"}"#),
            Touched::default()
        );
        assert_eq!(
            files_in_payload(Some("Edit"), r#"{"file_path":"/p/cut"#),
            Touched::default()
        );
        assert_eq!(fact_tool(r#"Edit: {"file_path":"/p/a.rs"}"#), Some("Edit"));
        assert_eq!(fact_tool("no tool here"), None);
    }
}
