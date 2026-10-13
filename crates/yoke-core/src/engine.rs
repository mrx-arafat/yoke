use std::process::Command;

use crate::config::{Config, Gate};
use crate::event::{AgentEvent, EventKind};

#[derive(Debug, Default)]
pub struct Outcome {
    /// False when a gate vetoed a blocking event.
    pub allow: bool,
    /// Names of the gates that ran, in order.
    pub ran: Vec<String>,
    /// Failure reasons from gates that exited non-zero.
    pub reasons: Vec<String>,
}

impl Outcome {
    pub fn blocked(&self) -> bool {
        !self.allow
    }
}

fn matches(gate: &Gate, ev: &AgentEvent) -> bool {
    if let Some(tool) = &gate.match_tool {
        if ev.tool_name.as_deref() != Some(tool.as_str()) {
            return false;
        }
    }
    if let Some(needle) = &gate.match_contains {
        if !ev.haystack().contains(needle.as_str()) {
            return false;
        }
    }
    true
}

/// Run one gate. Returns the failure reason, or `None` on success.
fn run_gate(gate: &Gate, kind: EventKind, ev: &AgentEvent) -> Option<String> {
    let shell = gate.shell.as_deref().unwrap_or("sh");
    let output = Command::new(shell)
        .arg("-c")
        .arg(&gate.run)
        .env("YOKE_EVENT", format!("{kind:?}"))
        .env("YOKE_GATE", &gate.name)
        .env(
            "YOKE_TOOL_NAME",
            ev.tool_name.as_deref().unwrap_or_default(),
        )
        .env("YOKE_TOOL_INPUT", ev.tool_input.to_string())
        .output();

    match output {
        Ok(o) if o.status.success() => None,
        Ok(o) => Some(gate.message.clone().unwrap_or_else(|| {
            let stderr = String::from_utf8_lossy(&o.stderr);
            match stderr.trim() {
                "" => format!(
                    "gate '{}' failed (exit {})",
                    gate.name,
                    o.status.code().unwrap_or(-1)
                ),
                msg => msg.to_string(),
            }
        })),
        Err(e) => Some(format!("gate '{}' could not run: {e}", gate.name)),
    }
}

/// Run every gate registered for `kind` whose matchers accept `ev`.
pub fn evaluate(cfg: &Config, kind: EventKind, ev: &AgentEvent) -> Outcome {
    let mut out = Outcome {
        allow: true,
        ..Default::default()
    };
    for gate in cfg.gates.iter().filter(|g| g.on == kind && matches(g, ev)) {
        out.ran.push(gate.name.clone());
        if let Some(reason) = run_gate(gate, kind, ev) {
            out.reasons.push(reason);
            if kind.is_blocking() {
                out.allow = false;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bash_event(command: &str) -> AgentEvent {
        AgentEvent {
            tool_name: Some("Bash".into()),
            tool_input: serde_json::json!({ "command": command }),
            ..Default::default()
        }
    }

    fn guard() -> Config {
        Config::parse(
            r#"
            [[gate]]
            name = "no-force-push"
            on = "pre_tool_use"
            match_tool = "Bash"
            match_contains = "push --force"
            run = "exit 1"
            message = "force push blocked"
            "#,
        )
        .unwrap()
    }

    #[test]
    fn failing_gate_vetoes_matching_pre_tool_use() {
        let out = evaluate(
            &guard(),
            EventKind::PreToolUse,
            &bash_event("git push --force"),
        );
        assert!(out.blocked());
        assert_eq!(out.reasons, vec!["force push blocked"]);
    }

    #[test]
    fn non_matching_event_is_allowed_without_running_gate() {
        let out = evaluate(&guard(), EventKind::PreToolUse, &bash_event("git push"));
        assert!(!out.blocked());
        assert!(out.ran.is_empty());
    }

    #[test]
    fn failing_gate_on_observing_event_does_not_block() {
        let cfg = Config::parse(
            r#"
            [[gate]]
            name = "lint"
            on = "post_tool_use"
            run = "exit 3"
            "#,
        )
        .unwrap();
        let out = evaluate(&cfg, EventKind::PostToolUse, &bash_event("ls"));
        assert!(!out.blocked());
        assert_eq!(out.reasons, vec!["gate 'lint' failed (exit 3)"]);
    }
}
