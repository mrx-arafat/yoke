//! Human-only project record relocation; directories are moved separately by their owner.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Subcommand;
use yoke_core::{project, store::Store};

use crate::commands;

#[derive(Subcommand)]
pub enum ProjectCommand {
    /// Move a project's stored records after moving its directory; never approves changed policy.
    Move {
        /// Exact absolute project path recorded before the directory moved.
        old: PathBuf,
        /// Existing new project directory.
        new: PathBuf,
        /// Print every affected table and row count without changing records.
        #[arg(long)]
        dry_run: bool,
        /// Explicitly combine histories when the target already has records; conflicting plan slugs still refuse.
        #[arg(long)]
        merge: bool,
        /// Print the table counts and trust result as JSON.
        #[arg(long)]
        json: bool,
    },
}

pub fn run(command: ProjectCommand) -> Result<()> {
    commands::require_human("project move")?;
    match command {
        ProjectCommand::Move {
            old,
            new,
            dry_run,
            merge,
            json,
        } => {
            let home = commands::home()?;
            let store = if dry_run {
                Store::open_read_only(home.join(".yoke/state.db")).context("no readable project records; start Yoke in the project before moving its records")?
            } else {
                commands::open_store(&home)?
            };
            let report = project::move_records(&store, &old, &new, merge, dry_run)?;
            if json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                println!(
                    "dry_run: {dry_run}\nold: {}\nnew: {}",
                    commands::quote(&old.display().to_string()),
                    commands::quote(&new.display().to_string())
                );
                println!("tables[{}]{{name,rows}}:", report.tables.len());
                for (table, count) in &report.tables {
                    println!("  {table},{count}");
                }
                println!(
                    "trust_moved: {}\ntrust_dropped: {}",
                    report.trust_moved, report.trust_dropped
                );
                if report.trust_moved == 0 {
                    println!("trust: untrusted\nhelp: \"Review the new project's files, then run yoke trust in your own terminal.\"");
                }
                println!("help: \"Keep agents stopped during the move. Files were not moved. Restart sessions in the new directory; rerun or reapprove plan checks if they refer to the old path.\"");
            }
        }
    }
    Ok(())
}
