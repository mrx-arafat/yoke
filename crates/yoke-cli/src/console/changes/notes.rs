//! Notes the agent reads. A note is a plan comment written from the console by the human, so
//! `yoke plan status` and the agent's existing checkpoints already surface it. Notes on a plan
//! block use the plan's own anchors; notes on a code line use `file:<path>:<line>`.
use super::super::{
    actions::Outcome,
    render::{escape, excerpt},
    Console,
};
use super::str_param;
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use yoke_core::{
    plan::{self, Actor, Feedback, PlanDetail, PlanRecord},
    store::Store,
    timefmt,
};

const MAX_NOTE_CHARS: usize = 4000;

/// Plans a new note can attach to, and the one the page selected.
pub(crate) struct Choice {
    pub plans: Vec<PlanRecord>,
    pub chosen: Option<usize>,
}

impl Choice {
    pub(crate) fn plan(&self) -> Option<&PlanRecord> {
        self.chosen.and_then(|index| self.plans.get(index))
    }
}

/// Unfinished plans of the console's project, most recently updated first. The plan asked for
/// wins, then one that is building, then the most recent.
pub(crate) fn choose(store: &Store, c: &Console, wanted: Option<&str>) -> Result<Choice> {
    let mut plans: Vec<PlanRecord> = plan::list(store, &c.project)?
        .into_iter()
        .filter(|plan| !matches!(plan.state.as_str(), "done" | "cancelled"))
        .collect();
    plans.sort_by_key(|plan| std::cmp::Reverse(plan.updated_at));
    let chosen = wanted
        .and_then(|slug| plans.iter().position(|plan| plan.slug == slug))
        .or_else(|| plans.iter().position(|plan| plan.state == "building"))
        .or(if plans.is_empty() { None } else { Some(0) });
    Ok(Choice { plans, chosen })
}

/// `note.add`: record a human note on `anchor` of plan `slug`. Core validation decides whether
/// the anchor exists, so a stale page cannot invent one.
pub(crate) fn add(store: &Store, c: &Console, params: &Value) -> Result<Outcome> {
    let slug = str_param(params, "slug")?;
    let anchor = str_param(params, "anchor")?;
    let text = str_param(params, "text")?;
    ensure!(!text.trim().is_empty(), "write the note first");
    ensure!(
        text.chars().count() <= MAX_NOTE_CHARS,
        "keep a note under {MAX_NOTE_CHARS} characters"
    );
    let item = plan::comment(store, &c.project, slug, anchor, text, Actor::Human)?;
    Ok(Outcome::Done(json!({
        "message": format!("Note #{} added to {slug} at {anchor}. The agent sees it with yoke plan status.", item.id),
    })))
}

/// `note.export`: the plan and its notes as one Markdown file the browser saves locally.
pub(crate) fn export(store: &Store, c: &Console, params: &Value) -> Result<Outcome> {
    let slug = str_param(params, "slug")?;
    let detail = plan::detail(store, &c.project, slug)?;
    Ok(Outcome::Done(json!({
        "message": format!("Review file for {slug} downloaded. Nothing was uploaded."),
        "download": {"filename": format!("{slug}-review.md"), "content": markdown(&detail)},
    })))
}

fn markdown(detail: &PlanDetail) -> String {
    let top: Vec<&Feedback> = detail
        .feedback
        .iter()
        .filter(|item| item.parent_id.is_none())
        .collect();
    let open = top
        .iter()
        .filter(|item| item.state.as_deref() == Some("open"))
        .count();
    let mut out = format!(
        "# Review of plan {}\n\nState: {}.\nRevision: {}.\nNotes: {} open of {}.\nTimes are UTC.\n\n",
        detail.plan.slug,
        detail.plan.state,
        detail.plan.revision,
        open,
        top.len()
    );
    out.push_str("## Notes\n\n");
    if top.is_empty() {
        out.push_str("No notes yet.\n\n");
    }
    for item in top {
        let state = item.state.as_deref().unwrap_or("open");
        let stale = if item.anchored {
            ""
        } else {
            " (anchor no longer exists)"
        };
        out.push_str(&format!(
            "### `{}` - {state}{stale}\n\n{} wrote at {}:\n\n{}\n\n",
            item.anchor,
            item.author,
            timefmt::full_time_secs(item.ts, 0),
            quote(&item.text)
        ));
        for reply in detail
            .feedback
            .iter()
            .filter(|reply| reply.parent_id == Some(item.id))
        {
            out.push_str(&format!(
                "Reply from {}:\n\n{}\n\n",
                reply.author,
                quote(&reply.text)
            ));
        }
    }
    let ticks = detail
        .content
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(ticks.max(2) + 1);
    out.push_str(&format!(
        "## Plan text\n\n{fence}markdown\n{}\n{fence}\n",
        detail.content.trim_end()
    ));
    out
}

fn quote(text: &str) -> String {
    text.lines()
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A hover "+" that opens the note form for `anchor`; `label` names the block for screen readers.
pub(crate) fn button(anchor: &str, label: &str) -> String {
    format!(
        r#"<button type="button" class="note-add" data-note-anchor="{}" data-note-label="{}" aria-label="Add a note on {}">+</button>"#,
        escape(anchor),
        escape(label),
        escape(label)
    )
}

/// Attributes that make a block annotatable.
pub(crate) fn attrs(anchor: &str) -> String {
    format!(r#" data-anchor="{}""#, escape(anchor))
}

/// Count of open notes on `anchor`, as a small badge; empty when there are none.
pub(crate) fn count(items: &[Feedback], anchor: &str) -> String {
    let open = items
        .iter()
        .filter(|item| {
            item.parent_id.is_none()
                && item.anchor == anchor
                && item.state.as_deref() == Some("open")
        })
        .count();
    if open == 0 {
        String::new()
    } else {
        format!(r#" <span class="note-count" title="{open} open note(s) here">{open}</span>"#)
    }
}

/// Give each rendered Markdown heading its `heading:<slug>` anchor and a "+" button. Headings are
/// matched by order, so any mismatch between the renderer and the anchor scanner (setext headings,
/// headings inside quotes) skips decoration entirely instead of attaching notes to the wrong block.
pub(crate) fn decorate_headings(html: &str, content: &str, items: &[Feedback]) -> String {
    let anchors = plan::heading_anchors(content).unwrap_or_default();
    let mut openers: Vec<(usize, char)> = (b'1'..=b'6')
        .flat_map(|level| {
            let tag = format!("<h{}>", level as char);
            html.match_indices(&tag)
                .map(|(at, _)| (at, level as char))
                .collect::<Vec<_>>()
        })
        .collect();
    openers.sort_unstable();
    if anchors.is_empty() || openers.len() != anchors.len() {
        return html.to_owned();
    }
    let mut out = String::with_capacity(html.len() + anchors.len() * 160);
    let mut last = 0;
    for ((at, level), anchor) in openers.into_iter().zip(&anchors) {
        let close = format!("</h{level}>");
        let Some(end) = html[at..].find(&close).map(|offset| at + offset) else {
            return html.to_owned();
        };
        out.push_str(&html[last..at]);
        out.push_str(&format!("<h{level}{}>", attrs(anchor)));
        out.push_str(&html[at + 4..end]);
        out.push_str(&count(items, anchor));
        out.push_str(&button(
            anchor,
            &format!("heading {}", anchor.trim_start_matches("heading:")),
        ));
        out.push_str(&close);
        last = end + close.len();
    }
    out.push_str(&html[last..]);
    out
}

/// The notes sidebar of a plan page, with the export button.
pub(crate) fn sidebar(slug: &str, items: &[Feedback]) -> String {
    let top: Vec<&Feedback> = items
        .iter()
        .filter(|item| item.parent_id.is_none())
        .collect();
    let open = top
        .iter()
        .filter(|item| item.state.as_deref() == Some("open"))
        .count();
    let mut list = String::new();
    for item in &top {
        let state = item.state.as_deref().unwrap_or("open");
        list.push_str(&format!(
            r#"<li><code>{}</code> <span class="badge {}">{}</span> <span class="muted">{}</span><br>{}</li>"#,
            escape(&item.anchor),
            if state == "open" { "waiting" } else { "done" },
            escape(state),
            escape(&item.author),
            excerpt(&item.text)
        ));
    }
    if list.is_empty() {
        list.push_str(r#"<li class="muted">No notes yet.</li>"#);
    }
    format!(
        r#"<aside class="panel notes-sidebar" aria-labelledby="notes-title"><div class="list-caption"><h3 id="notes-title">Notes - {open} open of {}</h3><button class="quiet" type="button" data-note-export="{}">Export for review</button></div><ul>{list}</ul><p class="muted">Hover a block and press + to add a note. The agent reads notes with <code>yoke plan status</code>. Export saves one Markdown file on this computer; nothing is uploaded.</p></aside>"#,
        top.len(),
        escape(slug)
    )
}
