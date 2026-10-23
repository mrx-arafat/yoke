//! `yoke trial report`: release-trial evidence read from local state without writing it.

use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use anyhow::{ensure, Context, Result};
use clap::Subcommand;
use yoke_core::store::Store;
use yoke_core::trial::{self, TrialQuery, TrialReport};

use crate::commands::{self, quote};
use crate::AgentArg;

/// Older error.log lines beyond this tail are not read.
const ERROR_LOG_TAIL: u64 = 4 * 1024 * 1024;

#[derive(Subcommand)]
pub enum TrialCommand {
    /// Summarize observe and enforce trial evidence per agent; never creates or migrates state.
    Report {
        /// Inclusive start: YYYY-MM-DD (local midnight) or RFC 3339 with offset.
        #[arg(long)]
        since: Option<String>,
        /// Exclusive end: YYYY-MM-DD (local midnight) or RFC 3339 with offset.
        #[arg(long)]
        until: Option<String>,
        /// Only decisions from this agent.
        #[arg(long, value_enum)]
        agent: Option<AgentArg>,
        /// Flagged decisions per page.
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u16).range(1..=1000))]
        limit: u16,
        /// Continue flagged decisions below this id, from a previous next_before.
        #[arg(long)]
        before: Option<i64>,
        /// Print the report as JSON.
        #[arg(long)]
        json: bool,
    },
}

pub fn run(command: TrialCommand) -> Result<()> {
    let TrialCommand::Report {
        since,
        until,
        agent,
        limit,
        before,
        json,
    } = command;
    let home = commands::home()?;
    let path = home.join(".yoke/state.db");
    let store = Store::open_read_only(&path).with_context(|| {
        if path.exists() {
            format!(
                "open {} read-only; a copied WAL database also needs its -wal and -shm files",
                path.display()
            )
        } else {
            format!(
                "no state at {}; trial evidence exists only after an installed hook has run",
                path.display()
            )
        }
    })?;
    let query = TrialQuery {
        since: since
            .as_deref()
            .map(|text| trial::parse_time(&store, text, true))
            .transpose()?,
        until: until
            .as_deref()
            .map(|text| trial::parse_time(&store, text, true))
            .transpose()?,
        agent: agent.map(|agent| yoke_core::event::Agent::from(agent).as_str().into()),
        limit: usize::from(limit),
        before,
        local_time: true,
    };
    let error_log = read_tail(&home.join(".yoke/error.log"))?;
    let report = trial::report(&store, &query, error_log.as_deref())?;
    if json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        render(&report, since.as_deref(), until.as_deref());
    }
    Ok(())
}

fn read_tail(path: &Path) -> Result<Option<String>> {
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("read error.log"),
    };
    let metadata = file.metadata()?;
    ensure!(metadata.is_file(), "error.log must be a regular file");
    let size = metadata.len();
    let start = size.saturating_sub(ERROR_LOG_TAIL);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(size - start).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    // A tail that starts mid-line drops the partial first line.
    Ok(Some(match (start > 0, text.split_once('\n')) {
        (true, Some((_, rest))) => rest.to_owned(),
        _ => text,
    }))
}

fn render(report: &TrialReport, since: Option<&str>, until: Option<&str>) {
    println!(
        "window: {{since: {}, until: {}, agent: {}, time_zone: {}}}",
        quote(since.unwrap_or("start")),
        quote(until.unwrap_or("now")),
        quote(report.agent.as_deref().unwrap_or("all")),
        quote(report.time_zone)
    );
    println!(
        "cohorts[{}]{{agent,mode,first,last,decisions,sessions,active_days,active_hours,working_days}}:",
        report.cohorts.len()
    );
    for cohort in &report.cohorts {
        println!(
            "  {},{},{},{},{},{},{},{},{}",
            quote(&cohort.agent),
            quote(&cohort.mode),
            quote(&cohort.first_time),
            quote(&cohort.last_time),
            cohort.decisions,
            cohort.sessions,
            cohort.active_days,
            cohort.active_hours,
            cohort.working_days
        );
    }
    let outcomes: Vec<_> = report
        .cohorts
        .iter()
        .flat_map(|cohort| cohort.events.iter().map(move |event| (cohort, event)))
        .collect();
    println!(
        "outcomes[{}]{{agent,mode,event,allow,deny,would_deny,rewrite,error}}:",
        outcomes.len()
    );
    for (cohort, event) in outcomes {
        println!(
            "  {},{},{},{},{},{},{},{}",
            quote(&cohort.agent),
            quote(&cohort.mode),
            quote(&event.event),
            event.allow,
            event.deny,
            event.would_deny,
            event.rewrite,
            event.error
        );
    }
    println!(
        "flagged[{} of {}]{{id,time,agent,mode,event,tool,outcome,gates,reason}}:",
        report.flagged.len(),
        report.flagged_total
    );
    for row in &report.flagged {
        println!(
            "  {},{},{},{},{},{},{},{},{}",
            row.id,
            quote(&row.time),
            quote(&row.agent),
            quote(&row.mode),
            quote(&row.event),
            quote(row.tool.as_deref().unwrap_or("")),
            quote(&row.outcome),
            quote(&row.gates.join(" ")),
            quote(row.reason.as_deref().unwrap_or(""))
        );
    }
    if let Some(before) = report.next_before {
        println!("next_before: {before}");
    }
    println!(
        "incidents[{}]{{time,kind,project,plan,detail}}:",
        report.incidents.len()
    );
    for incident in &report.incidents {
        println!(
            "  {},{},{},{},{}",
            quote(&incident.time),
            quote(&incident.kind),
            quote(incident.project.as_deref().unwrap_or("")),
            quote(incident.plan.as_deref().unwrap_or("")),
            quote(&incident.detail)
        );
    }
    println!(
        "acceptance[{}]{{criterion,agent,status,detail}}:",
        report.acceptance.len()
    );
    for criterion in &report.acceptance {
        println!(
            "  {},{},{},{}",
            quote(&criterion.criterion),
            quote(criterion.agent.as_deref().unwrap_or("all")),
            quote(criterion.status),
            quote(&criterion.detail)
        );
    }
    let mut help = String::from(
        "Record each flagged decision and incident as expected or unexpected in docs/trials.md.",
    );
    if let Some(before) = report.next_before {
        help.push_str(&format!(
            " Run again with --before {before} for older flagged decisions."
        ));
    }
    println!("help: {}", quote(&help));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn error_log_tail_refuses_links_and_non_regular_files() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("outside");
        let log = dir.path().join("error.log");
        std::fs::write(&target, "private text").unwrap();
        symlink(&target, &log).unwrap();
        assert!(read_tail(&log).is_err());
        assert!(read_tail(dir.path()).is_err());
        std::fs::remove_file(&log).unwrap();
        let fifo = std::ffi::CString::new(log.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: fifo is a live, NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        // Keep the old read-open from blocking; seek on the pipe still fails.
        use std::os::unix::fs::OpenOptionsExt;
        let _writer = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&log)
            .unwrap();
        assert!(read_tail(&log).is_err());
    }
}
