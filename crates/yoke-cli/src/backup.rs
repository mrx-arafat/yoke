//! `yoke backup restore`: replace the state database with a backup made in the console.
//! Human-only: an agent tool process or piped input is refused, the backup is shown first and
//! the human types `restore` to confirm. A safety backup of the current state is written first.

use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Subcommand;
use yoke_core::backup;

use crate::commands::{self, quote};

#[derive(Subcommand)]
pub enum BackupCommand {
    /// Replace the state database with a backup after writing a safety backup. Requires a human terminal.
    Restore {
        /// Backup file, or the name of one in ~/.yoke/state-backups.
        file: PathBuf,
    },
}

/// A bare backup name means the file of that name in the backup folder.
fn source(directory: &Path, file: &Path) -> PathBuf {
    match file.to_str() {
        Some(name) if backup::is_backup_name(name) => directory.join(name),
        _ => file.to_owned(),
    }
}

pub fn run(command: BackupCommand) -> Result<()> {
    let BackupCommand::Restore { file } = command;
    commands::require_human("backup restore")?;
    let home = commands::home()?;
    let directory = backup::directory(&home);
    let source = source(&directory, &file);
    let inspection = backup::inspect(&source)?;
    backup::restorable(&inspection)?;
    let live = home.join(".yoke/state.db");
    println!("backup: {}", quote(&source.display().to_string()));
    println!(
        "created: {}",
        quote(&yoke_core::timefmt::local_time_secs(
            inspection.file.created
        ))
    );
    println!(
        "schema: {} (this yoke opens 1 to {})",
        inspection.schema, inspection.supported
    );
    println!("integrity: {}", inspection.integrity);
    println!("sha256: {}", inspection.sha256);
    println!("tables[{}]{{name,rows}}:", inspection.tables.len());
    for (table, rows) in &inspection.tables {
        println!("  {table},{rows}");
    }
    println!("live: {}", quote(&live.display().to_string()));
    println!(
        "safety_backup_folder: {}",
        quote(&directory.display().to_string())
    );
    println!("note: {}", quote("Everything in the live database is replaced by this backup: decisions, sessions, memories, plans, jobs and the records of trusted policy. A safety backup of the current state is written first. Stop running agents before you confirm."));
    if !commands::confirm("restore", false)? {
        println!("restored: false\nhelp: \"Nothing changed.\"");
        return Ok(());
    }
    let restored = backup::restore(&live, &source, &directory, crate::memory::timestamp()?)?;
    println!("restored: true");
    println!(
        "safety_backup: {}",
        quote(&restored.safety.path.display().to_string())
    );
    println!("schema: {}", restored.schema);
    println!("help: {}", quote("Run yoke health. Trusted policy is now what was trusted when the backup was made; if your policy files changed since, review them and run yoke trust. To undo, run yoke backup restore with the safety backup."));
    Ok(())
}
