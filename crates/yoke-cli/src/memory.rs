use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{ensure, Context, Result};
use clap::Subcommand;
use serde::Deserialize;
use yoke_core::config::LoadedConfig;
use yoke_core::memory;
use yoke_core::{config, jobs, review, store::Store};

use crate::commands;
use crate::jobs::{Permanent, LEASE_SECS};

pub(crate) const DIGEST_JOB: &str = "memory_digest";
/// Orphaned sessions one session start may queue digests for; the rest wait for the next.
const ORPHAN_SWEEP_LIMIT: usize = 20;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DigestPayload {
    session: String,
    transcript: Option<PathBuf>,
}

#[derive(Subcommand)]
pub enum MemoryCommand {
    /// Write a portable plaintext archive. Memory may contain sensitive content.
    Export {
        /// Archive to write; refuses to replace an existing file without --overwrite.
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
        /// Archive written by yoke memory export.
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
        /// Memory id from yoke recall.
        id: i64,
        /// Remove the pin instead of adding it.
        #[arg(long)]
        unpin: bool,
    },
    /// Generate memories from pending facts with the configured read-only agent.
    Digest {
        /// Recorded session id whose pending facts to digest.
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

pub fn recall(query: &str, limit: u16, json: bool, explain: bool) -> Result<()> {
    let (store, loaded) = commands::load_project()?;
    let project = loaded
        .project_root
        .to_str()
        .context("project path must be UTF-8")?;
    if explain {
        return explain_context(&store, &loaded, project, query, json);
    }
    let rows = memory::recall(&store, project, query, usize::from(limit))?;
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

/// Show what session start would inject for this query, and why; never part of the injection.
fn explain_context(
    store: &Store,
    loaded: &LoadedConfig,
    project: &str,
    query: &str,
    json: bool,
) -> Result<()> {
    let tokens = loaded.config.memory.inject_tokens;
    let items = memory::context_items(
        store,
        project,
        query,
        loaded.knowledge.as_deref().unwrap_or(""),
        tokens,
    )?;
    if json {
        println!("{}", serde_json::to_string(&items)?);
        return Ok(());
    }
    let used = items
        .iter()
        .map(|item| item.text.chars().count())
        .sum::<usize>()
        + items.len().saturating_sub(1);
    println!(
        "budget: {{inject_tokens: {tokens}, chars: {}, used: {used}}}",
        tokens.saturating_mul(4).min(1_000_000)
    );
    println!(
        "selected[{}]{{reason,id,source,rank,score,matched,facts,truncated,preview}}:",
        items.len()
    );
    for item in &items {
        let join = |values: Vec<String>| commands::quote(&values.join(" "));
        println!(
            "  {},{},{},{},{},{},{},{},{}",
            item.reason,
            item.memory_id.map(|id| id.to_string()).unwrap_or_default(),
            item.source.as_deref().unwrap_or(""),
            item.rank.map(|rank| rank.to_string()).unwrap_or_default(),
            item.score
                .map(|score| format!("{score:.3}"))
                .unwrap_or_default(),
            join(item.matched.clone()),
            join(item.facts.iter().map(i64::to_string).collect()),
            item.truncated,
            commands::quote(&item.text.chars().take(80).collect::<String>())
        );
    }
    println!("help: \"Session start injects this selection for an empty query: pinned, trusted knowledge, then recent memories within memory.inject_tokens. Lower BM25 scores are better matches. Explanations are never injected.\"");
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
            let report = digest(
                &store,
                &loaded,
                &session,
                transcript.as_deref(),
                &mut || Ok(()),
            )?;
            println!("memories_created: {}", report.memories);
            println!("facts_digested: {}", report.facts);
            if report.pending {
                println!("facts_pending: true");
            }
        }
    }
    Ok(())
}

fn digest(
    store: &Store,
    loaded: &LoadedConfig,
    session: &str,
    transcript: Option<&Path>,
    heartbeat: &mut dyn FnMut() -> Result<()>,
) -> Result<memory::DigestReport> {
    if !loaded.config.memory.digest {
        return Err(Permanent(
            "memory.digest is disabled; a human must enable and trust it before model execution"
                .into(),
        )
        .into());
    }
    let reviewer = match loaded.config.memory.digest_agent.as_str() {
        "claude" => review::Reviewer::Claude,
        "codex" => review::Reviewer::Codex,
        _ => anyhow::bail!("invalid configured digest agent"),
    };
    let mut command = review::command(
        reviewer,
        loaded.config.memory.digest_models.get(reviewer.as_str()),
    );
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
    memory::digest_with_command(
        store,
        session,
        loaded
            .project_root
            .to_str()
            .context("project path must be UTF-8")?,
        &String::from_utf8_lossy(&tail),
        timestamp()?,
        &mut command,
        Duration::from_secs(600),
        heartbeat,
    )
}

/// Run one queued digest; `true` means facts remain and a follow-up job is needed.
pub(crate) fn run_digest_job(store: &Store, home: &Path, job: &jobs::Job) -> Result<bool> {
    let payload: DigestPayload = serde_json::from_str(&job.payload)
        .map_err(|error| Permanent(format!("invalid digest job payload: {error}")))?;
    let loaded = config::load_trusted(store, Path::new(&job.project), home)?;
    let report = digest(
        store,
        &loaded,
        &payload.session,
        payload.transcript.as_deref(),
        &mut || jobs::heartbeat(store, job, timestamp()?, LEASE_SECS),
    )?;
    Ok(report.pending)
}

/// Queue a durable digest job for this session and start a worker; see `yoke jobs`.
pub fn schedule_digest(
    store: &Store,
    project: &Path,
    home: &Path,
    session: &str,
    transcript: Option<&str>,
    now: i64,
    debounce: bool,
) -> Result<()> {
    let project = project.to_str().context("project path must be UTF-8")?;
    if enqueue_digest(store, project, session, transcript, now, debounce)? && jobs::due(store, now)?
    {
        crate::jobs::spawn_worker(home)?;
    }
    Ok(())
}

/// Queue digests for sessions of this project whose agent was killed, or whose end-of-session
/// hook never queued one, so their pending facts are not lost; see `memory::orphaned_sessions`.
/// A session whose digest job is queued, running, failed or dead is left alone, so a dead job
/// waits for `yoke jobs retry` instead of being recreated at every session start. Returns how
/// many were queued; the caller starts the worker, normally through `jobs::tick`.
pub fn schedule_orphan_digests(
    store: &Store,
    project: &Path,
    current: &str,
    now: i64,
) -> Result<usize> {
    let project = project.to_str().context("project path must be UTF-8")?;
    let mut queued = 0;
    for session in memory::orphaned_sessions(
        store,
        project,
        current,
        now,
        memory::ORPHAN_IDLE_SECS,
        ORPHAN_SWEEP_LIMIT,
    )? {
        if jobs::unfinished(store, &dedupe_key(&session, project))?.is_some() {
            continue;
        }
        if enqueue_digest(store, project, &session, None, now, false)? {
            queued += 1;
        }
    }
    Ok(queued)
}

fn dedupe_key(session: &str, project: &str) -> String {
    format!("digest:{}", memory::digest_identity(session, project))
}

/// Queue the digest when it is due and no attempt was claimed recently; `true` means a job is
/// now active for it. The job's dedupe key keeps two schedulers from queuing the same work.
fn enqueue_digest(
    store: &Store,
    project: &str,
    session: &str,
    transcript: Option<&str>,
    now: i64,
    debounce: bool,
) -> Result<bool> {
    if !memory::digest_due(
        store,
        session,
        project,
        now,
        if debounce { 1800 } else { 0 },
    )? {
        return Ok(false);
    }
    let identity = memory::digest_identity(session, project);
    if !store.claim_interval(
        &format!("memory.digest.attempt.{identity}"),
        now,
        if debounce { 1800 } else { 30 },
    )? {
        return Ok(false);
    }
    let payload = serde_json::json!({ "session": session, "transcript": transcript });
    jobs::enqueue(
        store,
        DIGEST_JOB,
        &dedupe_key(session, project),
        project,
        &payload.to_string(),
        now,
    )?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use yoke_core::jobs;

    #[test]
    fn next_session_start_queues_a_digest_for_each_idle_session_with_pending_facts() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let home = directory.path().join("home");
        std::fs::create_dir_all(home.join(".yoke"))?;
        let project = directory.path().join("project");
        std::fs::create_dir_all(&project)?;
        let project_text = project.to_str().unwrap();
        let store = Store::open(home.join(".yoke/state.db"))?;
        // "a" and "b" were killed; "resumed" is starting again now; "live" is still working.
        for session in ["a", "b", "resumed"] {
            memory::record_fact(&store, session, project_text, "file_write", "Edit: x", 0)?;
        }
        memory::record_fact(&store, "live", project_text, "file_write", "Edit: y", 5_000)?;
        // Job upkeep alone never queued anything for a killed session.
        crate::jobs::tick(&store, &home, 5_000, 30)?;
        assert!(memory::digest_due(&store, "a", project_text, 5_000, 0)?);
        assert!(jobs::list(&store, false, 10)?.is_empty());
        assert_eq!(
            schedule_orphan_digests(&store, &project, "resumed", 5_000)?,
            2
        );
        let queued = jobs::list(&store, false, 10)?;
        assert_eq!(queued.len(), 2);
        assert!(queued
            .iter()
            .all(|job| job.kind == DIGEST_JOB && job.state == "queued"));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&queued[1].payload)?,
            serde_json::json!({"session": "a", "transcript": null})
        );
        assert_eq!(
            schedule_orphan_digests(&store, &project, "resumed", 5_001)?,
            0
        );
        // A dead job waits for a human retry; the sweep must not recreate it.
        let running = jobs::claim(&store, 5_001, 1, 60)?.unwrap();
        assert_eq!(running.id, queued[1].id);
        assert_eq!(
            jobs::fail(&store, &running, 5_001, "digest CLI unauthenticated", false)?,
            "dead"
        );
        assert_eq!(
            schedule_orphan_digests(&store, &project, "resumed", 5_100)?,
            0
        );
        // A succeeded job that still left facts pending does not block a new one.
        let running = jobs::claim(&store, 5_100, 1, 60)?.unwrap();
        assert_eq!(running.id, queued[0].id);
        jobs::succeed(&store, &running, 5_100, false)?;
        assert_eq!(
            schedule_orphan_digests(&store, &project, "resumed", 5_200)?,
            1
        );
        assert_eq!(
            jobs::unfinished(&store, &dedupe_key("b", project_text))?
                .unwrap()
                .state,
            "queued"
        );
        Ok(())
    }
}
