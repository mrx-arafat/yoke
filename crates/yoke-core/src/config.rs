use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{ensure, Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::event::EventKind;
use crate::store::Store;
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
    pub stop: StopConfig,
    pub plans: PlansConfig,
    pub memory: MemoryConfig,
    pub review: ReviewConfig,
    #[serde(rename = "gate")]
    pub gates: Vec<Gate>,
    #[serde(skip)]
    pub(crate) specified: BTreeSet<String>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Meta {
    pub name: String,
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
}
impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            digest: false,
            digest_agent: "claude".into(),
            inject_tokens: 1500,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ReviewConfig {
    pub reviewer: String,
}
impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            reviewer: "auto".into(),
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

impl Config {
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
            matches!(self.memory.digest_agent.as_str(), "claude" | "codex"),
            "memory.digest_agent must be claude or codex"
        );
        ensure!(
            matches!(self.review.reviewer.as_str(), "auto" | "claude" | "codex"),
            "review.reviewer must be auto, claude or codex"
        );
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
        merge!("stop.max_blocks", stop.max_blocks);
        merge!("plans.dir", plans.dir);
        merge!("memory.digest", memory.digest);
        merge!("memory.digest_agent", memory.digest_agent);
        merge!("memory.inject_tokens", memory.inject_tokens);
        merge!("review.reviewer", review.reviewer);
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

/// Global config path; executable settings always come from a trusted snapshot.
pub fn global_config_path() -> Option<PathBuf> {
    std::env::home_dir().map(|home| home.join(".yoke/config.toml"))
}

/// Discover the nearest disk config; this does not parse or execute it.
pub fn find_project_config(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .map(|dir| dir.join("yoke.toml"))
        .find(|path| path.is_file())
}

/// Compatibility entry point using trusted state in the current user's home.
pub fn load(cwd: &Path) -> Result<Config> {
    let home = std::env::home_dir().context("cannot determine home directory")?;
    let state_dir = home.join(".yoke");
    std::fs::create_dir_all(&state_dir)
        .with_context(|| format!("creating state directory {}", state_dir.display()))?;
    let store = Store::open(state_dir.join("state.db"))?;
    Ok(load_trusted(&store, cwd, &home)?.config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compatibility_load_initializes_fresh_home_without_trusting_disk_config() {
        // Re-execute this test to isolate HOME from every other test thread.
        if let Some(home) = std::env::var_os("YOKE_TEST_FRESH_HOME") {
            let home = PathBuf::from(home);
            assert!(!home.join(".yoke").exists());
            let config = load(&home).unwrap();
            assert_eq!(config.mode, Mode::Observe);
            assert!(config.gates.is_empty());
            assert!(home.join(".yoke/state.db").is_file());
            return;
        }
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("yoke.toml"), "mode='enforce'").unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "config::tests::compatibility_load_initializes_fresh_home_without_trusting_disk_config",
                "--nocapture",
            ])
            .env("HOME", home.path())
            .env("YOKE_TEST_FRESH_HOME", home.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
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
