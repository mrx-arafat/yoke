use std::fmt;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use clap::Subcommand;
use yoke_core::{jobs, store::Store};

use crate::commands;
use crate::memory::timestamp;

/// Each digest batch may run 600 seconds; the worker renews its lease before every batch.
pub(crate) const LEASE_SECS: i64 = 720;
const JOBS_PER_WORKER: usize = 5;

#[derive(Subcommand)]
pub enum JobsCommand {
    /// Give a dead job a fresh set of attempts. Requires a human terminal.
    Retry {
        /// Dead job id from yoke jobs.
        id: i64,
    },
    /// Run due jobs; hooks start this automatically in the background.
    #[command(hide = true)]
    Work,
}

/// A failure that more attempts cannot fix, such as disabled configuration.
#[derive(Debug)]
pub(crate) struct Permanent(pub String);

impl fmt::Display for Permanent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Permanent {}

pub fn run(command: Option<JobsCommand>, all: bool, json: bool) -> Result<()> {
    match command {
        None => list(all, json),
        Some(JobsCommand::Retry { id }) => {
            commands::require_human("jobs retry")?;
            let home = commands::home()?;
            let store = commands::open_store(&home)?;
            let job = jobs::retry(&store, id, timestamp()?)?;
            spawn_worker(&home)?;
            println!("id: {}\nstate: {}", job.id, job.state);
            println!("help: \"A background worker started; run yoke jobs to follow it.\"");
            Ok(())
        }
        Some(JobsCommand::Work) => work(),
    }
}

fn list(all: bool, json: bool) -> Result<()> {
    // Listing never creates or migrates state; see `commands::open_current`.
    let rows = match commands::open_current(&commands::home()?)? {
        Some(store) => jobs::list(&store, all, 100)?,
        None if json => Vec::new(),
        None => {
            println!("jobs[0]{{id,kind,state,attempts,run_after,project,last_error}}:\nhelp: \"No Yoke state exists yet; the first hook event creates it.\"");
            return Ok(());
        }
    };
    if json {
        println!("{}", serde_json::to_string(&rows)?);
        return Ok(());
    }
    println!(
        "jobs[{}]{{id,kind,state,attempts,run_after,project,last_error}}:",
        rows.len()
    );
    for job in &rows {
        println!(
            "  {},{},{},\"{}/{}\",{},{},{}",
            job.id,
            commands::quote(&job.kind),
            commands::quote(&job.state),
            job.attempts,
            job.max_attempts,
            job.run_after,
            commands::quote(&job.project),
            commands::quote(job.last_error.as_deref().unwrap_or(""))
        );
    }
    println!("help: \"Failed jobs retry after run_after at the next session start; run yoke jobs retry <id> in your own terminal for dead jobs. --all includes succeeded jobs.\"");
    Ok(())
}

/// Opportunistic upkeep at session start: requeue expired leases, prune old finished jobs
/// and start a worker when work is due. There is no daemon.
pub fn tick(store: &Store, home: &Path, now: i64, retention_days: u32) -> Result<()> {
    jobs::recover(store, now)?;
    jobs::prune(store, now, retention_days)?;
    if jobs::due(store, now)? {
        spawn_worker(home)?;
    }
    Ok(())
}

pub(crate) fn spawn_worker(home: &Path) -> Result<()> {
    let errors = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(home.join(".yoke/error.log"))?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["jobs", "work"])
        .current_dir(home)
        .env("YOKE_OFF", "1")
        .env(crate::hook::BACKGROUND, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(errors);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command.spawn().context("start background job worker")?;
    Ok(())
}

fn work() -> Result<()> {
    let home = commands::home()?;
    let store = commands::open_store(&home)?;
    let owner = std::process::id();
    jobs::recover(&store, timestamp()?)?;
    for _ in 0..JOBS_PER_WORKER {
        let Some(job) = jobs::claim(&store, timestamp()?, owner, LEASE_SECS)? else {
            break;
        };
        let result = match job.kind.as_str() {
            crate::memory::DIGEST_JOB => crate::memory::run_digest_job(&store, &home, &job),
            other => Err(Permanent(format!("unknown job kind {other}")).into()),
        };
        match result {
            Ok(follow_up) => {
                jobs::succeed(&store, &job, timestamp()?, follow_up)?;
            }
            Err(error) => {
                let retryable = error.downcast_ref::<Permanent>().is_none();
                let message = format!("{error:#}");
                let state = jobs::fail(&store, &job, timestamp()?, &message, retryable)?;
                crate::hook::diagnostic(&format!(
                    "yoke: job {} ({}) {state}: {message}",
                    job.id, job.kind
                ));
            }
        }
    }
    Ok(())
}
