//! Minimal hook output from the public Claude and Codex hook contracts.

use anyhow::{ensure, Result};
use serde_json::{json, Value};

use crate::event::{Agent, EventKind};

/// Context is restricted to the shared, documented startup/prompt contract.
pub fn context(agent: Agent, kind: EventKind, text: &str) -> Option<Value> {
    if text.is_empty() || !agent.supports(kind) {
        return None;
    }
    let event = match kind {
        EventKind::SessionStart => "SessionStart",
        EventKind::UserPromptSubmit => "UserPromptSubmit",
        _ => return None,
    };
    Some(json!({"hookSpecificOutput":{"hookEventName":event,"additionalContext":text}}))
}

/// Blocking output is absent on advisory events.
pub fn deny(agent: Agent, kind: EventKind, reason: &str) -> Option<Value> {
    if !agent.supports(kind) || !kind.is_blocking() {
        return None;
    }
    Some(match kind {
        EventKind::PreToolUse => json!({"hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason
        }}),
        _ => json!({"decision": "block", "reason": reason}),
    })
}

/// Validate the narrower Codex rewrite contract before emitting any output.
pub fn rewrite(agent: Agent, tool: Option<&str>, input: &Value) -> Result<Value> {
    ensure!(input.is_object(), "rewritten tool input must be an object");
    if agent == Agent::Codex && matches!(tool, Some("Bash" | "apply_patch")) {
        ensure!(
            input.get("command").and_then(Value::as_str).is_some(),
            "Codex Bash/apply_patch rewrites require a string command"
        );
    }
    let mut fields = json!({"hookEventName": "PreToolUse", "updatedInput": input});
    // Claude accepts updatedInput alone, preserving its normal permission checks.
    // Codex requires allow together with updatedInput.
    if agent == Agent::Codex {
        fields["permissionDecision"] = json!("allow");
    }
    Ok(json!({"hookSpecificOutput": fields}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denies_only_blocking_events_with_exact_public_fields() {
        for agent in [Agent::Claude, Agent::Codex] {
            assert_eq!(
                deny(agent, EventKind::Stop, "check failed"),
                Some(json!({"decision":"block","reason":"check failed"}))
            );
            assert_eq!(
                deny(agent, EventKind::PreToolUse, "check failed"),
                Some(json!({"hookSpecificOutput":{"hookEventName":"PreToolUse",
                    "permissionDecision":"deny","permissionDecisionReason":"check failed"}}))
            );
            assert_eq!(deny(agent, EventKind::SessionEnd, "failed"), None);
        }
        assert!(!Agent::Codex.supports(EventKind::Notification));
        assert!(Agent::Codex.supports(EventKind::SessionEnd));
    }

    #[test]
    fn rewrites_preserve_claude_permissions_and_validate_codex_command() {
        let input = json!({"command":"echo safe","timeout":1000});
        assert_eq!(
            rewrite(Agent::Claude, Some("Bash"), &input).unwrap(),
            json!({"hookSpecificOutput":{"hookEventName":"PreToolUse","updatedInput":input}})
        );
        assert_eq!(
            rewrite(Agent::Codex, Some("Bash"), &input).unwrap(),
            json!({"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","updatedInput":input}})
        );
        assert!(rewrite(Agent::Codex, Some("apply_patch"), &json!({"command":3})).is_err());
        assert!(rewrite(Agent::Codex, Some("mcp__x__y"), &json!({"query":"a"})).is_ok());
    }
}
