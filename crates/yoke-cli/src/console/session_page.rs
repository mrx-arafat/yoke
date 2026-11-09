//! The Sessions page: search by prompt text, session ID or project, and one session in full
//! (`sessions?id=<id>`). Read-only.
use super::super::{
    projects,
    render::{self, encode, escape},
    usage, Console,
};
use anyhow::Result;
use std::collections::BTreeMap;
use yoke_core::{
    session_detail::{self, SessionDetail},
    store::{SessionSummary, Store},
};

/// Sessions listed at most.
const LISTED: usize = 200;
/// Touched files listed per group before "and N more".
const FILES_SHOWN: usize = 200;

pub(super) fn page(
    store: &Store,
    c: &Console,
    params: &BTreeMap<String, String>,
) -> Result<String> {
    let text = |key: &str| {
        params
            .get(key)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
    };
    match text("id") {
        Some(id) => detail(store, c, id),
        None => list(
            store,
            c,
            params,
            text("q"),
            params.get("all").is_some_and(|v| v == "1"),
        ),
    }
}

/// Query suffix that opens `project` in project-scoped pages; empty for the launch project.
fn scoped(c: &Console, project: &str) -> String {
    if project == c.project.to_string_lossy() {
        String::new()
    } else {
        format!("&amp;project={}", projects::project_id(project))
    }
}

fn plan_binding(c: &Console, row: &SessionSummary) -> String {
    match (&row.plan_slug, &row.plan_state) {
        (Some(slug), Some(state)) if row.project == c.project.to_string_lossy() => format!(
            r#"<a data-nav href="{}">{}</a> ({})"#,
            render::href("plans", &format!("slug={}", encode(slug))),
            escape(slug),
            escape(state)
        ),
        (Some(slug), Some(state)) => format!("{} ({})", escape(slug), escape(state)),
        _ => "No plan bound".into(),
    }
}

fn resume(row: &SessionSummary) -> String {
    projects::resume_command(&row.agent, &row.project, &row.id)
        .map(|command| format!("<code class=\"resume\">{}</code>", escape(&command)))
        .unwrap_or_else(|| "Unavailable for this agent or session id".into())
}

fn lifecycle(row: &SessionSummary) -> String {
    format!(
        r#"Started <time data-time="{0}">{0}</time> &middot; {1}"#,
        row.started_at,
        row.ended_at
            .map(|v| format!("ended <time data-time=\"{v}\">{v}</time>"))
            .unwrap_or_else(|| "no end event recorded".into())
    )
}

fn ledger(c: &Console, row: &SessionSummary) -> String {
    format!(
        r#"{} <a data-nav href="{}">Open in the decision ledger</a>"#,
        row.decision_count,
        render::href(
            "decisions",
            &format!("session={}{}", encode(&row.id), scoped(c, &row.project))
        )
    )
}

fn list(
    store: &Store,
    c: &Console,
    params: &BTreeMap<String, String>,
    query: Option<&str>,
    all: bool,
) -> Result<String> {
    let scope = projects::scope(store, c, params)?;
    let project = (!all).then_some(scope.path.as_str());
    let rows = match query {
        Some(query) => session_detail::search(store, project, query, LISTED)?,
        None => store.sessions(project, LISTED)?,
    };
    let mut out = render::heading(
        "sessions",
        "Trace each agent session from its first prompt to its latest decision.",
    );
    out.push_str(&format!(
        r#"<form class="filters" data-filter action="sessions" method="get">{}<label class="grow">Search<input name="q" value="{}" placeholder="Prompt text, session ID or project path" autocomplete="off"></label><label class="sessions-all"><input type="checkbox" name="all" value="1"{}> All projects</label><button type="submit">Show sessions</button></form>"#,
        projects::picker(&scope),
        escape(query.unwrap_or("")),
        if all { " checked" } else { "" }
    ));
    if all {
        out.push_str(r#"<p class="scope">Showing sessions from every recorded project.</p>"#);
    } else {
        out.push_str(&projects::banner(c, &scope, "sessions"));
    }
    if rows.is_empty() {
        if query.is_some() {
            out.push_str(&render::empty(
                "No sessions match",
                "No session ID, project path or recorded prompt contains that text. Try fewer words, or tick All projects.",
                "",
            ));
        } else {
            out.push_str(&render::empty(
                "No sessions yet",
                "Start Claude or Codex with Yoke hooks to record a session.",
                "yoke install --dry-run",
            ));
        }
    } else if !all {
        out.push_str(&format!(
            "<p class=\"scope\">Protection now: {}</p>",
            projects::protection(c, store, &scope.path)
        ));
    }
    for row in &rows {
        // Hook payloads that failed before parsing record their decisions under an empty id.
        let heading = if row.id.is_empty() {
            "<h2>No session id recorded</h2>".to_owned()
        } else {
            format!(
                r#"<h2><a data-row data-nav href="{}">{}</a></h2>"#,
                render::href("sessions", &format!("id={}", encode(&row.id))),
                escape(&row.id)
            )
        };
        let project = if all {
            format!(
                "<dt>Project</dt><dd><code>{}</code></dd>",
                escape(&row.project)
            )
        } else {
            String::new()
        };
        let prompt = session_detail::first_prompt(store, &row.id)?
            .map(|text| format!("<dt>First prompt</dt><dd>{}</dd>", render::excerpt(&text)))
            .unwrap_or_default();
        out.push_str(&format!(
            r#"<article class="panel"><div class="list-caption">{heading}<span class="badge">{}</span></div><dl class="facts oversight">{project}<dt>Lifecycle</dt><dd>{}</dd>{prompt}<dt>Last event</dt><dd>{}</dd><dt>Decisions</dt><dd>{}</dd><dt>Plan binding</dt><dd>{}</dd><dt>Mode at last event</dt><dd>{}</dd><dt>Resume</dt><dd>{}</dd></dl></article>"#,
            escape(&row.agent),
            lifecycle(row),
            row.last_event_at
                .map(|v| format!("<time data-time=\"{v}\">{v}</time>"))
                .unwrap_or_else(|| "No decisions recorded".into()),
            ledger(c, row),
            plan_binding(c, row),
            row.last_mode
                .as_deref()
                .map(escape)
                .unwrap_or_else(|| "Unknown (no decisions)".into()),
            resume(row)
        ));
    }
    out.push_str(&format!(
        "<p class=\"muted\">Up to {LISTED} recent sessions{}. Search reads session IDs, project paths and the newest {} recorded prompts. Yoke shows resume commands for you to copy and never runs them; a session without an end event may still be running elsewhere.</p></main>",
        if all { " across all projects" } else { " in this project" },
        session_detail::PROMPT_SEARCH_LIMIT
    ));
    Ok(out)
}

fn detail(store: &Store, c: &Console, id: &str) -> Result<String> {
    let mut out = render::heading(
        "sessions",
        "One agent session: what it was asked, what it did, what Yoke decided and what it cost.",
    );
    let Some(d) = session_detail::detail(store, id)? else {
        out.push_str(&render::empty(
            "No such session",
            "Yoke has no recorded session with this ID. Open one from the Sessions list.",
            "",
        ));
        out.push_str(&format!(
            r#"<p class="actions"><a data-nav class="button quiet" href="{}">Back to sessions</a></p></main>"#,
            render::link(c, "sessions")
        ));
        return Ok(out);
    };
    let s = &d.session;
    let counts = store.session_counts(&s.id)?;
    out.push_str(&format!(
        r#"<p><a data-nav href="{}">Back to sessions</a></p><section class="panel" aria-labelledby="session-id"><div class="list-caption"><h2 id="session-id"><code>{}</code></h2><span class="badge">{}</span></div><div class="actions session-actions"><button type="button" class="quiet" data-copy-session="{1}">Copy session ID</button></div><dl class="facts oversight"><dt>Project</dt><dd><code>{}</code></dd><dt>Lifecycle</dt><dd>{}</dd><dt>First prompt</dt><dd>{}</dd><dt>Decisions</dt><dd>{}</dd><dt>Outcomes</dt><dd>{} allowed &middot; {} would deny &middot; {} blocked &middot; {} errors</dd><dt>Plan binding</dt><dd>{}</dd><dt>Mode at last event</dt><dd>{}</dd><dt>Resume</dt><dd>{}</dd></dl></section>"#,
        render::href("sessions", &scoped(c, &s.project)),
        escape(&s.id),
        escape(&s.agent),
        escape(&s.project),
        lifecycle(s),
        d.first_prompt
            .as_deref()
            .map(render::excerpt)
            .unwrap_or_else(|| "No prompt recorded".into()),
        ledger(c, s),
        counts.allowed,
        counts.would_deny,
        counts.blocked,
        counts.errors,
        plan_binding(c, s),
        s.last_mode
            .as_deref()
            .map(escape)
            .unwrap_or_else(|| "Unknown (no decisions)".into()),
        resume(s)
    ));
    out.push_str(&usage_panel(c, &d));
    out.push_str(&tools_panel(&d));
    out.push_str(&files_panel(&d));
    out.push_str(&memories_panel(c, &d));
    out.push_str(&timeline_panel(&d));
    out.push_str("</main>");
    Ok(out)
}

fn usage_panel(c: &Console, d: &SessionDetail) -> String {
    let mut out = String::from(
        r#"<section class="panel" aria-labelledby="session-usage"><h2 id="session-usage">Usage and estimated cost</h2>"#,
    );
    if d.usage.is_empty() {
        out.push_str(&format!(
            r#"<p class="muted">No token usage is recorded for this session. <a data-nav href="{}">Open Usage</a></p></section>"#,
            render::link(c, "usage")
        ));
        return out;
    }
    out.push_str(r#"<div class="table-scroll" tabindex="0" role="region" aria-labelledby="session-usage"><table><thead><tr><th scope="col">Model</th><th scope="col">Input</th><th scope="col">Output</th><th scope="col">Cache read</th><th scope="col">Cache write</th><th scope="col">Estimated USD</th></tr></thead><tbody>"#);
    for row in &d.usage {
        out.push_str(&format!(
            r#"<tr><th scope="row"><code>{}</code></th><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>"#,
            escape(&row.model),
            row.input_tokens,
            row.output_tokens,
            row.cache_read_tokens,
            row.cache_write_tokens,
            escape(&usage::cost(
                row.estimated_cost_usd.as_deref(),
                row.known_cost_usd.as_deref(),
                row.unknown_pricing_samples
            ))
        ));
    }
    let t = &d.usage_total;
    out.push_str(&format!(
        r#"</tbody><tfoot><tr><th scope="row">Total</th><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr></tfoot></table></div><p class="muted">Estimates from list prices for the models in the session transcript; an unknown price is never shown as a definite number.</p></section>"#,
        t.input_tokens,
        t.output_tokens,
        t.cache_read_tokens,
        t.cache_write_tokens,
        escape(&usage::cost(
            t.estimated_cost_usd.as_deref(),
            t.known_cost_usd.as_deref(),
            t.unknown_pricing_samples
        ))
    ));
    out
}

fn tools_panel(d: &SessionDetail) -> String {
    let mut out = String::from(
        r#"<section class="panel" aria-labelledby="session-tools"><h2 id="session-tools">Tools</h2>"#,
    );
    if d.tools.is_empty() {
        out.push_str(r#"<p class="muted">No tool calls were recorded.</p></section>"#);
        return out;
    }
    out.push_str(r#"<div class="table-scroll" tabindex="0" role="region" aria-labelledby="session-tools"><table><thead><tr><th scope="col">Tool</th><th scope="col">Calls</th><th scope="col">Denied or would deny</th></tr></thead><tbody>"#);
    for tool in &d.tools {
        out.push_str(&format!(
            r#"<tr><th scope="row"><code>{}</code></th><td>{}</td><td>{}</td></tr>"#,
            escape(&tool.tool),
            tool.calls,
            tool.denied
        ));
    }
    out.push_str("</tbody></table></div></section>");
    out
}

fn file_list(title: &str, paths: &[&String]) -> String {
    let mut out = format!(
        r#"<h3>{} <span class="muted">({})</span></h3>"#,
        escape(title),
        paths.len()
    );
    if paths.is_empty() {
        out.push_str(r#"<p class="muted">None recorded.</p>"#);
        return out;
    }
    out.push_str(&format!(
        r#"<div class="session-scroll" tabindex="0" role="region" aria-label="{} files"><ul class="session-files">"#,
        escape(title)
    ));
    for path in paths.iter().take(FILES_SHOWN) {
        out.push_str(&format!("<li><code>{}</code></li>", escape(path)));
    }
    out.push_str("</ul></div>");
    if paths.len() > FILES_SHOWN {
        out.push_str(&format!(
            r#"<p class="muted">and {} more</p>"#,
            paths.len() - FILES_SHOWN
        ));
    }
    out
}

fn files_panel(d: &SessionDetail) -> String {
    let modified: Vec<&String> = d.files.modified.iter().collect();
    let read: Vec<&String> = d
        .files
        .read
        .iter()
        .filter(|path| !d.files.modified.contains(path))
        .collect();
    format!(
        r#"<section class="panel" aria-labelledby="session-files"><h2 id="session-files">Files</h2>{}{}<p class="muted">From the recorded inputs of Read, Edit, Write, MultiEdit, NotebookEdit and apply_patch calls, with secrets redacted. Shell commands are not parsed{}.</p></section>"#,
        file_list("Modified", &modified),
        file_list("Read only", &read),
        if d.files_partial {
            format!(
                ", and only the newest {} tool calls were read",
                session_detail::FILE_SCAN_LIMIT
            )
        } else {
            String::new()
        }
    )
}

fn memories_panel(c: &Console, d: &SessionDetail) -> String {
    let mut out = String::from(
        r#"<section class="panel" aria-labelledby="session-memories"><h2 id="session-memories">Memories from this session</h2>"#,
    );
    if d.memories.is_empty() {
        out.push_str(r#"<p class="muted">No memory was made from this session's facts. Notes saved with <code>yoke remember</code> carry no session.</p></section>"#);
        return out;
    }
    let launch = d.session.project == c.project.to_string_lossy();
    out.push_str(r#"<div class="session-scroll" tabindex="0" role="region" aria-labelledby="session-memories"><ul class="session-memories">"#);
    for m in &d.memories {
        let link = if launch {
            format!(
                r#" <a data-nav href="{}">Details</a>"#,
                render::href("memory", &format!("id={}", m.id))
            )
        } else {
            String::new()
        };
        out.push_str(&format!(
            r#"<li><span class="badge">{}</span> {} <small><time data-time="{2}">{2}</time></small>{link}</li>"#,
            escape(&m.memory_type),
            render::excerpt(&m.text),
            m.ts
        ));
    }
    out.push_str("</ul></div></section>");
    out
}

fn timeline_panel(d: &SessionDetail) -> String {
    let mut out = format!(
        r#"<section class="panel" aria-labelledby="session-timeline"><h2 id="session-timeline">Timeline</h2><p class="muted">{}, oldest first. Allowed post-tool records are left out, so each tool call shows once.</p>"#,
        if d.timeline_total as usize > d.timeline.len() {
            format!(
                "The newest {} of {} events",
                d.timeline.len(),
                d.timeline_total
            )
        } else {
            format!("{} events", d.timeline.len())
        }
    );
    if d.timeline.is_empty() {
        out.push_str(r#"<p class="muted">No events were recorded.</p></section>"#);
        return out;
    }
    out.push_str(r#"<ol class="session-timeline">"#);
    for e in &d.timeline {
        let summary = e
            .summary
            .as_deref()
            .map(|text| {
                format!(
                    r#"<code class="decision-preview">{}</code>"#,
                    render::excerpt(text)
                )
            })
            .unwrap_or_default();
        let reason = e
            .reason
            .as_deref()
            .filter(|text| !text.trim().is_empty())
            .map(|text| {
                format!(
                    r#"<span class="decision-reason">{}</span>"#,
                    render::excerpt(text)
                )
            })
            .unwrap_or_default();
        out.push_str(&format!(
            r#"<li><time data-time="{0}">{0}</time><span class="badge {1}">{2}</span><strong>{3}</strong>{4}{summary}{reason}</li>"#,
            e.ts,
            escape(&e.outcome),
            escape(&e.outcome.replace('_', " ")),
            escape(&e.event),
            e.tool
                .as_deref()
                .map(|tool| format!(" <span class=\"session-tool\">{}</span>", escape(tool)))
                .unwrap_or_default()
        ));
    }
    out.push_str("</ol></section>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use yoke_core::store::DecisionRecord;

    #[test]
    fn sessions_link_to_a_detail_page_with_prompt_files_and_copy_and_search_finds_prompts() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let store = Store::open(root.join("state.db")).unwrap();
        let project = root.to_string_lossy().into_owned();
        for (ts, event, tool, payload) in [
            (
                10,
                "user_prompt_submit",
                None,
                r#"{"prompt":"Ship the <b>search</b> box"}"#,
            ),
            (
                11,
                "pre_tool_use",
                Some("Write"),
                r#"{"tool_name":"Write","tool_input":{"file_path":"src/search.rs"}}"#,
            ),
        ] {
            store
                .record_decision(
                    &DecisionRecord {
                        policy_hash: None,
                        ts,
                        session_id: "sess-42".into(),
                        agent: "claude".into(),
                        project: project.clone(),
                        event: event.into(),
                        tool: tool.map(Into::into),
                        mode: "observe".into(),
                        outcome: "allow".into(),
                        reason: None,
                        gates: serde_json::json!([]),
                        payload: payload.into(),
                    },
                    4096,
                )
                .unwrap();
        }
        let c = Console {
            home: root.clone(),
            project: root.clone(),
            database: root.join("state.db"),
            token: "token".into(),
            csrf: "csrf".into(),
            host: "127.0.0.1:1".into(),
        };
        let params = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect::<BTreeMap<_, _>>()
        };
        let list = page(&store, &c, &params(&[])).unwrap();
        assert!(
            list.contains(r#"<a data-row data-nav href="sessions?id=sess-42">sess-42</a>"#),
            "{list}"
        );
        assert!(
            list.contains("<dt>First prompt</dt><dd>Ship the &lt;b&gt;search&lt;/b&gt; box</dd>"),
            "{list}"
        );

        let found = page(&store, &c, &params(&[("q", "SHIP the")])).unwrap();
        assert!(found.contains("sessions?id=sess-42"), "{found}");
        let none = page(&store, &c, &params(&[("q", "nothing like this")])).unwrap();
        assert!(none.contains("No sessions match") && !none.contains("sessions?id=sess-42"));

        let detail = page(&store, &c, &params(&[("id", "sess-42")])).unwrap();
        assert!(
            detail.contains(r#"data-copy-session="sess-42">Copy session ID</button>"#),
            "{detail}"
        );
        assert!(
            detail.contains("<li><code>src/search.rs</code></li>"),
            "{detail}"
        );
        assert!(
            detail.contains(r#"href="decisions?session=sess-42">Open in the decision ledger</a>"#),
            "{detail}"
        );
        let missing = page(&store, &c, &params(&[("id", "nope")])).unwrap();
        assert!(missing.contains("No such session"));
    }
}
