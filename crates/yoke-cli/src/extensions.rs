//! `yoke extensions`: read-only inventory of agent extensions and configuration drift.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use anyhow::Result;
use serde::Serialize;
use yoke_core::extensions::{self, Drift, DriftKind, Kind, Owner, Report, Roots};
use yoke_core::redact::redact;
use yoke_core::trust;

use crate::commands::{self, quote};
use crate::install;

/// Print the inventory; exit 1 when a stale or dangling definition is found so scripts can
/// gate on it. Shadowed names are reported without failing (see `Drift::fails_check`).
pub fn run(json: bool) -> Result<()> {
    let home = commands::home()?;
    let project = trust::project_root(&commands::inspect_store(&home)?, &std::env::current_dir()?)?;
    let mut report = extensions::scan(&Roots::detect(&home, &project));
    managed_hooks(&home, &mut report)?;
    if json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        print(&report);
    }
    if report.drift.iter().any(Drift::fails_check) {
        std::io::stdout().flush()?;
        std::process::exit(1);
    }
    Ok(())
}

/// Mark receipted hooks as Yoke-managed and report those a fresh install would change.
fn managed_hooks(home: &Path, report: &mut Report) -> Result<()> {
    let options = install::InstallOptions {
        dry_run: true,
        ..install::options(home, std::env::current_exe()?, Vec::new())?
    };
    for (agent, owner, name) in [
        (install::Agent::Claude, Owner::Claude, "claude"),
        (install::Agent::Codex, Owner::Codex, "codex"),
    ] {
        let managed = match install::drift::managed(&options, agent) {
            Ok(Some(managed)) => managed,
            Ok(None) => continue,
            Err(error) => {
                report.drift.push(Drift {
                    kind: DriftKind::Stale,
                    agent: owner,
                    name: "receipt".into(),
                    detail: redact(&format!("installation receipt unusable: {error:#}")),
                    origins: vec![options.state_dir.join(format!("install-{name}.json"))],
                });
                continue;
            }
        };
        for item in &mut report.items {
            item.managed |= item.agent == owner
                && item.kind == Kind::Hook
                && item.origin == managed.settings
                && item
                    .command
                    .as_ref()
                    .is_some_and(|command| managed.commands.contains(command));
        }
        for (name, detail) in managed.stale {
            report.drift.push(Drift {
                kind: DriftKind::Stale,
                agent: owner,
                name,
                detail,
                origins: vec![managed.settings.clone()],
            });
        }
    }
    Ok(())
}

fn tag(value: impl Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// One line of counts, so the reader knows the size and the verdict before the long list.
fn summary(report: &Report) -> String {
    let mut agents = BTreeMap::new();
    for item in &report.items {
        *agents.entry(item.agent).or_insert(0usize) += 1;
    }
    let counted = |counts: Vec<String>| {
        if counts.is_empty() {
            String::new()
        } else {
            format!(" ({})", counts.join(", "))
        }
    };
    let by_agent = counted(
        agents
            .iter()
            .map(|(agent, count)| format!("{} {count}", tag(*agent)))
            .collect(),
    );
    let by_kind = counted(
        [DriftKind::Shadowed, DriftKind::Stale, DriftKind::Dangling]
            .into_iter()
            .filter_map(|kind| {
                let count = report
                    .drift
                    .iter()
                    .filter(|drift| drift.kind == kind)
                    .count();
                (count > 0).then(|| format!("{} {count}", tag(kind)))
            })
            .collect(),
    );
    let mut line = format!(
        "definitions {}{by_agent}, drift {}{by_kind}",
        report.items.len(),
        report.drift.len()
    );
    let links = report
        .notes
        .iter()
        .filter(|note| note.is_outside_link())
        .count();
    if links > 0 {
        line.push_str(&format!(", symlinks outside the scanned roots {links}"));
    }
    line
}

/// Text view: summary, then drift (the part to act on), then the inventory, then the notes
/// that mean a file was not read. Links outside the scanned roots are only counted here;
/// `--json` lists each target.
fn print(report: &Report) {
    println!("summary: {}", summary(report));
    println!("drift[{}]{{kind,agent,name,detail}}:", report.drift.len());
    for drift in &report.drift {
        println!(
            "  {},{},{},{}",
            tag(drift.kind),
            tag(drift.agent),
            quote(&drift.name),
            quote(&drift.detail)
        );
    }
    println!(
        "extensions[{}]{{agent,kind,name,scope,managed,origin,detail}}:",
        report.items.len()
    );
    for item in &report.items {
        println!(
            "  {},{},{},{},{},{},{}",
            tag(item.agent),
            tag(item.kind),
            quote(&item.name),
            tag(item.scope),
            item.managed,
            quote(&redact(&item.origin.display().to_string())),
            quote(&item.detail)
        );
    }
    let notes: Vec<_> = report
        .notes
        .iter()
        .filter(|note| !note.is_outside_link())
        .collect();
    if !notes.is_empty() {
        println!("notes[{}]{{path,note}}:", notes.len());
        for note in notes {
            println!(
                "  {},{}",
                quote(&redact(&note.path.display().to_string())),
                quote(&redact(&note.note))
            );
        }
    }
    let help = if report.drift.is_empty() {
        "No drift. Run yoke extensions --json for redacted details, symlink targets and environment key names."
    } else if report.drift.iter().any(Drift::fails_check) {
        "Drift found (exit 1). Run yoke extensions --json for every origin; nothing was changed."
    } else {
        "Only shadowed definitions found (exit 0); each row names the winner where the agent documents one. Run yoke extensions --json for every origin; nothing was changed."
    };
    println!("help: {}", quote(help));
}
