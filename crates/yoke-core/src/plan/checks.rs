use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};

use super::{acceptance, files, get, now, storage, CheckRun, CheckSummary, PlanCheck, PlanRecord};
use crate::engine::execute_bounded;
use crate::review::{ReviewReport, Reviewer};
use crate::store::Store;

/// Execute approved commands and required independent review; only Yoke can mark done.
pub fn check(
    store: &Store,
    project: &Path,
    slug: &str,
    reviewer: Option<(Reviewer, Option<&str>)>,
) -> Result<CheckSummary> {
    let plan = get(store, project, slug)?;
    if plan.state != "building" {
        return Err(super::wrong_state(
            &plan,
            "only a building plan can run checks",
        ));
    }
    let header = files::approved(&plan)?;
    // Held until this function returns so `plan resume` can tell a live check from a dead one.
    let _owner =
        files::claim_check(&plan)?.context("another process is already checking this plan")?;
    let root = Path::new(&plan.project);
    let before = files::fingerprint(root)?;
    // Completion pins this tree's content to a commit, so committing the same bytes keeps it.
    let base = match &plan.base_ref {
        Some(base) => base.clone(),
        None => files::head(root)?,
    };
    // Without a content anchor, completion keeps the HEAD-bound `before` fingerprint.
    let anchor = files::content_fingerprint(root, &base)
        .ok()
        .flatten()
        .map(|content| format!("{base}:{content}"));
    let checking = storage::transition(store, &plan, "checking", "agent", None, None, None)?;
    let mut runs = Vec::new();
    for declared in &header.checks {
        runs.push(execute(&checking, declared, &before));
    }
    if header.review {
        let run = match reviewer {
            Some((reviewer, model)) => {
                match crate::review::run(root, reviewer, model, plan.base_ref.as_deref()) {
                    Ok(report) => review_run(&checking, &before, &report),
                    Err(error) => error_run(
                        &checking,
                        "review",
                        &before,
                        format!("independent review failed: {error:#}"),
                    ),
                }
            }
            None => error_run(
                &checking,
                "review",
                &before,
                "independent review is required; select an independent reviewer".into(),
            ),
        };
        runs.push(run);
    }
    let mut reason = None;
    let after = match files::fingerprint(root) {
        Ok(after) => after,
        Err(error) => {
            reason = Some(format!("post-check fingerprint failed: {error:#}"));
            String::new()
        }
    };
    if before != after && reason.is_none() {
        reason = Some("working tree changed during checks; evidence is void. Put generated screenshots and other check outputs in a Git-ignored directory, then rerun checks".into());
    }
    if let Err(error) = files::approved(&checking) {
        reason = Some(format!("approved header changed during checks: {error:#}"));
    }
    match acceptance::attempt(store, &checking, &header, &runs, reason.is_none()) {
        Ok(None) => {}
        Ok(Some((accepted, findings, summary))) => {
            let mut run = error_run(&checking, acceptance::RUN, &before, summary);
            run.command = "requirements and tasks".into();
            run.result = if accepted { "pass" } else { "fail" }.into();
            run.findings = findings;
            runs.push(run);
        }
        Err(error) => runs.push(error_run(
            &checking,
            acceptance::RUN,
            &before,
            format!("acceptance evaluation failed: {error:#}"),
        )),
    }
    if let Some(reason) = &reason {
        runs.push(error_run(&checking, "evidence", &before, reason.clone()));
    }
    let passed = reason.is_none() && runs.iter().all(|run| !run.blocking || run.result == "pass");
    let (plan, applied) = storage::finish(
        store,
        &checking,
        if passed { "done" } else { "building" },
        &mut runs,
        reason.as_deref(),
    )?;
    if let Some(anchor) = anchor.filter(|_| passed && applied) {
        storage::anchor_done(store, &plan, &anchor)?;
    }
    Ok(CheckSummary {
        plan,
        passed: passed && applied,
        fingerprint_before: before,
        fingerprint_after: after,
        checks: runs,
    })
}

fn execute(plan: &PlanRecord, declared: &PlanCheck, fingerprint: &str) -> CheckRun {
    let mut run = empty_run(plan, &declared.name, fingerprint);
    run.command = declared.command.clone();
    run.blocking = declared.blocking;
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(&declared.command)
        .current_dir(&plan.project)
        .env("YOKE_PROJECT", &plan.project)
        .env("YOKE_PLAN", &plan.slug)
        .env("YOKE_CHECK", &declared.name);
    match execute_bounded(
        &mut command,
        &[],
        Duration::from_secs(declared.timeout),
        4096,
    ) {
        Ok(output) => {
            run.exit_code = output.exit_code;
            run.ms = output.ms;
            run.output_tail = tail(&format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
            run.result = if output.timed_out {
                "timeout"
            } else if output.success {
                "pass"
            } else {
                "fail"
            }
            .into();
            if output.timed_out {
                run.output_tail = tail(&format!(
                    "{}\ncheck timed out after {} seconds",
                    run.output_tail, declared.timeout
                ));
            }
            match files::artifacts(Path::new(&plan.project), &declared.artifacts) {
                Ok(captured) => {
                    run.artifacts = captured.iter().map(|(path, _)| path.clone()).collect();
                    run.artifact_hashes = captured.into_iter().collect();
                }
                Err(error) => {
                    run.result = "error".into();
                    run.output_tail = tail(&format!(
                        "{}\ninvalid declared artifact: {error:#}",
                        run.output_tail
                    ));
                }
            }
        }
        Err(error) => {
            run.result = "error".into();
            run.output_tail = format!("could not execute approved check: {error}");
        }
    }
    run
}

fn empty_run(plan: &PlanRecord, name: &str, fingerprint: &str) -> CheckRun {
    CheckRun {
        id: 0,
        plan_id: plan.id,
        ts: plan.updated_at,
        name: name.into(),
        command: String::new(),
        exit_code: None,
        output_tail: String::new(),
        fingerprint: fingerprint.into(),
        blocking: true,
        ms: 0,
        result: "error".into(),
        findings: serde_json::json!([]),
        artifacts: Vec::new(),
        artifact_hashes: Default::default(),
    }
}

/// Evidence that a check's owning process ended before recording any result.
pub(super) fn interrupted_run(plan: &PlanRecord) -> Result<CheckRun> {
    let mut run = empty_run(plan, "check", "");
    run.ts = now()?;
    run.command = format!("yoke plan check {}", plan.slug);
    run.result = "interrupted".into();
    run.output_tail =
        "the check process ended before recording results; none of its output is evidence".into();
    Ok(run)
}

fn error_run(plan: &PlanRecord, name: &str, fingerprint: &str, error: String) -> CheckRun {
    let mut run = empty_run(plan, name, fingerprint);
    run.output_tail = tail(&error);
    run
}

fn review_run(plan: &PlanRecord, fingerprint: &str, report: &ReviewReport) -> CheckRun {
    let mut run = empty_run(plan, "review", fingerprint);
    run.command = match &report.agent {
        Some(agent) => format!(
            "independent review by {agent} (requested model: {}; reported model: unknown)",
            report.model.as_deref().unwrap_or("CLI default")
        ),
        None => "independent review".into(),
    };
    run.exit_code = report.exit_code;
    run.ms = report.ms;
    run.result = if report.passed && report.error.is_none() {
        "pass"
    } else {
        "fail"
    }
    .into();
    run.output_tail = tail(&format!(
        "{}\n{}",
        report.output_tail,
        report.error.as_deref().unwrap_or_default()
    ));
    // Finding is a serializable domain type; serialization cannot lose its evidence silently.
    match serde_json::to_value(&report.findings) {
        Ok(findings) => run.findings = findings,
        Err(error) => {
            run.result = "error".into();
            run.output_tail = format!("cannot serialize review findings: {error}");
        }
    }
    run
}

/// Record standalone review evidence without granting completion or approval.
/// Without a pre-review fingerprint, evidence cannot be recorded as passing.
pub fn record_review(
    store: &Store,
    project: &Path,
    slug: &str,
    report: &ReviewReport,
) -> Result<()> {
    record_review_with_fingerprint(store, project, slug, report, "").map(|_| ())
}

/// Bind standalone evidence to the tree captured before invoking the reviewer.
/// Returns the persisted success verdict; storage failures propagate as errors.
pub fn record_review_with_fingerprint(
    store: &Store,
    project: &Path,
    slug: &str,
    report: &ReviewReport,
    before: &str,
) -> Result<bool> {
    let plan = get(store, project, slug)?;
    let mut run = review_run(&plan, before, report);
    let invalid = match files::fingerprint(Path::new(&plan.project)) {
        Err(error) => Some(("error", format!("review evidence fingerprint failed: {error:#}"))),
        Ok(_) if before.is_empty() => Some(("void", "review evidence lacks a pre-review fingerprint; rerun review with before/after binding".into())),
        Ok(after) if after != before => Some(("void", "working tree changed during review; evidence is void; rerun review on the current tree".into())),
        Ok(_) => None,
    };
    if let Some((result, reason)) = invalid {
        run.result = result.into();
        run.output_tail = tail(&format!("{}\n{reason}", run.output_tail));
    }
    run.ts = now()?;
    storage::save_review(store, &mut run)?;
    Ok(run.result == "pass")
}

fn tail(text: &str) -> String {
    let mut start = text.len().saturating_sub(4096);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].into()
}
