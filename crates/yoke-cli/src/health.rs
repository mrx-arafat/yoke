//! `yoke health`: a read-only report of installation and protection state with next steps.
//! It never creates, migrates or writes state, settings or receipts; activating trust or
//! enforcement stays a human action in the user's own terminal.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;
use yoke_core::config::{self, Mode};
use yoke_core::store::{MissingSidecars, Store};

use crate::{commands, hook, install};

#[derive(Serialize)]
struct Report {
    status: &'static str,
    /// What `status` means, so `attention` is not mistaken for a fault.
    meaning: &'static str,
    executable: String,
    state: State,
    mode: Option<&'static str>,
    project: String,
    policy: Vec<Policy>,
    agents: Vec<AgentReport>,
    plans: Option<Vec<PlanReport>>,
    memory: Option<MemoryReport>,
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
    /// Whether the file is on disk; an absent file is not something to review.
    exists: bool,
}

#[derive(Serialize)]
struct AgentReport {
    #[serde(flatten)]
    install: install::AgentStatus,
    last_event: Option<LastEvent>,
    /// Whether `last_event` was read; `None` then means no event was recorded.
    #[serde(skip)]
    event_readable: bool,
}

#[derive(Serialize)]
struct LastEvent {
    ts: i64,
    event: String,
    mode: String,
    outcome: String,
}

#[derive(Serialize)]
struct MemoryReport {
    /// Facts of this project that no digest has turned into memories yet.
    pending_facts: i64,
    digest: bool,
    note: Option<&'static str>,
}

#[derive(Serialize)]
struct PlanReport {
    slug: String,
    state: String,
}

/// Everything the report reads from the environment, so tests build it from a temp home.
struct Inputs {
    home: PathBuf,
    cwd: PathBuf,
    executable: PathBuf,
    /// The settings files each agent reads, or why they cannot be located.
    options: Result<install::InstallOptions>,
}

/// Print the report; exit 1 when a check fails so scripts can gate on it.
pub fn run(json_output: bool) -> Result<()> {
    let home = commands::home()?;
    let executable = std::env::current_exe()?;
    let report = collect(Inputs {
        options: install::options(&home, executable.clone(), Vec::new()),
        cwd: std::env::current_dir()?,
        home,
        executable,
    })?;
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

/// The first problem `yoke health` would report for the current directory, for the statusline's
/// background check. It reads exactly what `run` reads and writes nothing.
pub(crate) fn first_problem() -> Result<Option<String>> {
    let home = commands::home()?;
    let executable = std::env::current_exe()?;
    let report = collect(Inputs {
        options: install::options(&home, executable.clone(), Vec::new()),
        cwd: std::env::current_dir()?,
        home,
        executable,
    })?;
    Ok(report.problems.into_iter().next())
}

fn collect(inputs: Inputs) -> Result<Report> {
    let Inputs {
        home,
        cwd,
        executable,
        options,
    } = inputs;
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
    // `file_schema` is what the file says even when it is unusable; `readable` means the store
    // answers queries, so trust, decisions and plans are only read from a readable store.
    let (store, readable, file_schema) = match opened {
        Ok(store) => match store.schema_version()?.0 {
            // A file with no committed schema (for example zero bytes after a crash) has no
            // tables; say so once instead of failing every query against it.
            0 if path.exists() => {
                problems.push(format!(
                    "the Yoke state database {} is empty or incomplete; run yoke config from your own terminal, or start your agent, to initialise it",
                    path.display()
                ));
                (Store::open(":memory:")?, false, Some(0))
            }
            schema => (store, true, path.exists().then_some(schema)),
        },
        Err(error) => {
            let detail = format!("{error:#}");
            // Missing WAL files already say how to get them back; a disk hint would mislead.
            let hint = if error.downcast_ref::<MissingSidecars>().is_some() {
                String::new()
            } else {
                format!(
                    "; {}",
                    hook::newer_state_hint(&detail)
                        .unwrap_or("check free disk space and file permissions")
                )
            };
            problems.push(format!("cannot read {}: {detail}{hint}", path.display()));
            (Store::open(":memory:")?, false, None)
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
                exists: snapshot.exists,
            })
            .collect();
        if readable {
            policy_guidance(loaded, &mut guidance);
        }
    }
    let agents = match &options {
        Ok(options) => agent_reports(options, &project, &store, readable, &mut problems),
        Err(error) => {
            problems.push(format!("cannot locate agent settings: {error:#}"));
            Vec::new()
        }
    };
    // The ways to switch Yoke off lead the list, so the statusline shows them first.
    problems.splice(
        0..0,
        crate::bypass::switch_problems(&home, options.as_ref().ok(), &project),
    );
    for agent in &agents {
        let name = agent.install.agent;
        // An unreadable receipt is a problem to fix, not an absent installation to repeat:
        // `yoke install` refuses it as well.
        problems.extend(
            agent
                .install
                .problems
                .iter()
                .map(|problem| format!("{name}: {problem}")),
        );
        if !agent.install.installed {
            if agent.install.problems.is_empty() {
                guidance.push(format!(
                    "{name} is not protected; run `yoke install --agent {name}`."
                ));
            }
            continue;
        }
        if agent.event_readable && agent.last_event.is_none() {
            guidance.push(format!(
                "No {name} hook event is recorded yet; start {name} in a project to confirm its hooks run.{}",
                if name == "codex" {
                    " Codex skips hooks you have not approved, with no warning, including in `codex exec`."
                } else {
                    ""
                }
            ));
        }
        if name == "claude" && !agent.install.statusline {
            guidance.push("Optional: Claude Code's status bar does not show Yoke's line; `yoke install --agent claude --statusline` adds it (a statusline you already have is wrapped, with its colors removed and its lines joined) and shows Yoke's mode, plan, alerts and your quota, and `yoke usage` prints the same quota without it.".into());
        }
        if name == "codex" {
            guidance.extend(agent.install.notes.iter().cloned());
        }
    }
    if !agents.is_empty()
        && agents
            .iter()
            .all(|agent| !agent.install.installed && agent.install.problems.is_empty())
    {
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
    if readable && current {
        let silent = project.to_str().and_then(|project| {
            let (silent, newer) = (crate::bypass::SILENT_SECS, 12 * 3600);
            store
                .silent_sessions(project, now_secs(), (silent, newer))
                .ok()
        });
        for (id, agent, seconds) in silent.into_iter().flatten() {
            guidance.push(format!(
                "Session {id} ({agent}) is open but recorded no hook decision for {}; if its agent is still running, its hooks may be switched off (check env.YOKE_OFF and disableAllHooks in the settings above, and yoke pause).",
                yoke_core::timefmt::duration_text(seconds)
            ));
        }
    }
    if readable && current {
        // The hourly cleanup is a background job; a failing one means history keeps piling up.
        let latest = yoke_core::jobs::list(&store, true, 1000)
            .ok()
            .and_then(|jobs| {
                jobs.into_iter()
                    .find(|job| job.kind == crate::retention::RETENTION_JOB)
            });
        if let Some(job) = latest.filter(|job| matches!(job.state.as_str(), "failed" | "dead")) {
            guidance.push(format!(
                "Retention cleanup is {} (job {}: {}), so old history is not being removed; see `yoke jobs`.",
                if job.state == "dead" { "failing" } else { "retrying after a failure" },
                job.id,
                job.last_error.as_deref().unwrap_or("no error recorded")
            ));
        }
    }
    match &plans {
        Some(plans) if plans.is_empty() => guidance.push("No unfinished plan in this project, so Stop is not held for plan checks; start one with `yoke plan new <slug>`.".into()),
        Some(plans) => guidance.extend(plans.iter().filter_map(plan_step)),
        None => {}
    }
    // Facts need the memory tables, which a state older than the current schema may lack.
    let memory = loaded
        .as_ref()
        .zip(project.to_str())
        .filter(|_| readable && current)
        .and_then(|(loaded, project)| match store.pending_fact_count(project) {
            Ok(pending_facts) => Some(MemoryReport {
                pending_facts,
                digest: loaded.config.memory.digest,
                note: (pending_facts > 0 && !loaded.config.memory.digest).then_some(
                    "Facts become memories only when a human sets digest = true under [memory] in yoke.toml or ~/.yoke/config.toml and runs yoke trust.",
                ),
            }),
            Err(error) => {
                problems.push(format!("cannot count pending memory facts: {error:#}"));
                None
            }
        });
    let (status, meaning) = if !problems.is_empty() {
        (
            "failing",
            "A check failed; fix the problems below. This command exits 1.",
        )
    } else if !guidance.is_empty() {
        ("attention", "Setup is unfinished or a step is waiting for you; nothing is broken. The guidance below lists the next steps.")
    } else {
        ("ok", "Everything Yoke checks is in place.")
    };
    Ok(Report {
        status,
        meaning,
        executable: executable.display().to_string(),
        state: State {
            path: path.display().to_string(),
            exists: path.exists(),
            schema: file_schema,
            supported,
        },
        mode: loaded.as_ref().map(|loaded| loaded.config.mode.as_str()),
        project: project.display().to_string(),
        policy,
        agents,
        plans,
        memory,
        problems,
        guidance,
    })
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

fn policy_guidance(loaded: &config::LoadedConfig, guidance: &mut Vec<String>) {
    // Only files that exist are work for the user; the project and knowledge files usually do not.
    let mut waiting = false;
    for snapshot in loaded
        .snapshots
        .iter()
        .filter(|snapshot| !snapshot.trusted && snapshot.exists)
    {
        waiting = true;
        let trust = if snapshot.scope == "global" {
            "yoke trust --global"
        } else {
            "yoke trust"
        };
        guidance.push(format!(
            "{} is not trusted, so Yoke ignores it; review it and run `{trust}` in your own terminal.",
            snapshot.file.display()
        ));
    }
    if !waiting && !loaded.snapshots.iter().any(|snapshot| snapshot.trusted) {
        guidance.push("No policy exists yet: run `yoke init` in a project to write a starter yoke.toml, then `yoke trust`; or run `yoke preset add git-safety --global`, then `yoke trust --global`, to cover every project.".into());
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

/// The next step for an unfinished plan, in the words of `yoke plan` itself.
fn plan_step(plan: &PlanReport) -> Option<String> {
    let slug = &plan.slug;
    Some(match plan.state.as_str() {
        "draft" => format!("Plan {slug} is a draft: a human approves it with `yoke plan approve {slug}` in their own terminal, then the builder runs `yoke plan start {slug} --session <session-id>`."),
        "approved" => format!("Plan {slug} is approved: the builder runs `yoke plan start {slug} --session <session-id>`."),
        "building" => format!("Plan {slug} is being built: the builder runs `yoke plan check {slug} --agent <claude|codex>` when the work is ready."),
        "waiting" | "stalled" => format!("Plan {slug} is {}: a human resumes it with `yoke plan resume {slug}` in their own terminal.", plan.state),
        "checking" => format!("Plan {slug} is being checked: if the check process is gone, a human runs `yoke plan resume {slug}`."),
        _ => return None,
    })
}

/// Installation state never depends on the database: a failed decision lookup (for example a
/// locked database) is reported for that agent, and the agent stays in the report.
fn agent_reports(
    options: &install::InstallOptions,
    project: &Path,
    store: &Store,
    readable: bool,
    problems: &mut Vec<String>,
) -> Vec<AgentReport> {
    install::status(options, project)
        .into_iter()
        .map(|install| {
            let (last_event, event_readable) =
                match readable.then(|| store.last_decision(install.agent)) {
                    None => (None, false),
                    Some(Ok(last)) => (
                        last.map(|(ts, event, mode, outcome)| LastEvent {
                            ts,
                            event,
                            mode,
                            outcome,
                        }),
                        true,
                    ),
                    Some(Err(error)) => {
                        problems.push(format!(
                            "cannot read the last {} decision: {error:#}",
                            install.agent
                        ));
                        (None, false)
                    }
                };
            AgentReport {
                install,
                last_event,
                event_readable,
            }
        })
        .collect()
}

fn print_text(report: &Report) {
    let quote = |value: &str| commands::quote(value);
    println!("health: {}", report.status);
    println!("meaning: {}", quote(report.meaning));
    println!("executable: {}", quote(&report.executable));
    println!(
        "state: {} schema={} supported={}",
        quote(&report.state.path),
        report.state.schema.map_or_else(
            // An existing file whose schema could not be read is unknown, not missing.
            || if report.state.exists {
                "unknown"
            } else {
                "missing"
            }
            .to_owned(),
            |schema| schema.to_string()
        ),
        report.state.supported
    );
    println!("mode: {}", report.mode.unwrap_or("unknown"));
    println!("project: {}", quote(&report.project));
    println!(
        "policy[{}]{{file,trusted,stale,exists}}:",
        report.policy.len()
    );
    for policy in &report.policy {
        println!(
            "  {},{},{},{}",
            quote(&policy.file),
            policy.trusted,
            policy.stale,
            policy.exists
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
            last.map_or_else(
                || "none".to_owned(),
                |last| quote(&yoke_core::timefmt::local_time(last.ts))
            ),
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
    match &report.memory {
        Some(memory) => {
            println!(
                "memory: pending_facts={} digest={}",
                memory.pending_facts,
                if memory.digest { "on" } else { "off" }
            );
            if let Some(note) = memory.note {
                println!("  note: {}", quote(note));
            }
        }
        None => println!("memory: unknown"),
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

#[cfg(test)]
#[path = "health/tests.rs"]
mod tests;
