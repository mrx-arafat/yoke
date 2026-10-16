//! Read-only inventory of Claude Code and Codex extension definitions, with drift findings.
//!
//! Locations and precedence are cited in docs/decisions/0003-extension-inventory.md.
//! Nothing found here is executed or started and reads are size-capped. Shape-based
//! redaction cannot recognise every secret, so values are never shown: commands are reduced
//! to their program and argument count, URLs to scheme and host, and env and headers to key
//! names. Displayed text still passes through `redact`.

mod read;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Serialize, Serializer};
use serde_json::Value;

use crate::preset;
use crate::redact::redact;
use read::{Reader, CONFIG_CAP, HEAD_CAP};

/// Scheme and `host[:port]`, past any userinfo; the host must be a plain name or IP literal.
static ORIGIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([A-Za-z][A-Za-z0-9+.-]*://)(?:[^/?#\\\s]*@)?((?:[\p{L}\p{N}.-]+|\[[0-9A-Fa-f:.]+\])(?::[0-9]+)?)(?:[/?#\\\s]|$)").unwrap()
});

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Owner {
    Claude,
    Codex,
    Yoke,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Skill,
    Command,
    Agent,
    Plugin,
    Mcp,
    Hook,
    Memory,
    Rule,
    Preset,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Managed,
    Local,
    Project,
    User,
    System,
}

#[derive(Debug, Serialize)]
pub struct Item {
    pub agent: Owner,
    pub kind: Kind,
    pub name: String,
    /// Original identity for conflict comparisons; redaction must not merge names.
    #[serde(skip)]
    identity: String,
    pub scope: Scope,
    #[serde(serialize_with = "serialize_path")]
    pub origin: PathBuf,
    pub managed: bool,
    /// State, program name with argument count, or URL scheme and host; never argument text,
    /// URL paths, or environment and header values.
    pub detail: String,
    /// Environment variable and header names only.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<String>,
    /// Unredacted hook or MCP command for existence and ownership checks; never displayed.
    #[serde(skip)]
    pub command: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DriftKind {
    Shadowed,
    Stale,
    Dangling,
}

#[derive(Debug, Serialize)]
pub struct Drift {
    pub kind: DriftKind,
    pub agent: Owner,
    pub name: String,
    pub detail: String,
    #[serde(serialize_with = "serialize_paths")]
    pub origins: Vec<PathBuf>,
}

impl Drift {
    /// Whether this finding makes `yoke extensions` exit 1. A shadowed name is often a
    /// deliberate override, and the report already states which definition wins, so it is
    /// shown without failing; a stale managed hook or a missing program is real breakage.
    pub fn fails_check(&self) -> bool {
        self.kind != DriftKind::Shadowed
    }
}

/// Start of the note for a link that leaves every scanned root; the target follows.
pub(crate) const OUTSIDE_LINK: &str = "symlink outside scanned roots: ";

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Note {
    #[serde(serialize_with = "serialize_path")]
    pub path: PathBuf,
    #[serde(serialize_with = "serialize_text")]
    pub note: String,
}

impl Note {
    /// Links to a skill manager's store are normal and cannot be acted on, so text output
    /// counts them instead of listing each one.
    pub fn is_outside_link(&self) -> bool {
        self.note.starts_with(OUTSIDE_LINK)
    }
}

// Redact only output: filesystem access and managed-hook ownership need the raw paths.
fn serialize_path<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
    serialize_text(&path.to_string_lossy(), serializer)
}

fn serialize_paths<S: Serializer>(paths: &[PathBuf], serializer: S) -> Result<S::Ok, S::Error> {
    paths
        .iter()
        .map(|path| redact(&path.to_string_lossy()))
        .collect::<Vec<_>>()
        .serialize(serializer)
}

fn serialize_text<S: Serializer>(text: &str, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&redact(text))
}

#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub items: Vec<Item>,
    pub drift: Vec<Drift>,
    pub notes: Vec<Note>,
}

/// Every root the scan reads below; tests point them at temporary directories.
pub struct Roots {
    pub home: PathBuf,
    pub project: PathBuf,
    pub claude_dir: PathBuf,
    pub claude_json: PathBuf,
    pub claude_managed: PathBuf,
    pub codex_home: PathBuf,
    pub codex_system: PathBuf,
}

impl Roots {
    /// Documented defaults, honoring `CLAUDE_CONFIG_DIR` and `CODEX_HOME`.
    pub fn detect(home: &Path, project: &Path) -> Self {
        let env = |name| std::env::var_os(name).filter(|value| !value.is_empty());
        let claude_dir = env("CLAUDE_CONFIG_DIR").map(PathBuf::from);
        Self {
            claude_json: claude_dir
                .as_ref()
                .map_or_else(|| home.join(".claude.json"), |dir| dir.join(".claude.json")),
            claude_dir: claude_dir.unwrap_or_else(|| home.join(".claude")),
            claude_managed: PathBuf::from(if cfg!(target_os = "macos") {
                "/Library/Application Support/ClaudeCode"
            } else {
                "/etc/claude-code"
            }),
            codex_home: env("CODEX_HOME").map_or_else(|| home.join(".codex"), PathBuf::from),
            codex_system: PathBuf::from("/etc/codex"),
            home: home.to_path_buf(),
            project: project.to_path_buf(),
        }
    }
}

struct Scan<'a> {
    roots: &'a Roots,
    reader: Reader,
    items: Vec<Item>,
    drift: Vec<Drift>,
}

/// Inventory every documented location and report shadowed, stale and dangling definitions.
pub fn scan(roots: &Roots) -> Report {
    let agents_home = roots.home.join(".agents");
    let yoke_home = roots.home.join(".yoke");
    let mut scan = Scan {
        roots,
        reader: Reader::new(&[
            &roots.project,
            &roots.claude_dir,
            &roots.claude_json,
            &roots.claude_managed,
            &roots.codex_home,
            &roots.codex_system,
            &agents_home,
            &yoke_home,
        ]),
        items: Vec::new(),
        drift: Vec::new(),
    };
    scan.claude();
    scan.codex();
    scan.presets();
    let Scan {
        mut items,
        mut drift,
        reader,
        ..
    } = scan;
    items.sort_by(|a, b| {
        (a.agent, a.kind, &a.name, a.scope, &a.origin)
            .cmp(&(b.agent, b.kind, &b.name, b.scope, &b.origin))
    });
    drift.extend(shadowed(&items));
    drift.extend(dangling(&items, roots));
    let mut notes = reader.notes;
    notes.sort();
    notes.dedup();
    Report {
        items,
        drift,
        notes,
    }
}

impl Scan<'_> {
    fn push(
        &mut self,
        agent: Owner,
        kind: Kind,
        scope: Scope,
        origin: &Path,
        name: String,
        detail: String,
    ) {
        self.items.push(Item {
            agent,
            kind,
            name: redact(&name),
            identity: name,
            scope,
            origin: origin.to_path_buf(),
            managed: false,
            detail: redact(&detail),
            keys: Vec::new(),
            command: None,
        });
    }

    fn json(&mut self, path: &Path) -> Option<Value> {
        let text = self.reader.text(path, CONFIG_CAP, true)?;
        match serde_json::from_str(&text) {
            Ok(value) => Some(value),
            Err(error) => {
                self.reader.note(path, format!("invalid JSON: {error}"));
                None
            }
        }
    }

    fn toml(&mut self, path: &Path) -> Option<Value> {
        let text = self.reader.text(path, CONFIG_CAP, true)?;
        match toml::from_str::<toml::Value>(&text).map(serde_json::to_value) {
            Ok(Ok(value)) => Some(value),
            _ => {
                self.reader.note(path, "invalid TOML");
                None
            }
        }
    }

    fn claude(&mut self) {
        let roots = self.roots;
        let managed = &roots.claude_managed;
        let user = &roots.claude_dir;
        let project = roots.project.join(".claude");
        let mut settings = vec![
            (Scope::Managed, managed.join("managed-settings.json")),
            (Scope::User, user.join("settings.json")),
            (Scope::Project, project.join("settings.json")),
            (Scope::Local, project.join("settings.local.json")),
        ];
        for path in self.reader.entries(&managed.join("managed-settings.d")) {
            if path.extension().is_some_and(|ext| ext == "json") {
                settings.push((Scope::Managed, path));
            }
        }
        for (scope, path) in settings {
            let Some(value) = self.json(&path) else {
                continue;
            };
            self.hooks(Owner::Claude, scope, &path, &value["hooks"]);
            if let Some(plugins) = value["enabledPlugins"].as_object() {
                for (name, enabled) in plugins {
                    let state = if enabled.as_bool() == Some(true) {
                        "enabled"
                    } else {
                        "disabled"
                    };
                    self.push(
                        Owner::Claude,
                        Kind::Plugin,
                        scope,
                        &path,
                        name.clone(),
                        state.into(),
                    );
                }
            }
        }
        for (scope, dir) in [
            (Scope::Managed, managed.join(".claude/skills")),
            (Scope::User, user.join("skills")),
            (Scope::Project, project.join("skills")),
        ] {
            self.skills(Owner::Claude, scope, &dir);
        }
        for (scope, dir) in [
            (Scope::User, user.join("commands")),
            (Scope::Project, project.join("commands")),
        ] {
            for path in self.reader.walk(&dir, "md") {
                let relative = path.strip_prefix(&dir).unwrap_or(&path).with_extension("");
                let name = relative.to_string_lossy().replace('/', ":");
                self.push(
                    Owner::Claude,
                    Kind::Command,
                    scope,
                    &path,
                    name,
                    String::new(),
                );
            }
        }
        for (scope, dir) in [
            (Scope::Managed, managed.join(".claude/agents")),
            (Scope::User, user.join("agents")),
            (Scope::Project, project.join("agents")),
        ] {
            for path in self.reader.walk(&dir, "md") {
                let name = self
                    .reader
                    .text(&path, HEAD_CAP, false)
                    .and_then(|text| read::frontmatter_name(&text))
                    .unwrap_or_else(|| stem(&path));
                self.push(
                    Owner::Claude,
                    Kind::Agent,
                    scope,
                    &path,
                    name,
                    String::new(),
                );
            }
        }
        if let Some(value) = self.json(&managed.join("managed-mcp.json")) {
            self.mcp(
                Owner::Claude,
                Scope::Managed,
                &managed.join("managed-mcp.json"),
                &value["mcpServers"],
            );
        }
        let claude_json = roots.claude_json.clone();
        if let Some(value) = self.json(&claude_json) {
            self.mcp(
                Owner::Claude,
                Scope::User,
                &claude_json,
                &value["mcpServers"],
            );
            let local = roots
                .project
                .to_str()
                .map_or(&Value::Null, |key| &value["projects"][key]["mcpServers"]);
            self.mcp(Owner::Claude, Scope::Local, &claude_json, local);
        }
        let shared = roots.project.join(".mcp.json");
        if let Some(value) = self.json(&shared) {
            self.mcp(Owner::Claude, Scope::Project, &shared, &value["mcpServers"]);
        }
        for (scope, path) in [
            (Scope::Managed, managed.join("CLAUDE.md")),
            (Scope::User, user.join("CLAUDE.md")),
            (Scope::Project, roots.project.join("CLAUDE.md")),
            (Scope::Project, project.join("CLAUDE.md")),
            (Scope::Local, roots.project.join("CLAUDE.local.md")),
        ] {
            self.memory(Owner::Claude, scope, &path);
        }
        for (scope, dir) in [
            (Scope::User, user.join("rules")),
            (Scope::Project, project.join("rules")),
        ] {
            self.rules(Owner::Claude, scope, &dir, "md");
        }
    }

    fn codex(&mut self) {
        let roots = self.roots;
        let user = &roots.codex_home;
        let project = roots.project.join(".codex");
        for (scope, path) in [
            (Scope::System, roots.codex_system.join("config.toml")),
            (Scope::User, user.join("config.toml")),
            (Scope::Project, project.join("config.toml")),
        ] {
            let Some(value) = self.toml(&path) else {
                continue;
            };
            self.mcp(Owner::Codex, scope, &path, &value["mcp_servers"]);
            self.hooks(Owner::Codex, scope, &path, &value["hooks"]);
            if let Some(plugins) = value["plugins"].as_object() {
                for (name, table) in plugins {
                    let state = if table["enabled"].as_bool() == Some(false) {
                        "disabled"
                    } else {
                        "enabled"
                    };
                    self.push(
                        Owner::Codex,
                        Kind::Plugin,
                        scope,
                        &path,
                        name.clone(),
                        state.into(),
                    );
                }
            }
        }
        for (scope, path) in [
            (Scope::User, user.join("hooks.json")),
            (Scope::Project, project.join("hooks.json")),
        ] {
            if let Some(value) = self.json(&path) {
                self.hooks(Owner::Codex, scope, &path, &value["hooks"]);
            }
        }
        for (scope, dir) in [
            (Scope::Managed, roots.codex_system.join("skills")),
            (Scope::User, roots.home.join(".agents/skills")),
            (Scope::Project, roots.project.join(".agents/skills")),
        ] {
            self.skills(Owner::Codex, scope, &dir);
        }
        for (scope, dir) in [
            (Scope::User, user.join("agents")),
            (Scope::Project, project.join("agents")),
        ] {
            for path in self.reader.walk(&dir, "toml") {
                let name = self
                    .toml(&path)
                    .and_then(|value| value["name"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| stem(&path));
                self.push(Owner::Codex, Kind::Agent, scope, &path, name, String::new());
            }
        }
        for path in self.reader.entries(&user.join("prompts")) {
            if path.extension().is_some_and(|ext| ext == "md") && self.reader.exists(&path) {
                self.push(
                    Owner::Codex,
                    Kind::Command,
                    Scope::User,
                    &path,
                    stem(&path),
                    "deprecated custom prompt".into(),
                );
            }
        }
        for (scope, dir) in [
            (Scope::User, user.clone()),
            (Scope::Project, roots.project.clone()),
        ] {
            for file in ["AGENTS.override.md", "AGENTS.md"] {
                self.memory(Owner::Codex, scope, &dir.join(file));
            }
        }
        for (scope, dir) in [
            (Scope::User, user.join("rules")),
            (Scope::Project, project.join("rules")),
        ] {
            self.rules(Owner::Codex, scope, &dir, "rules");
        }
    }

    /// Preset blocks are Yoke-managed; a block that no longer matches its bundled text is stale.
    fn presets(&mut self) {
        let roots = self.roots;
        for (scope, path) in [
            (Scope::User, roots.home.join(".yoke/config.toml")),
            (Scope::Project, roots.project.join("yoke.toml")),
        ] {
            let Some(text) = self.reader.text(&path, CONFIG_CAP, true) else {
                continue;
            };
            for name in text
                .lines()
                .filter_map(|line| line.strip_prefix("# Yoke preset: "))
            {
                self.push(
                    Owner::Yoke,
                    Kind::Preset,
                    scope,
                    &path,
                    name.to_owned(),
                    String::new(),
                );
                self.items.last_mut().unwrap().managed = true;
                // Presets that need options cannot be re-rendered here and are not judged.
                if let Ok(block) = preset::render(name, &preset::PresetOptions::default()) {
                    if !text.contains(&block) {
                        self.drift.push(Drift {
                            kind: DriftKind::Stale,
                            agent: Owner::Yoke,
                            name: name.to_owned(),
                            detail: "preset block differs from the bundled version; compare with yoke preset add output and edit it explicitly".into(),
                            origins: vec![path.clone()],
                        });
                    }
                }
            }
        }
    }

    fn skills(&mut self, agent: Owner, scope: Scope, dir: &Path) {
        for skill in self.reader.entries(dir) {
            self.reader.link(&skill);
            let file = skill.join("SKILL.md");
            if !self.reader.exists(&file) {
                continue;
            }
            let name = self
                .reader
                .text(&file, HEAD_CAP, false)
                .and_then(|text| read::frontmatter_name(&text))
                .unwrap_or_else(|| stem(&skill));
            self.push(agent, Kind::Skill, scope, &file, name, String::new());
        }
    }

    fn memory(&mut self, agent: Owner, scope: Scope, path: &Path) {
        if self.reader.exists(path) {
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            self.push(agent, Kind::Memory, scope, path, name, String::new());
        }
    }

    fn rules(&mut self, agent: Owner, scope: Scope, dir: &Path, extension: &str) {
        for path in self.reader.walk(dir, extension) {
            let name = path
                .strip_prefix(dir)
                .unwrap_or(&path)
                .display()
                .to_string();
            self.push(agent, Kind::Rule, scope, &path, name, String::new());
        }
    }

    fn hooks(&mut self, agent: Owner, scope: Scope, origin: &Path, hooks: &Value) {
        let Some(events) = hooks.as_object() else {
            return;
        };
        for (event, groups) in events {
            for group in groups.as_array().into_iter().flatten() {
                for handler in group["hooks"].as_array().into_iter().flatten() {
                    let command = handler["command"]
                        .as_str()
                        .filter(|_| handler["type"] == "command");
                    let detail = match command {
                        Some(command) => summary(read::program(command)),
                        None => format!("type: {}", handler["type"].as_str().unwrap_or("unknown")),
                    };
                    self.push(agent, Kind::Hook, scope, origin, event.clone(), detail);
                    self.items.last_mut().unwrap().command = command.map(str::to_owned);
                }
            }
        }
    }

    fn mcp(&mut self, agent: Owner, scope: Scope, origin: &Path, servers: &Value) {
        let Some(servers) = servers.as_object() else {
            return;
        };
        for (name, server) in servers {
            let command = server["command"].as_str();
            let mut detail = match (command, server["url"].as_str()) {
                (Some(command), _) => {
                    let args = server["args"].as_array().map_or(0, Vec::len);
                    summary(read::program(command).map(|(program, words)| (program, words + args)))
                }
                (None, Some(url)) => scheme_and_host(url),
                (None, None) => String::new(),
            };
            if let Some(transport) = server["type"].as_str() {
                detail = format!("{transport}: {detail}");
            }
            let mut keys = Vec::new();
            for field in ["env", "headers", "http_headers", "env_http_headers"] {
                keys.extend(
                    server[field]
                        .as_object()
                        .into_iter()
                        .flat_map(|map| map.keys().cloned()),
                );
            }
            keys.extend(
                server["env_vars"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|value| {
                        value
                            .as_str()
                            .or_else(|| value["name"].as_str())
                            .map(str::to_owned)
                    }),
            );
            self.push(agent, Kind::Mcp, scope, origin, name.clone(), detail);
            let item = self.items.last_mut().unwrap();
            item.keys = keys.iter().map(|key| redact(key)).collect();
            item.command = command.map(str::to_owned);
        }
    }
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// `program (N args)`; argument text is never shown because it can hold any secret.
fn summary(program: Option<(String, usize)>) -> String {
    match program {
        Some((program, 1)) => format!("{program} (1 arg)"),
        Some((program, count)) => format!("{program} ({count} args)"),
        None => "unparsed command".into(),
    }
}

/// Paths, queries and userinfo can hold webhook keys or tokens, so only the origin is shown.
fn scheme_and_host(url: &str) -> String {
    ORIGIN.captures(url).map_or_else(
        || "unparsed URL".into(),
        |found| format!("{}{}", &found[1], &found[2]),
    )
}

/// Documented winner order (lower wins); `None` where the agent documents no precedence.
fn rank(item: &Item) -> Option<u8> {
    use Scope::*;
    let scope = match (item.agent, item.kind, item.scope) {
        (Owner::Claude, Kind::Skill, Managed) => 0,
        (Owner::Claude, Kind::Skill, User) => 1,
        (Owner::Claude, Kind::Skill, _) => 2,
        // A skill beats a command; user versus project commands is undocumented.
        (Owner::Claude, Kind::Command, _) => return Some(10),
        (Owner::Claude, Kind::Agent, Managed) => 0,
        (Owner::Claude, Kind::Agent, Project) => 1,
        (Owner::Claude, Kind::Agent, _) => 2,
        (Owner::Claude, Kind::Mcp | Kind::Plugin, Managed) => 0,
        (Owner::Claude, Kind::Mcp | Kind::Plugin, Local) => 1,
        (Owner::Claude, Kind::Mcp | Kind::Plugin, Project) => 2,
        (Owner::Claude, Kind::Mcp | Kind::Plugin, _) => 3,
        (Owner::Codex, Kind::Mcp | Kind::Plugin, Project) => 0,
        (Owner::Codex, Kind::Mcp | Kind::Plugin, User) => 1,
        (Owner::Codex, Kind::Mcp | Kind::Plugin, _) => 2,
        _ => return None,
    };
    Some(scope)
}

/// Same-name definitions in different files: which one the agent uses, per its docs.
fn shadowed(items: &[Item]) -> Vec<Drift> {
    let mut groups: BTreeMap<(Owner, &str, &str), Vec<&Item>> = BTreeMap::new();
    for item in items {
        // Claude skills and commands share the slash-command namespace.
        let family = match (item.agent, item.kind) {
            (Owner::Claude, Kind::Skill | Kind::Command) => "command",
            (_, Kind::Skill) => "skill",
            (_, Kind::Agent) => "agent",
            (_, Kind::Mcp) => "mcp",
            (_, Kind::Plugin) => "plugin",
            _ => continue,
        };
        groups
            .entry((item.agent, family, &item.identity))
            .or_default()
            .push(item);
    }
    let mut drift = Vec::new();
    for ((agent, family, name), group) in groups {
        // Agreeing plugin switches at several scopes are redundant, not conflicting.
        if group.len() < 2
            || (family == "plugin" && group.iter().all(|item| item.detail == group[0].detail))
        {
            continue;
        }
        let mut origins: Vec<PathBuf> = group.iter().map(|item| item.origin.clone()).collect();
        origins.sort();
        origins.dedup();
        let ranks: Vec<_> = group.iter().map(|item| rank(item)).collect();
        let best = ranks.iter().flatten().min();
        let winners: Vec<_> = group
            .iter()
            .zip(&ranks)
            .filter(|(_, rank)| rank.is_some() && rank.as_ref() == best)
            .collect();
        let resolution = if agent == Owner::Codex && family == "skill" {
            "both load; Codex does not merge same-name skills".to_owned()
        } else if ranks.iter().all(Option::is_some) && winners.len() == 1 {
            let winner = winners[0].0;
            let mut text = format!(
                "the {} {} definition wins",
                scope_name(winner.scope),
                kind_name(winner.kind)
            );
            if agent == Owner::Codex && family == "mcp" {
                text.push_str("; whether fields merge across layers is not documented");
            }
            text
        } else {
            "which definition wins is not documented".to_owned()
        };
        let definitions: Vec<_> = group
            .iter()
            .map(|item| format!("{} {}", scope_name(item.scope), kind_name(item.kind)))
            .collect();
        drift.push(Drift {
            kind: DriftKind::Shadowed,
            agent,
            name: redact(name),
            detail: format!("defined as {}: {resolution}", definitions.join(", ")),
            origins,
        });
    }
    drift
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Skill => "skill",
        Kind::Command => "command",
        Kind::Agent => "agent",
        Kind::Plugin => "plugin",
        Kind::Mcp => "mcp server",
        Kind::Hook => "hook",
        Kind::Memory => "memory file",
        Kind::Rule => "rule",
        Kind::Preset => "preset",
    }
}

fn scope_name(scope: Scope) -> &'static str {
    match scope {
        Scope::Managed => "managed",
        Scope::Local => "local",
        Scope::Project => "project",
        Scope::User => "user",
        Scope::System => "system",
    }
}

/// Hook and MCP commands whose program is missing or not executable.
fn dangling(items: &[Item], roots: &Roots) -> Vec<Drift> {
    let mut drift = Vec::new();
    for item in items {
        let Some(command) = &item.command else {
            continue;
        };
        let shown = read::program(command).map(|(program, _)| program);
        // MCP commands name a program; hook commands are shell command lines.
        let program = match item.kind {
            Kind::Mcp => Some(command.clone()),
            _ => shown.clone(),
        };
        let Some(program) = program else {
            continue;
        };
        if read::resolves(&program, &roots.home, &roots.project) == Some(false) {
            drift.push(Drift {
                kind: DriftKind::Dangling,
                agent: item.agent,
                name: item.name.clone(),
                detail: redact(&format!(
                    "{} runs {}, which is not an executable file or on PATH",
                    kind_name(item.kind),
                    shown.as_deref().unwrap_or("an unparsed command")
                )),
                origins: vec![item.origin.clone()],
            });
        }
    }
    drift
}

#[cfg(test)]
#[path = "extensions/tests.rs"]
mod tests;
