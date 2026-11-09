//! Activity panels shared by the Memory and Usage pages: memories by type per day, and what each
//! project recorded. Server-side SVG and `<meter>`; colours come from classes, since the CSP
//! forbids inline styles. Every chart has the same numbers in a table.
use super::super::render::escape;
use anyhow::Result;
use std::collections::BTreeMap;
use std::path::Path;
use yoke_core::{analytics, memory, store::Store, timefmt};

/// Calendar days both panels cover, today included.
pub(in crate::console) const DAYS: i64 = 30;

/// `12,345`.
pub(in crate::console) fn grouped(value: impl Into<u128>) -> String {
    let digits = value.into().to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn window_start(now: i64, offset_minutes: i32) -> i64 {
    timefmt::day_start(now, i64::from(offset_minutes) * 60) - (DAYS - 1) * 86_400
}

fn day_label(at: i64, offset_minutes: i32) -> String {
    timefmt::full_time(at, i64::from(offset_minutes) * 60)[..10].to_owned()
}

/// Memories saved per day by type over the last `DAYS` days, as a stacked bar chart and a table.
/// `project` limits it to one project; `None` covers all of them.
pub(in crate::console) fn type_timeline(
    store: &Store,
    project: Option<&str>,
    now: i64,
    offset_minutes: i32,
) -> Result<String> {
    let since = window_start(now, offset_minutes);
    let days = analytics::memories_by_type(store, project, since, offset_minutes)?;
    let mut out =
        format!("<section class=\"panel activity\"><h2>Memories by type, last {DAYS} days</h2>");
    if days.is_empty() {
        out.push_str(&format!(
            "<p class=\"muted\">No memories saved in the last {DAYS} days{}.</p></section>",
            if project.is_some() {
                " in this project"
            } else {
                ""
            }
        ));
        return Ok(out);
    }
    let by_day: BTreeMap<&str, &BTreeMap<String, i64>> = days
        .iter()
        .map(|day| (day.day.as_str(), &day.counts))
        .collect();
    let mut totals: BTreeMap<&str, i64> = BTreeMap::new();
    for day in &days {
        for (name, count) in &day.counts {
            *totals.entry(name.as_str()).or_default() += count;
        }
    }
    let types: Vec<&str> = memory::MEMORY_TYPES
        .iter()
        .copied()
        .filter(|name| totals.contains_key(name))
        .collect();
    let max = days
        .iter()
        .map(|day| day.counts.values().sum::<i64>())
        .max()
        .unwrap_or(1)
        .max(1) as f64;
    let labels: Vec<String> = (0..DAYS)
        .map(|index| day_label(since + index * 86_400, offset_minutes))
        .collect();
    let mut bars = String::new();
    for (index, label) in labels.iter().enumerate() {
        let Some(counts) = by_day.get(label.as_str()) else {
            continue;
        };
        let mut stacked = 0.0;
        for name in &types {
            let Some(count) = counts.get(*name).copied().filter(|count| *count > 0) else {
                continue;
            };
            let height = count as f64 / max * 100.0;
            stacked += height;
            bars.push_str(&format!(
                "<rect class=\"activity-{name}\" x=\"{}\" y=\"{:.2}\" width=\"8\" height=\"{height:.2}\"><title>{label}: {count} {name}</title></rect>",
                index * 10 + 1,
                100.0 - stacked
            ));
        }
    }
    let summary = format!(
        "Memories saved per day by type from {} to {}, at most {} on one day",
        labels[0],
        labels[labels.len() - 1],
        max as i64
    );
    out.push_str(&format!(
        "<svg class=\"activity-chart\" viewBox=\"0 0 {} 100\" preserveAspectRatio=\"none\" role=\"img\" aria-label=\"{summary}\"><title>{summary}</title>{bars}</svg><div class=\"activity-axis\"><span>{}</span><span>{}</span></div><ul class=\"activity-legend\">",
        DAYS * 10,
        labels[0],
        labels[labels.len() - 1]
    ));
    for name in &types {
        out.push_str(&format!(
            "<li><svg viewBox=\"0 0 10 10\" aria-hidden=\"true\"><rect class=\"activity-{name}\" width=\"10\" height=\"10\"/></svg>{name} {}</li>",
            totals[name]
        ));
    }
    out.push_str("</ul><details><summary>Counts by day</summary><div class=\"table-scroll\" tabindex=\"0\" role=\"region\" aria-label=\"Memories by day and type\"><table><caption>Days with saved memories</caption><thead><tr><th scope=\"col\">Day</th>");
    for name in &types {
        out.push_str(&format!("<th scope=\"col\">{name}</th>"));
    }
    out.push_str("<th scope=\"col\">Total</th></tr></thead><tbody>");
    for day in &days {
        out.push_str(&format!("<tr><th scope=\"row\">{}</th>", escape(&day.day)));
        for name in &types {
            out.push_str(&format!(
                "<td>{}</td>",
                day.counts.get(*name).copied().unwrap_or(0)
            ));
        }
        out.push_str(&format!(
            "<td>{}</td></tr>",
            day.counts.values().sum::<i64>()
        ));
    }
    out.push_str("</tbody></table></div></details></section>");
    Ok(out)
}

/// Sessions, decisions, memories and tokens per project over the last `DAYS` days, busiest first.
pub(in crate::console) fn project_activity(
    store: &Store,
    now: i64,
    offset_minutes: i32,
    this_project: &str,
) -> Result<String> {
    let rows = analytics::project_activity(store, window_start(now, offset_minutes), 10)?;
    let mut out =
        format!("<section class=\"panel activity\"><h2>Activity by project, last {DAYS} days</h2>");
    if rows.is_empty() {
        out.push_str(&format!(
            "<p class=\"muted\">Nothing was recorded in the last {DAYS} days.</p></section>"
        ));
        return Ok(out);
    }
    let max = rows.iter().map(|row| row.tokens).max().unwrap_or(0).max(1);
    out.push_str("<div class=\"table-scroll\" tabindex=\"0\" role=\"region\" aria-label=\"Activity by project\"><table class=\"activity-projects\"><caption>Up to 10 projects, most tokens first; tokens are input plus output</caption><thead><tr><th scope=\"col\">Project</th><th scope=\"col\">Sessions</th><th scope=\"col\">Decisions</th><th scope=\"col\">Memories</th><th scope=\"col\">Tokens</th></tr></thead><tbody>");
    for row in &rows {
        let name = Path::new(&row.project)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&row.project);
        let tokens = u64::try_from(row.tokens).unwrap_or(0);
        out.push_str(&format!(
            "<tr><th scope=\"row\" title=\"{}\">{}{}</th><td>{}</td><td>{}</td><td>{}</td><td><span class=\"activity-tokens\">{}</span><meter min=\"0\" max=\"{max}\" value=\"{}\" aria-label=\"{} tokens, relative to the busiest project\">{}</meter></td></tr>",
            escape(&row.project),
            escape(name),
            if row.project == this_project {
                " <span class=\"badge\">this project</span>"
            } else {
                ""
            },
            grouped(u64::try_from(row.sessions).unwrap_or(0)),
            grouped(u64::try_from(row.decisions).unwrap_or(0)),
            grouped(u64::try_from(row.memories).unwrap_or(0)),
            grouped(tokens),
            row.tokens.max(0),
            escape(name),
            grouped(tokens)
        ));
    }
    out.push_str("</tbody></table></div></section>");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_panels_stack_types_per_day_and_rank_projects() {
        let store = Store::open(":memory:").unwrap();
        // 2026-11-10 12:00 UTC.
        let now = 1_794_312_000;
        memory::remember(&store, "/a", "plain", false, now - 3600).unwrap();
        memory::remember_typed(&store, "/a", "chose <WAL>", "decision", false, now - 3600).unwrap();
        memory::remember_typed(&store, "/a", "old", "bugfix", false, now - 40 * 86_400).unwrap();
        let html = type_timeline(&store, Some("/a"), now, 0).unwrap();
        assert!(
            html.contains("<title>2026-11-10: 1 decision</title>"),
            "{html}"
        );
        assert!(html.contains("<title>2026-11-10: 1 note</title>"), "{html}");
        assert!(!html.contains("bugfix"), "older than 30 days: {html}");
        assert!(html.contains("from 2026-10-12 to 2026-11-10"), "{html}");
        assert!(!html.contains("style="));
        assert!(type_timeline(&store, Some("/b"), now, 0)
            .unwrap()
            .contains("No memories saved in the last 30 days in this project"));

        let projects = project_activity(&store, now, 0, "/a").unwrap();
        assert!(projects.contains("<th scope=\"row\" title=\"/a\">a <span class=\"badge\">this project</span></th><td>0</td><td>0</td><td>2</td>"), "{projects}");
        assert_eq!(grouped(1_234_567_u64), "1,234,567");
    }
}
