//! `yoke health`: a read-only report of installation and protection state with next steps.
//! It never creates, migrates or writes state, settings or receipts; activating trust or
//! enforcement stays a human action in the user's own terminal.

use std::io::Write;
use std::path::Path;

use anyhow::Result;
use serde::Serialize;
use yoke_core::config::{self, Mode};
use yoke_core::store::Store;

use crate::{commands, install};

#[derive(Serialize)]
struct Report {
    status: &'static str,
    executable: String,
    state: State,
    mode: Option<&'static str>,
    project: String,
    policy: Vec<Policy>,
    agents: Vec<AgentReport>,
    plans: Option<Vec<PlanReport>>,
    problems: Vec<String>,
    guidance: Vec<String>,
}

#[derive(Serialize)]
struct State {
    path: String,
    exists: bool,
    schema: Option<usize>,
    supported: usize,
}

#[derive(Serialize)]
struct Policy {
    file: String,
    trusted: bool,
    stale: bool,
}

#[derive(Serialize)]
struct AgentReport {
    #[serde(flatten)]
    install: install::AgentStatus,
    last_event: Option<LastEvent>,
}

#[derive(Serialize)]
struct LastEvent {
    ts: i64,
    event: String,
    mode: String,
    outcome: String,
}

#[derive(Serialize)]
struct PlanReport {
    slug: String,
    state: String,
}

/// Print the report; exit 1 when a check fails so scripts can gate on it.
pub fn run(json_output: bool) -> Result<()> {
    let report = collect()?;
    let failing = report.status == "failing";
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_text(&report);
    }
    if failing {
        std::io::stdout().flush()?;
        std::process::exit(1);
    }
    Ok(())
}

fn collect() -> Result<Report> {
    let home = commands::home()?;
    let cwd = std::env::current_dir()?;
    let executable = std::env::current_exe()?;
    let mut problems = Vec::new();
    let mut guidance = Vec::new();
    let path = home.join(".yoke/state.db");
    // A missing database stays missing: an empty in-memory store answers with defaults.
    let opened = if path.exists() {
        Store::open_read_only(&path)
    } else {
        guidance.push("No Yoke state exists yet; the first hook event creates it.".into());
        Store::open(":memory:")
    };
    let (store, readable) = match opened {
        Ok(store) => (store, true),
        Err(error) => {
            problems.push(format!(
                "cannot read {}: {error:#}; check free disk space and file permissions",
                path.display()
            ));
            (Store::open(":memory:")?, false)
        }
    };
    let (schema, supported) = store.schema_version()?;
    let current = schema == supported;
    if readable && path.exists() && !current {
        guidance.push(format!(
            "The state schema is version {schema}; the next hook or command that writes upgrades it to {supported}."
        ));
    }
    let loaded = match config::load_trusted(&store, &cwd, &home) {
        Ok(loaded) => Some(loaded),
        Err(error) => {
            problems.push(format!("cannot load trusted policy: {error:#}"));
            None
        }
    };
    let project = loaded
        .as_ref()
        .map_or_else(|| cwd.clone(), |loaded| loaded.project_root.clone());
    let mut policy = Vec::new();
    if let Some(loaded) = &loaded {
        policy = loaded
            .snapshots
            .iter()
            .map(|snapshot| Policy {
                file: snapshot.file.display().to_string(),
                trusted: snapshot.trusted,
                stale: snapshot.stale,
            })
            .collect();
        if readable {
            policy_guidance(loaded, &mut guidance);
        }
    }
    let agents = agent_reports(&home, executable.clone(), &project, &store, readable)
        .unwrap_or_else(|error| {
            problems.push(format!("cannot locate agent settings: {error:#}"));
            Vec::new()
        });
    for agent in &agents {
        let name = agent.install.agent;
        if !agent.install.installed {
            guidance.push(format!(
                "{name} is not protected; run `yoke install --agent {name}`."
            ));
            continue;
        }
        problems.extend(
            agent
                .install
                .problems
                .iter()
                .map(|problem| format!("{name}: {problem}")),
        );
        if readable && agent.last_event.is_none() {
            guidance.push(format!(
                "No {name} hook event is recorded yet; start {name} in a project to confirm its hooks run."
            ));
        }
        if name == "codex" {
            guidance.extend(agent.install.notes.iter().cloned());
        }
    }
    if !agents.is_empty() && agents.iter().all(|agent| !agent.install.installed) {
        problems.push("Yoke hooks are not installed for any agent.".into());
    }
    let plans = (readable && current)
        .then(|| yoke_core::plan::list(&store, &project))
        .transpose()
        .unwrap_or_else(|error| {
            problems.push(format!("cannot read plans: {error:#}"));
            None
        })
        .map(|plans| {
            plans
                .into_iter()
                .filter(|plan| !matches!(plan.state.as_str(), "done" | "cancelled"))
                .map(|plan| PlanReport {
                    slug: plan.slug,
                    state: plan.state,
                })
                .collect::<Vec<_>>()
        });
    if plans.as_ref().is_some_and(Vec::is_empty) {
        guidance.push("No unfinished plan in this project, so Stop is not held for plan checks; start one with `yoke plan new <slug>`.".into());
    }
    let status = if !problems.is_empty() {
        "failing"
    } else if !guidance.is_empty() {
        "attention"
    } else {
        "ok"
    };
    Ok(Report {
        status,
        executable: executable.display().to_string(),
        state: State {
            path: path.display().to_string(),
            exists: path.exists(),
            schema: path.exists().then_some(schema),
            supported,
        },
        mode: loaded.as_ref().map(|loaded| loaded.config.mode.as_str()),
        project: project.display().to_string(),
        policy,
        agents,
        plans,
        problems,
        guidance,
    })
}

fn policy_guidance(loaded: &config::LoadedConfig, guidance: &mut Vec<String>) {
    if !loaded.snapshots.iter().any(|snapshot| snapshot.trusted) {
        guidance.push("No trusted policy applies here; review yoke.toml (`yoke init` writes a starter) and run `yoke trust` in your own terminal.".into());
    }
    for snapshot in loaded.snapshots.iter().filter(|snapshot| snapshot.stale) {
        guidance.push(format!(
            "{} changed after it was trusted; Yoke keeps using the trusted bytes until you review it and run `yoke trust`.",
            snapshot.file.display()
        ));
    }
    if loaded.config.mode == Mode::Observe {
        guidance.push("Observe mode records what would be denied but blocks nothing; review `yoke log --outcome would_deny`, then set mode = \"enforce\" in yoke.toml and run `yoke trust` in your own terminal.".into());
    }
}

fn agent_reports(
    home: &Path,
    executable: std::path::PathBuf,
    project: &Path,
    store: &Store,
    readable: bool,
) -> Result<Vec<AgentReport>> {
    let options = install::options(home, executable, Vec::new())?;
    install::status(&options, project)
        .into_iter()
        .map(|install| {
            let last_event = if readable {
                store
                    .last_decision(install.agent)?
                    .map(|(ts, event, mode, outcome)| LastEvent {
                        ts,
                        event,
                        mode,
                        outcome,
                    })
            } else {
                None
            };
            Ok(AgentReport {
                install,
                last_event,
            })
        })
        .collect()
}

fn print_text(report: &Report) {
    let quote = |value: &str| commands::quote(value);
    println!("health: {}", report.status);
    println!("executable: {}", quote(&report.executable));
    println!(
        "state: {} schema={} supported={}",
        quote(&report.state.path),
        report
            .state
            .schema
            .map_or_else(|| "missing".to_owned(), |schema| schema.to_string()),
        report.state.supported
    );
    println!("mode: {}", report.mode.unwrap_or("unknown"));
    println!("project: {}", quote(&report.project));
    println!("policy[{}]{{file,trusted,stale}}:", report.policy.len());
    for policy in &report.policy {
        println!(
            "  {},{},{}",
            quote(&policy.file),
            policy.trusted,
            policy.stale
        );
    }
    println!(
        "agents[{}]{{name,installed,settings,last_event,last_mode,last_outcome}}:",
        report.agents.len()
    );
    for agent in &report.agents {
        let last = agent.last_event.as_ref();
        println!(
            "  {},{},{},{},{},{}",
            agent.install.agent,
            agent.install.installed,
            quote(&agent.install.settings.display().to_string()),
            last.map_or_else(|| "none".to_owned(), |last| last.ts.to_string()),
            last.map_or("none", |last| last.mode.as_str()),
            last.map_or("none", |last| last.outcome.as_str())
        );
    }
    match &report.plans {
        Some(plans) => {
            println!("plans[{}]{{slug,state}}:", plans.len());
            for plan in plans {
                println!("  {},{}", quote(&plan.slug), plan.state);
            }
        }
        None => println!("plans: unknown"),
    }
    for (name, lines) in [
        ("problems", &report.problems),
        ("guidance", &report.guidance),
    ] {
        println!("{name}[{}]:", lines.len());
        for line in lines {
            println!("  {}", quote(line));
        }
    }
    println!(
        "help: \"Run yoke health --json for structured output; it exits 1 when a check fails.\""
    );
}
