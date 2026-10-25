use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{ensure, Context, Result};
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
            if json {
                println!("{}", serde_json::to_string(&job)?);
            } else {
                println!("id: {}\nstate: {}", job.id, job.state);
                println!("help: \"A background worker started; run yoke jobs to follow it.\"");
            }
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
            commands::quote(&yoke_core::timefmt::local_time(job.run_after)),
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

/// `error.log` for the worker's stderr; a new file is private, because it can quote payloads.
fn open_error_log(home: &Path) -> std::io::Result<std::fs::File> {
    yoke_core::private::open_append(&home.join(".yoke/error.log"))
}

pub(crate) fn spawn_worker(home: &Path) -> Result<()> {
    let errors = open_error_log(home)?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["jobs", "work"])
        .current_dir(home)
        // The worker must open the state the hook chose, even when a settings file injected a
        // different HOME into the hook's environment.
        .env("HOME", home)
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

/// The directories a digest may read a transcript from: the job's project and the transcript
/// directories of Claude Code and Codex, which `CLAUDE_CONFIG_DIR` and `CODEX_HOME` relocate.
fn transcript_roots(home: &Path, project: &Path) -> Vec<PathBuf> {
    let directory = |variable: &str, default: &str| {
        std::env::var_os(variable)
            .filter(|value| !value.is_empty())
            .map_or_else(|| home.join(default), PathBuf::from)
    };
    vec![
        project.to_path_buf(),
        directory("CLAUDE_CONFIG_DIR", ".claude").join("projects"),
        directory("CODEX_HOME", ".codex").join("sessions"),
    ]
}

/// The worker is not inside the agent's sandbox, and a job's transcript path came from a hook
/// payload that the agent can forge, so resolve it and refuse anything that is not a `.jsonl`
/// file inside one of `roots`. Returns the resolved path, so the file read is the file checked.
fn resolve_transcript(path: &Path, roots: &[PathBuf]) -> Result<PathBuf> {
    ensure!(path.is_absolute(), "the transcript path is not absolute");
    let resolved = path
        .canonicalize()
        .context("cannot resolve the transcript path")?;
    ensure!(
        resolved
            .extension()
            .is_some_and(|extension| extension == "jsonl"),
        "the transcript is not a .jsonl file"
    );
    ensure!(
        std::fs::metadata(&resolved)?.is_file(),
        "the transcript is not a regular file"
    );
    ensure!(
        roots
            .iter()
            .filter_map(|root| root.canonicalize().ok())
            .any(|root| resolved.starts_with(root)),
        "the transcript is outside the project and the agents' transcript directories"
    );
    Ok(resolved)
}

/// A copy of a digest job whose transcript path is safe to open. A path that fails the check is
/// dropped and reported in `error.log`, so the digest runs from its facts alone.
fn checked_digest_job(job: &jobs::Job, roots: &[PathBuf]) -> jobs::Job {
    let mut checked = job.clone();
    // A payload that is not JSON is reported by the digest itself as a permanent failure.
    let Ok(mut payload) = serde_json::from_str::<serde_json::Value>(&job.payload) else {
        return checked;
    };
    let Some(path) = payload
        .get("transcript")
        .and_then(serde_json::Value::as_str)
    else {
        return checked;
    };
    payload["transcript"] = match resolve_transcript(Path::new(path), roots) {
        // A path that is not UTF-8 cannot be written back into the JSON payload.
        Ok(resolved) => resolved
            .to_str()
            .map_or(serde_json::Value::Null, serde_json::Value::from),
        Err(error) => {
            crate::hook::diagnostic(&format!(
                "yoke: job {} ignored transcript {path:?}: {error:#}",
                job.id
            ));
            serde_json::Value::Null
        }
    };
    checked.payload = payload.to_string();
    checked
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
            crate::memory::DIGEST_JOB => {
                let roots = transcript_roots(&home, Path::new(&job.project));
                crate::memory::run_digest_job(&store, &home, &checked_digest_job(&job, &roots))
            }
            crate::retention::RETENTION_JOB => {
                crate::retention::run_job(&store, &job, timestamp()?)
            }
            yoke_core::notifications::HISTORY_JOB => {
                yoke_core::notifications::drain_background(&store, timestamp()?)
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn digest_reads_only_jsonl_transcripts_inside_the_project_or_agent_directories() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let transcripts = base.join("claude/projects");
        let outside = base.join("outside");
        std::fs::create_dir_all(transcripts.join("p")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let good = transcripts.join("p/s.jsonl");
        let secret = outside.join("secret.jsonl");
        std::fs::write(&good, "{}").unwrap();
        std::fs::write(&secret, "token").unwrap();
        std::fs::write(transcripts.join("p/notes.txt"), "x").unwrap();
        symlink(&secret, transcripts.join("p/link.jsonl")).unwrap();
        let roots = vec![transcripts.clone()];

        assert_eq!(resolve_transcript(&good, &roots).unwrap(), good);
        // A symlink in a root is resolved before the check, so it cannot lead out of it.
        let dotdot = transcripts.join("p/../../outside/secret.jsonl");
        let refused = [
            secret,
            transcripts.join("p/link.jsonl"),
            dotdot,
            transcripts.join("p/notes.txt"),
            transcripts.join("p"),
            PathBuf::from("p/s.jsonl"),
            transcripts.join("p/missing.jsonl"),
        ];
        for path in refused {
            assert!(
                resolve_transcript(&path, &roots).is_err(),
                "{} was accepted",
                path.display()
            );
        }
    }

    #[test]
    fn a_new_worker_error_log_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".yoke")).unwrap();

        let log = open_error_log(home.path()).unwrap();

        assert_eq!(log.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        drop(log);
        let path = home.path().join(".yoke/error.log");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let log = open_error_log(home.path()).unwrap();
        assert_eq!(log.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        drop(log);
        std::fs::remove_file(&path).unwrap();
        let target = home.path().join("outside");
        std::fs::write(&target, "unchanged").unwrap();
        symlink(&target, &path).unwrap();
        assert!(open_error_log(home.path()).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"unchanged");
    }

    #[test]
    fn a_forged_transcript_path_is_dropped_from_the_job_and_a_good_one_is_resolved() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let root = base.join("sessions");
        std::fs::create_dir_all(&root).unwrap();
        let good = root.join("s.jsonl");
        std::fs::write(&good, "{}").unwrap();
        std::fs::write(base.join("hidden.jsonl"), "token").unwrap();
        let job = |transcript: serde_json::Value| jobs::Job {
            id: 1,
            kind: "memory_digest".into(),
            dedupe_key: "digest:x".into(),
            project: base.join("project").display().to_string(),
            payload: serde_json::json!({"session": "s", "transcript": transcript}).to_string(),
            state: "running".into(),
            attempts: 1,
            max_attempts: 3,
            run_after: 0,
            owner_pid: None,
            lease_expires: None,
            last_error: None,
            created_at: 0,
            updated_at: 0,
        };
        let payload = |job: &jobs::Job| {
            serde_json::from_str::<serde_json::Value>(&job.payload).unwrap()["transcript"].clone()
        };
        let roots = vec![root];

        let forged = checked_digest_job(
            &job(base.join("hidden.jsonl").to_str().unwrap().into()),
            &roots,
        );
        let kept = checked_digest_job(&job(good.to_str().unwrap().into()), &roots);
        let none = checked_digest_job(&job(serde_json::Value::Null), &roots);

        assert_eq!(payload(&forged), serde_json::Value::Null);
        assert_eq!(payload(&kept), serde_json::json!(good));
        assert_eq!(payload(&none), serde_json::Value::Null);
    }
}
