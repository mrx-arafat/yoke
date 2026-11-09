//! Retention view: the trusted policy, what the hourly pass removes, a preview of what a pass
//! would remove now, and run-now after the human confirms the exact counts.
use super::super::{actions::Outcome, render::escape, Console};
use super::now;
use anyhow::Result;
use serde_json::json;
use yoke_core::{config, jobs, retention, store::Store};

/// `log.retention_days` from this project's trusted policy, and the file it came from.
fn policy(store: &Store, c: &Console) -> Result<(u32, String)> {
    let loaded = config::load_trusted(store, &c.project, &c.home)?;
    let origin = match loaded.origins.get("log.retention_days") {
        Some(file) if file != "default" => file.clone(),
        _ => "the built-in default".into(),
    };
    Ok((loaded.config.log.retention_days, origin))
}

fn time(ts: i64) -> String {
    format!(
        r#"<time data-time="{ts}">{}</time>"#,
        escape(&yoke_core::timefmt::local_time_secs(ts))
    )
}

pub(super) fn section(store: &Store, c: &Console) -> Result<String> {
    let (days, origin) = policy(store, c)?;
    let preview = retention::preview(store, now(), days)?;
    let queued = crate::retention::last_scheduled(store)?.map_or_else(|| "never".to_owned(), time);
    let last = jobs::list(store, true, 1000)?
        .into_iter()
        .find(|job| job.kind == crate::retention::RETENTION_JOB)
        .map_or_else(
            || "none recorded".to_owned(),
            |job| {
                format!(
                    "job {} {} at {}{}",
                    job.id,
                    escape(&job.state),
                    time(job.updated_at),
                    job.last_error
                        .map(|error| format!(": {}", escape(&error)))
                        .unwrap_or_default()
                )
            },
        );
    let row = |what: &str, older: i64, total: Option<i64>| {
        format!(
            r#"<tr><th scope="row">{what}</th><td>{older}</td><td>{}</td></tr>"#,
            total.map_or_else(String::new, |total| total.to_string())
        )
    };
    Ok(format!(
        r#"<section class="panel" aria-labelledby="sys-retention"><div class="list-caption"><h2 id="sys-retention">Retention</h2><span>Keeps {days} days</span></div><dl class="facts oversight"><dt>Policy</dt><dd><code>log.retention_days = {days}</code> from {origin}</dd><dt>Applies to</dt><dd>Every project in this state database. Each session start queues the hourly pass with its own project's trusted value; this view uses this project's.</dd><dt>Last queued</dt><dd>{queued}</dd><dt>Last pass</dt><dd>{last}</dd></dl><p>The hourly pass deletes decisions, memory facts and the check runs of done or cancelled plans recorded before the boundary, in short batches. Trusted policy, memories and the history of unfinished plans stay. Session start also removes succeeded and dead background jobs older than the boundary. Change the number of days in <code>yoke.toml</code> or <code>~/.yoke/config.toml</code> under <code>[log]</code>, then run <code>yoke trust</code> in your own terminal.</p><h3>Preview</h3><p>Recorded before {boundary}:</p><div class="table-scroll" tabindex="0" role="region" aria-label="Retention preview"><table><thead><tr><th scope="col">Records</th><th scope="col">Would be removed</th><th scope="col">Total</th></tr></thead><tbody>{decisions}{facts}{checks}{jobs}</tbody></table></div><div class="actions action-zone"><button type="button" class="danger-button" data-system-act="system.retention-run" data-params="{{}}">Run retention now</button><span class="muted">Asks you to confirm the exact counts first. A backup keeps a copy.</span></div></section>"#,
        origin = escape(&origin),
        boundary = time(preview.cutoff),
        decisions = row(
            "Decisions",
            preview.decisions,
            Some(preview.total_decisions)
        ),
        facts = row("Memory facts", preview.facts, Some(preview.total_facts)),
        checks = row("Check runs of finished plans", preview.check_runs, None),
        jobs = row(
            "Finished background jobs (at the next session start)",
            preview.jobs,
            None
        ),
    ))
}

/// Queue the hourly pass now, after the human confirmed the counts it deletes. `start` launches
/// the background worker; the console passes `jobs::spawn_worker`.
pub(super) fn run(
    store: &Store,
    c: &Console,
    confirmed: bool,
    start: &mut dyn FnMut() -> Result<()>,
) -> Result<Outcome> {
    let (days, origin) = policy(store, c)?;
    if !confirmed {
        let preview = retention::preview(store, now(), days)?;
        return Ok(Outcome::Confirm(format!(
            "Run retention now with log.retention_days = {days} (from {origin}): the background worker deletes {} decisions, {} memory facts and {} check runs of finished plans recorded before {}, in every project. This cannot be undone.",
            preview.decisions,
            preview.facts,
            preview.check_runs,
            yoke_core::timefmt::local_time_secs(preview.cutoff)
        )));
    }
    let (job, created) = crate::retention::queue_now(store, now(), days)?;
    if !created {
        return Ok(Outcome::Done(json!({
            "message": format!(
                "Retention job {} is already {}; it keeps the days it was queued with. See Jobs.",
                job.id, job.state
            )
        })));
    }
    start()?;
    Ok(Outcome::Done(json!({
        "message": format!("Retention job {} queued and the background worker started. See Jobs.", job.id)
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_previews_counts_and_queues_one_job_only_after_confirming() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let c = super::super::test_console(&root);
        let store = Store::open(&c.database).unwrap();
        store
            .record_decision(
                &yoke_core::store::DecisionRecord {
                    policy_hash: None,
                    ts: 1,
                    session_id: "s".into(),
                    agent: "codex".into(),
                    project: c.project.display().to_string(),
                    event: "pre_tool_use".into(),
                    tool: None,
                    mode: "observe".into(),
                    outcome: "allow".into(),
                    reason: None,
                    gates: json!([]),
                    payload: "{}".into(),
                },
                4096,
            )
            .unwrap();
        let html = section(&store, &c).unwrap();
        assert!(html.contains("<code>log.retention_days = 90</code> from the built-in default"));
        assert!(html.contains(r#"<th scope="row">Decisions</th><td>1</td><td>1</td>"#));

        let mut started = 0;
        let mut start = || {
            started += 1;
            Ok(())
        };
        let Outcome::Confirm(message) = run(&store, &c, false, &mut start).unwrap() else {
            panic!("running retention must ask first")
        };
        assert!(message.contains("deletes 1 decisions") && message.contains("cannot be undone"));
        assert!(
            jobs::list(&store, true, 10).unwrap().is_empty(),
            "nothing queued before confirming"
        );
        run(&store, &c, true, &mut start).unwrap();
        let Outcome::Done(again) = run(&store, &c, true, &mut start).unwrap() else {
            panic!("a confirmed run finishes")
        };
        assert!(again["message"]
            .as_str()
            .unwrap()
            .contains("already queued"));
        let queued = jobs::list(&store, true, 10).unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].payload, r#"{"retention_days":90}"#);
        assert_eq!(started, 1);
    }
}
