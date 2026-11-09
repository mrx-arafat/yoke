//! Plan statistics for the Plans page and the human-only cancel action.
//! Loaded as a child of `dashboard`.
use super::super::{
    actions::Outcome,
    render::{self, escape},
    Console,
};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use yoke_core::{
    overview::{self, PlanStats},
    plan::{self, PlanRecord},
    store::Store,
    timefmt,
};

const REASON_LIMIT: usize = 1000;

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn tile(label: &str, value: impl std::fmt::Display, hint: &str) -> String {
    format!(
        r#"<div class="stat"><dt>{}</dt><dd>{value}</dd><small>{}</small></div>"#,
        escape(label),
        escape(hint)
    )
}

fn day_label(day_start: i64) -> String {
    let at = timefmt::full_time(day_start, timefmt::utc_offset_secs(day_start));
    at.chars().take(10).collect()
}

/// Totals, check rounds, completions per day and the newest completions for the launch project.
pub(in super::super) fn panel(store: &Store, c: &Console) -> Result<String> {
    let stats = overview::plan_stats(store, &c.project.to_string_lossy(), now())?;
    if stats.total == 0 {
        return Ok(String::new());
    }
    Ok(render_panel(&stats))
}

fn render_panel(stats: &PlanStats) -> String {
    let mut out = String::from(
        r#"<section class="panel" aria-label="Plan statistics"><div class="list-caption"><h2>Plan statistics</h2></div><dl class="stats">"#,
    );
    out.push_str(&tile("Plans", stats.total, "in this project"));
    out.push_str(&tile(
        "Done",
        stats.done,
        "passed their checks and requirements",
    ));
    out.push_str(&tile(
        "In progress",
        stats.in_progress,
        "approved, building, checking, waiting or stalled",
    ));
    out.push_str(&tile("Drafts", stats.draft, "awaiting approval"));
    out.push_str(&tile("Cancelled", stats.cancelled, "abandoned by a human"));
    out.push_str(&tile(
        "Check rounds per done plan",
        stats
            .avg_check_rounds
            .map_or_else(|| "none yet".to_owned(), |avg| format!("{avg:.1}")),
        "times a plan entered checking",
    ));
    out.push_str("</dl>");
    if !stats.timeline.is_empty() {
        let most = stats.timeline.iter().map(|d| d.count).max().unwrap_or(1);
        out.push_str("<h3>Completed in the last 30 days</h3><ol class=\"timeline\">");
        for day in &stats.timeline {
            out.push_str(&format!(
                r#"<li><span>{}</span><progress max="{most}" value="{}" aria-label="{} completed on {}"></progress><b>{}</b></li>"#,
                day_label(day.day_start),
                day.count,
                day.count,
                day_label(day.day_start),
                day.count
            ));
        }
        out.push_str("</ol>");
    }
    if !stats.recently_done.is_empty() {
        out.push_str("<h3>Recently done</h3><ul class=\"dash-list\">");
        for plan in &stats.recently_done {
            out.push_str(&format!(
                r#"<li><a data-row data-nav href="{}">{}</a> <span class="muted">done <time data-time="{}">{}</time> &middot; {} check rounds</span></li>"#,
                render::href("plans", &format!("slug={}", render::encode(&plan.slug))),
                escape(&plan.slug),
                plan.done_at,
                plan.done_at,
                plan.check_rounds
            ));
        }
        out.push_str("</ul>");
    }
    out.push_str("</section>");
    out
}

/// The cancel control for an unfinished plan; empty for plans already done or cancelled.
pub(in super::super) fn cancel_zone(plan: &PlanRecord) -> String {
    if matches!(plan.state.as_str(), "done" | "cancelled") {
        return String::new();
    }
    let params = json!({"slug": plan.slug, "revision": plan.revision}).to_string();
    format!(
        r#"<details class="action-zone cancel-zone"><summary>Cancel this plan...</summary><p class="muted">Cancelling is a human decision, it needs a reason, and it cannot be undone. The plan stays on record as cancelled.</p><label>Reason<textarea data-field="reason" maxlength="{REASON_LIMIT}" rows="3" required></textarea></label><div class="actions"><button type="button" class="danger-button" data-plans-run="plans.cancel" data-params="{}" data-confirm-label="Confirm cancellation">Review cancellation</button></div></details>"#,
        escape(&params)
    )
}

fn text<'a>(params: &'a Value, key: &str) -> Result<&'a str> {
    params
        .get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("Missing {key}"))
}

/// `plans.cancel` with `{slug, revision, reason}`: confirm first, then cancel.
pub(in super::super) fn cancel(
    store: &Store,
    c: &Console,
    params: &Value,
    confirmed: bool,
) -> Result<Outcome> {
    let slug = text(params, "slug")?;
    let reason = text(params, "reason")?.trim();
    ensure!(
        !reason.is_empty(),
        "Give a reason for cancelling this plan."
    );
    ensure!(
        reason.chars().count() <= REASON_LIMIT,
        "Keep the reason under {REASON_LIMIT} characters."
    );
    let seen = params
        .get("revision")
        .and_then(Value::as_i64)
        .context("Missing revision")?;
    let current = plan::get(store, &c.project, slug)?;
    if current.revision != seen {
        bail!("This plan changed since you opened it; reload the page and review it again.");
    }
    if !confirmed {
        return Ok(Outcome::Confirm(format!(
            "Cancel plan \"{slug}\" (now {})? Its session is released and the plan can never resume. Reason recorded: {reason}",
            current.state
        )));
    }
    plan::cancel(store, &c.project, slug, reason)?;
    Ok(Outcome::Done(
        json!({"message": format!("Plan {slug} cancelled.")}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, Store, Console) {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().canonicalize().unwrap().join("repo");
        std::fs::create_dir(&project).unwrap();
        let database = temp.path().join("state.db");
        let store = Store::open(&database).unwrap();
        plan::create(&store, &project, ".yoke/plans", "alpha", "Alpha").unwrap();
        let c = Console {
            home: temp.path().to_owned(),
            project,
            database,
            token: "token".into(),
            csrf: "csrf".into(),
            host: "127.0.0.1:1".into(),
        };
        (temp, store, c)
    }

    #[test]
    fn cancelling_needs_a_reason_the_current_revision_and_a_confirmation() {
        let (_temp, store, c) = fixture();
        let revision = plan::get(&store, &c.project, "alpha").unwrap().revision;
        let ask = |reason: &str, revision: i64, confirmed: bool| {
            cancel(
                &store,
                &c,
                &json!({"slug": "alpha", "reason": reason, "revision": revision}),
                confirmed,
            )
        };

        assert!(ask("  ", revision, false).is_err(), "reason required");
        assert!(
            ask("scope changed", revision + 1, false).is_err(),
            "stale view refused"
        );
        let Outcome::Confirm(message) = ask("scope changed", revision, false).unwrap() else {
            panic!("expected a confirmation step");
        };
        assert!(message.contains("scope changed") && message.contains("alpha"));
        assert_eq!(
            plan::get(&store, &c.project, "alpha").unwrap().state,
            "draft",
            "asking must not cancel"
        );

        assert!(matches!(
            ask("scope changed", revision, true).unwrap(),
            Outcome::Done(_)
        ));
        let cancelled = plan::get(&store, &c.project, "alpha").unwrap();
        assert_eq!(cancelled.state, "cancelled");
        assert!(cancel_zone(&cancelled).is_empty());
        assert!(
            ask("again", cancelled.revision, true).is_err(),
            "a cancelled plan cannot be cancelled twice"
        );
        let html = panel(&store, &c).unwrap();
        assert!(html.contains("Cancelled</dt><dd>1</dd>"), "{html}");
    }
}
