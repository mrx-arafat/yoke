//! Dashboard: overview across all recorded projects. Also handles plans., prd. and docs. actions.
//! Owner: parity-plans (Pilot parity fleet).
use super::{
    actions::Outcome,
    projects,
    render::{self, encode, escape},
    Console,
};
use anyhow::Result;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use yoke_core::{overview, prd, store::Store};

#[path = "plan_stats.rs"]
pub(super) mod plan_stats;
#[path = "plan_worktrees.rs"]
pub(super) mod plan_worktrees;

const DAY: i64 = 86_400;
const RECENT: usize = 5;
/// Requirement folders are read for this many most recently active projects.
const PROJECTS_SCANNED: usize = 50;

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn project_name(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
}

fn time(ts: i64) -> String {
    format!("<time data-time=\"{ts}\">{ts}</time>")
}

fn badge(class: &str, label: &str) -> String {
    format!(
        r#"<span class="badge {}">{}</span>"#,
        escape(class),
        escape(label)
    )
}

fn stat(label: &str, value: impl std::fmt::Display, hint: &str) -> String {
    format!(
        r#"<div class="stat"><dt>{}</dt><dd>{value}</dd><small>{}</small></div>"#,
        escape(label),
        escape(hint)
    )
}

/// A panel with a title, a "Show all" link, and either rows or an empty note.
fn section(title: &str, all_page: &str, all_label: &str, rows: &[String], empty: &str) -> String {
    let mut out = format!(
        r#"<section class="panel dash-section"><div class="list-caption"><h2>{}</h2><a data-nav href="{all_page}">{}</a></div>"#,
        escape(title),
        escape(all_label)
    );
    if rows.is_empty() {
        out.push_str(&format!(r#"<p class="muted">{}</p>"#, escape(empty)));
    } else {
        out.push_str("<ul class=\"dash-list\">");
        for row in rows {
            out.push_str(&format!("<li>{row}</li>"));
        }
        out.push_str("</ul>");
    }
    out.push_str("</section>");
    out
}

pub(super) fn page(
    store: &Store,
    c: &Console,
    _params: &BTreeMap<String, String>,
) -> Result<String> {
    let launch = c.project.to_string_lossy().into_owned();
    let recorded = store.projects(now() - DAY, 1000)?;
    let open_sessions: i64 = recorded.iter().map(|p| p.open_sessions).sum();
    let denials: i64 = recorded.iter().map(|p| p.recent_denials).sum();
    let open_plans: i64 = recorded.iter().map(|p| p.open_plans).sum();
    // The shell already refreshed notifications for this render; read the count without repeating it.
    let unread = yoke_core::notifications::unread_count(store, &c.project).unwrap_or(0);
    let is_recorded = |path: &str| recorded.iter().any(|p| p.project == path);
    // Page links keep the project when it is not the launch project and is a recorded one.
    let scoped = |page: &str, query: &str, path: &str| -> Option<String> {
        if path == launch {
            Some(render::href(page, query))
        } else if is_recorded(path) {
            Some(render::href(
                page,
                &format!("{query}&amp;project={}", projects::project_id(path)),
            ))
        } else {
            None
        }
    };

    let mut out = render::heading(
        "dashboard",
        "What is happening across every project this Yoke home has recorded.",
    );
    out.push_str("<section class=\"panel\" aria-label=\"Status\"><dl class=\"stats\">");
    out.push_str(&stat("Projects", recorded.len(), "recorded so far"));
    out.push_str(&stat(
        "Sessions without an end",
        open_sessions,
        "not proof an agent still runs",
    ));
    out.push_str(&stat("Denials, last 24 h", denials, "denied or would deny"));
    out.push_str(&stat("Open plans", open_plans, "not done or cancelled"));
    out.push_str(&stat("Unread notifications", unread, "in this project"));
    out.push_str("</dl></section>");

    out.push_str("<div class=\"dash-grid\">");
    let sessions: Vec<String> = store
        .sessions(None, RECENT)?
        .into_iter()
        .map(|row| {
            let id = if row.id.is_empty() {
                "No session id recorded".to_owned()
            } else {
                match scoped("decisions", &format!("session={}", encode(&row.id)), &row.project) {
                    Some(href) => format!(
                        r#"<a data-row data-nav href="{href}">{}</a>"#,
                        escape(&row.id)
                    ),
                    None => escape(&row.id),
                }
            };
            let plan = match (&row.plan_slug, &row.plan_state) {
                (Some(slug), Some(state)) => {
                    format!(" &middot; plan {} ({})", escape(slug), escape(state))
                }
                _ => String::new(),
            };
            format!(
                "{} {id}<br><span class=\"muted\">{} &middot; started {} &middot; {} decisions{plan}</span>",
                badge("", &row.agent),
                escape(project_name(&row.project)),
                time(row.started_at),
                row.decision_count
            )
        })
        .collect();
    out.push_str(&section(
        "Recent sessions",
        "sessions",
        "Show all sessions",
        &sessions,
        "No sessions yet. Start Claude or Codex with Yoke hooks to record one.",
    ));

    let plans: Vec<String> = overview::recent_plans(store, RECENT)?
        .into_iter()
        .map(|plan| {
            let name = match scoped("plans", &format!("slug={}", encode(&plan.slug)), &plan.project) {
                // Plans open only for projects the console can read; others stay plain text.
                Some(href) if plan.project == launch => format!(
                    r#"<a data-row data-nav href="{href}">{}</a>"#,
                    escape(&plan.slug)
                ),
                _ => escape(&plan.slug),
            };
            format!(
                "{name} {}<br><span class=\"muted\">{} &middot; updated {} &middot; revision {}</span>",
                badge(&plan.state, &plan.state),
                escape(project_name(&plan.project)),
                time(plan.updated_at),
                plan.revision
            )
        })
        .collect();
    out.push_str(&section(
        "Recent plans",
        "plans",
        "Show all plans",
        &plans,
        "No plans yet. Create one in your terminal with yoke plan new.",
    ));

    let memories: Vec<String> = overview::recent_memories(store, RECENT)?
        .into_iter()
        .map(|memory| {
            format!(
                "{}{}<br><span class=\"muted\">{} &middot; {} &middot; {}</span>",
                if memory.pinned {
                    badge("", "Pinned")
                } else {
                    String::new()
                },
                render::excerpt(&memory.text),
                escape(project_name(&memory.project)),
                escape(&memory.source),
                time(memory.ts)
            )
        })
        .collect();
    out.push_str(&section(
        "Recent memories",
        "memory",
        "Show all memories",
        &memories,
        "No memories yet. Save one with yoke remember.",
    ));

    // The launch project first, then the most recently active recorded projects.
    let mut scan: Vec<&str> = vec![launch.as_str()];
    scan.extend(
        recorded
            .iter()
            .map(|p| p.project.as_str())
            .filter(|path| *path != launch)
            .take(PROJECTS_SCANNED),
    );
    let requirements: Vec<String> = prd::recent(scan, RECENT)
        .into_iter()
        .map(|(project, doc)| {
            let title = match scoped("requirements", &format!("doc={}", encode(&doc.file)), &project) {
                Some(href) if project == launch => format!(
                    r#"<a data-row data-nav href="{href}">{}</a>"#,
                    escape(&doc.title)
                ),
                _ => escape(&doc.title),
            };
            let status = doc
                .meta
                .get("Status")
                .map(|s| badge("", s))
                .unwrap_or_default();
            format!(
                "{title} {status}<br><span class=\"muted\">{} &middot; {} sections &middot; changed {}</span>",
                escape(project_name(&project)),
                doc.sections.len(),
                time(doc.modified_at)
            )
        })
        .collect();
    out.push_str(&section(
        "Recent requirements",
        "requirements",
        "Show all requirements",
        &requirements,
        "No requirement documents yet. Put Markdown files in docs/prd.",
    ));
    out.push_str("</div>");
    out.push_str("<p class=\"muted\">Plans, memory and requirements pages open the launch project; other projects appear here as plain text. Sessions and decisions link to their own project.</p></main>");
    Ok(out)
}

/// Handle a delegated console action. Return `Outcome::Confirm(message)` for a step that needs
/// the human to confirm; the action is then re-run with `confirmed = true` after they do.
pub(super) fn act(
    store: &Store,
    c: &Console,
    name: &str,
    params: &Value,
    confirmed: bool,
) -> Result<Outcome> {
    match name {
        "plans.cancel" => plan_stats::cancel(store, c, params, confirmed),
        _ if name.starts_with("plans.worktree.") => {
            plan_worktrees::act(store, c, name, params, confirmed)
        }
        _ if name.starts_with("prd.") => {
            super::requirements::act(store, c, name, params, confirmed)
        }
        // The documentation page is read-only, so `docs.` has no actions.
        _ => anyhow::bail!("Unknown action"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yoke_core::{memory, plan, store::DecisionRecord};

    fn record(store: &Store, session: &str, project: &str) {
        store
            .record_decision(
                &DecisionRecord {
                    policy_hash: None,
                    ts: 1_760_000_000,
                    session_id: session.into(),
                    agent: "claude".into(),
                    project: project.into(),
                    event: "pre_tool_use".into(),
                    tool: Some("Bash".into()),
                    mode: "enforce".into(),
                    outcome: "deny".into(),
                    reason: None,
                    gates: serde_json::json!([]),
                    payload: "{}".into(),
                },
                4096,
            )
            .unwrap();
    }

    #[test]
    fn dashboard_summarises_every_project_and_links_only_what_the_console_can_open() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (alpha, beta) = (root.join("alpha"), root.join("beta"));
        for dir in [&alpha, &beta] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::create_dir_all(alpha.join("docs/prd")).unwrap();
        std::fs::write(
            alpha.join("docs/prd/2026-11-01-export.md"),
            "# Export <b>\nStatus: Draft\n## Problem\n",
        )
        .unwrap();
        let database = root.join("state.db");
        let store = Store::open(&database).unwrap();
        let (a, b) = (alpha.to_string_lossy(), beta.to_string_lossy());
        record(&store, "s-alpha", &a);
        record(&store, "s-beta", &b);
        plan::create(&store, &alpha, ".yoke/plans", "launch-plan", "Launch").unwrap();
        plan::create(&store, &beta, ".yoke/plans", "other-plan", "Other").unwrap();
        memory::remember(
            &store,
            &b,
            "beta <script>alert(1)</script> fact",
            false,
            1_760_000_100,
        )
        .unwrap();
        let c = Console {
            home: root.clone(),
            project: alpha.clone(),
            database,
            token: "token".into(),
            csrf: "csrf".into(),
            host: "127.0.0.1:1".into(),
        };

        let html = page(&store, &c, &BTreeMap::new()).unwrap();

        for heading in [
            "Recent sessions",
            "Recent plans",
            "Recent memories",
            "Recent requirements",
        ] {
            assert!(html.contains(heading), "{heading}");
        }
        assert!(html.contains("Projects</dt><dd>2</dd>"), "{html}");
        // The launch plan and requirement open; the other project's plan is plain text.
        assert!(html.contains(r#"href="plans?slug=launch-plan""#));
        assert!(html.contains(r#"href="requirements?doc=2026-11-01-export.md""#));
        assert!(html.contains("other-plan") && !html.contains("slug=other-plan"));
        // Another project's session still links, scoped to that project.
        assert!(html.contains(&format!(
            "session=s-beta&amp;project={}",
            projects::project_id(&b)
        )));
        // Stored text is escaped everywhere.
        assert!(html.contains("&lt;script&gt;") && !html.contains("<script>alert"));
        assert!(html.contains("Export &lt;b&gt;"));
    }
}
