//! `yoke usage`: quota windows, recent usage and the newest sessions from local records.
//! It never creates, migrates or writes state, so it is safe to run while hooks are live.

use std::fmt::Write as _;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::Serialize;
use yoke_core::quota::{self, Quota};
use yoke_core::timefmt;
use yoke_core::usage::{self, AgentTotals, SessionUsage, UsageTotals, TIME_SCHEMA};

use crate::commands::{self, quote};

const DAY: i64 = 86_400;

#[derive(Serialize)]
struct Report {
    now: i64,
    time_zone: String,
    utc_offset_secs: i64,
    state: State,
    quota: Vec<QuotaRow>,
    usage: Periods,
    sessions: Vec<SessionRow>,
    /// Why something is missing, and what produces it.
    notes: Vec<String>,
}

#[derive(Serialize)]
struct State {
    path: String,
    exists: bool,
    schema: Option<usize>,
    supported: usize,
    /// Whether usage totals and sessions could be read from this schema.
    usage_available: bool,
}

#[derive(Serialize)]
struct QuotaRow {
    agent: String,
    window: String,
    window_minutes: u32,
    used_percent: f64,
    remaining_percent: f64,
    resets_at: i64,
    /// The reset time has passed, so the figures describe the previous window.
    expired: bool,
    /// When a source last confirmed these figures.
    observed_at: i64,
    plan: Option<String>,
    stale: bool,
    #[serde(flatten)]
    trend: quota::Trend,
    history: Vec<quota::Observation>,
}

#[derive(Serialize)]
struct Periods {
    /// Since local midnight.
    today: Vec<AgentTotals>,
    /// The 7 x 24 hours before now.
    last_7_days: Vec<AgentTotals>,
}

#[derive(Serialize)]
struct SessionRow {
    #[serde(flatten)]
    session: SessionUsage,
    /// From the recorded session start to its last usage sample.
    duration_secs: Option<i64>,
}

pub fn run(json: bool, limit: u16) -> Result<()> {
    let now: i64 = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_secs()
        .try_into()?;
    let report = collect(
        &commands::home()?,
        now,
        timefmt::utc_offset_secs(now),
        usize::from(limit),
    )?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", text(&report));
    }
    Ok(())
}

fn collect(home: &Path, now: i64, offset_secs: i64, limit: usize) -> Result<Report> {
    let path = home.join(".yoke/state.db");
    let exists = path.exists();
    // A missing database stays missing: an empty in-memory store answers with nothing.
    let store = commands::inspect_store(home)?;
    let (schema, supported) = store.schema_version()?;
    let usage_available = exists && schema >= TIME_SCHEMA;
    let mut notes = Vec::new();
    if !exists {
        notes.push("No Yoke state exists yet; usage appears after the first hook event.".into());
    } else if !usage_available {
        notes.push(format!(
            "The state schema is version {schema}, older than the {TIME_SCHEMA} that usage times need; the next hook event from an up-to-date yoke upgrades it."
        ));
    }

    let mut readings: Vec<Quota> = Vec::new();
    for (agent, reading, missing) in [
        (
            "claude",
            quota::read_file(&quota::claude_file(home), "claude"),
            "Claude quota is not recorded. It comes from the Claude statusline (`yoke install --agent claude --statusline`) once Claude Code reports rate limits to it.",
        ),
        (
            "codex",
            if exists { quota::load(&store, "codex") } else { Ok(None) },
            "Codex quota is not recorded. It comes from Codex session records on prompts, periodically after tools, and when a session stops or ends, if Codex includes rate limits there.",
        ),
    ] {
        match reading {
            Ok(Some(reading)) => readings.push(reading),
            Ok(None) => notes.push(missing.into()),
            Err(error) => notes.push(format!(
                "The {agent} quota record is unreadable ({error:#}); the next time {agent} reports its quota it is replaced."
            )),
        }
    }
    let quota = readings
        .into_iter()
        .flat_map(|reading| {
            reading.windows.clone().into_iter().map(move |w| QuotaRow {
                agent: reading.agent.clone(),
                window: w.label(),
                window_minutes: w.window_minutes,
                used_percent: w.used_percent,
                remaining_percent: w.remaining_percent(),
                resets_at: w.resets_at,
                expired: w.expired(now),
                observed_at: reading.latest(&w),
                plan: reading.plan.clone(),
                stale: !reading.fresh(&w, now),
                trend: reading.trend(&w, now),
                history: reading
                    .history
                    .iter()
                    .filter(|s| s.window.window_minutes == w.window_minutes)
                    .cloned()
                    .collect(),
            })
        })
        .collect::<Vec<_>>();
    if quota.iter().any(|row| row.expired) {
        notes.push("A window whose reset time has passed shows its last reading until the agent reports the next one.".into());
    }

    let (today, last_7_days, sessions) = if usage_available {
        let sessions = usage::recent_sessions(&store, limit)?;
        if sessions.is_empty() {
            notes.push(
                "No usage is recorded yet; transcripts are imported when a session stops or ends."
                    .into(),
            );
        }
        let partial = usage::partial_imports(&store)?;
        if let Some(limit) = partial.values().map(|p| p.limit_bytes).max() {
            notes.push(format!(
                "{} recorded session(s) are only partly counted: their transcripts are over {}, so usage in the older part is not counted and totals under-report. They show their share in the coverage column.",
                partial.len(),
                mib(limit)
            ));
        }
        (
            usage::totals_by_agent(&store, timefmt::day_start(now, offset_secs))?,
            usage::totals_by_agent(&store, now - 7 * DAY)?,
            sessions,
        )
    } else {
        Default::default()
    };
    Ok(Report {
        now,
        time_zone: timefmt::zone_label(offset_secs),
        utc_offset_secs: offset_secs,
        state: State {
            path: path.display().to_string(),
            exists,
            schema: exists.then_some(schema),
            supported,
            usage_available,
        },
        quota,
        usage: Periods { today, last_7_days },
        sessions: sessions
            .into_iter()
            .map(|session| SessionRow {
                duration_secs: session
                    .started_at
                    .zip(session.last_at)
                    .map(|(a, b)| (b - a).max(0)),
                session,
            })
            .collect(),
        notes,
    })
}

/// `41.3 MiB`.
fn mib(bytes: u64) -> String {
    format!("{:.1} MiB", bytes as f64 / 1_048_576.0)
}

/// 9876, 531k, 1.02M.
fn tokens(count: u64) -> String {
    match count {
        0..10_000 => count.to_string(),
        10_000..1_000_000 => format!("{}k", (count + 500) / 1_000),
        _ => format!("{:.2}M", count as f64 / 1_000_000.0),
    }
}

/// A sum that includes an unknown price is never shown as a definite number.
fn cost(totals: &UsageTotals) -> String {
    let dollars = |text: &str| {
        text.parse::<f64>()
            .map_or_else(|_| "?".into(), |n| format!("${n:.4}"))
    };
    match (&totals.estimated_cost_usd, &totals.known_cost_usd) {
        (Some(estimated), _) => dollars(estimated),
        (None, Some(known)) => format!(
            "{}+{} unpriced",
            dollars(known),
            totals.unknown_pricing_samples
        ),
        (None, None) => "unknown".into(),
    }
}

/// Rows under their headings, indented, with each column as wide as its widest cell and two
/// spaces apart. Columns listed in `right` align right so figures line up.
fn table(headings: &[&str], rows: &[Vec<String>], right: &[usize]) -> Vec<String> {
    let header: Vec<String> = headings.iter().map(|h| (*h).to_owned()).collect();
    let widths: Vec<usize> = (0..header.len())
        .map(|col| {
            std::iter::once(&header)
                .chain(rows)
                .map(|row| row[col].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    std::iter::once(&header)
        .chain(rows)
        .map(|row| {
            let cells: Vec<String> = row
                .iter()
                .enumerate()
                .map(|(col, cell)| {
                    let width = widths[col];
                    if right.contains(&col) {
                        format!("{cell:>width$}")
                    } else {
                        format!("{cell:<width$}")
                    }
                })
                .collect();
            format!("  {}", cells.join("  ")).trim_end().to_owned()
        })
        .collect()
}

fn totals_row(totals: &UsageTotals) -> String {
    format!(
        "{},{},{},{},{}",
        tokens(totals.input_tokens),
        tokens(totals.output_tokens),
        tokens(totals.cache_read_tokens),
        tokens(totals.cache_write_tokens),
        quote(&cost(totals))
    )
}

fn text(report: &Report) -> String {
    let (now, zone, offset) = (report.now, &report.time_zone, report.utc_offset_secs);
    let mut out = String::new();
    let mut line = |text: String| writeln!(out, "{text}").expect("writing to a String cannot fail");
    line(format!("now: {}", quote(&timefmt::full_time(now, offset))));
    line(format!("time_zone: {}", quote(zone)));
    line(format!(
        "state: {} schema={} supported={}",
        quote(&report.state.path),
        report
            .state
            .schema
            .map_or("missing".to_owned(), |s| s.to_string()),
        report.state.supported
    ));
    if report.quota.is_empty() {
        line("quota: none recorded".into());
    } else {
        line("quota:".into());
        let rows: Vec<Vec<String>> = report
            .quota
            .iter()
            .map(|row| {
                vec![
                    row.agent.clone(),
                    row.window.clone(),
                    format!("{}%", quota::percent(row.used_percent)),
                    format!("{}%", quota::percent(row.remaining_percent)),
                    timefmt::full_time(row.resets_at, offset),
                    if row.expired {
                        "reset".to_owned()
                    } else {
                        timefmt::duration_text(row.resets_at - now)
                    },
                    format!("{} ago", timefmt::duration_text(now - row.observed_at)),
                    row.plan.clone().unwrap_or_else(|| "-".to_owned()),
                ]
            })
            .collect();
        let headings = [
            "agent", "window", "used", "left", "resets", "in", "observed", "plan",
        ];
        for row in table(&headings, &rows, &[2, 3]) {
            line(row);
        }
        line("quota_outlook:".into());
        for row in &report.quota {
            let outlook = if row.expired {
                "reset passed; waiting for the next reading".into()
            } else if row.stale {
                "stale reading; no projection (freshness limit: 5m)".into()
            } else if row.used_percent >= 100.0 {
                "limit reached; waiting for reset".into()
            } else if let Some(rate) = row.trend.percent_per_hour {
                let end = row.trend.exhausts_at.map_or_else(
                    || "no exhaustion before reset at this rate".into(),
                    |at| {
                        format!(
                            "may reach 100% at {} (in {})",
                            timefmt::full_time(at, offset),
                            timefmt::duration_text((at - now).max(0))
                        )
                    },
                );
                format!(
                    "{rate:.1} percentage points/h over {}; {end}",
                    timefmt::duration_text(row.trend.span_secs.unwrap_or(0))
                )
            } else {
                "need at least 1m of recent same-window readings to estimate burn".into()
            };
            line(format!("  {} {}: {outlook}", row.agent, row.window));
        }
    }
    let rows = report
        .usage
        .today
        .iter()
        .map(|t| ("today", t))
        .chain(report.usage.last_7_days.iter().map(|t| ("7d", t)));
    let mut rows = rows.collect::<Vec<_>>();
    rows.sort_by(|a, b| a.1.agent.cmp(&b.1.agent).then(a.0.cmp(b.0).reverse()));
    line(format!(
        "usage[{}]{{agent,period,input,output,cache_read,cache_write,cost}}:",
        rows.len()
    ));
    for (period, row) in rows {
        line(format!(
            "  {},{period},{}",
            row.agent,
            totals_row(&row.totals)
        ));
    }
    line(format!(
        "sessions[{}]{{agent,session,project,models,started,duration,input,output,cache_read,cache_write,cost,coverage}}:",
        report.sessions.len()
    ));
    for row in &report.sessions {
        let session = &row.session;
        let project = Path::new(&session.project)
            .file_name()
            .map_or(session.project.as_str(), |name| name.to_str().unwrap_or(""));
        line(format!(
            "  {},{},{},{},{},{},{},{}",
            session.agent,
            quote(&session.session_id),
            quote(project),
            quote(&session.models.join(" ")),
            session
                .started_at
                .map_or("unknown".to_owned(), |t| quote(&timefmt::full_time(
                    t, offset
                ))),
            row.duration_secs
                .map_or("unknown".to_owned(), |s| quote(&timefmt::duration_text(s))),
            totals_row(&session.totals),
            session
                .partial
                .as_ref()
                .map_or("full".to_owned(), |p| quote(&format!(
                    "counted {} of {}",
                    mib(p.transcript_bytes - p.skipped_bytes),
                    mib(p.transcript_bytes)
                )))
        ));
    }
    line(format!("notes[{}]:", report.notes.len()));
    for note in &report.notes {
        line(format!("  {}", quote(note)));
    }
    line("help: \"Quota percentages are what each agent last reported. Costs are API-price estimates, not subscription bills. Run yoke usage --json for structured output.\"".into());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use yoke_core::store::Store;

    /// 2026-10-16 10:00 UTC, a Friday.
    const NOW: i64 = 1_792_144_800;

    fn claude_row(id: &str, session: &str, model: &str, input: u64, ts: &str) -> serde_json::Value {
        json!({"timestamp":ts,"type":"assistant","sessionId":session,"message":{"id":id,"model":model,"usage":{"input_tokens":input,"output_tokens":100,"service_tier":"standard","inference_geo":"global","speed":"standard"}}})
    }

    fn import(store: &Store, dir: &Path, agent: &str, session: &str, rows: &[serde_json::Value]) {
        let path = dir.join("transcript.jsonl");
        let lines: String = rows.iter().map(|row| format!("{row}\n")).collect();
        std::fs::write(&path, lines).unwrap();
        usage::ingest(store, agent, session, "/work/app", &path).unwrap();
    }

    #[test]
    fn reports_quota_usage_and_sessions_from_both_agents_without_writing_anything() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".yoke")).unwrap();
        {
            let store = Store::open(home.path().join(".yoke/state.db")).unwrap();
            import(
                &store,
                home.path(),
                "claude",
                "recent",
                &[
                    claude_row(
                        "m1",
                        "recent",
                        "claude-haiku-5-5",
                        1000,
                        "2026-10-16T08:00:00Z",
                    ),
                    claude_row(
                        "m2",
                        "recent",
                        "claude-haiku-5-5",
                        500,
                        "2026-10-16T09:30:00Z",
                    ),
                ],
            );
            import(
                &store,
                home.path(),
                "claude",
                "older",
                &[claude_row(
                    "m3",
                    "older",
                    "unknown",
                    70,
                    "2026-10-12T08:00:00Z",
                )],
            );
            import(
                &store,
                home.path(),
                "codex",
                "cx",
                &[
                    json!({"timestamp":"2026-10-16T09:00:00Z","type":"event_msg","payload":{"type":"token_count",
                "info":{"total_token_usage":{"input_tokens":40,"output_tokens":1}},
                "rate_limits":{"plan_type":"pro","primary":{"used_percent":35.0,"window_minutes":10080,"resets_at":1_792_649_595},"secondary":null}}}),
                ],
            );
            // The 41,312,576-byte transcript of this session was only counted from its newest 32 MiB.
            store
                .set_kv(
                    "usage.partial.claude.older",
                    &json!({"transcript_bytes":41_312_576_u64,"skipped_bytes":7_758_144_u64,"limit_bytes":33_554_432_u64}).to_string(),
                )
                .unwrap();
        }
        let reading = Quota::from_claude(
            &json!({"rate_limits":{"five_hour":{"used_percentage":8,"resets_at":NOW + 3_600},"seven_day":{"used_percentage":63,"resets_at":NOW + 3 * DAY}}}),
            NOW - 120,
        )
        .unwrap();
        assert!(quota::record_file(&quota::claude_file(home.path()), &reading, NOW - 120).unwrap());
        let before = std::fs::read(home.path().join(".yoke/state.db")).unwrap();

        let report = collect(home.path(), NOW, 6 * 3_600, 10).unwrap();
        assert_eq!(
            std::fs::read(home.path().join(".yoke/state.db")).unwrap(),
            before
        );
        assert_eq!(report.notes.len(), 1, "{:?}", report.notes);
        assert!(report.notes[0].starts_with(
            "1 recorded session(s) are only partly counted: their transcripts are over 32.0 MiB"
        ));
        let quota_rows: Vec<_> = report
            .quota
            .iter()
            .map(|r| {
                (
                    r.agent.as_str(),
                    r.window.as_str(),
                    r.used_percent,
                    r.remaining_percent,
                )
            })
            .collect();
        assert_eq!(
            quota_rows,
            [
                ("claude", "5h", 8.0, 92.0),
                ("claude", "7d", 63.0, 37.0),
                ("codex", "7d", 35.0, 65.0)
            ]
        );
        // Local midnight at UTC+6 is 18:00 UTC the day before, so both samples of "recent" count.
        let tokens_of = |totals: &[AgentTotals]| -> Vec<(String, u64)> {
            totals
                .iter()
                .map(|t| (t.agent.clone(), t.totals.input_tokens))
                .collect()
        };
        assert_eq!(
            tokens_of(&report.usage.today),
            [("claude".into(), 1500), ("codex".into(), 40)]
        );
        assert_eq!(
            tokens_of(&report.usage.last_7_days),
            [("claude".into(), 1570), ("codex".into(), 40)]
        );
        let sessions: Vec<_> = report
            .sessions
            .iter()
            .map(|s| (s.session.session_id.as_str(), s.duration_secs))
            .collect();
        assert_eq!(
            sessions,
            [("recent", Some(5_400)), ("cx", Some(0)), ("older", Some(0))]
        );

        let text = text(&report);
        // Quota reads as an aligned table for people; the structured form is `--json`.
        let quota_block: Vec<&str> = text
            .lines()
            .skip_while(|l| *l != "quota:")
            .take(5)
            .collect();
        assert_eq!(
            quota_block,
            [
                "quota:",
                "  agent   window  used  left  resets            in      observed   plan",
                "  claude  5h        8%   92%  2026-10-16 17:00  1h 0m   2m ago     -",
                "  claude  7d       63%   37%  2026-10-19 16:00  3d 0h   2m ago     -",
                "  codex   7d       35%   65%  2026-10-22 12:13  5d 20h  1h 0m ago  pro",
            ]
        );
        assert_eq!(
            serde_json::to_value(&report).unwrap()["quota"][2]["plan"],
            "pro"
        );
        for expected in [
            "time_zone: \"UTC+06:00\"",
            "  claude,today,1500,200,0,0,\"$0.0003\"",
            "  claude,\"recent\",\"app\",\"claude-haiku-5-5\",\"2026-10-16 14:00\",\"1h 30m\",1500,200,0,0,\"$0.0003\",full",
            "  claude,\"older\",\"app\",\"unknown\",\"2026-10-12 14:00\",\"<1m\",70,100,0,0,\"unknown\",\"counted 32.0 MiB of 39.4 MiB\"",
            "notes[1]:",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in\n{text}");
        }
    }

    #[test]
    fn missing_state_says_what_is_missing_and_what_produces_it() {
        let home = tempfile::tempdir().unwrap();
        let report = collect(home.path(), NOW, 0, 10).unwrap();
        assert!(!report.state.exists && !report.state.usage_available);
        assert!(report.quota.is_empty() && report.sessions.is_empty());
        assert!(text(&report).contains("\nquota: none recorded\n"));
        let notes = report.notes.join("\n");
        for expected in [
            "No Yoke state exists yet",
            "Claude quota is not recorded",
            "Codex quota is not recorded",
        ] {
            assert!(notes.contains(expected), "{notes}");
        }
        assert!(!home.path().join(".yoke").exists());
    }
}
