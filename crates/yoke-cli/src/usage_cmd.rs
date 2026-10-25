//! `yoke usage`: quota windows, recent usage and the newest sessions from local records.
//! It never creates, migrates or writes state, so it is safe to run while hooks are live.

use std::collections::BTreeSet;
use std::io::IsTerminal;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::Serialize;
use unicode_width::UnicodeWidthStr;
use yoke_core::quota::{self, Quota};
use yoke_core::timefmt;
use yoke_core::usage::{self, AgentTotals, SessionUsage, UsageTotals, TIME_SCHEMA};

use crate::commands;
use crate::terminal::{self, quota_tone, Tone};

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
        print!("{}", text(&report, &Style::from_env()));
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

/// Only the priced portion, with the unpriced sample count in its own column.
fn cost(totals: &UsageTotals) -> String {
    totals
        .estimated_cost_usd
        .as_ref()
        .or(totals.known_cost_usd.as_ref())
        .and_then(|value| value.parse::<f64>().ok())
        .map_or_else(|| "unknown".into(), |value| format!("${value:.4}"))
}

struct Style {
    width: usize,
    color: bool,
}

impl Style {
    fn from_env() -> Self {
        let terminal = std::io::stdout().is_terminal();
        let width = std::env::var("COLUMNS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|n| *n > 0)
            .or_else(|| {
                if !terminal {
                    return None;
                }
                // SAFETY: TIOCGWINSZ only writes to the local winsize for a live descriptor.
                let mut size: libc::winsize = unsafe { std::mem::zeroed() };
                (unsafe {
                    libc::ioctl(
                        std::io::stdout().as_raw_fd(),
                        libc::TIOCGWINSZ as _,
                        &mut size,
                    )
                } == 0
                    && size.ws_col > 0)
                    .then_some(usize::from(size.ws_col))
            })
            .unwrap_or(80);
        Self {
            width,
            color: terminal::color_allowed(
                terminal,
                std::env::var_os("NO_COLOR")
                    .map(|v| v.to_string_lossy().into_owned())
                    .as_deref(),
                std::env::var("TERM").ok().as_deref(),
            ),
        }
    }

    fn paragraph(&self, out: &mut Vec<String>, text: &str, indent: usize, tone: Tone) {
        let prefix = " ".repeat(indent.min(self.width.saturating_sub(2)));
        let room = self.width.saturating_sub(prefix.len()).max(1);
        let mut line = String::new();
        for word in clean(text).split_whitespace() {
            if !line.is_empty() && line.width() + 1 + word.width() > room {
                out.push(tone.paint(&format!("{prefix}{line}"), self.color));
                line.clear();
            }
            if !line.is_empty() {
                line.push(' ');
            }
            for ch in word.chars() {
                let mut next = line.clone();
                next.push(ch);
                if next.width() > room && !line.is_empty() {
                    out.push(tone.paint(&format!("{prefix}{line}"), self.color));
                    line.clear();
                }
                line.push(ch);
            }
        }
        if !line.is_empty() {
            out.push(tone.paint(&format!("{prefix}{line}"), self.color));
        }
    }

    fn section(&self, out: &mut Vec<String>, name: &str) {
        out.push(String::new());
        self.paragraph(out, name, 0, Tone::Plain);
    }
}

fn clean(raw: &str) -> String {
    raw.chars()
        .map(|ch| {
            if ch.is_control() {
                ch.escape_default().to_string()
            } else {
                ch.to_string()
            }
        })
        .collect()
}

fn clipped(raw: &str, width: usize) -> String {
    let text = clean(raw);
    if text.width() <= width {
        return text;
    }
    let mut out = String::new();
    for ch in text.chars() {
        let mut next = out.clone();
        next.push(ch);
        if next.width() + 3 > width {
            break;
        }
        out.push(ch);
    }
    out + "..."
}

struct Cell {
    text: String,
    tone: Tone,
}

impl Cell {
    fn new(text: impl AsRef<str>, tone: Tone) -> Self {
        Self {
            text: clean(text.as_ref()),
            tone,
        }
    }

    fn plain(text: impl AsRef<str>) -> Self {
        Self::new(text, Tone::Plain)
    }

    fn padded(&self, width: usize, right: bool, color: bool) -> String {
        let pad = " ".repeat(width.saturating_sub(self.text.width()));
        let text = if right {
            format!("{pad}{}", self.text)
        } else {
            format!("{}{pad}", self.text)
        };
        self.tone.paint(&text, color)
    }
}

/// Narrow screens use labelled records; numbers and warnings are never cut to fit a table.
fn table(
    headings: &[&str],
    rows: &[Vec<Cell>],
    right: &[usize],
    details: &[Vec<String>],
    style: &Style,
) -> Vec<String> {
    let widths: Vec<_> = headings
        .iter()
        .enumerate()
        .map(|(col, heading)| {
            rows.iter()
                .map(|row| row[col].text.width())
                .chain([heading.width()])
                .max()
                .unwrap_or(0)
        })
        .collect();
    let wide =
        2 + widths.iter().sum::<usize>() + 2 * headings.len().saturating_sub(1) <= style.width;
    let draw = |row: &[Cell]| {
        let cells: Vec<_> = row
            .iter()
            .enumerate()
            .map(|(col, cell)| {
                let right = right.contains(&col);
                let width = if col + 1 == row.len() && !right {
                    cell.text.width()
                } else {
                    widths[col]
                };
                cell.padded(width, right, style.color)
            })
            .collect();
        format!("  {}", cells.join("  "))
    };
    let mut out = Vec::new();
    if wide {
        let header: Vec<_> = headings.iter().map(|h| Cell::new(h, Tone::Dim)).collect();
        out.push(draw(&header));
    }
    for (i, row) in rows.iter().enumerate() {
        if i > 0 && (!wide || !details.is_empty()) {
            out.push(String::new());
        }
        if wide {
            out.push(draw(row));
        } else {
            let label_width = headings.iter().map(|h| h.width()).max().unwrap_or(0);
            for (col, cell) in row.iter().enumerate() {
                let label = format!(
                    "{:<label_width$}",
                    format!("{}:", headings[col]),
                    label_width = label_width + 1
                );
                let room = style.width.saturating_sub(4 + label_width);
                if room >= cell.text.width() && room > 0 {
                    let value = cell.padded(
                        if right.contains(&col) {
                            room
                        } else {
                            cell.text.width()
                        },
                        right.contains(&col),
                        style.color,
                    );
                    out.push(format!("  {label} {value}"));
                } else {
                    style.paragraph(&mut out, &format!("{}:", headings[col]), 2, Tone::Dim);
                    style.paragraph(&mut out, &cell.text, 4, cell.tone);
                }
            }
        }
        if let Some(details) = details.get(i) {
            for text in details {
                style.paragraph(&mut out, text, 4, Tone::Dim);
            }
        }
    }
    out
}

fn quota_rank(row: &QuotaRow) -> u8 {
    if row.expired || row.stale {
        3
    } else if row.used_percent >= 100.0 {
        0
    } else if row.trend.exhausts_at.is_some() {
        1
    } else {
        2
    }
}

fn outlook(row: &QuotaRow, now: i64) -> String {
    if row.expired {
        "reset passed; awaiting reading".into()
    } else if row.stale {
        "stale; no forecast".into()
    } else if row.used_percent >= 100.0 {
        "limit reached".into()
    } else if let Some(at) = row.trend.exhausts_at {
        if at <= now {
            "estimated limit already due".into()
        } else {
            format!("~{} to limit", timefmt::duration_text(at - now))
        }
    } else if row.trend.percent_per_hour.is_some() {
        "not before reset at this rate".into()
    } else {
        "need more samples".into()
    }
}

fn text(report: &Report, style: &Style) -> String {
    let (now, offset) = (report.now, report.utc_offset_secs);
    let mut out = Vec::new();
    style.paragraph(
        &mut out,
        &format!(
            "Usage - {} ({})",
            timefmt::full_time(now, offset),
            report.time_zone
        ),
        0,
        Tone::Plain,
    );
    // Sort borrowed rows for people; JSON keeps the original report order and values.
    let mut quota: Vec<_> = report.quota.iter().collect();
    quota.sort_by(|a, b| {
        quota_rank(a)
            .cmp(&quota_rank(b))
            .then(
                a.trend
                    .exhausts_at
                    .unwrap_or(i64::MAX)
                    .cmp(&b.trend.exhausts_at.unwrap_or(i64::MAX)),
            )
            .then(b.used_percent.total_cmp(&a.used_percent))
    });
    if let Some(row) = quota.iter().find(|r| !r.stale && !r.expired) {
        let tone = if row.trend.exhausts_at.is_some() {
            Tone::Bad
        } else {
            quota_tone(row.used_percent)
        };
        style.paragraph(
            &mut out,
            &format!(
                "{} {}: {:.1}% left; {}; reset in {}.",
                row.agent,
                row.window,
                row.remaining_percent,
                outlook(row, now),
                timefmt::duration_text(row.resets_at - now)
            ),
            0,
            tone,
        );
    } else if !quota.is_empty() {
        style.paragraph(
            &mut out,
            "Quota readings are stale or reset; no current forecast.",
            0,
            Tone::Warn,
        );
    }
    style.section(&mut out, "Quota / account limits");
    if quota.is_empty() {
        style.paragraph(&mut out, "None recorded.", 2, Tone::Dim);
    } else {
        let rows = quota
            .iter()
            .map(|row| {
                let tone = if row.stale || row.expired {
                    Tone::Dim
                } else {
                    quota_tone(row.used_percent)
                };
                vec![
                    Cell::plain(&row.agent),
                    Cell::plain(&row.window),
                    Cell::new(format!("{:.1}%", row.remaining_percent), tone),
                    Cell::new(format!("{:.1}%", row.used_percent), tone),
                    Cell::plain(if row.expired {
                        "reset".into()
                    } else {
                        timefmt::duration_text(row.resets_at - now)
                    }),
                    Cell::new(
                        if row.expired {
                            "reset passed".into()
                        } else if row.stale {
                            format!(
                                "stale ({} ago)",
                                timefmt::duration_text(now - row.observed_at)
                            )
                        } else {
                            format!("{} ago", timefmt::duration_text(now - row.observed_at))
                        },
                        if row.stale { Tone::Warn } else { Tone::Dim },
                    ),
                ]
            })
            .collect::<Vec<_>>();
        out.extend(table(
            &["Agent", "Window", "Left", "Used", "Reset in", "Reading"],
            &rows,
            &[2, 3, 4],
            &[],
            style,
        ));
        style.section(&mut out, "Burn estimates");
        let rows = quota
            .iter()
            .map(|row| {
                let current = !row.stale && !row.expired;
                let tone = if !current {
                    Tone::Dim
                } else if row.used_percent >= 100.0 || row.trend.exhausts_at.is_some() {
                    Tone::Bad
                } else if row.trend.percent_per_hour.is_some() {
                    Tone::Good
                } else {
                    Tone::Dim
                };
                vec![
                    Cell::plain(&row.agent),
                    Cell::plain(&row.window),
                    Cell::new(
                        row.trend
                            .percent_per_hour
                            .filter(|_| current)
                            .map_or("-".into(), |rate| format!("{rate:.1}")),
                        tone,
                    ),
                    Cell::plain(
                        row.trend
                            .span_secs
                            .filter(|_| current)
                            .map_or("-".into(), timefmt::duration_text),
                    ),
                    Cell::new(outlook(row, now), tone),
                    Cell::plain(timefmt::full_time(row.resets_at, offset)),
                ]
            })
            .collect::<Vec<_>>();
        out.extend(table(
            &[
                "Agent",
                "Window",
                "pp/h",
                "Over",
                "100% estimate",
                "Resets at",
            ],
            &rows,
            &[2, 3],
            &[],
            style,
        ));
        style.paragraph(&mut out, "pp/h = percentage points/hour. Assumes recent burn continues; no forecast after 5m without a reading.", 2, Tone::Dim);
        let plans: BTreeSet<_> = quota
            .iter()
            .filter_map(|row| {
                row.plan
                    .as_ref()
                    .map(|plan| format!("{} plan: {plan}", row.agent))
            })
            .collect();
        for plan in plans {
            style.paragraph(&mut out, &plan, 2, Tone::Dim);
        }
    }
    style.section(&mut out, "Token history");
    let rows: Vec<_> = report
        .usage
        .today
        .iter()
        .map(|row| ("Today", row))
        .chain(report.usage.last_7_days.iter().map(|row| ("7d", row)))
        .map(|(period, row)| {
            let t = &row.totals;
            vec![
                Cell::plain(period),
                Cell::plain(&row.agent),
                Cell::plain(tokens(t.input_tokens)),
                Cell::plain(tokens(t.output_tokens)),
                Cell::plain(tokens(t.cache_read_tokens)),
                Cell::plain(tokens(t.cache_write_tokens)),
                Cell::plain(cost(t)),
                Cell::plain(t.unknown_pricing_samples.to_string()),
            ]
        })
        .collect();
    if rows.is_empty() {
        style.paragraph(&mut out, "None recorded.", 2, Tone::Dim);
    } else {
        out.extend(table(
            &[
                "Period",
                "Agent",
                "Input",
                "Output",
                "Cache R",
                "Cache W",
                "Known USD",
                "Unpriced",
            ],
            &rows,
            &[2, 3, 4, 5, 6, 7],
            &[],
            style,
        ));
    }
    style.paragraph(&mut out, "Today starts at local midnight; 7d is the last 7 x 24h. Known USD excludes unpriced samples; their tokens are included. API estimates, not subscription bills.", 2, Tone::Dim);
    style.section(&mut out, "Recent sessions");
    let rows: Vec<_> = report
        .sessions
        .iter()
        .map(|row| {
            let s = &row.session;
            vec![
                Cell::plain(&s.agent),
                Cell::plain(clipped(&s.session_id, 12)),
                Cell::plain(tokens(s.totals.input_tokens)),
                Cell::plain(tokens(s.totals.output_tokens)),
                Cell::plain(tokens(s.totals.cache_read_tokens)),
                Cell::plain(tokens(s.totals.cache_write_tokens)),
                Cell::plain(cost(&s.totals)),
            ]
        })
        .collect();
    let details: Vec<_> = report
        .sessions
        .iter()
        .map(|row| {
            let s = &row.session;
            let project = Path::new(&s.project)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(&s.project);
            let mut lines = vec![
                format!("Project: {project}; models: {}", s.models.join(" ")),
                format!(
                    "Started: {}; duration: {}; unpriced: {}",
                    s.started_at
                        .map_or("unknown".into(), |at| timefmt::full_time(at, offset)),
                    row.duration_secs
                        .map_or("unknown".into(), timefmt::duration_text),
                    s.totals.unknown_pricing_samples
                ),
                format!(
                    "Coverage: {}",
                    s.partial.as_ref().map_or("full".into(), |p| format!(
                        "counted {} of {}",
                        mib(p.transcript_bytes - p.skipped_bytes),
                        mib(p.transcript_bytes)
                    ))
                ),
            ];
            if clipped(&s.session_id, 12) != clean(&s.session_id) {
                lines.insert(0, format!("Session: {}", s.session_id));
            }
            lines
        })
        .collect();
    if rows.is_empty() {
        style.paragraph(&mut out, "None recorded.", 2, Tone::Dim);
    } else {
        out.extend(table(
            &[
                "Agent",
                "Session",
                "Input",
                "Output",
                "Cache R",
                "Cache W",
                "Known USD",
            ],
            &rows,
            &[2, 3, 4, 5, 6],
            &details,
            style,
        ));
    }
    if !report.notes.is_empty() {
        style.section(&mut out, "Notes");
        for note in &report.notes {
            style.paragraph(&mut out, &format!("- {note}"), 2, Tone::Dim);
        }
    }
    out.push(String::new());
    style.paragraph(
        &mut out,
        &format!(
            "State: {} (schema {} / {}).",
            report.state.path,
            report
                .state
                .schema
                .map_or("missing".into(), |s| s.to_string()),
            report.state.supported
        ),
        0,
        Tone::Dim,
    );
    style.paragraph(
        &mut out,
        "yoke usage --json: full structured records.",
        0,
        Tone::Dim,
    );
    out.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use yoke_core::store::Store;

    /// 2026-10-16 10:00 UTC, a Friday.
    const NOW: i64 = 1_792_144_800;

    #[test]
    fn table_aligns_unicode_labels_and_decimal_right_edges() {
        assert_eq!(Cell::plain("run\n\x1b[31m").text, "run\\n\\u{1b}[31m");
        let rows = vec![
            vec![
                Cell::plain("表a"),
                Cell::plain("7.5%"),
                Cell::plain("$0.0100"),
            ],
            vec![
                Cell::plain("e\u{301}"),
                Cell::plain("25.0%"),
                Cell::plain("$2.0000"),
            ],
        ];
        assert_eq!(
            table(
                &["Project", "Left", "Cost"],
                &rows,
                &[1, 2],
                &[],
                &Style {
                    width: 80,
                    color: false
                }
            ),
            [
                "  Project   Left     Cost",
                "  表a       7.5%  $0.0100",
                "  e\u{301}        25.0%  $2.0000",
            ]
        );
        assert_eq!(
            table(
                &["Project", "Left", "Cost"],
                &rows[..1],
                &[1, 2],
                &[],
                &Style {
                    width: 20,
                    color: false
                }
            ),
            [
                "  Project: 表a",
                "  Left:         7.5%",
                "  Cost:      $0.0100",
            ]
        );
    }

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

        let mut report = collect(home.path(), NOW, 6 * 3_600, 10).unwrap();
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

        let text = text(
            &report,
            &Style {
                width: 120,
                color: false,
            },
        );
        // Quota reads as an aligned table for people; the structured form is `--json`.
        let quota_block: Vec<&str> = text
            .lines()
            .skip_while(|l| *l != "Quota / account limits")
            .take(5)
            .collect();
        assert_eq!(
            quota_block,
            [
                "Quota / account limits",
                "  Agent   Window   Left   Used  Reset in  Reading",
                "  claude  7d      37.0%  63.0%     3d 0h  2m ago",
                "  claude  5h      92.0%   8.0%     1h 0m  2m ago",
                "  codex   7d      65.0%  35.0%    5d 20h  stale (1h 0m ago)",
            ]
        );
        assert_eq!(
            serde_json::to_value(&report).unwrap()["quota"][2]["plan"],
            "pro"
        );
        for expected in [
            "Usage - 2026-10-16 16:00 (UTC+06:00)",
            "  Period  Agent   Input  Output  Cache R  Cache W  Known USD  Unpriced",
            "  Today   claude   1500     200        0        0    $0.0003         0",
            "  7d      claude   1570     300        0        0    $0.0003         1",
            "  Agent   Session  Input  Output  Cache R  Cache W  Known USD",
            "  claude  recent    1500     200        0        0    $0.0003",
            "    Started: 2026-10-16 14:00; duration: 1h 30m; unpriced: 0",
            "    Coverage: counted 32.0 MiB of 39.4 MiB",
            "Notes\n",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in\n{text}");
        }
        assert!(
            text.find("Quota / account limits").unwrap() < text.find("Burn estimates").unwrap()
        );
        assert!(text.find("Burn estimates").unwrap() < text.find("Token history").unwrap());
        assert!(text.find("Token history").unwrap() < text.find("Recent sessions").unwrap());

        report.quota[0].used_percent = 92.5;
        report.quota[0].remaining_percent = 7.5;
        report.quota[0].trend.percent_per_hour = Some(25.0);
        report.quota[0].trend.span_secs = Some(600);
        report.quota[0].trend.exhausts_at = Some(NOW + 1080);
        report.sessions[0].session.session_id = "long-session-id-with-a-common-prefix-123".into();
        report.sessions[0].session.project = "/work/表 cafe\u{301}\n\x1b[31m".into();
        let json_before = serde_json::to_string_pretty(&report).unwrap();
        for width in [20, 32, 40, 60, 80, 120] {
            let plain = super::text(
                &report,
                &Style {
                    width,
                    color: false,
                },
            );
            let colored = super::text(&report, &Style { width, color: true });
            let mut stripped = colored;
            for code in ["0", "2", "31", "32", "33"] {
                stripped = stripped.replace(&format!("\x1b[{code}m"), "");
            }
            assert_eq!(stripped, plain, "color must not change padding at {width}");
            assert!(
                plain.lines().all(|line| line.width() <= width),
                "{width}:\n{plain}"
            );
            assert!(!plain.contains('\x1b'));
            assert!(plain.contains("7.5%"));
        }
        assert_eq!(
            serde_json::to_string_pretty(&report).unwrap(),
            json_before,
            "human layout must leave JSON bytes and row order unchanged"
        );
        let urgent = super::text(
            &report,
            &Style {
                width: 80,
                color: true,
            },
        );
        assert!(urgent
            .lines()
            .nth(1)
            .unwrap()
            .contains("7.5% left; ~18m to limit"));
        assert!(urgent.contains("\x1b[31m"));
        report.quota[0].expired = true;
        assert!(
            super::text(
                &report,
                &Style {
                    width: 80,
                    color: false
                }
            )
            .lines()
            .nth(1)
            .unwrap()
            .starts_with("claude 7d:"),
            "expired peaks must not drive the headline"
        );
        report.quota[1].used_percent = 100.0;
        report.quota[1].remaining_percent = 0.0;
        assert!(super::text(
            &report,
            &Style {
                width: 80,
                color: false
            }
        )
        .lines()
        .nth(1)
        .unwrap()
        .contains("0.0% left; limit reached"));
        for row in &mut report.quota {
            row.stale = true;
        }
        let stale = super::text(
            &report,
            &Style {
                width: 80,
                color: false,
            },
        );
        assert!(stale
            .lines()
            .nth(1)
            .unwrap()
            .contains("no current forecast"));
        assert!(
            !stale.contains("25.0"),
            "stale burn must not be shown as current"
        );
    }

    #[test]
    fn missing_state_says_what_is_missing_and_what_produces_it() {
        let home = tempfile::tempdir().unwrap();
        let report = collect(home.path(), NOW, 0, 10).unwrap();
        assert!(!report.state.exists && !report.state.usage_available);
        assert!(report.quota.is_empty() && report.sessions.is_empty());
        assert!(text(
            &report,
            &Style {
                width: 80,
                color: false
            }
        )
        .contains("Quota / account limits\n  None recorded."));
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
