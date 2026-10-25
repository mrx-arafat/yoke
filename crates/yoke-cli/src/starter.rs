//! The starter `yoke.toml` that `yoke init` writes.

const TEMPLATE: &str = include_str!("../starter.toml");

/// The template with `[meta] name` set to the project directory instead of a placeholder.
pub fn render(project_name: &str) -> String {
    let quoted = toml_edit::Value::from(project_name).to_string();
    TEMPLATE.replacen("\"my-project\"", quoted.trim(), 1)
}

#[cfg(test)]
mod tests {
    use super::render;
    use serde_json::json;
    use yoke_core::{
        config::Config,
        engine,
        event::{AgentEvent, EventKind},
        preset,
    };

    #[test]
    fn starter_force_push_matches_the_preset_across_shell_tools() {
        let starter = Config::parse(&render("project")).unwrap();
        let pack = Config::parse(
            &preset::render("git-safety", &preset::PresetOptions::default()).unwrap(),
        )
        .unwrap();
        let preset = pack
            .gates
            .iter()
            .find(|gate| gate.name == "preset-git-force-push")
            .unwrap();
        assert_eq!(starter.gates.len(), 1);
        assert_eq!(starter.gates[0].match_pattern, preset.match_pattern);
        assert_eq!(starter.gates[0].match_tool, preset.match_tool);
        assert!(starter.gates[0].match_contains.is_none());
        for tool in ["Bash", "exec_command", "shell_command", "shell"] {
            let field = if matches!(tool, "exec_command" | "shell") {
                "cmd"
            } else {
                "command"
            };
            for (command, blocked) in [
                ("git push --force origin main", true),
                ("git -C repo push -f origin main", true),
                (
                    "git push --force-with-lease=refs/heads/main origin main",
                    true,
                ),
                ("git push origin +main", true),
                ("git push origin main", false),
                ("git status --short", false),
                ("echo push --force", false),
                ("git push --forceful", false),
            ] {
                let event = AgentEvent {
                    tool_name: Some(tool.into()),
                    tool_input: json!({field:command}),
                    ..Default::default()
                };
                assert_eq!(
                    engine::evaluate(&starter, EventKind::PreToolUse, &event).blocked(),
                    blocked,
                    "{tool}: {command}"
                );
            }
        }
        let event = AgentEvent {
            tool_name: Some("Edit".into()),
            tool_input: json!({"command":"git push --force"}),
            ..Default::default()
        };
        assert!(!engine::evaluate(&starter, EventKind::PreToolUse, &event).blocked());
    }

    fn parsed_name(text: &str) -> String {
        let doc: toml_edit::DocumentMut = text.parse().unwrap();
        doc["meta"]["name"].as_str().unwrap().to_owned()
    }

    #[test]
    fn the_starter_is_named_after_the_directory_and_stays_valid_toml() {
        assert_eq!(parsed_name(&render("billing-api")), "billing-api");
        // Quotes and backslashes in a directory name must not break the file.
        assert_eq!(parsed_name(&render("a\"b\\c")), "a\"b\\c");
        let text = render("billing-api");
        assert!(!text.contains("my-project"), "{text}");
        assert!(text.contains("# [memory]"), "{text}");
    }
}
