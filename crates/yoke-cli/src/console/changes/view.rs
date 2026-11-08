//! HTML for the Changes page. Everything user- or repository-supplied is escaped; interaction is
//! wired by `changes.js` through `data-*` attributes (the CSP forbids inline script and style).
use super::super::render::{self, encode, escape, excerpt};
use super::notes;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::Path;
use yoke_core::git::{Branch, Entry, FileDiff, Kind, Line, LineKind, Side, Stash, Status};
use yoke_core::plan::{self, Feedback, PlanRecord};

/// Everything the diff panel needs to offer and show line notes.
pub(super) struct NoteContext<'a> {
    pub plan: Option<&'a PlanRecord>,
    pub items: &'a [Feedback],
    /// The path relative to the plan's project, when this file may be annotated.
    pub path: Option<String>,
}

pub(super) fn kind_letter(kind: Kind) -> &'static str {
    match kind {
        Kind::Modified => "M",
        Kind::Added => "A",
        Kind::Deleted => "D",
        Kind::Renamed => "R",
        Kind::Copied => "C",
        Kind::TypeChanged => "T",
    }
}

/// The side to show for `file`: the one asked for if it exists, else the first with changes.
pub(super) fn select(
    status: &Status,
    file: Option<&str>,
    side: Option<&str>,
) -> Option<(String, Side)> {
    let has = |entry: &Entry, side: Side| match side {
        Side::Staged => entry.staged.is_some(),
        Side::Unstaged => entry.unstaged.is_some() || entry.conflicted,
        Side::Untracked => entry.untracked,
    };
    let order = [Side::Unstaged, Side::Staged, Side::Untracked];
    if let Some(file) = file {
        let entry = status.entries.iter().find(|entry| entry.path == file)?;
        let wanted = side.and_then(Side::parse).filter(|side| has(entry, *side));
        let side = wanted.or_else(|| order.into_iter().find(|side| has(entry, *side)))?;
        return Some((entry.path.clone(), side));
    }
    order.into_iter().find_map(|side| {
        status
            .entries
            .iter()
            .find(|entry| has(entry, side))
            .map(|entry| (entry.path.clone(), side))
    })
}

fn params(value: serde_json::Value) -> String {
    escape(&value.to_string())
}

pub(super) fn bar(status: &Status, branches: &[Branch]) -> String {
    let branch = match (&status.branch, &status.head) {
        (Some(name), _) => format!(r#"<span class="badge done">{}</span>"#, escape(name)),
        (None, Some(head)) => format!(
            r#"<span class="badge waiting">detached HEAD at {}</span>"#,
            escape(head)
        ),
        (None, None) => r#"<span class="badge waiting">no commits yet</span>"#.to_owned(),
    };
    let tracking = match &status.upstream {
        Some(upstream) => format!(
            r#"tracks <code>{}</code> &middot; {} ahead, {} behind"#,
            escape(upstream),
            status.ahead,
            status.behind
        ),
        None => "no upstream".to_owned(),
    };
    let mut out = format!(
        r#"<section class="panel changes-bar" aria-label="Branch and remote"><div class="changes-branch">{branch} <span class="muted">{tracking}</span></div>"#
    );
    if let Some(operation) = status.operation {
        out.push_str(&format!(
            r#"<p class="error" role="alert">A {operation} is unfinished in this repository. Finish or abort it in a terminal (<code>git {operation} --abort</code>); switching, creating and pulling are blocked until then.</p>"#
        ));
    }
    let push_label = if status.upstream.is_some() {
        "Push"
    } else {
        "Push and set upstream"
    };
    let push = match (&status.branch, &status.head) {
        (Some(branch), Some(head)) => format!(
            r#"<button type="button" data-changes-act="changes.push" data-params="{}">{push_label}</button>"#,
            params(json!({"branch": branch, "head": head}))
        ),
        _ => r#"<button type="button" disabled title="Switch to a branch with a commit first">Push</button>"#.to_owned(),
    };
    out.push_str(&format!(
        r##"<div class="actions changes-remote"><button type="button" data-changes-act="changes.fetch">Fetch all</button><label class="sr-only" for="pull-strategy">Pull strategy</label><select id="pull-strategy"><option value="ff-only">Fast-forward only</option><option value="merge">Merge</option><option value="rebase">Rebase</option></select><button type="button" data-changes-act="changes.pull" data-from="{}">Pull</button>{push}<button type="button" class="quiet" data-refresh>Refresh</button></div>"##,
        params(json!({"strategy": "#pull-strategy"}))
    ));
    let option = |branch: &Branch| {
        format!(
            r#"<option value="{}"{}>{}{}</option>"#,
            escape(&branch.name),
            if branch.current { " selected" } else { "" },
            escape(&branch.name),
            if branch.track.is_empty() {
                String::new()
            } else {
                format!(" ({})", escape(&branch.track))
            }
        )
    };
    let local: String = branches.iter().filter(|b| !b.remote).map(option).collect();
    let remote: String = branches.iter().filter(|b| b.remote).map(option).collect();
    out.push_str(&format!(
        r##"<div class="actions changes-branches"><label for="branch-select">Branch</label><select id="branch-select"><optgroup label="Local">{local}</optgroup><optgroup label="Remote (creates a tracking branch)">{remote}</optgroup></select><button type="button" data-changes-act="changes.switch" data-from="{}">Switch</button><form class="inline" data-changes-form="changes.create-branch"><label class="sr-only" for="new-branch">New branch name</label><input id="new-branch" name="name" placeholder="new-branch-name" autocomplete="off" maxlength="200"><button type="submit">Create and switch</button></form></div></section>"##,
        params(json!({"name": "#branch-select"}))
    ));
    out
}

struct Row<'a> {
    entry: &'a Entry,
    side: Side,
    kind: Option<Kind>,
}

impl Row<'_> {
    fn label(&self) -> String {
        match (&self.entry.orig_path, self.kind) {
            (Some(orig), Some(Kind::Renamed | Kind::Copied)) => {
                format!("{orig} -> {}", self.entry.path)
            }
            _ => self.entry.path.clone(),
        }
    }

    fn render(
        &self,
        selected: Option<&(String, Side)>,
        plan_query: &str,
        show_dir: bool,
    ) -> String {
        let is_selected =
            selected.is_some_and(|(path, side)| *path == self.entry.path && *side == self.side);
        let (letter, class, title) = match (self.entry.conflicted, self.kind) {
            (true, _) => ("U", "conflicted", "conflicted"),
            (_, Some(kind)) => (
                kind_letter(kind),
                kind.label().split(' ').next().unwrap_or(""),
                kind.label(),
            ),
            (_, None) => ("?", "untracked", "untracked"),
        };
        let name = self.label();
        let shown = if show_dir {
            escape(&name)
        } else {
            escape(name.rsplit('/').next().unwrap_or(&name))
        };
        let (act, label, sign) = match self.side {
            Side::Staged => ("changes.unstage", "Unstage", "-"),
            _ => ("changes.stage", "Stage", "+"),
        };
        let href = render::href(
            "changes",
            &format!(
                "file={}&amp;side={}{plan_query}",
                encode(&self.entry.path),
                self.side.as_str()
            ),
        );
        format!(
            r#"<li class="file{}"><a data-nav data-row href="{href}"{} title="{}"><span class="kind kind-{class}" title="{title}">{letter}</span><span class="path">{shown}</span></a><button type="button" class="quiet icon" data-changes-act="changes.{}" data-params="{}" aria-label="{label} {}">{sign}</button></li>"#,
            if is_selected { " selected" } else { "" },
            if is_selected {
                r#" aria-current="true""#
            } else {
                ""
            },
            escape(&name),
            &act["changes.".len()..],
            params(json!({"paths": [&self.entry.path]})),
            escape(&self.entry.path),
        )
    }
}

fn tree(
    rows: &[&Row<'_>],
    prefix: &str,
    selected: Option<&(String, Side)>,
    plan_query: &str,
) -> String {
    let mut dirs: BTreeMap<&str, Vec<&Row<'_>>> = BTreeMap::new();
    let mut files = String::new();
    for row in rows {
        match row.entry.path[prefix.len()..].split_once('/') {
            Some((dir, _)) => dirs.entry(dir).or_default().push(row),
            None => files.push_str(&row.render(selected, plan_query, false)),
        }
    }
    let mut out = String::new();
    for (dir, children) in dirs {
        out.push_str(&format!(
            r#"<li class="dir"><details open><summary>{}/</summary><ul>{}</ul></details></li>"#,
            escape(dir),
            tree(&children, &format!("{prefix}{dir}/"), selected, plan_query)
        ));
    }
    out + &files
}

pub(super) fn files(
    status: &Status,
    selected: Option<&(String, Side)>,
    plan_query: &str,
) -> String {
    let mut out = String::from(
        r#"<section class="panel changes-files" aria-label="Changed files"><div class="changes-tools"><div class="seg" role="group" aria-label="File list layout"><button type="button" class="quiet" data-changes-layout="flat" aria-pressed="true">Flat</button><button type="button" class="quiet" data-changes-layout="tree" aria-pressed="false">Tree</button></div>"#,
    );
    let unstaged = status
        .entries
        .iter()
        .any(|e| e.unstaged.is_some() || e.untracked || e.conflicted);
    if unstaged {
        out.push_str(r#"<button type="button" class="quiet" data-changes-act="changes.stage-all">Stage all</button>"#);
    }
    out.push_str("</div>");
    if status.truncated {
        out.push_str(r#"<p class="muted" role="status">Only the first 5000 changed paths are listed. Commit or ignore generated files to see the rest.</p>"#);
    }
    let groups: [(&str, Side, Vec<Row<'_>>); 3] = [
        (
            "Staged",
            Side::Staged,
            status
                .entries
                .iter()
                .filter(|e| e.staged.is_some())
                .map(|entry| Row {
                    entry,
                    side: Side::Staged,
                    kind: entry.staged,
                })
                .collect(),
        ),
        (
            "Changes",
            Side::Unstaged,
            status
                .entries
                .iter()
                .filter(|e| e.unstaged.is_some() || e.conflicted)
                .map(|entry| Row {
                    entry,
                    side: Side::Unstaged,
                    kind: entry.unstaged,
                })
                .collect(),
        ),
        (
            "Untracked",
            Side::Untracked,
            status
                .entries
                .iter()
                .filter(|e| e.untracked)
                .map(|entry| Row {
                    entry,
                    side: Side::Untracked,
                    kind: None,
                })
                .collect(),
        ),
    ];
    for (title, side, rows) in &groups {
        if rows.is_empty() {
            continue;
        }
        let all = if *side == Side::Staged {
            r#" <button type="button" class="quiet" data-changes-act="changes.unstage-all">Unstage all</button>"#
        } else {
            ""
        };
        let capped: Vec<&Row<'_>> = rows.iter().take(500).collect();
        let flat: String = capped
            .iter()
            .map(|row| row.render(selected, plan_query, true))
            .collect();
        out.push_str(&format!(
            r#"<div class="file-group"><h3>{title} <span class="count">{}</span>{all}</h3><ul class="file-list flat">{flat}</ul><ul class="file-list tree">{}</ul>{}</div>"#,
            rows.len(),
            tree(&capped, "", selected, plan_query),
            if rows.len() > capped.len() { format!(r#"<p class="muted">Showing the first 500 of {}.</p>"#, rows.len()) } else { String::new() }
        ));
    }
    if groups.iter().all(|(_, _, rows)| rows.is_empty()) {
        out.push_str(&render::empty(
            "Working tree is clean",
            "Nothing is staged, modified or untracked. Edit files and return here to review them.",
            "git status",
        ));
    }
    out.push_str("</section>");
    out
}

fn text_cell(line: Option<&Line>) -> String {
    match line {
        Some(line) => format!("<code>{}</code>", escape(&line.text)),
        None => String::new(),
    }
}

fn note_rows(context: &NoteContext<'_>, line: Option<&Line>, columns: usize) -> String {
    let (Some(path), Some(number)) = (context.path.as_deref(), line.and_then(|l| l.new)) else {
        return String::new();
    };
    let anchor = format!("file:{path}:{number}");
    let here: Vec<&Feedback> = context
        .items
        .iter()
        .filter(|item| item.parent_id.is_none() && item.anchor == anchor)
        .collect();
    if here.is_empty() {
        return String::new();
    }
    let mut cell = String::new();
    for item in here {
        let state = item.state.as_deref().unwrap_or("open");
        cell.push_str(&format!(
            r#"<div class="note-inline"><span class="badge {}">{}</span> <strong>{}</strong>: {}"#,
            if state == "open" { "waiting" } else { "done" },
            escape(state),
            escape(&item.author),
            escape(&item.text)
        ));
        for reply in context
            .items
            .iter()
            .filter(|r| r.parent_id == Some(item.id))
        {
            cell.push_str(&format!(
                r#"<div class="muted">Reply from {}: {}</div>"#,
                escape(&reply.author),
                escape(&reply.text)
            ));
        }
        cell.push_str("</div>");
    }
    format!(r#"<tr class="note-row"><td colspan="{columns}">{cell}</td></tr>"#)
}

fn note_button(context: &NoteContext<'_>, line: &Line) -> String {
    let (Some(path), Some(number), true) =
        (context.path.as_deref(), line.new, context.plan.is_some())
    else {
        return String::new();
    };
    match plan::line_anchor(path, number) {
        Ok(anchor) => notes::button(&anchor, &format!("line {number} of {path}")),
        Err(_) => String::new(),
    }
}

fn line_class(kind: LineKind) -> &'static str {
    match kind {
        LineKind::Add => "add",
        LineKind::Remove => "del",
        LineKind::Context => "ctx",
        LineKind::NoNewline => "meta",
    }
}

fn unified(diff: &FileDiff, context: &NoteContext<'_>) -> String {
    let mut out = String::from(r#"<table class="diff-table diff-unified"><tbody>"#);
    for hunk in &diff.hunks {
        out.push_str(&format!(
            r#"<tr class="hunk"><td colspan="4"><code>{}</code></td></tr>"#,
            escape(&hunk.header)
        ));
        for line in &hunk.lines {
            let sign = match line.kind {
                LineKind::Add => "+",
                LineKind::Remove => "-",
                _ => "",
            };
            let number = |n: Option<u32>| n.map(|n| n.to_string()).unwrap_or_default();
            out.push_str(&format!(
                r#"<tr class="line {}"><td class="num old">{}</td><td class="num new">{}{}</td><td class="sign">{sign}</td><td class="text">{}</td></tr>"#,
                line_class(line.kind),
                number(line.old),
                number(line.new),
                note_button(context, line),
                text_cell(Some(line))
            ));
            out.push_str(&note_rows(context, Some(line), 4));
        }
    }
    out.push_str("</tbody></table>");
    out
}

/// Pair removed and added runs side by side; context spans both sides.
fn pairs(lines: &[Line]) -> Vec<(Option<&Line>, Option<&Line>)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        match lines[i].kind {
            LineKind::Context => {
                out.push((Some(&lines[i]), Some(&lines[i])));
                i += 1;
            }
            LineKind::NoNewline => {
                out.push((None, None));
                i += 1;
            }
            _ => {
                let start = i;
                while i < lines.len() && lines[i].kind == LineKind::Remove {
                    i += 1;
                }
                let mid = i;
                while i < lines.len() && lines[i].kind == LineKind::Add {
                    i += 1;
                }
                let (dels, adds) = (&lines[start..mid], &lines[mid..i]);
                for n in 0..dels.len().max(adds.len()) {
                    out.push((dels.get(n), adds.get(n)));
                }
            }
        }
    }
    out
}

fn split(diff: &FileDiff, context: &NoteContext<'_>) -> String {
    let mut out = String::from(r#"<table class="diff-table diff-split"><tbody>"#);
    for hunk in &diff.hunks {
        out.push_str(&format!(
            r#"<tr class="hunk"><td colspan="4"><code>{}</code></td></tr>"#,
            escape(&hunk.header)
        ));
        for (left, right) in pairs(&hunk.lines) {
            let number = |line: Option<&Line>, old: bool| {
                line.and_then(|l| if old { l.old } else { l.new })
                    .map(|n| n.to_string())
                    .unwrap_or_default()
            };
            let class = |line: Option<&Line>| line.map_or("empty", |l| line_class(l.kind));
            out.push_str(&format!(
                r#"<tr class="line"><td class="num old">{}</td><td class="text {}">{}</td><td class="num new">{}{}</td><td class="text {}">{}</td></tr>"#,
                number(left, true),
                class(left),
                text_cell(left),
                number(right, false),
                right.map(|l| note_button(context, l)).unwrap_or_default(),
                class(right),
                text_cell(right),
            ));
            out.push_str(&note_rows(context, right, 4));
        }
    }
    out.push_str("</tbody></table>");
    out
}

pub(super) fn diff_panel(
    selected: Option<&(String, Side)>,
    entry: Option<&Entry>,
    diff: Option<&Result<FileDiff, String>>,
    context: &NoteContext<'_>,
) -> String {
    let mut out = String::from(r#"<section class="panel changes-diff" aria-label="File diff""#);
    if let Some(plan) = context.plan.filter(|_| context.path.is_some()) {
        out.push_str(&format!(r#" data-note-plan="{}""#, escape(&plan.slug)));
    }
    out.push('>');
    let (Some((path, side)), Some(entry)) = (selected, entry) else {
        out.push_str(r#"<p class="muted">Select a changed file to see its diff.</p></section>"#);
        return out;
    };
    let kind = match side {
        Side::Staged => entry.staged,
        Side::Unstaged => entry.unstaged,
        Side::Untracked => None,
    };
    let kind_text = if entry.conflicted && *side == Side::Unstaged {
        "conflicted"
    } else {
        kind.map_or("untracked", Kind::label)
    };
    let (act, label) = match side {
        Side::Staged => ("changes.unstage", "Unstage file"),
        _ => ("changes.stage", "Stage file"),
    };
    out.push_str(&format!(
        r#"<div class="diff-head"><h2><code>{}</code></h2><span class="badge">{} &middot; {}</span>"#,
        escape(path),
        kind_text,
        side.as_str()
    ));
    if let Some(Ok(diff)) = diff {
        out.push_str(&format!(
            r#" <span class="stat add">+{}</span> <span class="stat del">-{}</span>"#,
            diff.additions, diff.deletions
        ));
    }
    out.push_str(&format!(
        r#" <button type="button" data-changes-act="{act}" data-params="{}">{label}</button><div class="seg" role="group" aria-label="Diff layout"><button type="button" class="quiet" data-changes-view="unified" aria-pressed="true">Unified</button><button type="button" class="quiet" data-changes-view="split" aria-pressed="false">Split</button></div><button type="button" class="quiet" data-changes-wrap aria-pressed="false">Wrap lines</button></div>"#,
        params(json!({"paths": [path]}))
    ));
    match diff {
        None => {}
        Some(Err(error)) => out.push_str(&format!(
            r#"<p class="error" role="alert">{}</p>"#,
            escape(error)
        )),
        Some(Ok(diff)) => {
            for note in &diff.notes {
                out.push_str(&format!(r#"<p class="muted">{}</p>"#, escape(note)));
            }
            if let Some(message) = &diff.message {
                out.push_str(&format!(r#"<p class="muted">{}</p>"#, escape(message)));
            } else if diff.binary {
                out.push_str(
                    r#"<p class="muted">Binary file - there is no text diff to show.</p>"#,
                );
            } else if let Some(raw) = &diff.raw {
                out.push_str(&format!(r#"<p class="muted">This is a merge conflict, shown as git prints it. Resolve it in your editor, then stage the file.</p><pre tabindex="0"><code>{}</code></pre>"#, escape(raw)));
            } else if diff.hunks.is_empty() {
                out.push_str(
                    r#"<p class="muted">No text changes (for example a mode change only).</p>"#,
                );
            } else {
                out.push_str(&format!(
                    r#"<div class="diff-scroll" tabindex="0" role="region" aria-label="Diff of {}">{}{}</div>"#,
                    escape(path),
                    unified(diff, context),
                    split(diff, context)
                ));
                if diff.truncated {
                    out.push_str(r#"<p class="muted" role="status">The diff was cut at 10000 lines; open the file in your editor for the rest.</p>"#);
                }
            }
        }
    }
    out.push_str("</section>");
    out
}

/// Where line notes go, and the notes already written for files.
pub(super) fn notes_panel(
    choice: &notes::Choice,
    items: &[Feedback],
    plan_query_of: impl Fn(&str) -> String,
) -> String {
    let mut out = String::from(
        r#"<section class="panel changes-notes" aria-labelledby="line-notes-title"><h3 id="line-notes-title">Line notes</h3>"#,
    );
    let Some(plan) = choice.plan() else {
        out.push_str(r#"<p class="muted">There is no unfinished plan in this project, so line notes are unavailable. Notes are saved on a plan and the agent reads them with <code>yoke plan status</code>.</p></section>"#);
        return out;
    };
    let options: String = choice
        .plans
        .iter()
        .map(|p| {
            format!(
                r#"<option value="{}"{}>{} ({})</option>"#,
                escape(&p.slug),
                if p.slug == plan.slug { " selected" } else { "" },
                escape(&p.slug),
                escape(&p.state)
            )
        })
        .collect();
    out.push_str(&format!(
        r#"<label for="note-plan">Save notes on plan</label> <select id="note-plan" data-changes-plan>{options}</select><p class="muted">Press + next to a line to write a note. The agent reads notes with <code>yoke plan status</code>.</p>"#
    ));
    let mut list = String::new();
    for item in items.iter().filter(|i| i.parent_id.is_none()) {
        let Some((path, _)) = plan::parse_line_anchor(&item.anchor) else {
            continue;
        };
        let state = item.state.as_deref().unwrap_or("open");
        list.push_str(&format!(
            r#"<li><a data-nav href="{}"><code>{}</code></a> <span class="badge {}">{}</span> {}</li>"#,
            render::href("changes", &format!("file={}&amp;side=unstaged{}", encode(path), plan_query_of(&plan.slug))),
            escape(&item.anchor),
            if state == "open" { "waiting" } else { "done" },
            escape(state),
            excerpt(&item.text)
        ));
    }
    if !list.is_empty() {
        out.push_str(&format!("<ul>{list}</ul>"));
    }
    out.push_str("</section>");
    out
}

pub(super) fn commit_panel(status: &Status, agents: &[String]) -> String {
    let staged = status.entries.iter().filter(|e| e.staged.is_some()).count();
    let draft = if agents.is_empty() {
        r#"<p class="muted">AI drafting needs the <code>claude</code> or <code>codex</code> CLI on PATH. Nothing is ever sent unless you ask.</p>"#.to_owned()
    } else {
        let buttons: String = agents
            .iter()
            .map(|agent| format!(r#"<button type="button" class="quiet" data-changes-draft="{0}"{1}>Draft message with {0}</button>"#, escape(agent), if staged == 0 { " disabled" } else { "" }))
            .collect();
        format!(
            r#"<div class="actions">{buttons}</div><div class="draft-preview" hidden></div><p class="muted">Drafting is off until you click it. You will see exactly which staged files are sent, then confirm.</p>"#
        )
    };
    format!(
        r#"<section class="panel changes-commit" aria-labelledby="commit-title"><h3 id="commit-title">Commit</h3><form data-changes-form="changes.commit"><label for="commit-subject">Message</label><input id="commit-subject" name="subject" maxlength="500" autocomplete="off" placeholder="Summarize the change"><label for="commit-body">Extended description</label><textarea id="commit-body" name="body" rows="4" placeholder="Why this change is needed (optional)"></textarea><div class="actions"><button type="submit"{}>Commit {staged} staged file(s)</button></div></form>{draft}</section>"#,
        if staged == 0 { " disabled" } else { "" }
    )
}

pub(super) fn stash_panel(stashes: &[Stash]) -> String {
    let mut out = String::from(
        r#"<section class="panel changes-stash" aria-labelledby="stash-title"><h3 id="stash-title">Stashes</h3><form data-changes-form="changes.stash-save" class="inline"><label class="sr-only" for="stash-message">Stash message</label><input id="stash-message" name="message" placeholder="Message (optional)" maxlength="200" autocomplete="off"><label><input type="checkbox" name="untracked"> Include untracked files</label><button type="submit">Stash changes</button></form>"#,
    );
    if stashes.is_empty() {
        out.push_str(r#"<p class="muted">No stashes.</p>"#);
    }
    for stash in stashes {
        let p = params(json!({"id": stash.id}));
        out.push_str(&format!(
            r#"<div class="stash"><span>{} <time data-time="{}">{}</time></span><span class="actions"><button type="button" class="quiet" data-changes-act="changes.stash-apply" data-params="{p}">Apply</button><button type="button" class="quiet" data-changes-act="changes.stash-pop" data-params="{p}">Pop</button><button type="button" class="quiet" data-changes-act="changes.stash-drop" data-params="{p}">Drop</button></span></div>"#,
            escape(&stash.message),
            stash.ts,
            stash.ts
        ));
    }
    out.push_str("</section>");
    out
}

/// The path relative to the plan's project, when a note on it can be anchored.
pub(super) fn project_path(repo_root: &Path, project: &Path, path: &str) -> Option<String> {
    let full = repo_root.join(path);
    full.strip_prefix(project).ok()?.to_str().map(str::to_owned)
}
