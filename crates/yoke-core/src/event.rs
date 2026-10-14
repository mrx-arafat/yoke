use serde::{Deserialize, Serialize};

/// Lifecycle points at which Yoke can run gates.
///
/// Names line up with the publicly documented coding-agent hook events so Yoke
/// can be registered as a hook handler. Both `snake_case` and `PascalCase`
/// spellings are accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    #[serde(alias = "PreToolUse")]
    PreToolUse,
    #[serde(alias = "PostToolUse")]
    PostToolUse,
    /// Claude Code only: a tool that started executing failed.
    #[serde(alias = "PostToolUseFailure")]
    PostToolUseFailure,
    #[serde(alias = "UserPromptSubmit")]
    UserPromptSubmit,
    #[serde(alias = "Stop")]
    Stop,
    #[serde(alias = "SessionStart")]
    SessionStart,
    #[serde(alias = "SessionEnd")]
    SessionEnd,
    #[serde(alias = "Notification")]
    Notification,
    #[serde(alias = "PreCompact")]
    PreCompact,
}

impl EventKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "pre_tool_use" | "PreToolUse" => Some(Self::PreToolUse),
            "post_tool_use" | "PostToolUse" => Some(Self::PostToolUse),
            "post_tool_use_failure" | "PostToolUseFailure" => Some(Self::PostToolUseFailure),
            "user_prompt_submit" | "UserPromptSubmit" => Some(Self::UserPromptSubmit),
            "stop" | "Stop" => Some(Self::Stop),
            "session_start" | "SessionStart" => Some(Self::SessionStart),
            "session_end" | "SessionEnd" => Some(Self::SessionEnd),
            "notification" | "Notification" => Some(Self::Notification),
            "pre_compact" | "PreCompact" => Some(Self::PreCompact),
            _ => None,
        }
    }

    /// Blocking events can veto the pending action; the rest only observe.
    pub fn is_blocking(self) -> bool {
        matches!(self, Self::PreToolUse | Self::UserPromptSubmit | Self::Stop)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::PreToolUse => "pre_tool_use",
            Self::PostToolUse => "post_tool_use",
            Self::PostToolUseFailure => "post_tool_use_failure",
            Self::UserPromptSubmit => "user_prompt_submit",
            Self::Stop => "stop",
            Self::SessionStart => "session_start",
            Self::SessionEnd => "session_end",
            Self::Notification => "notification",
            Self::PreCompact => "pre_compact",
        }
    }
}

/// Agent contracts verified against each vendor's public hook reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
}

impl Agent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    pub fn supports(self, kind: EventKind) -> bool {
        self != Self::Codex
            || !matches!(
                kind,
                EventKind::Notification | EventKind::PostToolUseFailure
            )
    }
}

/// A hook payload the coding agent sends on stdin.
#[derive(Debug, Default, Deserialize)]
pub struct AgentEvent {
    #[serde(default)]
    pub transcript_path: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub session_id: String,
    #[serde(default, alias = "hook_event_name")]
    pub event_name: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_input: serde_json::Value,
    #[serde(default)]
    pub prompt: Option<String>,
}

impl AgentEvent {
    /// Flattened text that `match_contains` searches.
    pub fn haystack(&self) -> String {
        let mut s = String::new();
        if let Some(t) = &self.tool_name {
            s.push_str(t);
            s.push('\n');
        }
        if let Some(p) = &self.prompt {
            s.push_str(p);
            s.push('\n');
        }
        if !self.tool_input.is_null() {
            s.push_str(&self.tool_input.to_string());
        }
        s
    }
}
