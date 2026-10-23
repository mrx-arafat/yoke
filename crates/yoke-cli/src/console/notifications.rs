//! Notifications page: actionable history with acknowledgement and searchable recovery help.
use super::{
    render::{self, encode, escape},
    Console,
};
use anyhow::Result;
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};
use yoke_core::{
    config,
    notifications::{self, HelpTopic, Notification},
    store::Store,
};

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or_default()
}

/// Materialize new notifications, then count unread ones for the navigation badge.
/// A failed refresh still reports the rows already stored.
pub(super) fn unread(store: &Store, c: &Console) -> i64 {
    let _ = refresh(store, c);
    notifications::unread_count(store, &c.project).unwrap_or(0)
}

fn refresh(store: &Store, c: &Console) -> Result<()> {
    let loaded = config::load_trusted(store, &c.project, &c.home)?;
    crate::notifications::refresh_quota(store, &c.home, loaded.config.usage.warn_percent, now())?;
    notifications::sync(store, &loaded, now()).map(|_| ())
}

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "hook_error" => "Hook error",
        "observe_would_deny" => "Observe mode",
        "plan_waiting" => "Plan waiting",
        "plan_stalled" => "Plan stalled",
        "check_failed" => "Check failed",
        "config_stale" => "Stale config",
        "usage_limit" => "Quota warning",
        _ => "Untrusted config",
    }
}

fn target(c: &Console, n: &Notification) -> (String, &'static str) {
    match n.kind.as_str() {
        "usage_limit" => (render::link(c, "usage"), "Inspect quota and recent burn"),
        "hook_error" => (
            render::href("decisions", "outcome=error"),
            "Open error decisions",
        ),
        "observe_would_deny" => (
            render::href("decisions", "outcome=would_deny"),
            "Open would-deny decisions",
        ),
        "plan_waiting" | "plan_stalled" | "check_failed" => (
            render::href("plans", &format!("slug={}", encode(&n.subject))),
            "Open plan evidence",
        ),
        _ => (render::link(c, "trust"), "Compare trusted and disk config"),
    }
}

fn topic(topic: &HelpTopic, heading: &str) -> String {
    let steps: String = topic
        .steps
        .iter()
        .map(|step| format!("<li>{}</li>", escape(step)))
        .collect();
    let commands: String = topic
        .commands
        .iter()
        .map(|command| format!("<li><code>{}</code></li>", escape(command)))
        .collect();
    format!(
        r#"<{heading}>{}</{heading}><p>{}</p><ol class="ntf-steps">{steps}</ol><p class="muted">Commands to run in your own terminal:</p><ul class="ntf-commands">{commands}</ul><p class="muted">Also documented in <code>{}</code>.</p>"#,
        escape(topic.title),
        escape(topic.summary),
        escape(topic.doc)
    )
}

fn item(c: &Console, n: &Notification) -> String {
    let unread = n.state == "unread";
    let (href, label) = target(c, n);
    let mut history = format!(
        r#"First seen <time data-time="{}">{}</time> &middot; last seen <time data-time="{}">{}</time>"#,
        n.first_seen, n.first_seen, n.last_seen, n.last_seen
    );
    if let Some(at) = n.acknowledged_at {
        history.push_str(&format!(
            r#" &middot; {} <time data-time="{at}">{at}</time>"#,
            if n.actor.as_deref() == Some("yoke") {
                "resolved before you saw it"
            } else {
                "acknowledged by you"
            }
        ));
    }
    if n.resolved_at.is_some() && n.actor.as_deref() == Some("human") {
        history.push_str(" &middot; condition resolved");
    }
    let acknowledge = if unread {
        format!(
            r#"<div class="actions action-zone"><button type="button" class="quiet" data-action="acknowledge" data-id="{}" aria-describedby="ntf-{}">Acknowledge</button></div>"#,
            n.id, n.id
        )
    } else {
        String::new()
    };
    format!(
        r#"<article class="panel ntf-item{}" aria-labelledby="ntf-{id}"><div class="list-caption"><span><span class="badge ntf-state-{}">{}</span> {}</span><span>{} {}</span></div><h2 id="ntf-{id}"><a data-nav data-row href="{href}" aria-label="{}: {label}">{}</a></h2><p class="ntf-detail">{}</p><p class="muted">{history}</p><details class="ntf-help"><summary>How to recover</summary>{}</details>{acknowledge}</article>"#,
        if unread { " ntf-unread" } else { "" },
        escape(&n.state),
        if unread { "Unread" } else { "Acknowledged" },
        kind_label(&n.kind),
        n.count,
        if n.count == 1 {
            "occurrence"
        } else {
            "occurrences"
        },
        escape(&n.title),
        escape(&n.title),
        escape(&n.detail),
        topic(notifications::help_for(&n.kind), "h3"),
        id = n.id,
    )
}

pub(super) fn page(
    store: &Store,
    c: &Console,
    params: &BTreeMap<String, String>,
) -> Result<String> {
    let refreshed = refresh(store, c);
    let unread_only = params.get("show").map(String::as_str) == Some("unread");
    let query = params.get("q").map(String::as_str).unwrap_or("");
    let rows = notifications::list(store, &c.project, unread_only, 200)?;
    let mut out = render::heading(
        "notifications",
        "Problems in this project and the global configuration that need a human, with the steps to recover. Other projects show theirs when the console or yoke notifications runs from that project.",
    );
    if let Err(error) = refreshed {
        out.push_str(&format!(
            r#"<p class="panel error" role="status">New notifications could not be collected: {}. Showing stored notifications.</p>"#,
            escape(&yoke_core::redact::redact(&format!("{error:#}")))
        ));
    }
    let unread_ids: Vec<String> = rows
        .iter()
        .filter(|n| n.state == "unread")
        .map(|n| n.id.to_string())
        .collect();
    let mut options = String::new();
    for (value, label) in [("", "All notifications"), ("unread", "Unread only")] {
        options.push_str(&format!(
            r#"<option value="{value}"{}>{label}</option>"#,
            if unread_only == (value == "unread") {
                " selected"
            } else {
                ""
            }
        ));
    }
    out.push_str(&format!(
        r#"<form class="filters" data-filter action="notifications" method="get"><input type="hidden" name="q" value="{}"><label>Show<select name="show">{options}</select></label><button type="submit">Apply</button></form>"#,
        escape(query)
    ));
    let mut list = format!(
        r#"<div class="list-caption ntf-toolbar"><span>{} shown &middot; {} unread</span><span class="action-zone">{}</span></div>"#,
        rows.len(),
        unread_ids.len(),
        if unread_ids.is_empty() {
            String::new()
        } else {
            format!(
                r#"<button type="button" class="quiet" data-action="acknowledge-all" data-ids="{}">Acknowledge all shown</button>"#,
                unread_ids.join(",")
            )
        }
    );
    if rows.is_empty() {
        list.push_str(&render::empty(
            if unread_only {
                "Nothing unread"
            } else {
                "No notifications yet"
            },
            "Hook errors, plans that wait for you, failed checks and configuration that needs trust appear here.",
            "yoke notifications --help",
        ));
    }
    for n in &rows {
        list.push_str(&item(c, n));
    }
    let topics = notifications::search_help(query);
    let mut help = format!(
        r#"<section class="panel ntf-help-index" aria-labelledby="recovery-help"><h2 id="recovery-help">Recovery help</h2><form class="filters" data-filter action="notifications" method="get" role="search"><input type="hidden" name="show" value="{}"><label class="grow">Search recovery help<input type="search" name="q" value="{}" placeholder="Try observe, stale, resume or hooks"></label><button type="submit">Search</button></form>"#,
        if unread_only { "unread" } else { "" },
        escape(query)
    );
    if topics.is_empty() {
        help.push_str(&format!(
            r#"<p role="status">No recovery topic matches &ldquo;{}&rdquo;.</p>"#,
            escape(query)
        ));
    }
    for t in topics {
        help.push_str(&format!(
            r#"<article class="ntf-topic" id="help-{}">{}</article>"#,
            escape(t.id),
            topic(t, "h3")
        ));
    }
    help.push_str("</section>");
    // A search is a request for help, so its results come first.
    if query.is_empty() {
        out.push_str(&list);
        out.push_str(&help);
    } else {
        out.push_str(&help);
        out.push_str(&list);
    }
    out.push_str("</main>");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use yoke_core::store::DecisionRecord;

    fn decide(store: &Store, project: &std::path::Path, outcome: &str, reason: &str) {
        store
            .record_decision(
                &DecisionRecord {
                    ts: now(),
                    session_id: "s1".into(),
                    agent: "codex".into(),
                    project: project.display().to_string(),
                    event: "pre_tool_use".into(),
                    tool: Some("shell".into()),
                    mode: "enforce".into(),
                    outcome: outcome.into(),
                    reason: Some(reason.into()),
                    gates: serde_json::json!([]),
                    payload: "{}".into(),
                },
                4096,
            )
            .unwrap();
    }

    #[test]
    fn page_is_project_scoped_escaped_acknowledged_by_action_and_searchable() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (home, project, other) = (root.join("home"), root.join("app"), root.join("other"));
        for dir in [home.join(".yoke"), project.clone(), other.clone()] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let store = Store::open(home.join(".yoke/state.db")).unwrap();
        decide(
            &store,
            &project,
            "error",
            "yoke error: <script>alert(1)</script>",
        );
        decide(&store, &project, "deny", "ordinary gate denial");
        decide(
            &store,
            &other,
            "error",
            "yoke error: foreign-project-sentinel",
        );
        let c = Console {
            home,
            project: project.clone(),
            database: root.join("unused.db"),
            token: "token".into(),
            csrf: "csrf".into(),
            host: "127.0.0.1:1".into(),
        };
        assert_eq!(unread(&store, &c), 1);
        let html = page(&store, &c, &BTreeMap::new()).unwrap();
        assert!(html.contains("Yoke hook failed on pre_tool_use in enforce mode"));
        assert!(html.contains("&lt;script&gt;") && !html.contains("<script>"));
        assert!(
            !html.contains("foreign-project-sentinel"),
            "other projects stay out"
        );
        assert_eq!(
            html.matches("data-action=\"acknowledge\" data-id=").count(),
            1
        );
        let mine = notifications::list(&store, &project, true, 10).unwrap()[0].id;
        let foreign = notifications::list(&store, &other, true, 10).unwrap()[0].id;
        assert!(html.contains(&format!(
            "data-action=\"acknowledge-all\" data-ids=\"{mine}\""
        )));

        let mut actions = super::super::actions::Actions::default();
        let mut apply = |value| actions.apply(&store, &c, serde_json::from_value(value).unwrap());
        assert!(apply(serde_json::json!({"action":"acknowledge","id":foreign})).is_err());
        let result = apply(serde_json::json!({"action":"acknowledge-all","ids":[mine]})).unwrap();
        assert_eq!(result["data"]["message"], "Notification acknowledged.");
        assert_eq!(unread(&store, &c), 0);
        assert_eq!(notifications::unread_count(&store, &other).unwrap(), 1);
        let html = page(&store, &c, &BTreeMap::new()).unwrap();
        assert!(!html.contains("data-action=\"acknowledge"));
        assert!(html.contains("acknowledged by you"));

        let search = |q: &str| {
            page(
                &store,
                &c,
                &BTreeMap::from([("q".to_owned(), q.to_owned())]),
            )
            .unwrap()
        };
        let html = search("disableAllHooks");
        assert!(
            html.contains("id=\"help-hooks-disabled\"")
                && !html.contains("id=\"help-stale-trust\"")
        );
        assert!(html.find("Recovery help").unwrap() < html.find("shown &middot;").unwrap());
        assert!(search("no-such-topic").contains("No recovery topic matches"));
        let at = now();
        let reading = yoke_core::quota::Quota::from_claude(&serde_json::json!({"rate_limits":{"five_hour":{"used_percentage":90,"resets_at":at+3600}}}), at).unwrap();
        yoke_core::quota::record_file(&yoke_core::quota::claude_file(&c.home), &reading, at)
            .unwrap();
        let html = page(&store, &c, &BTreeMap::new()).unwrap();
        assert!(html.contains("Quota warning"));
        assert!(html.contains("Inspect quota and recent burn"));
        assert!(html.contains("href=\"usage\""));
    }
}
