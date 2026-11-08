//! Requirements: requirement documents (`docs/prd/*.md`) for the launch project.
//! Owner: parity-plans (Pilot parity fleet). Its `prd.` actions arrive through `dashboard::act`.
use super::{
    actions::Outcome,
    content,
    render::{self, encode, escape},
    Console,
};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use yoke_core::{
    prd::{self, PrdEntry, Recovery},
    store::Store,
};

fn badge(label: &str) -> String {
    format!(r#"<span class="badge">{}</span>"#, escape(label))
}

fn facts(entry: &PrdEntry) -> String {
    let mut parts: Vec<String> = ["Author", "Agent", "Category", "Created"]
        .iter()
        .filter_map(|key| {
            entry
                .meta
                .get(*key)
                .map(|v| format!("{key}: {}", escape(v)))
        })
        .collect();
    parts.push(format!("{} sections", entry.sections.len()));
    parts.push(format!(
        "changed <time data-time=\"{0}\">{0}</time>",
        entry.modified_at
    ));
    parts.join(" &middot; ")
}

fn list(c: &Console) -> String {
    let docs = prd::list(&c.project);
    let mut out = render::heading(
        "requirements",
        "Requirement documents that plans are built from.",
    );
    if docs.is_empty() {
        out.push_str(&render::empty(
            "No requirement documents yet",
            "Requirements are Markdown files in docs/prd inside this project, for example docs/prd/2026-11-08-export.md. Start one with a # title, optional Author:, Status: and Category: lines, and ## sections such as Problem Statement and Core User Flows.",
            "mkdir -p docs/prd",
        ));
    }
    for doc in &docs {
        let mut tags = String::new();
        for key in ["Status", "Category"] {
            if let Some(value) = doc.meta.get(key) {
                tags.push_str(&badge(value));
            }
        }
        let sections: String = doc
            .sections
            .iter()
            .take(8)
            .map(|s| format!("<li>{}</li>", escape(s)))
            .collect();
        out.push_str(&format!(
            r#"<article class="panel"><div class="list-caption"><h2><a data-nav data-row href="{}">{}</a></h2><span>{tags}</span></div><p class="muted">{}</p><ul class="chips">{sections}</ul></article>"#,
            render::href("requirements", &format!("doc={}", encode(&doc.file))),
            escape(&doc.title),
            facts(doc)
        ));
    }
    out.push_str("<p class=\"muted\">Documents are read from docs/prd and are never sent anywhere. Deleting one needs your confirmation.</p></main>");
    out
}

fn recovery_note(recovery: Recovery, file: &str) -> String {
    match recovery {
        Recovery::Committed => format!(
            "Git has the committed version, so `git restore docs/prd/{file}` brings it back."
        ),
        Recovery::Edited => {
            "Git tracks this file, but its uncommitted edits would be lost.".to_owned()
        }
        Recovery::Untracked => {
            "Git does not track this file, so deleting it cannot be undone.".to_owned()
        }
        Recovery::Unknown => {
            "Yoke could not check Git for this file, so deleting it may not be undoable.".to_owned()
        }
    }
}

fn detail(c: &Console, file: &str) -> Result<String> {
    let doc = prd::read(&c.project, file)?;
    let entry = &doc.entry;
    let mut out = render::heading(
        "requirements",
        "Requirement documents that plans are built from.",
    );
    out.push_str(&format!(
        r#"<p><a data-nav href="{}">All requirements</a></p><section class="panel"><div class="list-caption"><h2>{}</h2><span>{}</span></div><p class="muted">{} &middot; docs/prd/{}</p>"#,
        render::link(c, "requirements"),
        escape(&entry.title),
        entry
            .meta
            .get("Status")
            .map(|s| badge(s))
            .unwrap_or_default(),
        facts(entry),
        escape(&entry.file)
    ));
    if !entry.sections.is_empty() {
        out.push_str("<h3>Sections</h3><ul class=\"chips\">");
        for section in &entry.sections {
            out.push_str(&format!("<li>{}</li>", escape(section)));
        }
        out.push_str("</ul>");
    }
    out.push_str(&format!(
        "<div class=\"prose\">{}</div></section>",
        content::markdown(&doc.content)
    ));
    let params = json!({"file": entry.file, "sha256": doc.sha256}).to_string();
    out.push_str(&format!(
        r#"<details class="panel action-zone"><summary>Delete this requirement...</summary><p class="muted">{}</p><div class="actions"><button type="button" class="danger-button" data-plans-run="prd.delete" data-params="{}" data-after="requirements" data-confirm-label="Delete requirement">Review deletion</button></div></details></main>"#,
        escape(&recovery_note(prd::recovery(&c.project, &entry.file), &entry.file)),
        escape(&params)
    ));
    Ok(out)
}

pub(super) fn page(
    _store: &Store,
    c: &Console,
    params: &BTreeMap<String, String>,
) -> Result<String> {
    match params.get("doc") {
        Some(file) => detail(c, file),
        None => Ok(list(c)),
    }
}

/// `prd.delete` with `{file, sha256}`: the hash is the content the human reviewed, so a document
/// edited after the page rendered is refused rather than deleted.
pub(super) fn act(
    _store: &Store,
    c: &Console,
    name: &str,
    params: &Value,
    confirmed: bool,
) -> Result<Outcome> {
    anyhow::ensure!(name == "prd.delete", "Unknown action");
    let file = params
        .get("file")
        .and_then(Value::as_str)
        .context("Missing file")?;
    let reviewed = params
        .get("sha256")
        .and_then(Value::as_str)
        .context("Missing content hash")?;
    let current = prd::read(&c.project, file)?;
    if current.sha256 != reviewed {
        anyhow::bail!("docs/prd/{file} changed since you opened it; reopen it and review it again");
    }
    if !confirmed {
        return Ok(Outcome::Confirm(format!(
            "Delete docs/prd/{file} ({} bytes)? {}",
            current.entry.bytes,
            recovery_note(prd::recovery(&c.project, file), file)
        )));
    }
    prd::delete(&c.project, file, reviewed)?;
    Ok(Outcome::Done(
        json!({"message": format!("Deleted docs/prd/{file}.")}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requirements_list_render_safely_and_delete_only_after_confirmation_of_the_reviewed_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().canonicalize().unwrap().join("repo");
        std::fs::create_dir_all(project.join("docs/prd")).unwrap();
        let file = "2026-11-01-export.md";
        let path = project.join("docs/prd").join(file);
        std::fs::write(
            &path,
            "# Export <img src=x>\nStatus: Draft\n\n## Problem\n<script>alert(1)</script>\n",
        )
        .unwrap();
        let database = temp.path().join("state.db");
        let store = Store::open(&database).unwrap();
        let c = Console {
            home: temp.path().to_owned(),
            project: project.clone(),
            database,
            token: "token".into(),
            csrf: "csrf".into(),
            host: "127.0.0.1:1".into(),
        };

        let list = page(&store, &c, &BTreeMap::new()).unwrap();
        assert!(list.contains(r#"href="requirements?doc=2026-11-01-export.md""#));
        assert!(list.contains("Export &lt;img src=x&gt;") && !list.contains("<img"));
        let params = BTreeMap::from([("doc".to_owned(), file.to_owned())]);
        let detail = page(&store, &c, &params).unwrap();
        assert!(detail.contains("Delete this requirement") && !detail.contains("<script>alert"));
        let stale = prd::read(&project, file).unwrap().sha256;

        let ask = |sha: &str, confirmed: bool| {
            act(
                &store,
                &c,
                "prd.delete",
                &json!({"file": file, "sha256": sha}),
                confirmed,
            )
        };
        let Outcome::Confirm(message) = ask(&stale, false).unwrap() else {
            panic!("expected a confirmation step");
        };
        assert!(message.contains("cannot be undone") || message.contains("may not be undoable"));
        assert!(path.exists(), "asking must not delete");
        std::fs::write(&path, "# Export\nedited after review\n").unwrap();
        assert!(ask(&stale, true).is_err(), "edited since review");
        assert!(path.exists());

        let fresh = prd::read(&project, file).unwrap().sha256;
        assert!(matches!(ask(&fresh, true).unwrap(), Outcome::Done(_)));
        assert!(!path.exists());
        assert!(act(
            &store,
            &c,
            "prd.delete",
            &json!({"file": "../x.md", "sha256": "0"}),
            true
        )
        .is_err());
    }
}
