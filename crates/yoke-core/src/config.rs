use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{ensure, Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::event::EventKind;
pub use crate::trust::{load_trusted, SnapshotStatus};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Observe,
    Enforce,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::Enforce => "enforce",
        }
    }
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub meta: Meta,
    pub mode: Mode,
    pub log: LogConfig,
    pub update: UpdateConfig,
    pub usage: UsageConfig,
    pub stop: StopConfig,
    pub plans: PlansConfig,
    pub memory: MemoryConfig,
    pub review: ReviewConfig,
    #[serde(rename = "gate")]
    pub gates: Vec<Gate>,
    #[serde(rename = "gates")]
    pub gate_environment: GateEnvironment,
    #[serde(skip)]
    pub(crate) specified: BTreeSet<String>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Meta {
    pub name: String,
}

/// Additional inherited environment names explicitly approved by a human.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct GateEnvironment {
    pub pass_env: Vec<String>,
}

pub(crate) fn environment_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.bytes().enumerate().all(|(index, byte)| {
            byte == b'_' || byte.is_ascii_alphabetic() || (index > 0 && byte.is_ascii_digit())
        })
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct UpdateConfig {
    pub check: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct UsageConfig {
    pub warn_percent: u8,
}

impl Default for UsageConfig {
    fn default() -> Self {
        Self { warn_percent: 85 }
    }
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self { check: true }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct LogConfig {
    pub retention_days: u32,
    pub payload_cap: usize,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            retention_days: 90,
            payload_cap: 4096,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct StopConfig {
    pub max_blocks: u32,
}
impl Default for StopConfig {
    fn default() -> Self {
        Self { max_blocks: 5 }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct PlansConfig {
    pub dir: String,
}
impl Default for PlansConfig {
    fn default() -> Self {
        Self {
            dir: ".yoke/plans".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct MemoryConfig {
    pub digest: bool,
    pub digest_agent: String,
    pub inject_tokens: usize,
    pub digest_models: AgentModels,
}
impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            digest: false,
            digest_agent: "claude".into(),
            inject_tokens: 1500,
            digest_models: AgentModels::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ReviewConfig {
    pub reviewer: String,
    pub models: AgentModels,
}
impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            reviewer: "auto".into(),
            models: AgentModels::default(),
        }
    }
}

/// Model passed to an agent CLI that Yoke launches; unset keeps that CLI's own default.
/// Per agent, so an automatically selected reviewer still gets its own model.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentModels {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex: Option<String>,
}

impl AgentModels {
    pub fn get(&self, agent: &str) -> Option<&str> {
        match agent {
            "claude" => self.claude.as_deref(),
            "codex" => self.codex.as_deref(),
            _ => None,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GateAction {
    #[default]
    Check,
    Deny,
    Rewrite,
}

/// A validated check, denial, or input rewrite on an agent event.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Gate {
    pub name: String,
    #[serde(default)]
    pub pass_env: Vec<String>,
    pub on: EventKind,
    #[serde(default)]
    pub action: GateAction,
    #[serde(default)]
    pub match_tool: Option<String>,
    #[serde(default)]
    pub match_contains: Option<String>,
    #[serde(default)]
    pub match_pattern: Option<String>,
    #[serde(default)]
    pub run: String,
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub shell: Option<String>,
    #[serde(default = "default_rewrite_field")]
    pub rewrite_field: String,
    #[serde(default)]
    pub rewrite_pattern: Option<String>,
    #[serde(default)]
    pub rewrite_to: Option<String>,
    #[serde(default)]
    pub require_command: Option<String>,
}

fn default_timeout() -> u64 {
    30
}
fn default_rewrite_field() -> String {
    "command".into()
}

#[derive(Debug, Serialize)]
pub struct LoadedConfig {
    pub config: Config,
    pub origins: BTreeMap<String, String>,
    pub project_root: PathBuf,
    pub snapshots: Vec<SnapshotStatus>,
    pub knowledge: Option<String>,
}

/// How long one blocking hook may run and what its gates may use of that, in seconds, or
/// `None` for events that cannot block. The ceilings stay under the agents' default hook
/// timeouts, after which an agent lets the action through: Claude Code cancels command hooks
/// after 600 s, and UserPromptSubmit hooks after 30 s
/// (https://code.claude.com/docs/en/hooks.md), and Codex after 600 s
/// (https://developers.openai.com/codex/hooks). The margin covers Yoke's own work.
pub fn hook_budget(event: EventKind) -> Option<(u64, u64)> {
    match event {
        EventKind::UserPromptSubmit => Some((25, 5)),
        EventKind::PreToolUse | EventKind::Stop => Some((540, 10)),
        _ => None,
    }
}

impl Config {
    /// Longest the check gates on `event` can run one after another, in seconds.
    pub fn gate_seconds(&self, event: EventKind) -> u64 {
        self.gates
            .iter()
            .filter(|gate| gate.on == event && gate.action == GateAction::Check)
            .fold(0, |total, gate| total.saturating_add(gate.timeout))
    }

    /// Refuse, before trust, check gates whose timeouts could keep a blocking hook running past
    /// what its agent allows. Persisted snapshots are not re-checked, so trusted policy still loads.
    pub fn check_hook_budgets(&self) -> Result<()> {
        for event in [
            EventKind::PreToolUse,
            EventKind::UserPromptSubmit,
            EventKind::Stop,
        ] {
            if let Some((ceiling, margin)) = hook_budget(event) {
                let total = self.gate_seconds(event);
                ensure!(
                    total.saturating_add(margin) <= ceiling,
                    "check gates on {} may run for {total} s in total, but that hook must answer within {ceiling} s of starting; lower their timeout values to {} s or less in total",
                    event.as_str(),
                    ceiling - margin
                );
            }
        }
        Ok(())
    }

    /// Parse one layer, retaining which settings were explicitly supplied.
    pub fn parse(s: &str) -> Result<Self> {
        let value: toml::Value = toml::from_str(s).context("parsing Yoke config (TOML)")?;
        let mut config: Self = value
            .clone()
            .try_into()
            .context("reading Yoke config fields")?;
        collect_settings("", &value, &mut config.specified);
        config.validate()?;
        Ok(config)
    }

    /// Validate before accepting a snapshot and before using persisted data.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=100).contains(&self.usage.warn_percent),
            "usage.warn_percent must be between 1 and 100"
        );
        for names in std::iter::once(&self.gate_environment.pass_env)
            .chain(self.gates.iter().map(|gate| &gate.pass_env))
        {
            ensure!(names.len() <= 64 && names.iter().all(|name| environment_name(name)), "pass_env requires at most 64 exact environment names (letters, digits and underscores, not starting with a digit); replace globs or invalid names and run yoke trust");
        }
        ensure!(
            matches!(self.memory.digest_agent.as_str(), "claude" | "codex"),
            "memory.digest_agent must be claude or codex"
        );
        ensure!(
            matches!(self.review.reviewer.as_str(), "auto" | "claude" | "codex"),
            "review.reviewer must be auto, claude or codex"
        );
        for (key, models) in [
            ("review.models", &self.review.models),
            ("memory.digest_models", &self.memory.digest_models),
        ] {
            for (agent, model) in [("claude", &models.claude), ("codex", &models.codex)] {
                // Availability cannot be checked offline; the value becomes one CLI argument.
                ensure!(
                    model.as_ref().is_none_or(|model| {
                        (1..=128).contains(&model.len())
                            && !model.starts_with('-')
                            && model
                                .bytes()
                                .all(|byte| byte.is_ascii_alphanumeric() || b"._:/@[]-".contains(&byte))
                    }),
                    "{key}.{agent} must be 1 to 128 letters, digits or . _ : / @ [ ] -, not starting with -"
                );
            }
        }
        let plans = Path::new(&self.plans.dir);
        ensure!(
            !self.plans.dir.trim().is_empty()
                && !plans.is_absolute()
                && !plans
                    .components()
                    .any(|part| matches!(part, std::path::Component::ParentDir)),
            "plans.dir must stay relative to the project root"
        );
        let mut names = BTreeSet::new();
        for gate in &self.gates {
            ensure!(!gate.name.trim().is_empty(), "gate name must not be empty");
            ensure!(
                names.insert(&gate.name),
                "duplicate gate name: {}",
                gate.name
            );
            ensure!(
                gate.timeout > 0,
                "gate {} timeout must be at least 1 second",
                gate.name
            );
            if let Some(pattern) = &gate.match_tool {
                Regex::new(&format!(r"\A(?:{pattern})\z"))
                    .with_context(|| format!("gate {} match_tool regex", gate.name))?;
            }
            for (key, pattern) in [
                ("match_pattern", &gate.match_pattern),
                ("rewrite_pattern", &gate.rewrite_pattern),
            ] {
                if let Some(pattern) = pattern {
                    Regex::new(pattern)
                        .with_context(|| format!("gate {} {key} regex", gate.name))?;
                }
            }
            match gate.action {
                GateAction::Check => {
                    ensure!(
                        !gate.run.trim().is_empty(),
                        "gate {} check requires run",
                        gate.name
                    );
                    ensure!(
                        gate.shell
                            .as_ref()
                            .is_none_or(|shell| !shell.trim().is_empty()),
                        "gate {} shell must not be empty",
                        gate.name
                    );
                }
                GateAction::Deny | GateAction::Rewrite => {
                    ensure!(
                        gate.run.is_empty() && gate.shell.is_none(),
                        "gate {} run/shell are only valid for check",
                        gate.name
                    );
                }
            }
            if gate.action == GateAction::Rewrite {
                if let Some(command) = &gate.require_command {
                    ensure!(
                        !command.is_empty()
                            && command != "."
                            && command != ".."
                            && command
                                .bytes()
                                .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte)),
                        "gate {} require_command must be an executable basename",
                        gate.name
                    );
                }
                ensure!(
                    gate.on == EventKind::PreToolUse,
                    "gate {} rewrite requires pre_tool_use",
                    gate.name
                );
                ensure!(
                    gate.rewrite_pattern.is_some() && gate.rewrite_to.is_some(),
                    "gate {} rewrite requires rewrite_pattern and rewrite_to",
                    gate.name
                );
                ensure!(
                    !gate.rewrite_field.trim().is_empty(),
                    "gate {} rewrite_field must not be empty",
                    gate.name
                );
            } else {
                ensure!(
                    gate.require_command.is_none(),
                    "gate {} require_command requires action rewrite",
                    gate.name
                );
                ensure!(
                    gate.rewrite_pattern.is_none()
                        && gate.rewrite_to.is_none()
                        && gate.rewrite_field == "command",
                    "gate {} rewrite fields require action rewrite",
                    gate.name
                );
            }
        }
        Ok(())
    }

    /// Override only explicitly supplied settings; gates replace by name in place.
    pub fn merge(&mut self, other: Config) {
        macro_rules! merge {
            ($key:literal, $($field:ident).+) => {
                if other.specified.contains($key) { self.$($field).+ = other.$($field).+.clone(); }
            };
        }
        merge!("meta.name", meta.name);
        merge!("mode", mode);
        merge!("log.retention_days", log.retention_days);
        merge!("log.payload_cap", log.payload_cap);
        merge!("update.check", update.check);
        merge!("usage.warn_percent", usage.warn_percent);
        merge!("gates.pass_env", gate_environment.pass_env);
        merge!("stop.max_blocks", stop.max_blocks);
        merge!("plans.dir", plans.dir);
        merge!("memory.digest", memory.digest);
        merge!("memory.digest_agent", memory.digest_agent);
        merge!("memory.inject_tokens", memory.inject_tokens);
        merge!("review.reviewer", review.reviewer);
        merge!("review.models.claude", review.models.claude);
        merge!("review.models.codex", review.models.codex);
        merge!("memory.digest_models.claude", memory.digest_models.claude);
        merge!("memory.digest_models.codex", memory.digest_models.codex);
        self.specified.extend(other.specified);
        for gate in other.gates {
            match self
                .gates
                .iter_mut()
                .find(|existing| existing.name == gate.name)
            {
                Some(existing) => *existing = gate,
                None => self.gates.push(gate),
            }
        }
    }

    pub(crate) fn merge_from(
        &mut self,
        other: Config,
        file: &Path,
        origins: &mut BTreeMap<String, String>,
    ) {
        let source = file.display().to_string();
        for key in &other.specified {
            origins.insert(key.clone(), source.clone());
        }
        for gate in &other.gates {
            origins.insert(format!("gate.{}", gate.name), source.clone());
        }
        self.merge(other);
    }

    pub(crate) fn default_origins() -> BTreeMap<String, String> {
        [
            "meta.name",
            "mode",
            "log.retention_days",
            "log.payload_cap",
            "update.check",
            "usage.warn_percent",
            "gates.pass_env",
            "stop.max_blocks",
            "plans.dir",
            "memory.digest",
            "memory.digest_agent",
            "memory.inject_tokens",
            "review.reviewer",
        ]
        .into_iter()
        .map(|key| (key.into(), "default".into()))
        .collect()
    }
}

fn collect_settings(prefix: &str, value: &toml::Value, keys: &mut BTreeSet<String>) {
    if let Some(table) = value.as_table() {
        for (key, value) in table {
            if prefix.is_empty() && key == "gate" {
                continue;
            }
            let name = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            if value.is_table() {
                collect_settings(&name, value, keys);
            } else {
                keys.insert(name);
            }
        }
    }
}

/// The home Yoke keeps its state under, from `HOME` only. An unset, empty or relative value
/// fails instead of falling back to the account's home, which an isolated run must never use.
pub fn home() -> Result<PathBuf> {
    home_from(std::env::var_os("HOME"))
}

pub fn home_from(value: Option<std::ffi::OsString>) -> Result<PathBuf> {
    let home = PathBuf::from(value.context("HOME is not set; Yoke needs it to find its state")?);
    ensure!(
        home.is_absolute(),
        "HOME must be an absolute path; Yoke never falls back to another home directory"
    );
    Ok(home)
}

/// Discover the nearest disk config; this does not parse or execute it.
pub fn find_project_config(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .map(|dir| dir.join("yoke.toml"))
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_environment_names_are_exact_and_global_overrides_merge_explicitly() {
        let mut config = Config::parse("[gates]\npass_env=['DATABASE_URL']").unwrap();
        config.merge(Config::parse("[meta]\nname='project'").unwrap());
        assert_eq!(config.gate_environment.pass_env, ["DATABASE_URL"]);
        config.merge(Config::parse("[gates]\npass_env=[]").unwrap());
        assert!(config.gate_environment.pass_env.is_empty());
        for name in ["*", "DATABASE_*", "1NAME", "NAME=VALUE", ""] {
            let value = serde_json::to_string(name).unwrap();
            assert!(Config::parse(&format!("[gates]\npass_env=[{value}]")).is_err());
            assert!(Config::parse(&format!(
                "[[gate]]\nname='env'\non='stop'\nrun='true'\npass_env=[{value}]"
            ))
            .is_err());
        }
    }

    #[test]
    fn update_checks_default_on_and_only_explicit_layers_override_them() {
        let mut config = Config::default();
        assert!(config.update.check);
        config.merge(Config::parse("[update]\ncheck = false").unwrap());
        assert!(!config.update.check);
        config.merge(Config::parse("mode = 'observe'").unwrap());
        assert!(!config.update.check);
        config.merge(Config::parse("[update]\ncheck = true").unwrap());
        assert!(config.update.check);
    }

    #[test]
    fn quota_warning_threshold_is_validated_and_inherits_only_explicit_values() {
        let mut config = Config::default();
        assert_eq!(config.usage.warn_percent, 85);
        config.merge(Config::parse("[usage]\nwarn_percent=70").unwrap());
        config.merge(Config::parse("mode='observe'").unwrap());
        assert_eq!(config.usage.warn_percent, 70);
        for value in ["0", "101", "-1", "85.5"] {
            assert!(Config::parse(&format!("[usage]\nwarn_percent={value}")).is_err());
        }
    }

    #[test]
    fn home_requires_an_absolute_home_and_never_falls_back() {
        for missing in [None, Some(""), Some("relative/home")] {
            assert!(home_from(missing.map(Into::into)).is_err(), "{missing:?}");
        }
        assert_eq!(
            home_from(Some("/isolated/home".into())).unwrap(),
            PathBuf::from("/isolated/home")
        );
    }

    #[test]
    fn rejects_invalid_regex_and_action_combinations_before_trust() {
        for fields in [
            "run='true'\nmatch_tool='['",
            "run='true'\nmatch_pattern='['",
            "run='true'\ntimeout=0",
            "action='rewrite'\nrewrite_pattern='x'\nrewrite_to='y'",
            "action='deny'\nrun='true'",
            "run='true'\nrewrite_pattern='x'",
            "run='true'\nunknown_matcher='x'",
            "action='check'",
            "action='deny'\nrequire_command='wrapper'",
        ] {
            let config = format!("[[gate]]\nname='bad'\non='stop'\n{fields}");
            assert!(
                Config::parse(&config).is_err(),
                "accepted invalid config: {config}"
            );
        }
        for fields in [
            "rewrite_pattern='['\nrewrite_to='x'",
            "rewrite_pattern='x'",
            "rewrite_to='x'",
        ] {
            assert!(Config::parse(&format!(
                "[[gate]]\nname='bad'\non='pre_tool_use'\naction='rewrite'\n{fields}"
            ))
            .is_err());
        }
        for command in ["", "../wrapper", "path/wrapper", "with space", ".", ".."] {
            let config = format!("[[gate]]\nname='rewrite'\non='pre_tool_use'\naction='rewrite'\nrewrite_pattern='x'\nrewrite_to='y'\nrequire_command='{command}'");
            assert!(Config::parse(&config).is_err());
        }
        assert!(Config::parse("[[gate]]\nname='rewrite'\non='pre_tool_use'\naction='rewrite'\nrewrite_pattern='x'\nrewrite_to='y'\nrequire_command='wrapper-cli'").is_ok());
    }

    #[test]
    fn partial_project_layer_preserves_global_fields_and_allows_explicit_defaults() {
        let mut config = Config::parse("mode='enforce'\n[log]\nretention_days=12\npayload_cap=512\n[memory]\ndigest=true\ndigest_agent='codex'").unwrap();
        config.merge(Config::parse("[log]\npayload_cap=4096\n[memory]\ndigest=false").unwrap());
        assert_eq!(config.mode, Mode::Enforce);
        assert_eq!(config.log.retention_days, 12);
        assert_eq!(config.log.payload_cap, 4096);
        assert!(!config.memory.digest);
        assert_eq!(config.memory.digest_agent, "codex");
        config.merge(Config::parse("mode='observe'\n[log]").unwrap());
        assert_eq!(config.mode, Mode::Observe);
        assert_eq!(config.log.retention_days, 12);
    }

    #[test]
    fn agent_models_validate_before_trust_and_merge_per_key() {
        for bad in [
            "[review.models]\nclaude='--dangerously-skip-permissions'",
            "[review.models]\ncodex=''",
            "[review.models]\nclaude='opus; rm -rf /'",
            "[review.models]\nunknown_agent='pro'",
            "[memory.digest_models]\nclaude='has space'",
        ] {
            assert!(Config::parse(bad).is_err(), "accepted {bad}");
        }
        let mut config = Config::parse("[review.models]\nclaude='opus'\ncodex='gpt-5.1-codex'\n[memory.digest_models]\nclaude='claude-haiku-4-5@20251001'").unwrap();
        config.merge(Config::parse("[review.models]\ncodex='o3'").unwrap());
        assert_eq!(config.review.models.get("claude"), Some("opus"));
        assert_eq!(config.review.models.get("codex"), Some("o3"));
        assert_eq!(
            config.memory.digest_models.get("claude"),
            Some("claude-haiku-4-5@20251001")
        );
        assert_eq!(config.memory.digest_models.get("codex"), None);
    }

    #[test]
    fn project_gate_replaces_in_place_and_valid_actions_have_defaults() {
        let mut config = Config::parse("[[gate]]\nname='first'\non='stop'\nrun='global'\n[[gate]]\nname='second'\non='stop'\naction='deny'").unwrap();
        config.merge(Config::parse("[[gate]]\nname='first'\non='PreToolUse'\naction='rewrite'\nrewrite_pattern='x'\nrewrite_to=''\ntimeout=1").unwrap());
        assert_eq!(
            config
                .gates
                .iter()
                .map(|gate| gate.name.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
        assert_eq!(config.gates[0].action, GateAction::Rewrite);
        assert_eq!(config.gates[0].rewrite_field, "command");
        assert_eq!(config.gates[0].timeout, 1);
        assert_eq!(config.gates[1].timeout, 30);
        assert!(Config::parse(
            "[[gate]]\nname='same'\non='stop'\nrun='x'\n[[gate]]\nname='same'\non='stop'\nrun='y'"
        )
        .is_err());
    }
}
