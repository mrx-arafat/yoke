use super::{
    content,
    render::{self, encode, escape},
    Console,
};
use anyhow::Result;
use yoke_core::plan::{self, EvidenceStatus, RequirementStatus, TaskState};
use yoke_core::store::Store;

fn badge(class: &str, label: &str) -> String {
    format!(
        r#"<span class="badge {}">{}</span>"#,
        escape(class),
        escape(label)
    )
}

/// One plan: contract, progress, feedback, transitions and check evidence.
pub(super) fn page(store: &Store, c: &Console, slug: &str) -> Result<String> {
    let detail = plan::detail(store, &c.project, slug)?;
    let state = detail.plan.state.as_str();
    let mut out = format!(
        r#"<p><a data-nav href="{}">All plans</a></p><section class="panel"><div class="list-caption"><h2>{}</h2>{}</div>"#,
        render::link(c, "plans"),
        escape(slug),
        badge(state, state)
    );
    match state {
        "draft" => out.push_str(&format!(r#"<div class="action-zone"><button data-action="prepare-approval" data-slug="{}">Review approval</button></div>"#, escape(slug))),
        "waiting" | "stalled" => out.push_str(&format!(r#"<div class="action-zone"><button data-action="resume" data-slug="{}">Resume work</button></div>"#, escape(slug))),
        "checking" => out.push_str(&format!(r#"<div class="action-zone"><p class="muted">If the check process died, recovery returns the plan to building and records the interrupted check as invalid evidence. It is refused while a live check still runs.</p><button data-action="resume" data-slug="{}">Recover interrupted check</button></div>"#, escape(slug))),
        _ => {}
    }
    progress(&mut out, &detail.progress);
    feedback(&mut out, slug, &detail.feedback);
    let (header, body) = split_header(&detail.content);
    out.push_str(&format!(
        "<div class=\"prose\">{}{}</div></section>",
        header
            .map(|header| format!(
                "<pre class=\"plan-header\"><code>{}</code></pre>",
                content::highlight(header)
            ))
            .unwrap_or_default(),
        content::markdown(body)
    ));
    if let Some(header) = &detail.plan.approved_header {
        out.push_str(&format!("<details class=\"panel\"><summary>Frozen approved header</summary><pre><code>{}</code></pre></details>", content::highlight(header)));
    }
    out.push_str("<section class=\"panel\"><h2>Transitions</h2>");
    if detail.transitions.is_empty() {
        out.push_str("<p class=\"muted\">No transitions recorded yet.</p>");
    }
    for t in &detail.transitions {
        let change = if t.from_state.is_empty() {
            format!("created as {}", escape(&t.to_state))
        } else {
            format!("{} &rarr; {}", escape(&t.from_state), escape(&t.to_state))
        };
        out.push_str(&format!(
            "<p><time data-time=\"{}\">{}</time> &middot; <strong>{}</strong>: {change}<br>{}</p>",
            t.ts,
            t.ts,
            escape(&t.actor),
            escape(t.note.as_deref().unwrap_or(""))
        ));
    }
    out.push_str("</section><section class=\"panel\"><h2>Checks and review evidence</h2>");
    if detail.checks.is_empty() {
        out.push_str(
            "<p class=\"muted\">No check runs yet. Run the plan checks after work begins.</p>",
        );
    }
    for check in &detail.checks {
        out.push_str(&format!("<details class=\"check\"><summary data-row>{} - {} ({} ms)</summary><p>{} &middot; exit {}</p><pre><code>{}</code></pre>{}<h3>Review findings</h3><pre><code>{}</code></pre>", escape(&check.name), escape(&check.result), check.ms, if check.blocking { "Blocking" } else { "Advisory" }, check.exit_code.map(|v| v.to_string()).unwrap_or_else(|| "unavailable".into()), content::highlight(&check.command), if check.output_tail.is_empty() { "<p class=\"muted\">No output captured.</p>".to_owned() } else { format!("<pre><code>{}</code></pre>", escape(&check.output_tail)) }, escape(&serde_json::to_string_pretty(&check.findings)?)));
        for (index, path) in check.artifacts.iter().enumerate() {
            let status = detail
                .evidence
                .iter()
                .find(|item| item.check_run_id == check.id && &item.path == path)
                .map_or(EvidenceStatus::Unrecorded, |item| item.status);
            match status {
                EvidenceStatus::Verified | EvidenceStatus::Unrecorded => {
                    let url = render::href(
                        "artifact",
                        &format!("slug={}&amp;check={}&amp;index={index}", encode(slug), check.id),
                    );
                    let note = if status == EvidenceStatus::Verified {
                        badge("done", "hash verified")
                    } else {
                        badge("waiting", "captured before hashes")
                    };
                    out.push_str(&format!(r#"<p><button class="quiet screenshot" data-image="{url}" data-caption="{} screenshot {}">Open screenshot {}</button> {note}</p>"#, escape(&check.name), index + 1, index + 1));
                }
                EvidenceStatus::Tampered => out.push_str(&format!("<p class=\"error\">Screenshot {} changed after the check captured it, so it is not shown as evidence.</p>", index + 1)),
                EvidenceStatus::Missing => out.push_str(&format!("<p class=\"error\">Screenshot {} no longer exists on disk.</p>", index + 1)),
            }
        }
        out.push_str("</details>");
    }
    out.push_str("</section>");
    Ok(out)
}

/// Separate the `+++` TOML header from the Markdown body so the header renders as code.
fn split_header(content: &str) -> (Option<&str>, &str) {
    let Some(rest) = content
        .strip_prefix("+++\n")
        .or_else(|| content.strip_prefix("+++\r\n"))
    else {
        return (None, content);
    };
    let mut end = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == "+++" {
            return (Some(&rest[..end]), &rest[end + line.len()..]);
        }
        end += line.len();
    }
    (None, content)
}

fn progress(out: &mut String, progress: &plan::Progress) {
    if progress.tasks.is_empty() && progress.requirements.is_empty() {
        return;
    }
    if !progress.requirements.is_empty() {
        let satisfied = progress
            .requirements
            .iter()
            .filter(|item| item.status != RequirementStatus::Unsatisfied)
            .count();
        out.push_str(&format!("<h3>Requirements &middot; {satisfied} of {} satisfied</h3><div class=\"table-scroll\" tabindex=\"0\" role=\"region\" aria-label=\"Requirements\"><table><thead><tr><th scope=\"col\">Requirement</th><th scope=\"col\">Accepted by</th><th scope=\"col\">Status</th></tr></thead><tbody>", progress.requirements.len()));
        for item in &progress.requirements {
            let by = if item.requirement.human {
                "Human evidence".to_owned()
            } else {
                format!("Checks: {}", item.requirement.checks.join(", "))
            };
            let status = match item.status {
                RequirementStatus::Check => badge("done", "Passing checks"),
                RequirementStatus::Human => badge("done", "Human evidence"),
                RequirementStatus::Unsatisfied => badge("error", "Unsatisfied"),
            };
            let evidence = item
                .completion
                .as_ref()
                .and_then(|done| done.evidence.as_deref())
                .map(|text| format!("<br><span class=\"muted\">{}</span>", escape(text)))
                .unwrap_or_default();
            out.push_str(&format!("<tr><th scope=\"row\"><code>{}</code><br>{}{evidence}</th><td>{}</td><td>{status}</td></tr>", escape(&item.requirement.id), escape(&item.requirement.text), escape(&by)));
        }
        out.push_str("</tbody></table></div>");
    }
    if !progress.tasks.is_empty() {
        let done = progress
            .tasks
            .iter()
            .filter(|item| item.state == TaskState::Done)
            .count();
        out.push_str(&format!("<h3>Tasks &middot; {done} of {} done</h3><div class=\"table-scroll\" tabindex=\"0\" role=\"region\" aria-label=\"Tasks\"><table><thead><tr><th scope=\"col\">Task</th><th scope=\"col\">Owner</th><th scope=\"col\">Depends on</th><th scope=\"col\">Status</th></tr></thead><tbody>", progress.tasks.len()));
        for item in &progress.tasks {
            let status = match item.state {
                TaskState::Done => badge("done", "Done"),
                TaskState::Ready => badge("waiting", "Ready"),
                TaskState::Blocked => badge("", "Blocked"),
            };
            let depends = if item.task.depends_on.is_empty() {
                "None".to_owned()
            } else {
                item.task.depends_on.join(", ")
            };
            out.push_str(&format!("<tr><th scope=\"row\"><code>{}</code><br>{}</th><td>{}</td><td>{}</td><td>{status}</td></tr>", escape(&item.task.id), escape(&item.task.title), item.task.owner.as_str(), escape(&depends)));
        }
        out.push_str("</tbody></table></div>");
    }
}

fn feedback(out: &mut String, slug: &str, items: &[plan::Feedback]) {
    if items.is_empty() {
        return;
    }
    let open = items
        .iter()
        .filter(|item| item.state.as_deref() == Some("open"))
        .count();
    out.push_str(&format!(
        "<h3>Review findings and feedback &middot; {open} open</h3>"
    ));
    for item in items.iter().filter(|item| item.parent_id.is_none()) {
        let kind = match item.severity.as_deref() {
            Some("blocking") => badge("error", "Blocking finding"),
            Some(_) => badge("waiting", "Advice"),
            None => badge("", "Comment"),
        };
        let state = item.state.as_deref().unwrap_or("open");
        let state_badge = badge(if state == "open" { "waiting" } else { "done" }, state);
        let mut notes = Vec::new();
        if !item.current {
            notes.push("written against an earlier plan revision");
        }
        if !item.anchored {
            notes.push("anchor no longer exists");
        }
        out.push_str(&format!("<article class=\"panel\"><div class=\"list-caption\"><span>#{} {kind} {state_badge} <code>{}</code></span><span>{} &middot; <time data-time=\"{}\">{}</time></span></div><p>{}</p>", item.id, escape(&item.anchor), escape(&item.author), item.ts, item.ts, escape(&item.text)));
        if !notes.is_empty() {
            out.push_str(&format!(
                "<p class=\"muted\">{}</p>",
                escape(&notes.join("; "))
            ));
        }
        if state != "open" {
            out.push_str(&format!(
                "<p class=\"muted\">{} by {}{}</p>",
                escape(state),
                escape(item.resolved_by.as_deref().unwrap_or("human")),
                item.note
                    .as_deref()
                    .map(|note| format!(": {}", escape(note)))
                    .unwrap_or_default()
            ));
        }
        for reply in items
            .iter()
            .filter(|reply| reply.parent_id == Some(item.id))
        {
            out.push_str(&format!(
                "<p class=\"scope\">Reply from {}: {}</p>",
                escape(&reply.author),
                escape(&reply.text)
            ));
        }
        if state == "open" {
            out.push_str(&format!("<p class=\"muted\">A human closes this in a terminal: <code>yoke plan resolve {} {}</code> or <code>yoke plan acknowledge {} {}</code>.</p>", escape(slug), item.id, escape(slug), item.id));
        }
        out.push_str("</article>");
    }
}

#[cfg(test)]
mod tests {
    use super::super::{actions, Console};
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::process::Command;
    use yoke_core::{plan, store::DecisionRecord, store::Store};

    /// Git writes are confined to the newly created temporary fixture repository.
    fn git(root: &Path, args: &[&str]) {
        assert!(root.starts_with(std::env::temp_dir().canonicalize().unwrap()));
        let output = Command::new("git")
            .current_dir(root)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .args([
                "-c",
                "user.name=Yoke Fixture",
                "-c",
                "user.email=fixture@example.invalid",
            ])
            .args([
                "-c",
                "commit.gpgSign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn screenshots_are_served_only_while_hashes_match_and_checking_offers_recovery() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().canonicalize().unwrap().join("repo");
        std::fs::create_dir(&project).unwrap();
        git(&project, &["init", "-q"]);
        std::fs::write(project.join(".gitignore"), "output/\n").unwrap();
        git(&project, &["add", ".gitignore"]);
        git(&project, &["commit", "-qm", "fixture"]);
        let database = temp.path().join("state.db");
        let store = Store::open(&database).unwrap();
        store
            .record_decision(
                &DecisionRecord {
                    policy_hash: None,
                    ts: 1,
                    session_id: "s".into(),
                    agent: "codex".into(),
                    project: project.to_str().unwrap().into(),
                    event: "session_start".into(),
                    tool: None,
                    mode: "observe".into(),
                    outcome: "allow".into(),
                    reason: None,
                    gates: serde_json::json!([]),
                    payload: "{}".into(),
                },
                4096,
            )
            .unwrap();
        let row = plan::create(&store, &project, ".yoke/plans", "shot", "Shot").unwrap();
        std::fs::write(&row.file, "+++\ntitle='Shot'\n[[check]]\nname='ui'\nrun='mkdir -p output && printf \"\\211PNG\\r\\n\\032\\nfirst\" > output/a.png'\nartifacts=['output/a.png']\n+++\n").unwrap();
        let prepared = plan::prepare_approval(&store, &project, "shot").unwrap();
        plan::approve(&store, &prepared).unwrap();
        plan::start(&store, &project, "shot", "s").unwrap();
        assert!(plan::check(&store, &project, "shot", None).unwrap().passed);
        let c = Console {
            home: temp.path().to_owned(),
            project: project.clone(),
            database: database.clone(),
            token: "token".into(),
            csrf: "csrf".into(),
            host: "127.0.0.1:1".into(),
        };
        let check = plan::detail(&store, &project, "shot").unwrap().checks[0].id;
        let params = BTreeMap::from([
            ("slug".to_owned(), "shot".to_owned()),
            ("check".to_owned(), check.to_string()),
            ("index".to_owned(), "0".to_owned()),
        ]);
        assert!(super::page(&store, &c, "shot")
            .unwrap()
            .contains("hash verified"));
        assert_eq!(
            actions::artifact(&store, &c, &params).unwrap().1,
            "image/png"
        );
        std::fs::write(project.join("output/a.png"), b"\x89PNG\r\n\x1a\nforged").unwrap();
        let refused = actions::artifact(&store, &c, &params).unwrap_err();
        assert!(refused.to_string().contains("changed after"), "{refused:#}");
        let html = super::page(&store, &c, "shot").unwrap();
        assert!(
            html.contains("changed after the check captured it")
                && !html.contains("Open screenshot")
        );
        assert!(html.contains(r#"<pre class="plan-header"><code>"#) && !html.contains("<p>+++"));
        assert!(html.contains("created as draft") && html.contains("No output captured."));
        let connection = rusqlite::Connection::open(&database).unwrap();
        connection
            .execute("UPDATE plans SET state='checking' WHERE slug='shot'", [])
            .unwrap();
        let html = super::page(&store, &c, "shot").unwrap();
        assert!(html.contains(r#"data-action="resume" data-slug="shot">Recover interrupted check"#));
    }
}
