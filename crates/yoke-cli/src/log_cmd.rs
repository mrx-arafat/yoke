//! `yoke log`: the newest recorded decisions as a readable table, or complete records as JSON.

use std::path::Path;

use anyhow::{bail, Context, Result};
use serde_json::Value;
use yoke_core::store::{DecisionFilter, DecisionRecord, Store};
use yoke_core::{config, timefmt, trust};

use crate::commands::{home, open_store, quote};
use crate::table;

const OUTCOMES: [&str; 5] = ["allow", "deny", "would_deny", "rewrite", "error"];
/// Tool input fields that say what a tool call acted on, most specific first.
const TOOL_TARGETS: [&str; 5] = ["command", "cmd", "file_path", "path", "notebook_path"];

pub struct Args {
    pub project: Option<String>,
    pub all: bool,
    pub session: Option<String>,
    pub outcome: Option<String>,
    pub tool: Option<String>,
    pub limit: u16,
    pub json: bool,
}

/// Whose decisions a listing covers.
enum Scope {
    All,
    /// The project asked for with `--project`, or the current one when `explicit` is false.
    Project {
        root: String,
        explicit: bool,
    },
}

pub fn run(args: Args) -> Result<()> {
    if let Some(outcome) = args.outcome.as_deref().filter(|o| !OUTCOMES.contains(o)) {
        bail!(
            "unknown outcome '{outcome}'; use one of {}",
            OUTCOMES.join(", ")
        );
    }
    let store = open_store(&home()?)?;
    let scope = match (&args.project, args.all) {
        (Some(path), _) => Scope::Project {
            root: resolve_project(&store, path)?,
            explicit: true,
        },
        // A session id names one session, so it is looked up everywhere unless a project is given.
        (None, true) => Scope::All,
        (None, false) if args.session.is_some() => Scope::All,
        (None, false) => Scope::Project {
            root: current_project(&store)?,
            explicit: false,
        },
    };
    let records = store.decisions(&DecisionFilter {
        project: match &scope {
            Scope::All => None,
            Scope::Project { root, .. } => Some(root.clone()),
        },
        session: args.session.clone(),
        outcome: args.outcome.clone(),
        tool: args.tool.clone(),
        limit: usize::from(args.limit),
    })?;
    if args.json {
        let mut rows = serde_json::to_value(&records)?;
        for (row, record) in rows.as_array_mut().into_iter().flatten().zip(&records) {
            row["payload_truncated"] = Value::Bool(is_truncated(&record.payload));
        }
        println!("{rows}");
        return Ok(());
    }
    match &scope {
        Scope::All => println!("project: all"),
        Scope::Project { root, .. } => println!("project: {}", quote(root)),
    }
    // Rows from several projects are told apart; a single project would only repeat itself.
    let projects = records.iter().any(|row| row.project != records[0].project);
    let mut columns = vec![("TIME", 19, 19), ("AGENT", 6, 6), ("SESSION", 7, 16)];
    if projects {
        columns.push(("PROJECT", 7, 24));
    }
    columns.extend([
        ("EVENT", 5, 18),
        ("TOOL", 4, 12),
        ("OUTCOME", 10, 10),
        ("GATE", 4, 20),
        ("DETAIL", 24, 180),
    ]);
    let mut rows = Vec::new();
    for row in &records {
        let mut fields = vec![
            timefmt::local_time_secs(row.ts),
            row.agent.clone(),
            row.session_id.clone(),
        ];
        if projects {
            fields.push(row.project.clone());
        }
        fields.extend([
            row.event.clone(),
            row.tool.clone().unwrap_or_default(),
            row.outcome.clone(),
            gate_names(row),
            detail(row),
        ]);
        rows.push(fields);
    }
    print!("{}", table::render("decisions", &columns, &rows));
    println!(
        "help: {}",
        quote(&help(&store, &scope, &args, records.len()))
    );
    Ok(())
}

/// The project of the working directory, found the way every other command finds it.
fn current_project(store: &Store) -> Result<String> {
    std::env::current_dir()
        .map_err(anyhow::Error::from)
        .and_then(|cwd| trust::project_root(store, &cwd))
        .map(|root| root.display().to_string())
        .context("find the current project; use yoke log --all or --project <path>")
}

/// What to do next: how to widen the scope, and why a listing is empty.
fn help(store: &Store, scope: &Scope, args: &Args, rows: usize) -> String {
    let widen = match scope {
        Scope::Project {
            explicit: false, ..
        } => " Only the current project is shown; yoke log --all shows every project.",
        _ => "",
    };
    if rows > 0 {
        return format!("Times are local. Run yoke log --json for stored records (payloads cut at log.payload_cap are flagged payload_truncated); --limit is bounded to 1000.{widen}");
    }
    if let Some(note) = no_gate_note(store, scope, args) {
        return format!("{note}{widen}");
    }
    let filtered = args.session.is_some() || args.outcome.is_some() || args.tool.is_some();
    match (filtered, scope) {
        (true, _) => format!("No decisions match: --tool and --session are exact, case-sensitive matches, and --project takes any path inside a project.{widen}"),
        (false, Scope::All) => "No decisions recorded yet; run yoke health to check that your agents' hooks are installed.".into(),
        (false, Scope::Project { .. }) => format!("No decisions recorded for this project yet; run yoke health to check that your agents' hooks are installed.{widen}"),
    }
}

/// An empty `deny` or `would_deny` listing is read as "nothing was blocked"; when the project has
/// no trusted gate, the real reason is that none has run.
fn no_gate_note(store: &Store, scope: &Scope, args: &Args) -> Option<String> {
    let outcome = args
        .outcome
        .as_deref()
        .filter(|o| matches!(*o, "deny" | "would_deny"))?;
    let Scope::Project { root, .. } = scope else {
        return None;
    };
    if args.session.is_some() || args.tool.is_some() || !Path::new(root).is_dir() {
        return None;
    }
    let loaded = config::load_trusted(store, Path::new(root), &home().ok()?).ok()?;
    if !loaded.config.gates.is_empty() {
        return None;
    }
    let untrusted: Vec<String> = loaded
        .snapshots
        .iter()
        .filter(|snapshot| snapshot.exists && !snapshot.trusted)
        .map(|snapshot| snapshot.file.display().to_string())
        .collect();
    Some(if untrusted.is_empty() {
        format!("No {outcome} decisions, and no gate has run: this project has no trusted gates. Add some with yoke preset add <name> (yoke preset list shows the bundled ones), then a human runs yoke trust in their own terminal.")
    } else {
        format!("No {outcome} decisions, and no gate has run: {} is not trusted, so its gates are ignored. Review it and run yoke trust in your own terminal.", untrusted.join(", "))
    })
}

/// The project that contains `path`, found the way every other command finds it; a path that is
/// not a directory (a project since removed) is matched as written.
fn resolve_project(store: &Store, path: &str) -> Result<String> {
    let path = Path::new(path);
    if path.is_dir() {
        Ok(trust::project_root(store, path)?.display().to_string())
    } else {
        Ok(path.display().to_string())
    }
}

/// A payload cut by `log.payload_cap` is no longer valid JSON.
fn is_truncated(payload: &str) -> bool {
    !payload.is_empty() && serde_json::from_str::<Value>(payload).is_err()
}

/// Gates that did more than pass, for example `no-force-push` or `watchdog(timeout)`.
fn gate_names(row: &DecisionRecord) -> String {
    row.gates
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|gate| {
            let (name, result) = (gate["name"].as_str()?, gate["result"].as_str()?);
            match result {
                "pass" | "skip" => None,
                "fail" | "deny" | "rewrite" => Some(name.to_owned()),
                other => Some(format!("{name}({other})")),
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// Put failures first so a long command cannot hide the reason in a bounded preview.
fn detail(row: &DecisionRecord) -> String {
    let target = serde_json::from_str::<Value>(&row.payload)
        .ok()
        .and_then(|payload| {
            TOOL_TARGETS
                .iter()
                .find_map(|key| payload["tool_input"][*key].as_str().map(str::to_owned))
        })
        .unwrap_or_default();
    let reason = row.reason.as_deref().unwrap_or("");
    let text = match (target.is_empty(), reason.is_empty()) {
        (false, false) => format!("{reason} - {target}"),
        (false, true) => target,
        (true, _) => reason.to_owned(),
    };
    text
}
