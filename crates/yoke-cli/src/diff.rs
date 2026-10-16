//! Read-only view of the exact change text a plan's independent review receives.

use std::path::Path;

use anyhow::Result;
use yoke_core::store::Store;
use yoke_core::{plan, review};

use crate::commands;

const UNTRACKED_MARKER: &str = "\nUNTRACKED FILE DATA: ";

pub fn run(store: &Store, project: &Path, slug: &str, json: bool) -> Result<()> {
    let record = plan::get(store, project, slug)?;
    let base = record.base_ref.as_deref();
    let diff = review::input(Path::new(&record.project), base)?;
    let untracked = diff.matches(UNTRACKED_MARKER).count();
    let base = base.unwrap_or("HEAD");
    if json {
        println!(
            "{}",
            serde_json::json!({
                "plan": slug,
                "state": record.state,
                "base": base,
                "bytes": diff.len(),
                "untracked_files": untracked,
                "diff": diff,
            })
        );
        return Ok(());
    }
    println!(
        "plan: {}\nstate: {}\nbase: {}\nbytes: {}\nuntracked_files: {untracked}",
        commands::quote(slug),
        record.state,
        commands::quote(base),
        diff.len()
    );
    if diff.is_empty() {
        println!(
            "diff: \"No tracked or untracked changes since base; review would see an empty diff.\""
        );
        return Ok(());
    }
    // Quoted lines keep terminal control sequences in reviewed files inert.
    let lines: Vec<&str> = diff.lines().filter(|line| !line.is_empty()).collect();
    println!("lines[{}]:", lines.len());
    for line in lines {
        println!("  {}", commands::quote(line));
    }
    println!("help: \"Run yoke plan diff {slug} --json for the exact unquoted review input.\"");
    Ok(())
}
