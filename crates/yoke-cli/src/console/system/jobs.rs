//! Jobs view: queued, running, failed and dead background jobs with their last error, and retry
//! for a dead job under the same rule as `yoke jobs retry`. The retry confirmation says what the
//! job will do, including what a memory digest sends to an agent.
use super::super::{
    actions::Outcome,
    render::{self, escape},
    Console,
};
use super::{int, now};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use yoke_core::{
    config,
    jobs::{self, Job},
    store::Store,
};

fn time(ts: i64) -> String {
    format!(
        r#"<time data-time="{ts}">{}</time>"#,
        escape(&yoke_core::timefmt::local_time_secs(ts))
    )
}

pub(super) fn section(store: &Store, params: &BTreeMap<String, String>) -> Result<String> {
    let all = params.get("all").is_some_and(|value| value == "1");
    let rows = jobs::list(store, all, 200)?;
    let mut out = format!(
        r#"<section class="panel" aria-labelledby="sys-jobs"><div class="list-caption"><h2 id="sys-jobs">Background jobs</h2><span>{} shown &middot; newest first</span></div><form class="filters" data-filter action="system" method="get"><input type="hidden" name="view" value="jobs"><label>Show<select name="all"><option value=""{}>Unfinished jobs</option><option value="1"{}>All jobs, including succeeded</option></select></label><button type="submit">Apply</button><button type="button" class="quiet" data-refresh>Refresh</button></form><p class="muted">There is no daemon: session start requeues expired work and starts a short-lived worker. A failed job retries by itself after its next run time; a dead job used all {} attempts and waits for a human retry.</p>"#,
        rows.len(),
        if all { "" } else { " selected" },
        if all { " selected" } else { "" },
        jobs::MAX_ATTEMPTS,
    );
    if rows.is_empty() {
        out.push_str(&render::empty(
            "No unfinished jobs",
            "Memory digests, retention passes and notification history run here in the background.",
            "yoke jobs --all",
        ));
    } else {
        out.push_str(r#"<div class="table-scroll" tabindex="0" role="region" aria-labelledby="sys-jobs"><table><thead><tr><th scope="col">Job</th><th scope="col">State</th><th scope="col">Attempts</th><th scope="col">Next run</th><th scope="col">Last error</th><th scope="col">Action</th></tr></thead><tbody>"#);
        for job in &rows {
            let project = if job.project.is_empty() {
                String::new()
            } else {
                format!(
                    r#"<small class="sys-project">{}</small>"#,
                    escape(&job.project)
                )
            };
            let level = match job.state.as_str() {
                "dead" => " sys-level-error",
                "failed" => " sys-level-warning",
                _ => "",
            };
            let next = if matches!(job.state.as_str(), "queued" | "failed") {
                time(job.run_after)
            } else {
                String::new()
            };
            let action = if job.state == "dead" {
                format!(
                    r#"<div class="actions action-zone"><button type="button" class="quiet" data-system-act="system.job-retry" data-params="{}">Retry</button></div>"#,
                    escape(&json!({"id": job.id}).to_string())
                )
            } else {
                String::new()
            };
            out.push_str(&format!(
                r#"<tr><th scope="row">{} <code>{}</code>{project}</th><td><span class="badge{level}">{}</span></td><td>{}/{}</td><td>{next}</td><td class="sys-message">{}</td><td>{action}</td></tr>"#,
                job.id,
                escape(&job.kind),
                escape(&job.state),
                job.attempts,
                job.max_attempts,
                escape(job.last_error.as_deref().unwrap_or("")),
            ));
        }
        out.push_str("</tbody></table></div>");
    }
    out.push_str("</section>");
    Ok(out)
}

/// What the worker does when it runs this job, in the words the human confirms.
fn effect(store: &Store, c: &Console, job: &Job) -> String {
    let payload: Value = serde_json::from_str(&job.payload).unwrap_or(Value::Null);
    match job.kind.as_str() {
        crate::memory::DIGEST_JOB => {
            let session = payload["session"].as_str().unwrap_or("unknown");
            let transcript = if payload["transcript"].is_string() {
                " and a bounded tail of its transcript"
            } else {
                ""
            };
            match config::load_trusted(store, Path::new(&job.project), &c.home) {
                Ok(loaded) if loaded.config.memory.digest => format!(
                    "sends the pending memory facts of session {session} in {}{transcript} to the {} command-line tool, which that project's trusted policy names as its digest agent, and saves the memories it writes",
                    job.project, loaded.config.memory.digest_agent
                ),
                Ok(_) => format!(
                    "fails again without sending anything, because memory.digest is off in the trusted policy of {}",
                    job.project
                ),
                Err(error) => format!(
                    "reads the trusted policy of {} first, which failed just now ({error:#}); if it loads, it sends the pending memory facts of session {session}{transcript} to that project's digest agent",
                    job.project
                ),
            }
        }
        crate::retention::RETENTION_JOB => format!(
            "deletes decisions, memory facts and finished plans' check runs older than {} days, in every project",
            payload["retention_days"]
                .as_u64()
                .map_or_else(|| "the queued number of".to_owned(), |days| days.to_string())
        ),
        yoke_core::notifications::HISTORY_JOB => {
            "turns queued history into notifications in short batches; nothing leaves this machine"
                .to_owned()
        }
        other => format!("runs job kind {other}"),
    }
}

/// Retry a dead job after the human confirms what it does. `start` launches the background
/// worker; the console passes `jobs::spawn_worker`.
pub(super) fn retry(
    store: &Store,
    c: &Console,
    params: &Value,
    confirmed: bool,
    start: &mut dyn FnMut() -> Result<()>,
) -> Result<Outcome> {
    let id = int(params, "id")?;
    let job = jobs::list(store, true, 10_000)?
        .into_iter()
        .find(|job| job.id == id)
        .ok_or_else(|| anyhow::anyhow!("Job {id} no longer exists"))?;
    anyhow::ensure!(
        job.state == "dead",
        "Only dead jobs can be retried; job {id} is {}",
        job.state
    );
    if !confirmed {
        return Ok(Outcome::Confirm(format!(
            "Retry job {id} ({}) with {} fresh attempts? The background worker {}.",
            job.kind,
            job.max_attempts,
            effect(store, c, &job)
        )));
    }
    let job = jobs::retry(store, id, now())?;
    start()?;
    Ok(Outcome::Done(json!({
        "message": format!("Job {} is {} and the background worker started.", job.id, job.state)
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dead(store: &Store, kind: &str, key: &str, project: &str, payload: &str) -> i64 {
        jobs::enqueue(store, kind, key, project, payload, 1).unwrap();
        let job = jobs::claim(store, 2, 7, 60).unwrap().unwrap();
        jobs::fail(store, &job, 3, "agent exited 1", false).unwrap();
        job.id
    }

    #[test]
    fn jobs_list_with_errors_and_retry_dead_ones_after_saying_what_they_send() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let c = super::super::test_console(&root);
        let store = Store::open(&c.database).unwrap();
        let project = c.project.display().to_string();
        let digest = dead(
            &store,
            "memory_digest",
            "digest:s1",
            &project,
            r#"{"session":"s1","transcript":null}"#,
        );
        let prune = dead(
            &store,
            "retention",
            "retention",
            "",
            r#"{"retention_days":30}"#,
        );
        jobs::enqueue(
            &store,
            "notifications-drain",
            "notifications-drain",
            "",
            "{}",
            5,
        )
        .unwrap();

        let html = section(&store, &BTreeMap::new()).unwrap();
        assert!(html.contains("agent exited 1") && html.contains("notifications-drain"));
        assert_eq!(
            html.matches("data-system-act=\"system.job-retry\"").count(),
            2,
            "only dead jobs offer retry"
        );

        let mut started = 0;
        let mut start = || {
            started += 1;
            Ok(())
        };
        let Outcome::Confirm(message) =
            retry(&store, &c, &json!({"id": digest}), false, &mut start).unwrap()
        else {
            panic!("retry must ask first")
        };
        assert!(
            message.contains("fails again without sending anything"),
            "{message}"
        );
        let Outcome::Confirm(message) =
            retry(&store, &c, &json!({"id": prune}), false, &mut start).unwrap()
        else {
            panic!("retry must ask first")
        };
        assert!(
            message.contains("older than 30 days, in every project"),
            "{message}"
        );
        retry(&store, &c, &json!({"id": prune}), true, &mut start).unwrap();
        assert!(
            retry(&store, &c, &json!({"id": prune}), true, &mut start).is_err(),
            "a queued job is not retried"
        );
        let state = jobs::list(&store, true, 10)
            .unwrap()
            .into_iter()
            .find(|job| job.id == prune)
            .unwrap();
        assert_eq!((state.state.as_str(), state.attempts), ("queued", 0));
        assert_eq!(started, 1);
    }
}
