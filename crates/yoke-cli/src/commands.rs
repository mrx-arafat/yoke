use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;
use yoke_core::config::{self, GateAction, LoadedConfig};
use yoke_core::store::Store;
use yoke_core::{guard, trust};

use crate::{install, memory, presets, update, workflow, AgentArg, Cmd};

pub fn quote(value: &str) -> String {
    serde_json::to_string(value).expect("strings serialize")
}

pub fn home() -> Result<PathBuf> {
    config::home()
}

/// Existing state opened read-only, or an empty in-memory store when there is none; it never
/// creates, migrates or writes the state database.
pub fn inspect_store(home: &Path) -> Result<Store> {
    let path = home.join(".yoke/state.db");
    if path.exists() {
        read_existing(&path)
    } else {
        Store::open(":memory:")
    }
}

/// A file with no committed schema (for example zero bytes after a crash) is reported as such,
/// not as a version mismatch or a missing table.
fn read_existing(path: &Path) -> Result<Store> {
    let store = Store::open_read_only(path).context("read Yoke state without changing it")?;
    ensure!(
        store.schema_version()?.0 > 0,
        "the Yoke state database {} is empty or incomplete; run yoke config from your own terminal, or start your agent, to initialise it",
        path.display()
    );
    Ok(store)
}

/// Existing state at the schema this binary writes, or `None` when there is no state yet.
/// Never creates or migrates: hooks that run an older yoke refuse a newer schema.
pub fn open_current(home: &Path) -> Result<Option<Store>> {
    let path = home.join(".yoke/state.db");
    if !path.exists() {
        return Ok(None);
    }
    let (schema, supported) = read_existing(&path)?.schema_version()?;
    ensure!(
        schema == supported,
        "the state database is at schema {schema} and this yoke needs schema {supported}; this command does not upgrade it, because hooks running an older yoke refuse a newer schema. Update the yoke your hooks run; its next hook event upgrades the database"
    );
    Store::open(&path).map(Some)
}

pub fn open_store(home: &Path) -> Result<Store> {
    let directory = home.join(".yoke");
    yoke_core::private::create_dir_all(&directory).context("create Yoke state directory")?;
    Store::open(directory.join("state.db"))
}

pub fn load_project() -> Result<(Store, LoadedConfig)> {
    let home = home()?;
    let store = open_store(&home)?;
    let loaded = config::load_trusted(&store, &std::env::current_dir()?, &home)?;
    Ok((store, loaded))
}

/// Agent memory scope comes from the hook-recorded session and OS process ancestry.
pub fn agent_memory_project(store: &Store) -> Result<Option<PathBuf>> {
    if guard::agent_process_signal().is_none() {
        return Ok(None);
    }
    let session = std::env::var("CODEX_THREAD_ID")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            std::env::var("CODEX_SESSION_ID")
                .ok()
                .filter(|value| !value.is_empty())
        });
    let agent = if session.is_some() { "codex" } else { "claude" };
    yoke_core::project::agent_scope(
        store,
        agent,
        session.as_deref(),
        &yoke_core::process::ancestors(),
    )
    .map(Some)
}

/// Load memory policy from the anchored project for agents and the current directory for humans.
pub fn load_memory_project() -> Result<(Store, LoadedConfig)> {
    let home = home()?;
    let store = open_store(&home)?;
    let loaded = match agent_memory_project(&store)? {
        Some(root) => {
            let anchor = trust::SessionAnchor {
                dir: root.clone(),
                root: root.clone(),
                not_remembered: None,
            };
            trust::load_for_event(&store, &anchor, &root, &home)?.loaded
        }
        None => config::load_trusted(&store, &std::env::current_dir()?, &home)?,
    };
    Ok((store, loaded))
}

pub fn require_human(action: &str) -> Result<()> {
    if let Some(signal) = guard::agent_process_signal() {
        bail!("{action} requires a human outside an agent tool process ({signal}); run yoke {action} in your own terminal");
    }
    ensure!(
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        "{action} requires an interactive human terminal; no noninteractive approval is supported"
    );
    Ok(())
}

/// Refuse to change how Yoke is installed from an agent's tool process. Unlike `require_human`
/// this does not need a terminal, so the installer script keeps working from a human's shell.
pub fn refuse_agent(action: &str) -> Result<()> {
    if let Some(signal) = guard::agent_process_signal() {
        bail!("{action} changes how Yoke is installed and cannot run from an agent tool process ({signal}); a human runs yoke {action} in their own terminal");
    }
    Ok(())
}

pub fn confirm(phrase: &str) -> Result<bool> {
    print!("Type {phrase} to confirm: ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(answer.trim() == phrase)
}

pub fn run(command: Option<Cmd>) -> Result<()> {
    match command {
        Some(Cmd::Console { no_open, port }) => {
            // The console can approve, resume and acknowledge, so only a human may start it.
            require_human("console")?;
            let home = home()?;
            let (_, loaded) = load_project()?;
            crate::console::run(&home, &loaded.project_root, !no_open, port)
        }
        Some(Cmd::Statusline {
            wrap,
            style,
            refresh_git,
            refresh_health,
        }) => crate::statusline::run(
            wrap.as_deref(),
            style,
            refresh_git.as_deref(),
            refresh_health.as_deref(),
        ),
        Some(Cmd::Plan { command }) => workflow::run(command),
        Some(Cmd::Review {
            slug,
            session,
            agent,
            json,
        }) => workflow::review(slug, session, agent, json),
        Some(Cmd::Remember { text, pinned, json }) => memory::remember(&text, pinned, json),
        Some(Cmd::Recall {
            query,
            limit,
            json,
            explain,
        }) => memory::recall(&query, limit, json, explain),
        Some(Cmd::Forget { id }) => memory::forget(id),
        Some(Cmd::Share { id }) => memory::share(id),
        Some(Cmd::Memory { command }) => memory::run(command),
        Some(Cmd::Project { command }) => crate::project::run(command),
        Some(Cmd::Jobs { command, all, json }) => crate::jobs::run(command, all, json),
        Some(Cmd::Preset { command }) => presets::run(command),
        Some(Cmd::Init) => init(),
        Some(Cmd::Trust { global }) => trust(global),
        Some(Cmd::Pause { duration, clear }) => crate::bypass::pause(duration, clear),
        Some(Cmd::Install {
            agent,
            dry_run,
            statusline,
            statusline_if_absent,
            style,
        }) => install(
            agent,
            dry_run,
            (
                if statusline {
                    install::Statusline::Wrap
                } else if statusline_if_absent {
                    install::Statusline::IfAbsent
                } else {
                    install::Statusline::Off
                },
                style,
            ),
            false,
        ),
        Some(Cmd::Uninstall { agent, dry_run }) => {
            install(agent, dry_run, (install::Statusline::Off, None), true)
        }
        Some(Cmd::Health { json }) => crate::health::run(json),
        Some(Cmd::Extensions { json }) => crate::extensions::run(json),
        Some(Cmd::Notifications {
            command,
            unread,
            json,
        }) => crate::notifications::run(command, unread, json),
        Some(Cmd::Update { check }) => {
            if !check {
                refuse_agent("update")?;
            }
            update::run(check, &open_store(&home()?)?)
        }
        Some(Cmd::Trial { command }) => crate::trial::run(command),
        Some(Cmd::Log {
            project,
            all,
            session,
            outcome,
            tool,
            limit,
            json,
        }) => crate::log_cmd::run(crate::log_cmd::Args {
            project,
            all,
            session,
            outcome,
            tool,
            limit,
            json,
        }),
        Some(Cmd::RunHook { .. }) => unreachable!("hook dispatch precedes normal commands"),
        other => {
            let home = home()?;
            let loaded =
                config::load_trusted(&open_store(&home)?, &std::env::current_dir()?, &home)?;
            match other {
                Some(Cmd::Gates { verbose, json }) => gates(&loaded, verbose, json)?,
                Some(Cmd::Config { json: true }) => println!("{}", serde_json::to_string(&loaded)?),
                Some(Cmd::Config { json: false }) => config(&loaded)?,
                None => {
                    println!(
                        "bin: {}",
                        quote(&std::env::current_exe()?.display().to_string())
                    );
                    println!("description: {}", quote(env!("CARGO_PKG_DESCRIPTION")));
                    println!("mode: {}", loaded.config.mode.as_str());
                    println!(
                        "project: {}",
                        quote(&loaded.project_root.display().to_string())
                    );
                    gates(&loaded, false, false)?;
                    println!("help: \"Run yoke health to check that your agents are protected, yoke config to inspect trust, yoke log for decisions, or yoke --help for commands.\"");
                }
                _ => unreachable!(),
            }
            Ok(())
        }
    }
}

fn init() -> Result<()> {
    // The project's root, not the current directory: a yoke.toml in a subdirectory would start
    // a second project with its own trust, memory and log.
    let directory = std::env::current_dir().context("resolve the working directory")?;
    let root = trust::config_home(&directory)?;
    let file = root.join("yoke.toml");
    let (place, at_root) = if root == directory.canonicalize()? {
        ("yoke.toml".to_owned(), "")
    } else {
        (file.display().to_string(), " at the project root")
    };
    match OpenOptions::new().write(true).create_new(true).open(&file) {
        Ok(mut created) => {
            let name = root
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("my-project");
            created.write_all(crate::starter::render(name).as_bytes())?;
            println!(
                "config: {}",
                quote(&format!(
                    "Created {place}{at_root}; it contributes no gates until a human runs yoke trust."
                ))
            );
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            println!(
                "config: {}",
                quote(&format!("{place} already exists{at_root}; unchanged."))
            );
        }
        Err(error) => return Err(error).with_context(|| format!("create {place}")),
    }
    Ok(())
}

fn gates(loaded: &LoadedConfig, verbose: bool, json: bool) -> Result<()> {
    let mut details = Vec::new();
    for gate in &loaded.config.gates {
        let mut detail = serde_json::to_value(gate)?;
        detail["source"] = loaded
            .origins
            .get(&format!("gate.{}", gate.name))
            .map(String::as_str)
            .unwrap_or("default")
            .into();
        if gate.action == GateAction::Check {
            detail["shell"] = gate.shell.as_deref().unwrap_or("/bin/sh").into();
            detail["expected_exit"] = 0.into();
        } else {
            detail["expected_exit"] = Value::Null;
        }
        details.push(detail);
    }
    if json {
        println!(
            "{}",
            serde_json::json!({
                "project": loaded.project_root,
                "mode": loaded.config.mode,
                "pass_env": loaded.config.gate_environment.pass_env,
                "gates": details,
                "snapshots": loaded.snapshots,
            })
        );
        return Ok(());
    }
    if verbose {
        println!(
            "project: {}",
            quote(&loaded.project_root.display().to_string())
        );
        println!("mode: {}", loaded.config.mode.as_str());
        println!(
            "pass_env: {}",
            serde_json::to_string(&loaded.config.gate_environment.pass_env)?
        );
    }
    let columns = if verbose {
        "name,event,action,source,settings"
    } else {
        "name,event,action,source"
    };
    println!("gates[{}]{{{columns}}}:", loaded.config.gates.len());
    for (gate, detail) in loaded.config.gates.iter().zip(details) {
        println!(
            "  {},{},{},{}{}",
            quote(&gate.name),
            gate.on.as_str(),
            serde_json::to_string(&gate.action).expect("gate action serializes"),
            detail["source"],
            if verbose {
                format!(",{}", quote(&detail.to_string()))
            } else {
                String::new()
            }
        );
    }
    trust_help(loaded);
    println!("help: \"Run yoke gates --verbose for full settings, yoke gates --json for structured inspection, or yoke log --json for recorded reasons, exit codes and timing.\"");
    Ok(())
}

/// Tables of ignored settings look the same as tables of nothing; say which one this is.
fn trust_help(loaded: &LoadedConfig) {
    let ignored: Vec<&yoke_core::trust::SnapshotStatus> = loaded
        .snapshots
        .iter()
        .filter(|snapshot| snapshot.exists && !snapshot.trusted)
        .collect();
    if !ignored.is_empty() {
        let files: Vec<String> = ignored
            .iter()
            .map(|snapshot| snapshot.file.display().to_string())
            .collect();
        let command = if ignored.iter().all(|snapshot| snapshot.scope == "global") {
            "yoke trust --global"
        } else {
            "yoke trust (yoke trust --global for the global file)"
        };
        println!(
            "help: {}",
            quote(&format!(
                "{} is not trusted, so its settings and gates are ignored; review it and run {command} in your own terminal.",
                files.join(", ")
            ))
        );
    }
    for snapshot in loaded.snapshots.iter().filter(|snapshot| snapshot.stale) {
        println!(
            "help: {}",
            quote(&format!(
                "{} changed after it was trusted; Yoke keeps using the trusted bytes until you review it and run yoke trust.",
                snapshot.file.display()
            ))
        );
    }
}

fn config(loaded: &LoadedConfig) -> Result<()> {
    fn flatten(prefix: &str, value: &Value, rows: &mut BTreeMap<String, String>) {
        if let Some(object) = value.as_object() {
            for (key, value) in object {
                if key == "gate" && prefix.is_empty() {
                    continue;
                }
                flatten(
                    &if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    },
                    value,
                    rows,
                );
            }
        } else {
            rows.insert(prefix.into(), value.to_string());
        }
    }
    let mut rows = BTreeMap::new();
    flatten("", &serde_json::to_value(&loaded.config)?, &mut rows);
    println!(
        "project: {}",
        quote(&loaded.project_root.display().to_string())
    );
    println!("config[{}]{{key,value,source}}:", rows.len());
    for (key, value) in rows {
        println!(
            "  {},{},{}",
            quote(&key),
            quote(&value),
            quote(
                loaded
                    .origins
                    .get(&key)
                    .map(String::as_str)
                    .unwrap_or("default")
            )
        );
    }
    println!(
        "gate_settings[{}]{{name,settings,source}}:",
        loaded.config.gates.len()
    );
    for gate in &loaded.config.gates {
        println!(
            "  {},{},{}",
            quote(&gate.name),
            quote(&serde_json::to_string(gate)?),
            quote(
                loaded
                    .origins
                    .get(&format!("gate.{}", gate.name))
                    .map(String::as_str)
                    .unwrap_or("default")
            )
        );
    }
    println!(
        "snapshots[{}]{{file,trusted,stale,exists,error}}:",
        loaded.snapshots.len()
    );
    for snapshot in &loaded.snapshots {
        println!(
            "  {},{},{},{},{}",
            quote(&snapshot.file.display().to_string()),
            snapshot.trusted,
            snapshot.stale,
            snapshot.exists,
            quote(snapshot.disk_error.as_deref().unwrap_or(""))
        );
    }
    trust_help(loaded);
    println!(
        "help: \"Run yoke config --json for structured configuration and snapshot metadata.\""
    );
    Ok(())
}

fn trust(global: bool) -> Result<()> {
    require_human("trust")?;
    let home = home()?;
    let store = open_store(&home)?;
    let prepared = if global {
        trust::prepare_global(&store, &home)?
    } else {
        trust::prepare_project(&store, &std::env::current_dir()?)?
    };
    for snapshot in &prepared.snapshots {
        println!(
            "file: {}\nsha256: {}",
            quote(&snapshot.file.display().to_string()),
            snapshot.sha256
        );
        if snapshot.previous.as_deref() == Some(&snapshot.content) {
            println!("change: unchanged");
            continue;
        }
        println!(
            "change: {}",
            if snapshot.previous.is_some() {
                "modified"
            } else {
                "new"
            }
        );
        for line in crate::preview::snapshot_lines(snapshot.previous.as_deref(), &snapshot.content)
        {
            println!("{line}");
        }
    }
    println!("Trust these exact snapshots?");
    if !confirm("trust")? {
        println!("trust: cancelled");
        return Ok(());
    }
    prepared.commit(&store)?;
    crate::hook::refresh_mode_record(&store, &home, &std::env::current_dir()?)?;
    println!("trust: saved");
    Ok(())
}

fn install(
    agents: Vec<AgentArg>,
    dry_run: bool,
    (statusline, statusline_style): (install::Statusline, Option<crate::statusline::Style>),
    remove: bool,
) -> Result<()> {
    ensure!(
        statusline_style.is_none() || statusline != install::Statusline::Off,
        "--style needs --statusline or --statusline-if-absent"
    );
    if !dry_run {
        refuse_agent(if remove { "uninstall" } else { "install" })?;
    }
    let agents = agents
        .into_iter()
        .map(|agent| match agent {
            AgentArg::Claude => install::Agent::Claude,
            AgentArg::Codex => install::Agent::Codex,
        })
        .collect();
    let options = install::InstallOptions {
        dry_run,
        statusline,
        statusline_style,
        ..install::options(&home()?, std::env::current_exe()?, agents)?
    };
    let report = if remove {
        install::uninstall(&options)?
    } else {
        install::install(&options)?
    };
    println!("changes: {}\ndry_run: {}", report.changes.len(), dry_run);
    for change in report.changes {
        println!("file: {}", quote(&change.path.display().to_string()));
        if dry_run {
            // Only owned hunks are shown; other settings may contain credentials.
            for line in install::preview_lines(&change) {
                println!("{line}");
            }
        }
    }
    for warning in report.warnings {
        println!("warning: {}", quote(&warning));
    }
    Ok(())
}
