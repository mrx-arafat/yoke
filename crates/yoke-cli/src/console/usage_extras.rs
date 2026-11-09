//! Usage extras: today and the daily average, a straight-line forecast for this month, cost by
//! model, Git correlation for the console's project, and commands that rewrite gates changed.
//! Everything is computed from what Yoke recorded; nothing is fetched or guessed.
use super::super::insights::activity::grouped;
use super::{cost, render::escape, Console};
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use yoke_core::{
    git,
    store::Store,
    usage::{self, Spend, UsageRange},
};

/// Calendar days in the daily average, today included (Pilot Shell also uses 14).
const PACE_DAYS: i64 = 14;
/// Commits read from the current branch, newest first.
const COMMIT_LIMIT: usize = 5000;
/// The Usage page re-renders on every recorded change, so Git is read at most this often.
const COMMIT_CACHE: Duration = Duration::from_secs(30);

type Commits = Arc<Vec<git::CommitTime>>;
static COMMITS: Mutex<Option<(Instant, PathBuf, Commits)>> = Mutex::new(None);

/// Commit times of `project`, reused for `COMMIT_CACHE`; failures are never cached.
fn commits(project: &Path) -> Result<Commits> {
    let mut cache = COMMITS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((at, path, commits)) = cache.as_ref() {
        if path == project && at.elapsed() < COMMIT_CACHE {
            return Ok(Arc::clone(commits));
        }
    }
    let commits = Arc::new(git::Repo::open(project)?.commit_times(COMMIT_LIMIT)?);
    *cache = Some((Instant::now(), project.to_owned(), Arc::clone(&commits)));
    Ok(commits)
}

/// `$12.34`, `<$0.01` for a positive amount under a cent.
pub(super) fn usd(nano: i64) -> String {
    let nano = nano.max(0);
    let cents = (nano + 5_000_000) / 10_000_000;
    if nano > 0 && cents == 0 {
        return "&lt;$0.01".into();
    }
    format!("${}.{:02}", grouped(cents as u64 / 100), cents % 100)
}

fn stat(label: &str, value: &str, hint: &str) -> String {
    format!(
        "<div class=\"usage-stat\"><dt>{}</dt><dd>{value}</dd><small>{}</small></div>",
        escape(label),
        hint
    )
}

fn unknown_note(unknown: u64) -> String {
    match unknown {
        0 => String::new(),
        1 => " 1 sample has no known price and is left out of these sums.".into(),
        n => format!(" {n} samples have no known price and are left out of these sums."),
    }
}

/// The four extras panels for one project and date range.
pub(super) fn panels(
    store: &Store,
    c: &Console,
    project: &str,
    range: &UsageRange,
    now: i64,
) -> Result<String> {
    let mut out = pace_panel(store, project, now, range.offset_minutes())?;
    out.push_str(&model_panel(store, project, range)?);
    out.push_str(&git_panel(store, c, project, range)?);
    out.push_str(&rewrite_panel(store, project, range)?);
    Ok(out)
}

fn pace_panel(store: &Store, project: &str, now: i64, offset_minutes: i32) -> Result<String> {
    let pace = usage::pace(store, Some(project), now, offset_minutes, PACE_DAYS)?;
    let tokens = |spend: &Spend| grouped(spend.tokens);
    let unknown = pace
        .window
        .unknown_pricing_samples
        .max(pace.month_to_date.unknown_pricing_samples);
    Ok(format!(
        "<section class=\"panel\"><h2>Today and pace</h2><dl class=\"usage-stats\">{}{}{}{}</dl><p class=\"muted\">These ignore the date filter: calendar days in {}, tokens are input plus output, and the forecast is a straight line from the month so far with today counted as a whole day.{}</p></section>",
        stat("Today", &usd(pace.today.known_nano_usd), &format!("{} tokens on {}", tokens(&pace.today), pace.today_label)),
        stat(
            "Average per day",
            &usd(pace.average_known_nano_usd()),
            &format!("{} tokens a day over the last {} days", grouped(pace.average_tokens()), pace.window_days)
        ),
        stat(
            &format!("{} so far", pace.month_label),
            &usd(pace.month_to_date.known_nano_usd),
            &format!("{} tokens; day {} of {}", tokens(&pace.month_to_date), pace.day_of_month, pace.days_in_month)
        ),
        stat(
            &format!("Forecast for {}", pace.month_label),
            &usd(pace.forecast_known_nano_usd()),
            "if the month continues at its current daily rate"
        ),
        super::offset_label(offset_minutes),
        unknown_note(unknown)
    ))
}

fn model_panel(store: &Store, project: &str, range: &UsageRange) -> Result<String> {
    let models = usage::by_model(store, Some(project), range)?;
    let mut out = String::from("<section class=\"panel\"><h2>Cost by model</h2>");
    if models.is_empty() {
        out.push_str("<p class=\"muted\">No usage in this range.</p></section>");
        return Ok(out);
    }
    out.push_str("<div class=\"table-scroll\" tabindex=\"0\" role=\"region\" aria-label=\"Cost by model\"><table class=\"usage-table\"><caption>Models in this range, most priced spend first</caption><thead><tr><th scope=\"col\">Model</th><th scope=\"col\">Samples</th><th scope=\"col\">Input</th><th scope=\"col\">Output</th><th scope=\"col\">Estimated USD</th></tr></thead><tbody>");
    for model in &models {
        let t = &model.totals;
        out.push_str(&format!(
            "<tr><th scope=\"row\">{}</th><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            escape(&model.model),
            t.samples,
            grouped(t.input_tokens),
            grouped(t.output_tokens),
            escape(&cost(
                t.estimated_cost_usd.as_deref(),
                t.known_cost_usd.as_deref(),
                t.unknown_pricing_samples
            ))
        ));
    }
    out.push_str("</tbody></table></div></section>");
    Ok(out)
}

fn git_panel(store: &Store, c: &Console, project: &str, range: &UsageRange) -> Result<String> {
    let mut out = String::from("<section class=\"panel\"><h2>Sessions that landed code</h2>");
    let console_project = c.project.to_string_lossy();
    if project != console_project {
        out.push_str(&format!(
            "<p class=\"muted\">Git correlation covers the console's own project, {}. Choose it in the project filter to see it.</p></section>",
            escape(
                Path::new(console_project.as_ref())
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(&console_project)
            )
        ));
        return Ok(out);
    }
    let windows = usage::session_windows(store, project, range)?;
    if windows.is_empty() {
        out.push_str(
            "<p class=\"muted\">No dated usage in this range, so there is nothing to match commits to.</p></section>",
        );
        return Ok(out);
    }
    let commits = match commits(&c.project) {
        Ok(commits) => commits,
        Err(error) => {
            out.push_str(&format!(
                "<p class=\"muted\">No commits to match: {}</p></section>",
                escape(&yoke_core::redact::redact(&format!("{error:#}")))
            ));
            return Ok(out);
        }
    };
    let times: Vec<i64> = commits.iter().map(|commit| commit.authored_at).collect();
    let result = usage::correlate(&windows, &times);
    let share = if result.total_known_nano_usd > 0 {
        result.landed_known_nano_usd as f64 / result.total_known_nano_usd as f64
    } else {
        0.0
    };
    out.push_str(&format!(
        "<dl class=\"usage-stats\">{}{}{}{}</dl><progress max=\"1\" value=\"{share:.4}\" aria-label=\"Share of priced spend in sessions that landed a commit\">{:.0}%</progress>",
        stat("Commits landed", &grouped(result.commits as u64), "authored during a recorded session"),
        stat(
            "Sessions that landed a commit",
            &format!("{} of {}", grouped(result.landed_sessions as u64), grouped(result.sessions as u64)),
            "with usage in this range"
        ),
        stat(
            "Spend that landed code",
            &usd(result.landed_known_nano_usd),
            &format!("of {} ({:.0}%)", usd(result.total_known_nano_usd), share * 100.0)
        ),
        stat(
            "Cost per landed commit",
            &result.known_nano_usd_per_commit().map(usd).unwrap_or_else(|| "No commits".into()),
            "spend of landing sessions divided by their commits"
        ),
        share * 100.0
    ));
    out.push_str(&format!(
        "<p class=\"muted\">A commit on the current branch counts for a session when it was authored between the session's first usage sample and one hour after its last, so concurrent sessions can share a commit.{}{}</p></section>",
        if commits.len() >= COMMIT_LIMIT {
            format!(" Only the newest {COMMIT_LIMIT} commits were read.")
        } else {
            String::new()
        },
        unknown_note(result.unknown_pricing_samples)
    ));
    Ok(out)
}

fn rewrite_panel(store: &Store, project: &str, range: &UsageRange) -> Result<String> {
    let rows = usage::rewrites(store, project, range)?;
    let total: i64 = rows.iter().map(|(_, count)| count).sum();
    let mut out = String::from("<section class=\"panel\"><h2>Commands rewritten by gates</h2>");
    if rows.is_empty() {
        out.push_str("<p class=\"muted\">No command was rewritten by a gate in this range.</p>");
    } else {
        out.push_str(&format!(
            "<p>Yoke's rewrite gates changed {} {} before they ran, in enforce mode.</p><ul>",
            grouped(total as u64),
            if total == 1 { "command" } else { "commands" }
        ));
        for (gate, count) in &rows {
            out.push_str(&format!(
                "<li><code>{}</code>: {}</li>",
                escape(gate),
                grouped(*count as u64)
            ));
        }
        out.push_str("</ul>");
    }
    out.push_str("<p class=\"muted\">This counts what Yoke recorded. It cannot measure how many tokens a rewrite saved, so it claims no savings.</p></section>");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use yoke_core::store::DecisionRecord;

    #[test]
    fn extras_show_pace_models_rewrites_and_explain_missing_git() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let project = root.join("project");
        std::fs::create_dir_all(&project).unwrap();
        let store = Store::open(root.join("state.db")).unwrap();
        let p = project.to_string_lossy().into_owned();
        // 2026-11-10 12:00 UTC.
        let now = 1_794_312_000;
        let transcript = root.join("t.jsonl");
        let line = |id: &str, model: &str, at: &str| {
            serde_json::json!({"type":"assistant","sessionId":"s1","timestamp":at,"message":{"id":id,"model":model,"usage":{"input_tokens":1000,"output_tokens":500}}}).to_string()
        };
        std::fs::write(
            &transcript,
            format!(
                "{}\n{}\n",
                line("a", "model-<x>", "2026-11-10T09:00:00Z"),
                line("b", "model-y", "2026-11-02T09:00:00Z")
            ),
        )
        .unwrap();
        usage::ingest(&store, "claude", "s1", &p, &transcript).unwrap();
        store
            .record_decision(
                &DecisionRecord {
                    policy_hash: None,
                    ts: now - 60,
                    session_id: "s1".into(),
                    agent: "claude".into(),
                    project: p.clone(),
                    event: "pre_tool_use".into(),
                    tool: Some("Bash".into()),
                    mode: "enforce".into(),
                    outcome: "rewrite".into(),
                    reason: None,
                    gates: serde_json::json!([{"name":"rtk","result":"rewrite","ms":1,"exit_code":null}]),
                    payload: "{}".into(),
                },
                4096,
            )
            .unwrap();
        let c = Console {
            home: root.clone(),
            project: project.clone(),
            database: root.join("state.db"),
            token: "token".into(),
            csrf: "csrf".into(),
            host: "127.0.0.1:1".into(),
        };
        let range = UsageRange::new(None, None, 0).unwrap();
        let html = panels(&store, &c, &p, &range, now).unwrap();
        assert!(html.contains("1,500 tokens on 2026-11-10"), "{html}");
        assert!(html.contains("3,000 tokens; day 10 of 30"), "{html}");
        assert!(html.contains("Forecast for 2026-11"));
        assert!(
            html.contains("<th scope=\"row\">model-&lt;x&gt;</th>"),
            "{html}"
        );
        assert!(
            html.contains("changed 1 command before they ran")
                && html.contains("<code>rtk</code>: 1"),
            "{html}"
        );
        assert!(html.contains("No commits to match:"), "{html}");
        let other = panels(&store, &c, "/elsewhere", &range, now).unwrap();
        assert!(
            other.contains("Git correlation covers the console's own project, project."),
            "{other}"
        );
        assert_eq!(
            (usd(1_234_567_890_123), usd(4_000_000)),
            ("$1,234.57".to_owned(), "&lt;$0.01".to_owned())
        );
    }
}
