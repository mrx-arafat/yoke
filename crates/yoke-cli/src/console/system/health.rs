//! Health view: the `yoke health` checks for this project as a read-only page, rendered from
//! the same JSON report `yoke health --json` prints.
use super::super::{render::escape, Console};
use serde_json::Value;

pub(super) fn section(c: &Console) -> String {
    match crate::health::report_json(&c.home, &c.project) {
        Ok(report) => render(&report),
        Err(error) => format!(
            r#"<section class="panel" aria-labelledby="sys-health"><h2 id="sys-health">Health</h2><p class="error" role="status">The health checks could not run: {}. Run yoke health in your own terminal.</p></section>"#,
            escape(&yoke_core::redact::redact(&format!("{error:#}")))
        ),
    }
}

fn text(value: &Value) -> String {
    escape(value.as_str().unwrap_or(""))
}

fn yes_no(value: &Value) -> &'static str {
    if value.as_bool().unwrap_or(false) {
        "yes"
    } else {
        "no"
    }
}

fn list(heading: &str, items: &Value, class: &str) -> String {
    let items: String = items
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| format!("<li>{}</li>", text(item)))
        .collect();
    if items.is_empty() {
        String::new()
    } else {
        format!(r#"<h3>{heading}</h3><ul class="{class}">{items}</ul>"#)
    }
}

fn render(report: &Value) -> String {
    let status = report["status"].as_str().unwrap_or("unknown");
    let state = &report["state"];
    let schema = match state["schema"].as_u64() {
        Some(schema) => schema.to_string(),
        None if state["exists"].as_bool() == Some(true) => "unknown".into(),
        None => "missing".into(),
    };
    let memory = match report["memory"].as_object() {
        Some(memory) => format!(
            "{} pending facts; digest {}{}",
            memory
                .get("pending_facts")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            if memory.get("digest").and_then(Value::as_bool) == Some(true) {
                "on"
            } else {
                "off"
            },
            memory
                .get("note")
                .and_then(Value::as_str)
                .map(|note| format!(". {}", escape(note)))
                .unwrap_or_default()
        ),
        None => "unknown".into(),
    };
    let agents: String = report["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|agent| {
            let last = &agent["last_event"];
            let last = if last.is_object() {
                format!(
                    r#"<time data-time="{ts}">{}</time> {} ({}, {})"#,
                    escape(&yoke_core::timefmt::local_time_secs(
                        last["ts"].as_i64().unwrap_or(0)
                    )),
                    text(&last["event"]),
                    text(&last["mode"]),
                    text(&last["outcome"]),
                    ts = last["ts"].as_i64().unwrap_or(0),
                )
            } else {
                "none recorded".into()
            };
            let notes: String = agent["problems"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|problem| format!(r#"<p class="error">{}</p>"#, text(problem)))
                .collect();
            format!(
                r#"<tr><th scope="row">{}</th><td>{}</td><td><code>{}</code>{notes}</td><td>{last}</td></tr>"#,
                text(&agent["agent"]),
                if agent["installed"].as_bool() == Some(true) {
                    "installed"
                } else {
                    "not installed"
                },
                text(&agent["settings"]),
            )
        })
        .collect();
    let policy: String = report["policy"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|file| {
            format!(
                r#"<tr><th scope="row"><code>{}</code></th><td>{}</td><td>{}</td><td>{}</td></tr>"#,
                text(&file["file"]),
                yes_no(&file["trusted"]),
                yes_no(&file["stale"]),
                yes_no(&file["exists"]),
            )
        })
        .collect();
    let plans = match report["plans"].as_array() {
        Some(plans) if plans.is_empty() => "None".to_owned(),
        Some(plans) => plans
            .iter()
            .map(|plan| {
                format!(
                    "<code>{}</code> {}",
                    text(&plan["slug"]),
                    text(&plan["state"])
                )
            })
            .collect::<Vec<_>>()
            .join(", "),
        None => "unknown".into(),
    };
    format!(
        r#"<section class="panel" aria-labelledby="sys-health"><div class="list-caption"><h2 id="sys-health">Health</h2><span class="badge sys-status-{status_class}">{status}</span></div><p>{meaning}</p>{problems}{guidance}<dl class="facts oversight"><dt>Project</dt><dd><code>{project}</code></dd><dt>Mode</dt><dd>{mode}</dd><dt>State</dt><dd><code>{path}</code> schema {schema} (this yoke writes {supported})</dd><dt>Executable</dt><dd><code>{executable}</code></dd><dt>Plans</dt><dd>{plans}</dd><dt>Memory</dt><dd>{memory}</dd></dl><h3>Agents</h3><div class="table-scroll" tabindex="0" role="region" aria-label="Agent hooks"><table><thead><tr><th scope="col">Agent</th><th scope="col">Hooks</th><th scope="col">Settings</th><th scope="col">Last event</th></tr></thead><tbody>{agents}</tbody></table></div><h3>Policy files</h3><div class="table-scroll" tabindex="0" role="region" aria-label="Policy files"><table><thead><tr><th scope="col">File</th><th scope="col">Trusted</th><th scope="col">Changed since trusted</th><th scope="col">On disk</th></tr></thead><tbody>{policy}</tbody></table></div><p class="muted">The same read-only checks as <code>yoke health</code>, run by this console process; a terminal with a different <code>CLAUDE_CONFIG_DIR</code> or <code>CODEX_HOME</code> can report differently. Yoke has no daemon to be online: hooks run once per agent event.</p></section>"#,
        status_class = match status {
            "ok" | "attention" | "failing" => status,
            _ => "unknown",
        },
        status = escape(status),
        meaning = text(&report["meaning"]),
        problems = list("Problems", &report["problems"], "sys-problems"),
        guidance = list("Next steps", &report["guidance"], "sys-guidance"),
        project = text(&report["project"]),
        mode = escape(report["mode"].as_str().unwrap_or("unknown")),
        path = text(&state["path"]),
        supported = state["supported"].as_u64().unwrap_or(0),
        executable = text(&report["executable"]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_view_renders_the_yoke_health_report_escaped_and_read_only() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let c = super::super::test_console(&root);
        drop(yoke_core::store::Store::open(&c.database).unwrap());
        let before = std::fs::metadata(&c.database).unwrap().modified().unwrap();
        let html = section(&c);
        assert!(
            html.contains(r#"<span class="badge sys-status-failing">failing</span>"#),
            "{html}"
        );
        assert!(html.contains("Yoke hooks are not installed for any agent."));
        assert!(html.contains(r#"<th scope="row">claude</th><td>not installed</td>"#));
        assert!(html.contains(&format!("<code>{}</code>", c.project.display())));
        assert_eq!(
            std::fs::metadata(&c.database).unwrap().modified().unwrap(),
            before
        );

        let hostile = serde_json::json!({
            "status": "attention", "meaning": "<b>m</b>", "state": {"path": "/s", "exists": true, "schema": null, "supported": 18},
            "problems": [], "guidance": ["<script>x</script>"], "agents": [], "policy": [], "plans": [], "memory": null
        });
        let html = render(&hostile);
        assert!(html.contains("&lt;script&gt;x&lt;/script&gt;") && !html.contains("<script>"));
        assert!(html.contains("schema unknown") && html.contains("<dd>None</dd>"));
    }
}
