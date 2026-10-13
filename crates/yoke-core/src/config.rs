use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::event::EventKind;

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub meta: Meta,
    #[serde(default, rename = "gate")]
    pub gates: Vec<Gate>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Meta {
    #[serde(default)]
    pub name: String,
}

/// A check that runs on an agent event. On a blocking event, a non-zero exit
/// from `run` vetoes the pending action.
#[derive(Debug, Clone, Deserialize)]
pub struct Gate {
    pub name: String,
    pub on: EventKind,
    #[serde(default)]
    pub match_tool: Option<String>,
    #[serde(default)]
    pub match_contains: Option<String>,
    pub run: String,
    #[serde(default)]
    pub message: Option<String>,
    /// Shell used as `<shell> -c <run>`. Defaults to `sh`.
    #[serde(default)]
    pub shell: Option<String>,
}

impl Config {
    pub fn parse(s: &str) -> Result<Self> {
        toml::from_str(s).context("parsing Yoke config (TOML)")
    }

    pub fn load_file(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        Self::parse(&raw).with_context(|| format!("in {}", path.display()))
    }

    /// Merge `other` into `self`. A gate in `other` replaces one with the same
    /// name, so project config overrides global config.
    pub fn merge(&mut self, other: Config) {
        if !other.meta.name.is_empty() {
            self.meta.name = other.meta.name;
        }
        for gate in other.gates {
            match self.gates.iter_mut().find(|g| g.name == gate.name) {
                Some(slot) => *slot = gate,
                None => self.gates.push(gate),
            }
        }
    }
}

/// Global config: `~/.yoke/config.toml`.
pub fn global_config_path() -> Option<PathBuf> {
    std::env::home_dir().map(|home| home.join(".yoke").join("config.toml"))
}

/// Nearest `yoke.toml` at or above `start`.
pub fn find_project_config(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .map(|dir| dir.join("yoke.toml"))
        .find(|candidate| candidate.is_file())
}

/// Global config merged with the project config (project wins).
pub fn load(cwd: &Path) -> Result<Config> {
    let mut cfg = Config::default();
    if let Some(global) = global_config_path().filter(|p| p.is_file()) {
        cfg.merge(Config::load_file(&global)?);
    }
    if let Some(project) = find_project_config(cwd) {
        cfg.merge(Config::load_file(&project)?);
    }
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_gate_overrides_global_gate_with_same_name() {
        let mut cfg = Config::parse(
            r#"
            [[gate]]
            name = "fmt"
            on = "post_tool_use"
            run = "global"
            "#,
        )
        .unwrap();
        cfg.merge(
            Config::parse(
                r#"
                [[gate]]
                name = "fmt"
                on = "PostToolUse"
                run = "project"
                "#,
            )
            .unwrap(),
        );
        assert_eq!(cfg.gates.len(), 1);
        assert_eq!(cfg.gates[0].run, "project");
    }
}
