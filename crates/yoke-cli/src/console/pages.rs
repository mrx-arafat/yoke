use super::{
    render::{self, encode, escape},
    Console,
};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use yoke_core::store::{DecisionFilter, Store};

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
    let filter = DecisionFilter {
        project: Some(console.project.display().to_string()),
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
    body.push_str(&format!(r#"<form class="filters" data-filter action="/decisions" method="get"><input type="hidden" name="token" value="{}"><label>Outcome<select name="outcome">{options}</select></label><label class="grow">Tool name<input name="tool" value="{}" placeholder="Filter by exact tool name"></label><button type="submit">Apply filters</button><a data-nav class="button quiet" href="{}">Reset</a></form><div class="list-caption"><span>Decision ledger</span><span>{} records &middot; newest first</span></div>"#,console.token,escape(tool.as_deref().unwrap_or("")),render::link(console,"decisions"),rows.len().min(50)));
    if let Some(session) = &session {
        let hidden = format!(
            r#"<input type="hidden" name="session" value="{}">"#,
            escape(session)
        );
        body = body.replacen("<label>Outcome", &format!("{hidden}<label>Outcome"), 1);
        body.push_str(&format!(r#"<p class="scope">Session: <code>{}</code> <a data-nav href="{}">Clear session filter</a></p>"#,escape(session),render::link(console,"decisions")));
    }
    if rows.is_empty() {
        body.push_str(&render::empty(
            "No decisions here yet",
            "Run an agent with Yoke hooks, or clear your filters to see earlier activity.",
            "yoke install --dry-run",
        ));
    } else {
        body.push_str(r#"<div class="ledger"><div class="ledger-labels" aria-hidden="true"><span>Outcome / event</span><span>Agent / tool</span><span>Mode</span><span>Recorded</span></div>"#);
        for row in rows.iter().take(50) {
            let r = &row.record;
            body.push_str(&format!(r#"<details class="record"><summary data-row><span class="event-cell"><span class="badge {}">{}</span><strong>{}</strong></span><span><strong>{}</strong><small>{}</small></span><span class="mode">{}</span><time data-time="{}">{}</time></summary><div class="record-detail"><p class="reason">{}</p><dl class="facts"><dt>Session</dt><dd>{}</dd><dt>Project</dt><dd>{}</dd></dl><details><summary>Gate results</summary><pre><code>{}</code></pre></details><details><summary>Redacted payload</summary><pre><code>{}</code></pre></details></div></details>"#,escape(&r.outcome),escape(&r.outcome.replace('_'," ")),escape(&r.event),escape(&r.agent),escape(r.tool.as_deref().unwrap_or("Lifecycle")),escape(&r.mode),r.ts,r.ts,escape(r.reason.as_deref().unwrap_or("No intervention was needed.")),escape(&r.session_id),escape(&r.project),escape(&serde_json::to_string_pretty(&r.gates)?),escape(&r.payload)));
        }
        body.push_str("</div>");
    }
    body.push_str("<nav class=\"pagination\" aria-label=\"Decision pages\">");
    let preserved = format!(
        "&amp;outcome={}&amp;tool={}&amp;session={}",
        encode(outcome.as_deref().unwrap_or("")),
        encode(tool.as_deref().unwrap_or("")),
        encode(session.as_deref().unwrap_or(""))
    );
    if before.is_some() {
        body.push_str(&format!(
            r#"<a class="button quiet" data-nav href="{}{preserved}">Latest decisions</a>"#,
            render::link(console, "decisions")
        ));
    }
    if rows.len() > 50 {
        let cursor = rows[49].id;
        body.push_str(&format!(r#"<a class="button" data-nav href="{}&amp;before={cursor}{preserved}">Older decisions -&gt;</a>"#,render::link(console,"decisions")));
    }
    body.push_str("</nav></main>");
    Ok(body)
}
