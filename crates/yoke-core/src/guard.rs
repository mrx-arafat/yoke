//! Best-effort protection from accidental self-unblocking, not a sandbox.

use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::event::{AgentEvent, EventKind};

static HUMAN_COMMAND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:^|[\s/;|&()'\"])yoke['\"]?\s+(?:trust\b|plan\s+(?:approve|resume)\b)"#)
        .expect("constant human-command regex")
});

/// Return the reason to refuse a PreToolUse operation, regardless of mode.
pub fn evaluate(kind: EventKind, event: &AgentEvent, home: &Path, cwd: &Path) -> Option<String> {
    if kind != EventKind::PreToolUse {
        return None;
    }
    let tool = event
        .tool_name
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let input = &event.tool_input;
    let is_patch = tool == "apply_patch";
    if !is_patch {
        for field in ["command", "cmd"] {
            if let Some(command) = input.get(field) {
                let text = match command {
                    Value::String(text) => text.clone(),
                    Value::Array(args) => args
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" "),
                    _ => continue,
                };
                if HUMAN_COMMAND.is_match(&text) {
                    return Some(
                        "Yoke trust and plan approval/resume commands require a human terminal"
                            .into(),
                    );
                }
                if text.contains("state.db") {
                    return Some("agent shell access to state.db is protected".into());
                }
            }
        }
    }
    let protected = resolved(&home.join(".yoke"));
    if matches!(
        tool.as_str(),
        "write" | "edit" | "multiedit" | "notebookedit" | "apply_patch"
    ) {
        if protected_target(input, home, cwd, &protected) {
            return Some("agent writes inside ~/.yoke are protected".into());
        }
        if is_patch {
            let patch = input
                .as_str()
                .or_else(|| input.get("command").and_then(Value::as_str));
            if let Some(patch) = patch {
                for line in patch.lines() {
                    for prefix in [
                        "*** Add File: ",
                        "*** Update File: ",
                        "*** Delete File: ",
                        "*** Move to: ",
                    ] {
                        if let Some(path) = line.strip_prefix(prefix) {
                            if is_protected(path, home, cwd, &protected) {
                                return Some("agent patches inside ~/.yoke are protected".into());
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

fn protected_target(value: &Value, home: &Path, cwd: &Path, protected: &Path) -> bool {
    match value {
        Value::Object(fields) => fields.iter().any(|(key, value)| {
            (matches!(key.as_str(), "file_path" | "path" | "notebook_path")
                && value
                    .as_str()
                    .is_some_and(|path| is_protected(path, home, cwd, protected)))
                || protected_target(value, home, cwd, protected)
        }),
        Value::Array(values) => values
            .iter()
            .any(|value| protected_target(value, home, cwd, protected)),
        _ => false,
    }
}

fn is_protected(path: &str, home: &Path, cwd: &Path, protected: &Path) -> bool {
    let path = if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else {
        cwd.join(path)
    };
    normalized(&path).starts_with(normalized(&home.join(".yoke")))
        || resolved(&path).starts_with(protected)
}

fn normalized(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            component => result.push(component.as_os_str()),
        }
    }
    result
}

fn resolved(path: &Path) -> PathBuf {
    // Resolve each existing prefix, preserving symlink semantics before `..`.
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            component => {
                result.push(component.as_os_str());
                if let Ok(canonical) = result.canonicalize() {
                    result = canonical;
                }
            }
        }
    }
    result
}

/// Recognize documented, mutable agent markers; absence does not prove humanity.
pub fn agent_process_signal() -> Option<String> {
    process_signal(|key| std::env::var(key).ok())
}

fn process_signal(get: impl Fn(&str) -> Option<String>) -> Option<String> {
    for key in ["CLAUDE_CODE_CHILD_SESSION", "CLAUDECODE"] {
        if get(key).as_deref() == Some("1") {
            return Some(key.into());
        }
    }
    for key in ["CODEX_THREAD_ID", "CODEX_SESSION_ID"] {
        if get(key).is_some_and(|value| !value.is_empty()) {
            return Some(key.into());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn examines_actual_tool_input_not_prompt_or_read_content() {
        let home = tempfile::tempdir().unwrap();
        let mut event = AgentEvent {
            tool_name: Some("Bash".into()),
            tool_input: json!({"command":"yoke trust"}),
            ..Default::default()
        };
        for command in [
            "yoke trust",
            "/usr/local/bin/yoke plan approve demo",
            "yoke plan resume demo",
            "sqlite3 ~/.yoke/state.db '.tables'",
        ] {
            event.tool_input = json!({"command": command});
            assert!(evaluate(EventKind::PreToolUse, &event, home.path(), home.path()).is_some());
            assert!(evaluate(EventKind::PostToolUse, &event, home.path(), home.path()).is_none());
        }
        event.prompt = Some("yoke trust".into());
        event.tool_input = json!({"command":"yoke config"});
        assert!(evaluate(EventKind::PreToolUse, &event, home.path(), home.path()).is_none());
        event.tool_name = Some("Read".into());
        event.tool_input = json!({"file_path":home.path().join(".yoke/config.toml")});
        assert!(evaluate(EventKind::PreToolUse, &event, home.path(), home.path()).is_none());
    }

    #[test]
    fn protects_write_and_patch_targets_including_symlink_parents() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".yoke")).unwrap();
        std::os::unix::fs::symlink(home.path().join(".yoke"), home.path().join("alias")).unwrap();
        std::fs::create_dir(home.path().join("outside")).unwrap();
        std::os::unix::fs::symlink(
            home.path().join("outside"),
            home.path().join(".yoke/external"),
        )
        .unwrap();
        let mut event = AgentEvent {
            tool_name: Some("Write".into()),
            ..Default::default()
        };
        for path in [
            "~/.yoke/config.toml",
            "alias/new-file",
            ".yoke/../.yoke/new-file",
            ".yoke/external/config.toml",
        ] {
            event.tool_input = json!({"file_path":path});
            assert!(
                evaluate(EventKind::PreToolUse, &event, home.path(), home.path()).is_some(),
                "{path}"
            );
        }
        event.tool_input = json!({"file_path":".yoke-other/config.toml", "content":"state.db"});
        assert!(evaluate(EventKind::PreToolUse, &event, home.path(), home.path()).is_none());
        event.tool_name = Some("apply_patch".into());
        event.tool_input =
            json!({"command":"*** Begin Patch\n*** Update File: alias/config.toml\n*** End Patch"});
        assert!(evaluate(EventKind::PreToolUse, &event, home.path(), home.path()).is_some());
    }

    #[test]
    fn human_detection_uses_verified_markers_without_assuming_tty_proves_humanity() {
        assert_eq!(
            process_signal(|key| (key == "CODEX_THREAD_ID").then(|| "test-session".into())),
            Some("CODEX_THREAD_ID".into())
        );
        assert_eq!(
            process_signal(|key| (key == "CLAUDE_CODE_CHILD_SESSION").then(|| "1".into())),
            Some("CLAUDE_CODE_CHILD_SESSION".into())
        );
        assert_eq!(process_signal(|_| Some(String::new())), None);
        assert_eq!(process_signal(|_| None), None);
    }
}
