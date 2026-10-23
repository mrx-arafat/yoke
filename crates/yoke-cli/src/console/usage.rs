//! Usage history. Days and months are calendar periods in one explicit UTC offset.

use super::{
    projects,
    render::{self, encode, escape},
    Console,
};
use anyhow::{ensure, Context, Result};
use std::collections::BTreeMap;
use yoke_core::{
    quota::{self, Quota},
    store::Store,
    timefmt,
    usage::{self, Period, UsageRange, UsageTotals},
};

fn quota_panel(reading: &Quota, now: i64, offset: i64) -> String {
    let mut out = format!("<section class=\"panel\"><h2>{} account quota</h2><p class=\"muted\">Account-wide; independent of project and date filters. Reported percentages, with estimates from recent burn.</p>", escape(&reading.agent));
    for window in &reading.windows {
        let at = reading.latest(window);
        let state = if window.expired(now) {
            "reset passed; awaiting a new reading"
        } else if !reading.fresh(window, now) {
            "stale reading; no projection"
        } else if window.used_percent >= 100.0 {
            "limit reached"
        } else {
            "current reading"
        };
        out.push_str(&format!(
            "<h3>{}: {}% used / {}% left</h3><p>{}; observed {} &middot; resets {}</p>",
            window.label(),
            quota::percent(window.used_percent),
            quota::percent(window.remaining_percent()),
            state,
            timefmt::full_time(at, offset),
            timefmt::full_time(window.resets_at, offset)
        ));
        out.push_str(&format!("<meter min=\"0\" max=\"100\" low=\"60\" high=\"85\" optimum=\"0\" value=\"{}\" aria-label=\"{} quota used percent\">{}%</meter>", window.used_percent, window.label(), quota::percent(window.used_percent)));
        let samples: Vec<_> = reading
            .history
            .iter()
            .filter(|s| {
                s.window.window_minutes == window.window_minutes
                    && s.window.resets_at == window.resets_at
                    && (0..=3600).contains(&at.saturating_sub(s.observed_at))
            })
            .collect();
        if let (Some(first), Some(last)) = (samples.first(), samples.last()) {
            if last.observed_at > first.observed_at {
                let span = (last.observed_at - first.observed_at) as f64;
                let points = samples
                    .iter()
                    .map(|s| {
                        format!(
                            "{:.2},{:.2}",
                            (s.observed_at - first.observed_at) as f64 / span * 100.0,
                            100.0 - s.window.used_percent
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                let label = format!(
                    "{} quota: {}% to {}% over {}",
                    window.label(),
                    quota::percent(first.window.used_percent),
                    quota::percent(last.window.used_percent),
                    timefmt::duration_text(span as i64)
                );
                out.push_str(&format!("<div class=\"chart-row\"><span>{label}</span><svg viewBox=\"0 0 100 100\" preserveAspectRatio=\"none\" role=\"img\" aria-label=\"{label}\"><title>{label}; vertical scale 0 to 100 percent used</title><polyline points=\"{points}\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" vector-effect=\"non-scaling-stroke\"/></svg></div>"));
            }
        }
        let trend = reading.trend(window, now);
        if let Some(rate) = trend
            .percent_per_hour
            .filter(|_| window.used_percent < 100.0)
        {
            out.push_str(&format!(
                "<p>Recent burn: {rate:.1} percentage points/hour over {}. ",
                timefmt::duration_text(trend.span_secs.unwrap_or(0))
            ));
            if let Some(end) = trend.exhausts_at {
                out.push_str(&format!(
                    "May reach 100% at {} if this rate continues.",
                    timefmt::full_time(end, offset)
                ));
            } else {
                out.push_str("No exhaustion before reset at this rate.");
            }
            out.push_str("</p>");
        } else if window.used_percent < 100.0 && reading.fresh(window, now) {
            out.push_str("<p class=\"muted\">Need at least one minute of recent same-window readings to estimate burn.</p>");
        }
    }
    out.push_str("</section>");
    out
}

// Offsets in use by current civil time zones.
const OFFSETS: [i32; 38] = [
    -720, -660, -600, -570, -540, -480, -420, -360, -300, -240, -210, -180, -120, -60, 0, 60, 120,
    180, 210, 240, 270, 300, 330, 345, 360, 390, 420, 480, 525, 540, 570, 600, 630, 660, 720, 765,
    780, 840,
];

pub(super) fn offset_label(minutes: i32) -> String {
    if minutes == 0 {
        return "UTC".into();
    }
    let sign = if minutes < 0 { '-' } else { '+' };
    format!(
        "UTC{sign}{:02}:{:02}",
        minutes.abs() / 60,
        minutes.abs() % 60
    )
}

/// Parse `+HH:MM` or `-HH:MM`; range and step are checked by `UsageRange::new`.
pub(super) fn parse_offset(value: &str) -> Result<i32> {
    let bytes = value.as_bytes();
    ensure!(
        bytes.len() == 6
            && matches!(bytes[0], b'+' | b'-')
            && bytes[3] == b':'
            && [1, 2, 4, 5].iter().all(|&i| bytes[i].is_ascii_digit()),
        "time zone offset must look like +06:00"
    );
    let minutes = value[1..3].parse::<i32>()? * 60 + value[4..6].parse::<i32>()?;
    Ok(if bytes[0] == b'-' { -minutes } else { minutes })
}

fn offset_value(minutes: i32) -> String {
    let sign = if minutes < 0 { '-' } else { '+' };
    format!("{sign}{:02}:{:02}", minutes.abs() / 60, minutes.abs() % 60)
}

/// The console is loopback-only, so the host's current offset is the viewer's default.
fn host_offset_minutes() -> i32 {
    // SAFETY: time and localtime_r only write to the locals passed in.
    let minutes = unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut local: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut local).is_null() {
            return 0;
        }
        local.tm_gmtoff / 60
    };
    i32::try_from(minutes)
        .ok()
        .filter(|m| (-720..=840).contains(m) && m % 15 == 0)
        .unwrap_or(0)
}

fn plural(count: u64) -> &'static str {
    if count == 1 {
        "sample"
    } else {
        "samples"
    }
}

/// A sum that includes an unknown price is never shown as a definite number.
pub(super) fn cost(estimated: Option<&str>, known: Option<&str>, unknown: u64) -> String {
    match (estimated, known) {
        (Some(value), _) => value.to_owned(),
        (None, Some(known)) => format!("{known} + unknown ({unknown} {})", plural(unknown)),
        (None, None) => format!("Unknown ({unknown} {})", plural(unknown)),
    }
}

fn total_cost(t: &UsageTotals) -> String {
    cost(
        t.estimated_cost_usd.as_deref(),
        t.known_cost_usd.as_deref(),
        t.unknown_pricing_samples,
    )
}

fn chart_row(label: &str, input: u64, output: u64, max: u64) -> String {
    let width = |n: u64| (n as f64 / max.max(1) as f64) * 100.0;
    let (input_width, output_width) = (width(input), width(output));
    format!(
        r#"<div class="chart-row"><span>{label}</span><svg viewBox="0 0 100 12" preserveAspectRatio="none" role="img" aria-label="{label}: input {input}, output {output}"><title>{label}: {input} input, {output} output</title><rect class="chart-input" width="{input_width}" height="12"/><rect class="chart-output" x="{input_width}" width="{output_width}" height="12"/></svg></div>"#
    )
}

pub(super) fn page(
    store: &Store,
    c: &Console,
    params: &BTreeMap<String, String>,
) -> Result<String> {
    let scope = projects::scope(store, c, params)?;
    let text = |key: &str| {
        params
            .get(key)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    };
    let session = text("session");
    let monthly = text("period") == Some("month");
    let mut out = render::heading(
        "usage",
        "Token usage over time, by session and model. Estimates use published API prices, not subscription bills.",
    );
    let range = (|| {
        let offset = match text("tz") {
            Some(value) => parse_offset(value)?,
            None => host_offset_minutes(),
        };
        UsageRange::new(text("from"), text("to"), offset).context("invalid usage filter")
    })();
    let offset = range.as_ref().map(|r| r.offset_minutes()).unwrap_or(0);
    let mut zones = String::new();
    let mut listed = OFFSETS.to_vec();
    if !listed.contains(&offset) {
        listed.push(offset);
        listed.sort_unstable();
    }
    for minutes in listed {
        zones.push_str(&format!(
            "<option value=\"{}\"{}>{}</option>",
            offset_value(minutes),
            if minutes == offset { " selected" } else { "" },
            offset_label(minutes)
        ));
    }
    let session_input = session
        .map(|s| {
            format!(
                r#"<input type="hidden" name="session" value="{}">"#,
                escape(s)
            )
        })
        .unwrap_or_default();
    out.push_str(&format!(r#"<form class="filters usage-filters" data-filter action="usage" method="get">{session_input}{}<label>From<input type="date" name="from" value="{}"></label><label>To<input type="date" name="to" value="{}"></label><label>Group by<select name="period"><option value="day"{}>Day</option><option value="month"{}>Month</option></select></label><label>Time zone<select name="tz">{zones}</select></label><button type="submit">Apply filters</button><a data-nav class="button quiet" href="{}">Reset</a></form>"#,
        projects::picker(&scope),
        escape(text("from").unwrap_or("")),
        escape(text("to").unwrap_or("")),
        if monthly { "" } else { " selected" },
        if monthly { " selected" } else { "" },
        render::href("usage", &scope.query())));
    out.push_str(&projects::banner(c, &scope, "usage"));
    let range = match range {
        Ok(range) => range,
        Err(error) => {
            out.push_str(&format!(
                r#"<section class="panel error" role="alert"><p>{}</p><p>Dates use YYYY-MM-DD and the start date cannot be after the end date.</p></section></main>"#,
                escape(&format!("{error:#}"))
            ));
            return Ok(out);
        }
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;
    for reading in [
        quota::load(store, "codex"),
        quota::read_file(&quota::claude_file(&c.home), "claude"),
    ] {
        match reading {
            Ok(Some(reading)) => out.push_str(&quota_panel(&reading, now, i64::from(offset) * 60)),
            Err(_) => out.push_str("<p class=\"muted\">A quota record is unreadable; awaiting the next provider reading.</p>"),
            Ok(None) => {}
        }
    }
    let window = match (range.from(), range.to()) {
        (None, None) => "All recorded time".to_owned(),
        (Some(from), None) => format!("From {from}"),
        (None, Some(to)) => format!("Through {to}"),
        (Some(from), Some(to)) => format!("{from} through {to}"),
    };
    // Links that change only the session keep every other filter.
    let keep = format!(
        "&amp;from={}&amp;to={}&amp;tz={}&amp;period={}{}",
        encode(range.from().unwrap_or("")),
        encode(range.to().unwrap_or("")),
        encode(&offset_value(offset)),
        if monthly { "month" } else { "day" },
        scope.query()
    );
    out.push_str(&format!(
        "<p class=\"scope\">{} &middot; calendar {} in <strong>{}</strong>",
        escape(&window),
        if monthly { "months" } else { "days" },
        offset_label(offset)
    ));
    if let Some(session) = session {
        out.push_str(&format!(
            r#" &middot; session <code>{}</code> <a data-nav href="{}">Clear session filter</a>"#,
            escape(session),
            render::href("usage", &keep),
        ));
    }
    out.push_str("</p>");
    let period = if monthly { Period::Month } else { Period::Day };
    let report = usage::report(store, Some(&scope.path), session, &range, period)?;
    if report.totals.samples == 0 && report.undated_samples == 0 {
        out.push_str(&render::empty(
            "No usage samples yet",
            "Usage appears when supported local transcript samples are available. Unknown prices stay unknown.",
            "yoke statusline",
        ));
        out.push_str("</main>");
        return Ok(out);
    }
    let t = &report.totals;
    out.push_str(&format!(r#"<section class="panel"><h2>Totals</h2><dl class="facts oversight"><dt>Samples</dt><dd>{}</dd><dt>Input</dt><dd>{}</dd><dt>Output</dt><dd>{}</dd><dt>Cache read</dt><dd>{}</dd><dt>Cache write</dt><dd>{}</dd><dt>Estimated USD</dt><dd>{}</dd></dl>"#,
        t.samples, t.input_tokens, t.output_tokens, t.cache_read_tokens, t.cache_write_tokens, escape(&total_cost(t))));
    if report.undated_samples > 0 {
        out.push_str(&format!(
            "<p class=\"muted\">{} no recorded time, so {}. Yoke records a time when a session's transcript is imported again.</p>",
            if report.undated_samples == 1 { "1 sample has".to_owned() } else { format!("{} samples have", report.undated_samples) },
            match (range.is_bounded(), report.undated_samples == 1) {
                (true, true) => "this date range leaves it out",
                (true, false) => "this date range leaves them out",
                (false, true) => "it appears as Time unknown",
                (false, false) => "they appear as Time unknown",
            }
        ));
    }
    out.push_str("<p class=\"muted\">Unknown prices are never guessed: a total that includes them shows the priced part plus the unknown samples.</p></section>");

    let max = report
        .periods
        .iter()
        .map(|b| b.totals.input_tokens.saturating_add(b.totals.output_tokens))
        .max()
        .unwrap_or(1);
    out.push_str(&format!(
        "<section class=\"panel\"><h2>By {}</h2><div class=\"usage-chart\">",
        if monthly { "month" } else { "day" }
    ));
    let label = |period: &Option<String>| {
        period
            .as_deref()
            .map(escape)
            .unwrap_or_else(|| "Time unknown".into())
    };
    for bucket in &report.periods {
        out.push_str(&chart_row(
            &label(&bucket.period),
            bucket.totals.input_tokens,
            bucket.totals.output_tokens,
            max,
        ));
    }
    out.push_str(&format!("</div><p class=\"muted\">Lavender: input / Sage: output. Newest first, up to 400 periods, in {}.</p><div class=\"table-scroll\" tabindex=\"0\" role=\"region\" aria-label=\"Usage by period\"><table class=\"usage-table\"><caption>Usage by {}</caption><thead><tr><th scope=\"col\">Period</th><th scope=\"col\">Samples</th><th scope=\"col\">Input</th><th scope=\"col\">Output</th><th scope=\"col\">Cache read</th><th scope=\"col\">Cache write</th><th scope=\"col\">Estimated USD</th></tr></thead><tbody>",
        offset_label(offset), if monthly { "month" } else { "day" }));
    for bucket in &report.periods {
        let b = &bucket.totals;
        out.push_str(&format!("<tr><th scope=\"row\">{}</th><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            label(&bucket.period), b.samples, b.input_tokens, b.output_tokens, b.cache_read_tokens, b.cache_write_tokens, escape(&total_cost(b))));
    }
    out.push_str("</tbody></table></div></section>");

    let rows = &report.sessions;
    let max = rows
        .iter()
        .map(|r| r.input_tokens.saturating_add(r.output_tokens))
        .max()
        .unwrap_or(1);
    out.push_str(
        "<section class=\"panel\"><h2>By session and model</h2><div class=\"usage-chart\">",
    );
    for row in rows {
        out.push_str(&chart_row(
            &format!("{} / {}", escape(&row.session_id), escape(&row.model)),
            row.input_tokens,
            row.output_tokens,
            max,
        ));
    }
    out.push_str("</div><div class=\"table-scroll\" tabindex=\"0\" role=\"region\" aria-label=\"Usage data table\"><table class=\"usage-table\"><caption>Usage by session and model</caption><thead><tr><th scope=\"col\">Session / agent / model</th><th scope=\"col\">Input</th><th scope=\"col\">Output</th><th scope=\"col\">Cache read</th><th scope=\"col\">Cache write</th><th scope=\"col\">Estimated USD</th></tr></thead><tbody>");
    for row in rows {
        out.push_str(&format!("<tr><th scope=\"row\"><a data-nav href=\"{}\">{}</a><br>{} / {}</th><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            render::href("usage", &format!("session={}{keep}", encode(&row.session_id))),
            escape(&row.session_id), escape(&row.agent), escape(&row.model),
            row.input_tokens, row.output_tokens, row.cache_read_tokens, row.cache_write_tokens,
            escape(&cost(row.estimated_cost_usd.as_deref(), row.known_cost_usd.as_deref(), row.unknown_pricing_samples))));
    }
    out.push_str("</tbody></table></div></section></main>");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn account_quota_shows_before_after_history_projection_and_staleness() {
        let now = 1_792_144_800;
        let reading = |used, at| {
            Quota::from_codex(&json!({"primary":{"used_percent":used,"window_minutes":300,"resets_at":now + 7200}}), at).unwrap()
        };
        let mut quota = reading(40, now);
        quota.absorb(&reading(50, now + 600));
        let html = quota_panel(&quota, now + 600, 0);
        assert!(html.contains("40% to 50% over 10m"));
        assert!(html.contains("60.0 percentage points/hour"));
        assert!(html.contains("May reach 100% at"));
        assert!(html.contains("<polyline"));
        let stale = quota_panel(&quota, now + 901, 0);
        assert!(stale.contains("stale reading; no projection"));
        assert!(!stale.contains("May reach 100%"));
        quota.absorb(&reading(100, now + 660));
        let exhausted = quota_panel(&quota, now + 660, 0);
        assert!(exhausted.contains("limit reached"));
        assert!(!exhausted.contains("No exhaustion before reset"));
    }
}
