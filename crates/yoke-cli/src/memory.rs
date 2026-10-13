use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{ensure, Context, Result};
use clap::Subcommand;
use sha2::{Digest, Sha256};
use yoke_core::memory;
use yoke_core::{config, review, store::Store};

use crate::commands;

#[derive(Subcommand)]
pub enum MemoryCommand {
    /// Write a portable plaintext archive. Memory may contain sensitive content.
    Export {
        file: PathBuf,
        /// Include all projects; otherwise export only the current project.
        #[arg(long)]
        all: bool,
        /// Explicitly replace an existing archive.
        #[arg(long)]
        overwrite: bool,
    },
    /// Validate and import plaintext memory data; never imports executable trust.
    Import {
        file: PathBuf,
        /// Explicit source=destination project mapping. Repeat for each project.
        #[arg(long = "map", value_name = "SOURCE=DESTINATION")]
        mappings: Vec<String>,
        /// Report additions and duplicates without writing imported records.
        #[arg(long)]
        dry_run: bool,
    },
    /// Pin a memory for context injection, or unpin it.
    Pin {
        id: i64,
        #[arg(long)]
        unpin: bool,
    },
    /// Generate memories from pending facts with the configured read-only agent.
    Digest {
        session: String,
        /// Optional transcript tail; only a bounded tail is passed to the agent.
        #[arg(long)]
        transcript: Option<PathBuf>,
    },
}

pub fn timestamp() -> Result<i64> {
    Ok(i64::try_from(
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
    )?)
}

pub fn remember(text: &str, pinned: bool) -> Result<()> {
    let (store, loaded) = commands::load_project()?;
    let row = memory::remember(
        &store,
        loaded
            .project_root
            .to_str()
            .context("project path must be UTF-8")?,
        text,
        pinned,
        timestamp()?,
    )?;
    println!("{}", serde_json::to_string(&row)?);
    Ok(())
}

pub fn recall(query: &str, limit: u16, json: bool) -> Result<()> {
    let (store, loaded) = commands::load_project()?;
    let rows = memory::recall(
        &store,
        loaded
            .project_root
            .to_str()
            .context("project path must be UTF-8")?,
        query,
        usize::from(limit),
    )?;
    if json {
        println!("{}", serde_json::to_string(&rows)?);
    } else {
        println!("memories[{}]{{id,ts,pinned,text}}:", rows.len());
        for row in rows {
            println!(
                "  {},{},{},{}",
                row.id,
                row.ts,
                row.pinned,
                commands::quote(&row.text)
            );
        }
    }
    Ok(())
}

pub fn forget(id: i64) -> Result<()> {
    ensure!(id > 0, "memory id must be positive");
    println!(
        "deleted: {}",
        memory::forget(&commands::open_store(&commands::home()?)?, id)?
    );
    Ok(())
}

pub fn share(id: i64) -> Result<()> {
    ensure!(id > 0, "memory id must be positive");
    let path = memory::share(&commands::open_store(&commands::home()?)?, id)?;
    println!(
        "file: {}\nhelp: \"Shared knowledge takes effect only after a human runs yoke trust.\"",
        commands::quote(&path.display().to_string())
    );
    Ok(())
}

pub fn run(command: MemoryCommand) -> Result<()> {
    match command {
        MemoryCommand::Export {
            file,
            all,
            overwrite,
        } => {
            let home = commands::home()?;
            let store = Store::open_read_only(home.join(".yoke/state.db"))?;
            let loaded = config::load_trusted(&store, &std::env::current_dir()?, &home)?;
            let project = loaded
                .project_root
                .to_str()
                .context("project path must be UTF-8")?;
            let report = memory::export_archive(
                &store,
                &file,
                if all { None } else { Some(project) },
                overwrite,
            )?;
            println!("{}", serde_json::to_string(&report)?);
        }
        MemoryCommand::Import {
            file,
            mappings,
            dry_run,
        } => {
            let mut projects = BTreeMap::new();
            for mapping in mappings {
                let (source, destination) = mapping
                    .split_once('=')
                    .context("--map requires SOURCE=DESTINATION")?;
                ensure!(
                    !source.is_empty() && !destination.is_empty(),
                    "--map paths must not be empty"
                );
                ensure!(
                    projects
                        .insert(source.to_owned(), destination.to_owned())
                        .is_none(),
                    "duplicate source project mapping: {source}"
                );
            }
            let home = commands::home()?;
            let path = home.join(".yoke/state.db");
            let store = if dry_run {
                if path.exists() {
                    Store::open_read_only(&path)?
                } else {
                    Store::open(":memory:")?
                }
            } else {
                commands::open_store(&home)?
            };
            let report = memory::import_archive(&store, &file, &projects, dry_run)?;
            println!("{}", serde_json::to_string(&report)?);
        }
        MemoryCommand::Pin { id, unpin } => {
            ensure!(id > 0, "memory id must be positive");
            memory::pin(&commands::open_store(&commands::home()?)?, id, !unpin)?;
            println!("id: {id}\npinned: {}", !unpin);
        }
        MemoryCommand::Digest {
            session,
            transcript,
        } => {
            let (store, loaded) = commands::load_project()?;
            ensure!(loaded.config.memory.digest, "memory.digest is disabled; a human must enable and trust it before model execution");
            let reviewer = match loaded.config.memory.digest_agent.as_str() {
                "claude" => review::Reviewer::Claude,
                "codex" => review::Reviewer::Codex,
                _ => anyhow::bail!("invalid configured digest agent"),
            };
            let mut command = review::command(reviewer);
            command.current_dir(&loaded.project_root);
            let mut tail = Vec::new();
            if let Some(path) = transcript {
                let mut file = std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                    .open(path)
                    .context("open digest transcript")?;
                ensure!(
                    file.metadata()?.is_file(),
                    "digest transcript must be a regular file"
                );
                let length = file.metadata()?.len();
                file.seek(SeekFrom::Start(length.saturating_sub(32768)))?;
                file.take(32768).read_to_end(&mut tail)?;
            }
            let count = memory::digest_with_command(
                &store,
                &session,
                loaded
                    .project_root
                    .to_str()
                    .context("project path must be UTF-8")?,
                &String::from_utf8_lossy(&tail),
                timestamp()?,
                &mut command,
                Duration::from_secs(600),
            )?;
            println!("memories_created: {count}");
        }
    }
    Ok(())
}

pub fn schedule_digest(
    store: &Store,
    project: &Path,
    home: &Path,
    session: &str,
    transcript: Option<&str>,
    now: i64,
    debounce: bool,
) -> Result<()> {
    let project_text = project.to_str().context("project path must be UTF-8")?;
    if !memory::digest_due(
        store,
        session,
        project_text,
        now,
        if debounce { 1800 } else { 0 },
    )? {
        return Ok(());
    }
    let key = format!(
        "memory.digest.attempt.{:x}",
        Sha256::digest(format!("{project_text}\0{session}"))
    );
    if !store.claim_interval(&key, now, if debounce { 1800 } else { 30 })? {
        return Ok(());
    }
    let errors = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(home.join(".yoke/error.log"))?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["memory", "digest", session])
        .current_dir(project)
        .env("YOKE_OFF", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(errors);
    if let Some(path) = transcript {
        command.arg("--transcript").arg(path);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command.spawn().context("start bounded memory digest")?;
    Ok(())
}
