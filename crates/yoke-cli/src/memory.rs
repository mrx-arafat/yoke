use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, ensure, Context, Result};
use clap::Subcommand;
use serde::Deserialize;
use yoke_core::config::{LoadedConfig, Mode};
use yoke_core::memory::{self, ImportOptions, SharedCopy};
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
        /// Print the export report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Validate and import plaintext memory data; never imports executable trust.
    Import {
        /// Archive written by yoke memory export.
        file: PathBuf,
        /// Required once for every project in the archive: source=destination, or source=source to
        /// restore in place.
        #[arg(long = "map", value_name = "SOURCE=DESTINATION")]
        mappings: Vec<String>,
        /// Report additions and duplicates, and show the text of the memories it would add, without
        /// writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Keep the archive's pinned flags. By default every imported memory is unpinned, because
        /// a pinned memory is injected at every session start; use this to restore your own export.
        #[arg(long)]
        keep_pins: bool,
        /// Print the import report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Pin a memory for context injection; add --unpin to remove the pin.
    Pin {
        /// Memory id from yoke recall in this project.
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

/// Why session start injects nothing, when it does not; `None` once notes are injected.
fn injection_notice(mode: Mode) -> Option<&'static str> {
    (mode != Mode::Enforce).then_some(
        "session start injects no notes while mode is observe; set mode = \"enforce\" in yoke.toml and run yoke trust in your own terminal",
    )
}

fn project_of(loaded: &LoadedConfig) -> Result<&str> {
    loaded
        .project_root
        .to_str()
        .context("project path must be UTF-8")
}

/// A pinned memory is injected at every session start and an import brings in text nobody here
/// wrote, so neither may be done from an agent's tool process. This closes the casual path only;
/// it needs no terminal, so scripts that a person runs keep working.
fn require_person(action: &str, why: &str) -> Result<()> {
    if let Some(signal) = yoke_core::guard::agent_process_signal() {
        bail!("{action} cannot run from an agent tool process ({signal}): {why}; run it in your own terminal");
    }
    Ok(())
}

pub fn remember(text: &str, pinned: bool, json: bool) -> Result<()> {
    if pinned {
        require_person(
            "yoke remember --pinned",
            "a pinned note is injected at every session start, so only a person may pin; save the note without --pinned",
        )?;
    }
    let (store, loaded) = commands::load_memory_project()?;
    let project = project_of(&loaded)?;
    let row = memory::remember(&store, project, text, pinned, timestamp()?)?;
    if json {
        println!("{}", serde_json::to_string(&row)?);
    } else {
        println!(
            "id: {}\ntime: {}\npinned: {}\ntext: {}",
            row.id,
            commands::quote(&yoke_core::timefmt::local_time(row.ts)),
            row.pinned,
            commands::quote(&row.text)
        );
    }
    // Said once, with the first note of a project, so a new user learns it without a nag per note.
    if let Some(notice) = injection_notice(loaded.config.mode) {
        if memory::recall(&store, project, "", 2)?.len() == 1 {
            eprintln!("yoke: {notice} (see yoke recall --explain)");
        }
    }
    Ok(())
}

pub fn recall(query: &str, limit: u16, json: bool, explain: bool) -> Result<()> {
    let (store, loaded) = commands::load_memory_project()?;
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
        println!("project: {}", commands::quote(project));
        println!("memories[{}]{{id,time,pinned,text}}:", rows.len());
        if rows.is_empty() {
            println!(
                "help: \"{}\"",
                if query.trim().is_empty() {
                    "No memories in this project; yoke remember <text> adds one. Memory belongs to the project directory (the git root inside a repository)."
                } else {
                    "No memory in this project matches that query; yoke recall without words lists them all."
                }
            );
        }
        for row in rows {
            println!(
                "  {},{},{},{}",
                row.id,
                commands::quote(&yoke_core::timefmt::local_time(row.ts)),
                row.pinned,
                commands::quote(&row.text)
            );
        }
    }
    Ok(())
}

/// Three decimals, with no sign on a score that rounds to zero (a term in half the notes weighs 0).
fn score_text(score: f64) -> String {
    match format!("{score:.3}").as_str() {
        "-0.000" => "0.000".into(),
        text => text.into(),
    }
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
    println!("mode: {}", loaded.config.mode.as_str());
    match injection_notice(loaded.config.mode) {
        Some(notice) => println!(
            "injection: {}",
            commands::quote(&format!("inactive - {notice}"))
        ),
        None => println!("injection: active"),
    }
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
            item.score.map(score_text).unwrap_or_default(),
            join(item.matched.clone()),
            join(item.facts.iter().map(i64::to_string).collect()),
            item.truncated,
            commands::quote(&item.text.chars().take(80).collect::<String>())
        );
    }
    println!(
        "help: \"{} Lower BM25 scores are better matches. Explanations are never injected.\"",
        if query.trim().is_empty() {
            "Session start injects this selection in enforce mode, inside a block that labels it untrusted data: pinned, trusted knowledge, then recent memories within memory.inject_tokens."
        } else {
            "Session start injects the selection for an empty query (pinned, trusted knowledge, then recent memories) in enforce mode, inside a block that labels it untrusted data; this shows how your words would rank within memory.inject_tokens."
        }
    );
    Ok(())
}

pub fn forget(id: i64) -> Result<()> {
    ensure!(id > 0, "memory id must be positive");
    let (store, loaded) = commands::load_memory_project()?;
    let forgotten = memory::forget_in_project(&store, project_of(&loaded)?, id)?
        .with_context(|| format!("memory {id} not found in this project; run yoke recall to list this project's memory ids"))?;
    println!("deleted: true");
    match forgotten.shared_copy {
        SharedCopy::None => {}
        SharedCopy::Removed(path) => println!(
            "shared_copy: removed from {}\nhelp: \"Run yoke trust in your own terminal so trusted project knowledge drops this note.\"",
            commands::quote(&path.display().to_string())
        ),
        SharedCopy::Failed(path, reason) => {
            println!(
                "shared_copy: still in {}",
                commands::quote(&path.display().to_string())
            );
            bail!(
                "memory {id} was deleted but its shared copy was not removed ({reason}); delete that entry by hand and run yoke trust"
            );
        }
    }
    Ok(())
}

pub fn share(id: i64) -> Result<()> {
    ensure!(id > 0, "memory id must be positive");
    let (store, loaded) = commands::load_memory_project()?;
    let shared = memory::share_in_project(&store, project_of(&loaded)?, id)?;
    println!(
        "file: {}\nhelp: \"{}\"",
        commands::quote(&shared.path.display().to_string()),
        if shared.added {
            "Shared knowledge takes effect only after a human runs yoke trust.".to_owned()
        } else {
            format!("Memory {id} was already shared; nothing changed.")
        }
    );
    Ok(())
}

pub fn run(command: MemoryCommand) -> Result<()> {
    match command {
        MemoryCommand::Export {
            file,
            all,
            overwrite,
            json,
        } => {
            let home = commands::home()?;
            let store = Store::open_read_only(home.join(".yoke/state.db"))?;
            let anchored = commands::agent_memory_project(&store)?;
            ensure!(anchored.is_none() || !all, "an agent can export only its session's project; remove --all, or export all projects from your own terminal");
            let root = match anchored {
                Some(root) => root,
                None => {
                    config::load_trusted(&store, &std::env::current_dir()?, &home)?.project_root
                }
            };
            let project = root.to_str().context("project path must be UTF-8")?;
            let report = memory::export_archive(
                &store,
                &file,
                if all { None } else { Some(project) },
                overwrite,
            )?;
            if json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                println!(
                    "file: {}\nprojects: {}\nmemories: {}\nfacts: {}\nbytes: {}",
                    commands::quote(&file.display().to_string()),
                    report.projects.len(),
                    report.memories,
                    report.facts,
                    report.bytes
                );
            }
        }
        MemoryCommand::Import {
            file,
            mappings,
            dry_run,
            keep_pins,
            json,
        } => {
            if !dry_run {
                require_person(
                    "yoke memory import",
                    "it adds text that nobody in this project wrote; read it first with --dry-run",
                )?;
            }
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
            if let Some(root) = commands::agent_memory_project(&store)? {
                for destination in projects.values() {
                    ensure!(Path::new(destination).canonicalize().ok().as_ref() == Some(&root), "agent import previews must map every destination to its session's project {}; change --map or run it in your own terminal", commands::quote(&root.display().to_string()));
                }
            }
            let options = ImportOptions {
                dry_run,
                keep_pins,
                now: timestamp()?,
            };
            let report = memory::import_archive(&store, &file, &projects, &options)?;
            if json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                println!("dry_run: {}\nprojects: {}\nadditions: {}\nduplicates: {}\npins_dropped: {}\nfacts_marked_digested: {}",
                    report.dry_run, report.projects.len(), report.additions, report.duplicates,
                    report.pins_dropped, report.facts_marked_digested);
                println!("conflicts[{}]:", report.conflicts.len());
                for conflict in &report.conflicts {
                    println!("  {}", commands::quote(conflict));
                }
            }
            print_import_notes(&report);
            // The real import refuses on conflicts, so a dry run must fail the same way for scripts.
            ensure!(
                report.conflicts.is_empty(),
                "the import would be refused because of {} conflict(s) listed above; no records changed",
                report.conflicts.len()
            );
        }
        MemoryCommand::Pin { id, unpin } => {
            ensure!(id > 0, "memory id must be positive");
            if !unpin {
                require_person(
                    "yoke memory pin",
                    "a pinned note is injected at every session start, so only a person may pin",
                )?;
            }
            let (store, loaded) = commands::load_memory_project()?;
            memory::pin_in_project(&store, project_of(&loaded)?, id, !unpin)?;
            println!("id: {id}\npinned: {}", !unpin);
        }
        MemoryCommand::Digest {
            session,
            transcript,
        } => {
            let (store, loaded) = commands::load_memory_project()?;
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

/// What an import took and left out, in words; a dry run also lists the text it would add.
/// It goes to stderr, so stdout stays the single JSON report that scripts parse.
fn print_import_notes(report: &memory::ImportReport) {
    if report.dry_run {
        eprintln!(
            "preview[{}]{{project,source,time,pinned,text}}:",
            report.preview.len()
        );
        for row in &report.preview {
            eprintln!(
                "  {},{},{},{},{}",
                commands::quote(&row.project),
                row.source,
                commands::quote(&yoke_core::timefmt::local_time(row.ts)),
                row.pinned,
                commands::quote(&row.text)
            );
        }
        if report.preview_omitted > 0 {
            eprintln!(
                "preview_omitted: {} more memories are not shown",
                report.preview_omitted
            );
        }
    }
    if report.pins_dropped > 0 {
        eprintln!(
            "note: \"{} pinned note(s) {} imported unpinned; pin one after reading it with yoke memory pin <id>, or restore your own export with --keep-pins.\"",
            report.pins_dropped,
            if report.dry_run { "would be" } else { "were" }
        );
    }
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
            "memory.digest is disabled; a human enables it by adding digest = true under [memory] in ~/.yoke/config.toml or this project's yoke.toml, then running yoke trust in their own terminal"
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
    )?;
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
