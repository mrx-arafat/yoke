use super::{
    projects,
    render::{self, encode, escape},
    Console,
};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use yoke_core::store::{DecisionFilter, Store};

/// Read only known input fields from the persisted, redacted payload. Byte-capped payloads
/// may no longer be JSON; the full stored payload remains available in the record details.
fn input_preview(payload: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    let input = value.get("tool_input")?;
    ["command", "file_path", "path"].iter().find_map(|key| {
        input
            .get(key)?
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map(render::excerpt)
    })
}

pub(super) fn decisions(
    store: &Store,
    console: &Console,
    params: &BTreeMap<String, String>,
) -> Result<String> {
    let outcome = params.get("outcome").filter(|s| !s.is_empty()).cloned();
    let tool = params.get("tool").filter(|s| !s.is_empty()).cloned();
    let session = params.get("session").filter(|s| !s.is_empty()).cloned();
    let before = params
        .get("before")
        .map(|s| s.parse::<i64>())
        .transpose()
        .context("invalid decision cursor")?;
    let scope = projects::scope(store, console, params)?;
    let filter = DecisionFilter {
        project: Some(scope.path.clone()),
        outcome: outcome.clone(),
        tool: tool.clone(),
        session: session.clone(),
        limit: 51,
    };
    let rows = store.decision_page(&filter, before)?;
    let mut body = render::heading(
        "decisions",
        "Every gate, every outcome. Follow the evidence behind your agent's next move.",
    );
    let mut options = String::new();
    for (value, label) in [
        ("", "All outcomes"),
        ("allow", "Allowed"),
        ("deny", "Denied"),
        ("would_deny", "Would deny"),
        ("rewrite", "Rewritten"),
        ("error", "Errors"),
    ] {
        options.push_str(&format!(
            "<option value=\"{value}\" {}>{label}</option>",
            if outcome.as_deref().unwrap_or("") == value {
                "selected"
            } else {
                ""
            }
        ));
    }
    body.push_str(&format!(r#"<form class="filters" data-filter action="decisions" method="get">{}<label>Outcome<select name="outcome">{options}</select></label><label class="grow">Tool name<input name="tool" value="{}" placeholder="Filter by exact tool name"></label><button type="submit">Apply filters</button><a data-nav class="button quiet" href="{}">Reset</a></form>{}<div class="list-caption"><span>Decision ledger</span><span>{} records &middot; newest first</span></div>"#,projects::picker(&scope),escape(tool.as_deref().unwrap_or("")),render::href("decisions",&scope.query()),projects::banner(console,&scope,"decisions"),rows.len().min(50)));
    if let Some(session) = &session {
        let hidden = format!(
            r#"<input type="hidden" name="session" value="{}">"#,
            escape(session)
        );
        body = body.replacen("<label>Outcome", &format!("{hidden}<label>Outcome"), 1);
        body.push_str(&format!(r#"<p class="scope">Session: <code>{}</code> <a data-nav href="{}">Clear session filter</a></p>"#,escape(session),render::href("decisions",&scope.query())));
    }
    if rows.is_empty() {
        if before.is_some() {
            body.push_str(&render::empty("No older decisions", "This is the end of the history for this selection. Return to the latest decisions below.", ""));
        } else if outcome.is_some() || tool.is_some() || session.is_some() {
            body.push_str(&render::empty(
                "No decisions match these filters",
                "Clear the outcome, tool and session filters to see this project's activity.",
                "",
            ));
            body.push_str(&format!(r#"<p class="actions"><a data-nav class="button quiet" href="{}">Clear filters</a></p>"#, render::href("decisions", &scope.query())));
        } else {
            body.push_str(&render::empty(
                "No decisions here yet",
                "Run an agent with Yoke hooks to record this project's first decision.",
                "yoke install --dry-run",
            ));
        }
    } else {
        body.push_str(r#"<div class="ledger"><div class="ledger-labels" aria-hidden="true"><span>Outcome / event</span><span>Agent / tool</span><span>Mode</span><span>Recorded</span></div>"#);
        for row in rows.iter().take(50) {
            let r = &row.record;
            let reason = r
                .reason
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(if r.outcome == "allow" {
                    "No intervention was needed."
                } else {
                    "No reason was recorded."
                });
            let preview = input_preview(&r.payload).map(|text| format!(r#"<code class="decision-preview"><span class="sr-only">Command or path: </span>{text}</code>"#)).unwrap_or_default();
            let summary_reason = if r.outcome != "allow" || r.reason.is_some() {
                format!(
                    r#"<span class="decision-reason">{}</span>"#,
                    render::excerpt(reason)
                )
            } else {
                String::new()
            };
            body.push_str(&format!(r#"<details class="record"><summary data-row><span class="event-cell"><span class="badge {}">{}</span><strong>{}</strong></span><span><strong>{}</strong><small>{}</small></span><span class="mode">{}</span><time data-time="{}">{}</time><span class="decision-context">{preview}{summary_reason}</span></summary><div class="record-detail"><p class="reason">{}</p><dl class="facts"><dt>Session</dt><dd>{}</dd><dt>Project</dt><dd>{}</dd></dl><details><summary>Gate results</summary><pre><code>{}</code></pre></details><details><summary>Redacted payload</summary><pre><code>{}</code></pre></details></div></details>"#,escape(&r.outcome),escape(&r.outcome.replace('_'," ")),escape(&r.event),escape(&r.agent),escape(r.tool.as_deref().unwrap_or("Lifecycle")),escape(&r.mode),r.ts,r.ts,escape(reason),escape(&r.session_id),escape(&r.project),escape(&serde_json::to_string_pretty(&r.gates)?),escape(&r.payload)));
        }
        body.push_str("</div>");
    }
    body.push_str("<nav class=\"pagination\" aria-label=\"Decision pages\">");
    let preserved = format!(
        "&amp;outcome={}&amp;tool={}&amp;session={}{}",
        encode(outcome.as_deref().unwrap_or("")),
        encode(tool.as_deref().unwrap_or("")),
        encode(session.as_deref().unwrap_or("")),
        scope.query()
    );
    if before.is_some() {
        body.push_str(&format!(
            r#"<a class="button quiet" data-nav href="{}">Latest decisions</a>"#,
            render::href("decisions", &preserved)
        ));
    }
    if rows.len() > 50 {
        let cursor = rows[49].id;
        body.push_str(&format!(
            r#"<a class="button" data-nav href="{}">Older decisions -&gt;</a>"#,
            render::href("decisions", &format!("before={cursor}{preserved}"))
        ));
    }
    body.push_str("</nav></main>");
    Ok(body)
}
