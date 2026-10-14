use super::{
    content, projects,
    render::{self, encode, escape},
    Console,
};
use anyhow::{ensure, Result};
use std::collections::BTreeMap;
use std::io::Read;
use yoke_core::{config, memory, plan, store::Store};

pub(super) fn sessions(
    store: &Store,
    c: &Console,
    params: &BTreeMap<String, String>,
) -> Result<String> {
    let scope = projects::scope(store, c, params)?;
    let rows = store.sessions(Some(&scope.path), 200)?;
    let mut out = render::heading(
        "sessions",
        "Trace each agent session from its first event to its latest decision.",
    );
    out.push_str(&format!(r#"<form class="filters" data-filter action="sessions" method="get">{}<button type="submit">Show sessions</button></form>"#,projects::picker(&scope)));
    out.push_str(&projects::banner(c, &scope, "sessions"));
    if rows.is_empty() {
        out.push_str(&render::empty(
            "No sessions yet",
            "Start Claude or Codex with Yoke hooks to record a session.",
            "yoke install --dry-run",
        ));
    } else {
        out.push_str(&format!(
            "<p class=\"scope\">Protection now: {}</p>",
            projects::protection(c, store, &scope.path)
        ));
    }
    for row in rows {
        let plan = match (&row.plan_slug, &row.plan_state) {
            (Some(slug), Some(state)) if scope.launch => format!(
                r#"<a data-nav href="{}">{}</a> ({})"#,
                render::href("plans", &format!("slug={}", encode(slug))),
                escape(slug),
                escape(state)
            ),
            (Some(slug), Some(state)) => format!("{} ({})", escape(slug), escape(state)),
            _ => "No plan bound".into(),
        };
        let resume = projects::resume_command(&row.agent, &row.project, &row.id)
            .map(|command| format!("<code class=\"resume\">{}</code>", escape(&command)))
            .unwrap_or_else(|| "Unavailable for this agent or session id".into());
        // Hook payloads that failed before parsing record their decisions under an empty id.
        let heading = if row.id.is_empty() {
            "<h2>No session id recorded</h2>".to_owned()
        } else {
            format!(
                r#"<h2><a data-row data-nav href="{}">{}</a></h2>"#,
                render::href(
                    "decisions",
                    &format!("session={}{}", encode(&row.id), scope.query())
                ),
                escape(&row.id)
            )
        };
        out.push_str(&format!(r#"<article class="panel"><div class="list-caption">{heading}<span class="badge">{}</span></div><dl class="facts oversight"><dt>Lifecycle</dt><dd>Started <time data-time="{}">{}</time> &middot; {}</dd><dt>Last event</dt><dd>{}</dd><dt>Decisions</dt><dd>{}</dd><dt>Plan binding</dt><dd>{}</dd><dt>Mode at last event</dt><dd>{}</dd><dt>Resume</dt><dd>{}</dd></dl></article>"#,
            escape(&row.agent),row.started_at,row.started_at,
            row.ended_at.map(|v|format!("ended <time data-time=\"{v}\">{v}</time>")).unwrap_or_else(||"no end event recorded".into()),
            row.last_event_at.map(|v|format!("<time data-time=\"{v}\">{v}</time>")).unwrap_or_else(||"No decisions recorded".into()),
            row.decision_count,plan,
            row.last_mode.as_deref().map(escape).unwrap_or_else(||"Unknown (no decisions)".into()),
            resume));
    }
    out.push_str("<p class=\"muted\">Up to 200 recent sessions in this project. Yoke shows resume commands for you to copy and never runs them; a session without an end event may still be running elsewhere.</p></main>");
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
        out.push_str(&super::plan_detail::page(store, c, slug)?);
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
            out.push_str(&format!(r#"<article class="panel"><div class="list-caption"><h2><a data-nav data-row href="{}">{}</a></h2><span class="badge {}">{}</span></div><p class="muted">Updated <time data-time="{}">{}</time> &middot; revision {}</p></article>"#,render::href("plans",&format!("slug={}",encode(&p.slug))),escape(&p.slug),escape(&p.state),escape(&p.state),p.updated_at,p.updated_at,p.revision));
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
    out.push_str(&format!(r#"<form data-filter action="memory" class="filters"><label class="grow">Search memories<input name="q" value="{}" placeholder="Search project knowledge"></label><button>Search</button></form>"#,escape(query)));
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
        "<pre><code>{}</code></pre><h3>Origins</h3><dl class=\"facts settings-facts\">",
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
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                out.push_str("<p class=\"muted\">Not on disk.</p>")
            }
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
