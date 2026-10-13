use super::{
    content,
    render::{self, encode, escape},
    Console,
};
use anyhow::{ensure, Result};
use std::collections::BTreeMap;
use std::io::Read;
use yoke_core::{config, memory, plan, store::Store, usage};

pub(super) fn sessions(store: &Store, c: &Console) -> Result<String> {
    let rows = store.sessions(Some(&c.project.to_string_lossy()), 200)?;
    let mut out = render::heading(
        "sessions",
        "Trace each agent session from its first event to its latest decision.",
    );
    if rows.is_empty() {
        out.push_str(&render::empty(
            "No sessions yet",
            "Start Claude or Codex with Yoke hooks to record a session.",
            "yoke install --dry-run",
        ));
    }
    for row in rows {
        out.push_str(&format!(r#"<article class="panel"><div class="list-caption"><h2><a data-row data-nav href="{}&amp;session={}">{}</a></h2><span class="badge">{}</span></div><dl class="facts"><dt>Project</dt><dd>{}</dd><dt>Decisions</dt><dd>{}</dd><dt>Active plan</dt><dd>{}</dd><dt>Started</dt><dd><time data-time="{}">{}</time></dd><dt>Ended</dt><dd>{}</dd></dl></article>"#,
            render::link(c,"decisions"),encode(&row.id),escape(&row.id),escape(&row.agent),escape(&row.project),row.decision_count,row.active_plan.map(|v|v.to_string()).unwrap_or_else(||"No active plan".into()),row.started_at,row.started_at,row.ended_at.map(|v|format!("<time data-time=\"{v}\">{v}</time>")).unwrap_or_else(||"Active".into())));
    }
    out.push_str("<p class=\"muted\">Up to 200 recent sessions in this project.</p></main>");
    Ok(out)
}

pub(super) fn plans(
    store: &Store,
    c: &Console,
    params: &BTreeMap<String, String>,
) -> Result<String> {
    let mut out = render::heading(
        "plans",
        "Review the contract, follow the checks, and decide when work can continue.",
    );
    if let Some(slug) = params.get("slug") {
        let detail = plan::detail(store, &c.project, slug)?;
        out.push_str(&format!(r#"<p><a data-nav href="{}">All plans</a></p><section class="panel"><div class="list-caption"><h2>{}</h2><span class="badge">{}</span></div>"#,render::link(c,"plans"),escape(slug),escape(&detail.plan.state)));
        match detail.plan.state.as_str() {
            "draft" => out.push_str(&format!(r#"<div class="action-zone"><button data-action="prepare-approval" data-slug="{}">Review approval</button></div>"#,escape(slug))),
            "waiting" | "stalled" => out.push_str(&format!(r#"<div class="action-zone"><button data-action="resume" data-slug="{}">Resume work</button></div>"#,escape(slug))),
            _ => {}
        }
        out.push_str(&format!(
            "<div class=\"prose\">{}</div></section>",
            content::markdown(&detail.content)
        ));
        if let Some(header) = &detail.plan.approved_header {
            out.push_str(&format!("<details class=\"panel\"><summary>Frozen approved header</summary><pre><code>{}</code></pre></details>",content::highlight(header)));
        }
        out.push_str("<section class=\"panel\"><h2>Transitions</h2>");
        if detail.transitions.is_empty() {
            out.push_str("<p class=\"muted\">No transitions recorded yet.</p>");
        }
        for t in detail.transitions {
            out.push_str(&format!("<p><time data-time=\"{}\">{}</time> &middot; <strong>{}</strong>: {} &rarr; {}<br>{}</p>",t.ts,t.ts,escape(&t.actor),escape(&t.from_state),escape(&t.to_state),escape(t.note.as_deref().unwrap_or(""))));
        }
        out.push_str("</section><section class=\"panel\"><h2>Checks and review evidence</h2>");
        if detail.checks.is_empty() {
            out.push_str(
                "<p class=\"muted\">No check runs yet. Run the plan checks after work begins.</p>",
            );
        }
        for check in detail.checks {
            out.push_str(&format!("<details class=\"check\"><summary data-row>{} - {} ({} ms)</summary><p>{} &middot; exit {}</p><pre><code>{}</code></pre><pre><code>{}</code></pre><h3>Review findings</h3><pre><code>{}</code></pre>",escape(&check.name),escape(&check.result),check.ms,if check.blocking{"Blocking"}else{"Advisory"},check.exit_code.map(|v|v.to_string()).unwrap_or_else(||"unavailable".into()),content::highlight(&check.command),escape(&check.output_tail),escape(&serde_json::to_string_pretty(&check.findings)?)));
            for (index, _) in check.artifacts.iter().enumerate() {
                let url = format!(
                    "{}&amp;slug={}&amp;check={}&amp;index={index}",
                    render::link(c, "artifact"),
                    encode(slug),
                    check.id
                );
                out.push_str(&format!(r#"<button class="quiet screenshot" data-image="{url}" data-caption="{} screenshot {}">Open screenshot {}</button>"#,escape(&check.name),index+1,index+1));
            }
            out.push_str("</details>");
        }
        out.push_str("</section>");
    } else {
        let plans = plan::list(store, &c.project)?;
        if plans.is_empty() {
            out.push_str(&render::empty(
                "Your next plan starts here",
                "Create a plan in your terminal, then return to review its contract and evidence.",
                "yoke plan new --help",
            ));
        }
        for p in plans {
            out.push_str(&format!(r#"<article class="panel"><div class="list-caption"><h2><a data-nav data-row href="{}&amp;slug={}">{}</a></h2><span class="badge">{}</span></div><p class="muted">Updated <time data-time="{}">{}</time> &middot; revision {}</p></article>"#,render::link(c,"plans"),encode(&p.slug),escape(&p.slug),escape(&p.state),p.updated_at,p.updated_at,p.revision));
        }
    }
    out.push_str("</main>");
    Ok(out)
}

pub(super) fn memories(
    store: &Store,
    c: &Console,
    params: &BTreeMap<String, String>,
) -> Result<String> {
    let query = params.get("q").map(String::as_str).unwrap_or("");
    let rows = memory::recall(store, &c.project.to_string_lossy(), query, 100)?;
    let mut out = render::heading(
        "memory",
        "Keep useful context close. Search what the project has learned, and choose what stays.",
    );
    out.push_str(&format!(r#"<form data-filter action="/memory" class="filters"><input type="hidden" name="token" value="{}"><label class="grow">Search memories<input name="q" value="{}" placeholder="Search project knowledge"></label><button>Search</button></form>"#,c.token,escape(query)));
    if rows.is_empty() {
        out.push_str(&render::empty(
            "No matching memories",
            "Try another search, or save a useful fact from your terminal.",
            "yoke remember --help",
        ));
    }
    for m in rows {
        out.push_str(&format!(r#"<article class="panel memory" tabindex="0" data-row><div class="list-caption"><span>{} {}</span><time data-time="{}">{}</time></div><div class="prose">{}</div><p class="muted">Source: {}</p><div class="actions action-zone"><button class="quiet" data-action="pin" data-id="{}" data-value="{}">{}</button><button class="quiet" data-action="share" data-id="{}" {}>{}</button><button class="quiet" data-confirm-forget="{}">Forget</button></div></article>"#,if m.pinned{"Pinned"}else{"Memory"},if m.shared{" / Shared"}else{""},m.ts,m.ts,content::markdown(&m.text),escape(&m.source),m.id,!m.pinned,if m.pinned{"Unpin"}else{"Pin"},m.id,if m.shared{"disabled"}else{""},if m.shared{"Shared"}else{"Share"},m.id));
    }
    out.push_str("<p class=\"muted\">Up to 100 matching memories. Sharing writes project knowledge; forgetting can be undone for 30 seconds.</p></main>");
    Ok(out)
}

pub(super) fn trust(store: &Store, c: &Console) -> Result<String> {
    let loaded = config::load_trusted(store, &c.project, &c.home)?;
    let mut out=render::heading("trust","See the configuration that actually runs. Trust changes remain an explicit terminal action.");
    out.push_str("<section class=\"panel\"><h2>Effective configuration</h2>");
    out.push_str(&format!(
        "<pre><code>{}</code></pre><h3>Origins</h3><dl class=\"facts\">",
        content::highlight(&serde_json::to_string_pretty(&loaded.config)?)
    ));
    for (key, value) in loaded.origins {
        out.push_str(&format!(
            "<dt>{}</dt><dd>{}</dd>",
            escape(&key),
            escape(&value)
        ));
    }
    out.push_str("</dl></section>");
    for status in loaded.snapshots {
        out.push_str(&format!(
            "<section class=\"panel\"><h2>{}</h2><p>{} / {}{}</p>",
            escape(&status.file.display().to_string()),
            escape(&status.scope),
            if status.trusted {
                "Trusted"
            } else {
                "Not trusted"
            },
            if status.stale { " / Disk changed" } else { "" }
        ));
        if let Some(error) = status.disk_error {
            out.push_str(&format!("<p class=\"error\">{}</p>", escape(&error)));
        }
        let root = if status.scope == "global" {
            &c.home
        } else {
            &loaded.project_root
        };
        let trusted = store.get_trust(
            &status.scope,
            &root.to_string_lossy(),
            &status.file.to_string_lossy(),
        )?;
        // Paths come only from trusted configuration discovery, never from request parameters.
        let disk = read_preview(&status.file);
        match disk {
            Ok(bytes) if bytes.len() <= 1024 * 1024 => {
                let disk = String::from_utf8(bytes)?;
                out.push_str(&content::diff(
                    trusted.as_ref().map(|v| v.content.as_str()).unwrap_or(""),
                    &disk,
                ));
            }
            Ok(_) => out.push_str("<p>Disk file exceeds the 1 MiB preview limit.</p>"),
            Err(error) => out.push_str(&format!("<p>{}</p>", escape(&error.to_string()))),
        }
        out.push_str("</section>");
    }
    out.push_str("<p class=\"muted\">Review and trust configuration from your terminal with <code>yoke trust --help</code>.</p></main>");
    Ok(out)
}

fn read_preview(path: &std::path::Path) -> Result<Vec<u8>> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(not(unix))]
    anyhow::bail!("Safe configuration preview is unavailable on this platform.");
    let file = options.open(path)?;
    ensure!(
        file.metadata()?.is_file(),
        "Configuration preview requires a regular file."
    );
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub(super) fn usage(
    store: &Store,
    c: &Console,
    params: &BTreeMap<String, String>,
) -> Result<String> {
    let session = params
        .get("session")
        .filter(|v| !v.is_empty())
        .map(String::as_str);
    let rows = usage::summaries(store, Some(&c.project.to_string_lossy()), session)?;
    let mut out=render::heading("usage","Token usage by session and model. Estimates use published API prices, not subscription bills.");
    if rows.is_empty() {
        out.push_str(&render::empty("No usage samples yet","Usage appears when supported local transcript samples are available. Unknown prices stay unknown.","yoke statusline"));
    }
    let max = rows
        .iter()
        .map(|r| r.input_tokens.saturating_add(r.output_tokens))
        .max()
        .unwrap_or(1)
        .max(1);
    out.push_str(
        "<section class=\"panel\"><h2>Input and output tokens</h2><div class=\"usage-chart\">",
    );
    for row in &rows {
        let input = (row.input_tokens as f64 / max as f64) * 100.0;
        let output = (row.output_tokens as f64 / max as f64) * 100.0;
        out.push_str(&format!(r#"<div class="chart-row"><span>{} / {}</span><svg viewBox="0 0 100 12" role="img" aria-label="{} input {}, output {}"><title>{}: {} input, {} output</title><rect class="chart-input" width="{input}" height="12"/><rect class="chart-output" x="{input}" width="{output}" height="12"/></svg></div>"#,escape(&row.session_id),escape(&row.model),escape(&row.model),row.input_tokens,row.output_tokens,escape(&row.model),row.input_tokens,row.output_tokens));
    }
    out.push_str("</div><p class=\"muted\">Lavender: input / Sage: output. Exact values, including cache tokens, follow.</p><div class=\"table-scroll\" tabindex=\"0\" role=\"region\" aria-label=\"Usage data table\"><table><caption>Usage by session and model</caption><thead><tr><th scope=\"col\">Session / agent / model</th><th scope=\"col\">Input</th><th scope=\"col\">Output</th><th scope=\"col\">Cache read</th><th scope=\"col\">Cache write</th><th scope=\"col\">Estimated USD</th></tr></thead><tbody>");
    for row in rows {
        out.push_str(&format!("<tr><th scope=\"row\">{}<br>{} / {}</th><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",escape(&row.session_id),escape(&row.agent),escape(&row.model),row.input_tokens,row.output_tokens,row.cache_read_tokens,row.cache_write_tokens,escape(&row.estimated_cost_usd.unwrap_or_else(||format!("Unknown ({} samples)",row.unknown_pricing_samples)))));
    }
    out.push_str("</tbody></table></div></section></main>");
    Ok(out)
}
