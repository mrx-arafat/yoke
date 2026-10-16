//! `yoke log`: the newest recorded decisions as a readable table, or complete records as JSON.

use std::path::Path;

use anyhow::{bail, Result};
use serde_json::Value;
use yoke_core::store::{DecisionFilter, DecisionRecord, Store};
use yoke_core::{timefmt, trust};

use crate::commands::{home, open_store, quote};

const OUTCOMES: [&str; 5] = ["allow", "deny", "would_deny", "rewrite", "error"];
const DETAIL_WIDTH: usize = 120;
/// Tool input fields that say what a tool call acted on, most specific first.
const TOOL_TARGETS: [&str; 5] = ["command", "cmd", "file_path", "path", "notebook_path"];

pub fn run(
    project: Option<String>,
    session: Option<String>,
    outcome: Option<String>,
    tool: Option<String>,
    limit: u16,
    json: bool,
) -> Result<()> {
    if let Some(outcome) = outcome.as_deref().filter(|o| !OUTCOMES.contains(o)) {
        bail!(
            "unknown outcome '{outcome}'; use one of {}",
            OUTCOMES.join(", ")
        );
    }
    let store = open_store(&home()?)?;
    let filtered = project.is_some() || session.is_some() || outcome.is_some() || tool.is_some();
    let records = store.decisions(&DecisionFilter {
        project: project
            .map(|path| resolve_project(&store, &path))
            .transpose()?,
        session,
        outcome,
        tool,
        limit: usize::from(limit),
    })?;
    if json {
        let mut rows = serde_json::to_value(&records)?;
        for (row, record) in rows.as_array_mut().into_iter().flatten().zip(&records) {
            row["payload_truncated"] = Value::Bool(is_truncated(&record.payload));
        }
        println!("{rows}");
        return Ok(());
    }
    // Rows from several projects are told apart; a single project would only repeat itself.
    let projects = records.iter().any(|row| row.project != records[0].project);
    let columns = if projects {
        "time,agent,project,event,tool,outcome,gate,detail"
    } else {
        "time,agent,event,tool,outcome,gate,detail"
    };
    println!("decisions[{}]{{{columns}}}:", records.len());
    for row in &records {
        let mut fields = vec![quote(&timefmt::local_time_secs(row.ts)), quote(&row.agent)];
        if projects {
            fields.push(quote(&row.project));
        }
        fields.extend([
            quote(&row.event),
            quote(row.tool.as_deref().unwrap_or("")),
            quote(&row.outcome),
            quote(&gate_names(row)),
            quote(&detail(row)),
        ]);
        println!("  {}", fields.join(","));
    }
    if records.is_empty() && filtered {
        println!("help: \"No decisions match: --tool and --session are exact, case-sensitive matches, and --project takes any path inside a project.\"");
    } else {
        println!("help: \"Times are local. Run yoke log --json for stored records (payloads cut at log.payload_cap are flagged payload_truncated); --limit is bounded to 1000.\"");
    }
    Ok(())
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

/// What the tool acted on, then the reason when there is one, on a single short line.
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
        (false, false) => format!("{target} - {reason}"),
        (false, true) => target,
        (true, _) => reason.to_owned(),
    };
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > DETAIL_WIDTH {
        format!(
            "{}...",
            line.chars().take(DETAIL_WIDTH - 3).collect::<String>()
        )
    } else {
        line
    }
}
