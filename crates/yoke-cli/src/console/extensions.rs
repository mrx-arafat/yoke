//! Extensions: browse and manage Claude Code and Codex extensions.
//! Everything `yoke extensions` inventories is listed; skills, commands, agents and rules also
//! open to their content and can be renamed, moved between user and project scope, or deleted
//! into a trash folder. Each change names the exact paths and waits for a confirmation ticket.
use super::{
    actions::Outcome,
    content,
    render::{self, encode, escape},
    Console,
};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
#[path = "extensions/remote.rs"]
mod remote;

use yoke_core::{
    extension_files::{self as files, Format, Target},
    extensions::{self, Item, Kind, Owner, Roots},
    redact::redact,
    store::Store,
};

/// Rows drawn at once; the filters narrow a longer inventory.
const ROW_LIMIT: usize = 300;
const CATEGORIES: [&str; 9] = [
    "skill", "rule", "command", "agent", "plugin", "mcp", "hook", "memory", "preset",
];
const AGENTS: [&str; 3] = ["claude", "codex", "yoke"];
const SCOPES: [&str; 5] = ["user", "project", "local", "managed", "system"];

fn category(kind: Kind) -> &'static str {
    match kind {
        Kind::Skill => "skill",
        Kind::Rule => "rule",
        Kind::Command => "command",
        Kind::Agent => "agent",
        Kind::Plugin => "plugin",
        Kind::Mcp => "mcp",
        Kind::Hook => "hook",
        Kind::Memory => "memory",
        Kind::Preset => "preset",
    }
}

fn agent_label(agent: Owner) -> &'static str {
    match agent {
        Owner::Claude => "claude",
        Owner::Codex => "codex",
        Owner::Yoke => "yoke",
    }
}

#[derive(Default)]
struct Filter {
    q: String,
    category: String,
    agent: String,
    scope: String,
}

impl Filter {
    fn from(params: &BTreeMap<String, String>) -> Self {
        let get = |name: &str| {
            params
                .get(name)
                .map(|v| v.trim().to_lowercase())
                .unwrap_or_default()
        };
        Self {
            q: get("q"),
            category: get("category"),
            agent: get("agent"),
            scope: get("scope"),
        }
    }

    fn matches(&self, item: &Item) -> bool {
        let same = |wanted: &str, actual: &str| wanted.is_empty() || wanted == actual;
        same(&self.category, category(item.kind))
            && same(&self.agent, agent_label(item.agent))
            && same(&self.scope, files::scope_label(item.scope))
            && (self.q.is_empty()
                || [&item.name, &item.detail, &item.origin.display().to_string()]
                    .iter()
                    .any(|text| text.to_lowercase().contains(&self.q)))
    }

    /// `&amp;`-separated query for links; `item` keeps a selection.
    fn query(&self, item: Option<&Path>) -> String {
        let mut pairs = Vec::new();
        for (name, value) in [
            ("q", self.q.as_str()),
            ("category", &self.category),
            ("agent", &self.agent),
            ("scope", &self.scope),
        ] {
            if !value.is_empty() {
                pairs.push(format!("{name}={}", encode(value)));
            }
        }
        if let Some(item) = item {
            pairs.push(format!("item={}", encode(&item.display().to_string())));
        }
        pairs.join("&amp;")
    }
}

fn select(name: &str, label: &str, current: &str, values: &[&str]) -> String {
    let mut options = String::from(r#"<option value="">All</option>"#);
    for value in values {
        let selected = if *value == current { " selected" } else { "" };
        options.push_str(&format!(
            r#"<option value="{value}"{selected}>{value}</option>"#
        ));
    }
    format!(r#"<label>{label}<select name="{name}">{options}</select></label>"#)
}

pub(super) fn page(
    _store: &Store,
    c: &Console,
    params: &BTreeMap<String, String>,
) -> Result<String> {
    render_page(c, &Roots::detect(&c.home, &c.project), params)
}

fn render_page(_c: &Console, roots: &Roots, params: &BTreeMap<String, String>) -> Result<String> {
    let report = extensions::scan(roots);
    let filter = Filter::from(params);
    let mut out = render::heading(
        "extensions",
        "Skills, rules, commands and agents for Claude Code and Codex, in your user folders and this project.",
    );
    let shown: Vec<&Item> = report
        .items
        .iter()
        .filter(|item| filter.matches(item))
        .collect();
    let selected = params.get("item").filter(|item| !item.is_empty());
    out.push_str(&format!(
        r#"<section class="panel" aria-labelledby="ext-find"><h2 id="ext-find">Find extensions</h2><form class="filters" data-filter action="extensions" method="get" role="search"><label class="grow">Search<input type="search" name="q" value="{}" placeholder="Name, detail or path"></label>{}{}{}<button type="submit">Apply</button><a data-nav class="button quiet" href="extensions">Reset</a></form><p class="muted">{} of {} found. Rename, move and delete need a confirmation that names the exact paths. Deleted items go to <code>~/.yoke/extension-trash</code>, never erased.</p></section>"#,
        escape(&filter.q),
        select("category", "Category", &filter.category, &CATEGORIES),
        select("agent", "Agent", &filter.agent, &AGENTS),
        select("scope", "Scope", &filter.scope, &SCOPES),
        shown.len(),
        report.items.len(),
    ));
    if let Some(origin) = selected {
        out.push_str(&detail(roots, Path::new(origin), &filter));
    }
    out.push_str(&list(&shown, &filter, selected.map(Path::new)));
    out.push_str(&remote::panel(
        roots,
        &roots.home.join(".yoke"),
        &report,
        params,
        &filter.query(None),
    ));
    out.push_str(&drift(&report));
    out.push_str("</main>");
    Ok(out)
}

fn shown_path(path: &Path) -> String {
    escape(&redact(&path.display().to_string()))
}

fn list(items: &[&Item], filter: &Filter, selected: Option<&Path>) -> String {
    if items.is_empty() {
        return render::empty(
            "No extensions match",
            "Nothing in the inventory fits these filters. Skills live in ~/.claude/skills, rules in ~/.claude/rules, commands in ~/.claude/commands.",
            "yoke extensions",
        );
    }
    let mut out = String::from(
        r#"<section class="panel" aria-labelledby="ext-list"><h2 id="ext-list">Inventory</h2><div class="table-scroll" tabindex="0" role="region" aria-labelledby="ext-list"><table><thead><tr><th scope="col">Name</th><th scope="col">Category</th><th scope="col">Agent</th><th scope="col">Scope</th><th scope="col">Detail</th></tr></thead><tbody>"#,
    );
    for item in items.iter().take(ROW_LIMIT) {
        let name = if files::is_file_kind(item.kind) {
            let current = if selected == Some(item.origin.as_path()) {
                r#" aria-current="true""#
            } else {
                ""
            };
            format!(
                r#"<a data-nav href="{}"{current}>{}</a>"#,
                render::href("extensions", &filter.query(Some(&item.origin))),
                escape(&item.name)
            )
        } else {
            escape(&item.name)
        };
        let mut detail = escape(&item.detail);
        if !item.keys.is_empty() {
            detail.push_str(&format!(
                " <span class=\"muted\">keys: {}</span>",
                escape(&item.keys.join(", "))
            ));
        }
        out.push_str(&format!(
            r#"<tr><th scope="row">{name}{}<br><span class="muted ext-path">{}</span></th><td>{}</td><td>{}</td><td>{}</td><td>{detail}</td></tr>"#,
            if item.managed { r#" <span class="badge">managed by Yoke</span>"# } else { "" },
            shown_path(&item.origin),
            category(item.kind),
            agent_label(item.agent),
            files::scope_label(item.scope),
        ));
    }
    out.push_str("</tbody></table></div>");
    if items.len() > ROW_LIMIT {
        out.push_str(&format!(
            r#"<p class="muted">Showing the first {ROW_LIMIT} of {}. Narrow the filters to see the rest.</p>"#,
            items.len()
        ));
    }
    out.push_str("</section>");
    out
}

fn drift(report: &extensions::Report) -> String {
    if report.drift.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        r#"<section class="panel" aria-labelledby="ext-drift"><h2 id="ext-drift">Findings</h2><p class="muted">Same-name definitions, stale managed hooks and missing programs. <code>yoke extensions</code> prints the same list.</p><ul>"#,
    );
    for finding in &report.drift {
        out.push_str(&format!(
            "<li><span class=\"badge\">{:?}</span> <strong>{}</strong> - {}</li>",
            finding.kind,
            escape(&finding.name),
            escape(&finding.detail)
        ));
    }
    out.push_str("</ul></section>");
    out
}

/// Content and actions for one selected extension file.
fn detail(roots: &Roots, origin: &Path, filter: &Filter) -> String {
    let close = render::href("extensions", &filter.query(None));
    let target = match files::find(roots, origin) {
        Ok(target) => target,
        Err(error) => {
            return format!(
                r#"<section class="panel" role="alert"><h2>Extension not available</h2><p>{}</p><p><a data-nav href="{close}">Back to the list</a></p></section>"#,
                escape(&format!("{error:#}"))
            )
        }
    };
    let body = match files::read(&target) {
        Ok(content) => format!(
            r#"<dl class="facts"><dt>Characters</dt><dd>{}</dd><dt>Lines</dt><dd>{}</dd><dt>Tokens</dt><dd>about {} (one per four characters)</dd></dl>{}{}"#,
            content.chars,
            content.lines,
            content.tokens,
            if content.truncated {
                r#"<p class="muted">Only the first 512 KiB is shown and counted.</p>"#
            } else {
                ""
            },
            render_content(&content.text, content.format),
        ),
        Err(error) => format!(r#"<p role="alert">{}</p>"#, escape(&format!("{error:#}"))),
    };
    format!(
        r#"<section class="panel ext-detail" aria-labelledby="ext-detail"><h2 id="ext-detail">{}</h2><p class="muted"><span class="badge">{}</span> {} / {} <code>{}</code></p>{}{body}<p><a data-nav href="{close}">Close</a></p></section>"#,
        escape(&target.name),
        category(target.kind),
        agent_label(target.agent),
        files::scope_label(target.scope),
        shown_path(&target.file),
        actions(roots, &target),
    )
}

fn actions(roots: &Roots, target: &Target) -> String {
    if !target.manageable() {
        return r#"<p class="muted">Read-only here: managed, local, system and memory files are changed where they live.</p>"#.into();
    }
    let stem = target
        .unit
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let origin = escape(&target.file.display().to_string());
    let mut out = format!(
        r#"<div class="ext-actions" data-ext-origin="{origin}"><form class="inline" data-ext-rename><label>New name<input name="name" value="{}" maxlength="64" required autocomplete="off" spellcheck="false"></label><button type="submit">Rename</button></form>"#,
        escape(&stem)
    );
    if let Some(other) = target.other_scope(roots) {
        let label = files::scope_label(other);
        out.push_str(&format!(
            r#"<button type="button" class="quiet" data-ext-move="{label}">Move to {label} scope</button>"#
        ));
    }
    out.push_str(r#"<button type="button" class="danger-button" data-ext-delete>Delete (to trash)</button></div>"#);
    out
}

fn render_content(text: &str, format: Format) -> String {
    match format {
        Format::Markdown => {
            let (front, body) = split_frontmatter(text);
            let front = front
                .map(|front| {
                    format!(
                        r#"<pre class="ext-code"><code>{}</code></pre>"#,
                        escape(front)
                    )
                })
                .unwrap_or_default();
            format!(
                r#"{front}<div class="prose ext-prose">{}</div>"#,
                content::markdown(body)
            )
        }
        _ => format!(
            r#"<pre class="ext-code"><code>{}</code></pre>"#,
            escape(text)
        ),
    }
}

/// Leading `---` block of a Markdown file, and what follows it.
fn split_frontmatter(text: &str) -> (Option<&str>, &str) {
    let Some(rest) = text.strip_prefix("---\n") else {
        return (None, text);
    };
    match rest.find("\n---") {
        Some(end) => {
            let after = &rest[end + 4..];
            (
                Some(&text[..end + 8]),
                after.strip_prefix('\n').unwrap_or(after),
            )
        }
        None => (None, text),
    }
}

/// Handle a delegated console action. Return `Outcome::Confirm(message)` for a step that needs
/// the human to confirm; the action is then re-run with `confirmed = true` after they do.
pub(super) fn act(
    _store: &Store,
    c: &Console,
    name: &str,
    params: &Value,
    confirmed: bool,
) -> Result<Outcome> {
    let roots = Roots::detect(&c.home, &c.project);
    let state = c.home.join(".yoke");
    if name.starts_with("ext.remote-") {
        return remote::act(&roots, &state, name, params, confirmed);
    }
    file_action(&roots, &state, name, params, confirmed)
}

fn text<'a>(params: &'a Value, key: &str) -> Result<&'a str> {
    params[key]
        .as_str()
        .filter(|value| !value.is_empty())
        .with_context(|| format!("Missing {key}"))
}

fn file_action(
    roots: &Roots,
    state: &Path,
    name: &str,
    params: &Value,
    confirmed: bool,
) -> Result<Outcome> {
    if !matches!(name, "ext.rename" | "ext.move" | "ext.delete") {
        bail!("Unknown action");
    }
    let target = files::find(roots, Path::new(text(params, "origin")?))?;
    let change = match name {
        "ext.rename" => files::plan_rename(&target, text(params, "name")?)?,
        "ext.move" => {
            let scope =
                files::parse_scope(text(params, "scope")?).context("Pick user or project")?;
            files::plan_move(roots, &target, scope)?
        }
        _ => {
            let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
            files::plan_delete(&target, state, stamp)?
        }
    };
    let what = format!("{} '{}'", category(target.kind), target.name);
    if !confirmed {
        let (from, to) = (change.from.display(), change.to.display());
        return Ok(Outcome::Confirm(match change.op {
            files::Op::Rename => format!(
                "Rename the {what} on disk.\nFrom: {from}\nTo: {to}\nNothing is overwritten. Text inside the file, such as a skill's own name, is not changed."
            ),
            files::Op::Move => format!(
                "Move the {what} to the {} scope.\nFrom: {from}\nTo: {to}\nThe original is removed once the copy is in place. Nothing is overwritten.",
                text(params, "scope")?
            ),
            files::Op::Delete => format!(
                "Delete the {what} by moving it into the Yoke trash.\nFrom: {from}\nTrash folder: {to}\nThe original path is saved beside it so you can put it back by hand. Nothing is erased."
            ),
        }));
    }
    files::apply(&change)?;
    let message = match change.op {
        files::Op::Rename => format!("Renamed {what}"),
        files::Op::Move => format!("Moved {what} to {}", change.to.display()),
        files::Op::Delete => format!("Moved {what} to the trash: {}", change.to.display()),
    };
    Ok(Outcome::Done(
        json!({ "message": message, "navigate": "extensions" }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn setup() -> (tempfile::TempDir, Roots, PathBuf, Console, Store) {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let home = base.join("home");
        let roots = Roots {
            project: base.join("project"),
            claude_dir: home.join(".claude"),
            claude_json: home.join(".claude.json"),
            claude_managed: base.join("managed"),
            codex_home: home.join(".codex"),
            codex_system: base.join("codex-system"),
            home: home.clone(),
        };
        fs::create_dir_all(home.join(".yoke")).unwrap();
        fs::create_dir_all(&roots.project).unwrap();
        let store = Store::open(home.join(".yoke/state.db")).unwrap();
        let console = Console {
            home: home.clone(),
            project: roots.project.clone(),
            database: home.join(".yoke/state.db"),
            token: "token".into(),
            csrf: "csrf".into(),
            host: "127.0.0.1:1".into(),
        };
        (temp, roots, home.join(".yoke"), console, store)
    }

    fn query(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect()
    }

    #[test]
    fn page_filters_and_shows_content_with_statistics() {
        let (_temp, roots, _state, console, _store) = setup();
        let command = roots.claude_dir.join("commands/ship.md");
        write(
            &command,
            "---\ndescription: ship it\n---\n# Ship\nRun <script>alert(1)</script>\n",
        );
        write(
            &roots.project.join(".claude/agents/helper.md"),
            "---\nname: helper\n---\n",
        );
        let all = render_page(&console, &roots, &query(&[])).unwrap();
        assert!(all.contains("ship") && all.contains("helper"));
        let only = render_page(
            &console,
            &roots,
            &query(&[("category", "agent"), ("q", "HELP")]),
        )
        .unwrap();
        assert!(
            only.contains("helper") && !only.contains(">ship<"),
            "filters narrow the list"
        );

        let item = command.display().to_string();
        let html = render_page(&console, &roots, &query(&[("item", &item)])).unwrap();
        assert!(html.contains("<dt>Characters</dt><dd>66</dd>"), "{html}");
        assert!(html.contains("<dt>Lines</dt><dd>5</dd>"));
        assert!(
            html.contains("&lt;script&gt;") && !html.contains("<script>alert"),
            "content stays inert"
        );
        assert!(html.contains("data-ext-rename") && html.contains("data-ext-move=\"project\""));
    }

    #[test]
    fn team_remote_pushes_only_what_was_reviewed_and_the_page_lists_it() {
        let (temp, roots, state, console, _store) = setup();
        let bare = temp.path().canonicalize().unwrap().join("team.git");
        let git = |dir: &Path, args: &[&str]| {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        };
        fs::create_dir_all(&bare).unwrap();
        git(&bare, &["init", "--bare", "--quiet"]);
        write(&roots.claude_dir.join("commands/ship.md"), "ship\n");
        let url = bare.display().to_string();

        let connect = json!({ "url": url });
        let Outcome::Done(done) =
            remote::act(&roots, &state, "ext.remote-connect", &connect, false).unwrap()
        else {
            panic!("connecting needs no confirmation");
        };
        assert!(done["message"].as_str().unwrap().contains("no commits yet"));
        for (key, value) in [
            ("user.name", "Test"),
            ("user.email", "t@example.com"),
            ("commit.gpgsign", "false"),
        ] {
            git(&state.join("team-remote/repo"), &["config", key, value]);
        }

        let origin = roots
            .claude_dir
            .join("commands/ship.md")
            .display()
            .to_string();
        let push = json!({ "origins": [origin] });
        let Outcome::Confirm(message) =
            remote::act(&roots, &state, "ext.remote-push", &push, false).unwrap()
        else {
            panic!("a push must be reviewed first");
        };
        assert!(
            message.contains("Add commands/ship.md") && message.contains(&url),
            "{message}"
        );
        let Outcome::Done(done) =
            remote::act(&roots, &state, "ext.remote-push", &push, true).unwrap()
        else {
            panic!("a reviewed push goes out");
        };
        assert!(done["message"]
            .as_str()
            .unwrap()
            .starts_with("Pushed 1 file(s)"));
        assert!(
            remote::act(&roots, &state, "ext.remote-push", &push, true).is_err(),
            "a review is used once"
        );

        let html = render_page(
            &console,
            &roots,
            &query(&[("remote", "claude/command/ship")]),
        )
        .unwrap();
        assert!(html.contains("Team remote") && html.contains("data-ext-remote-disconnect"));
        assert!(
            html.contains("claude/command/ship")
                && html.contains("Your user copy against the remote"),
            "{html}"
        );
    }

    #[test]
    fn rename_waits_for_confirmation_and_names_both_paths() {
        let (_temp, roots, state, _console, _store) = setup();
        let command = roots.claude_dir.join("commands/ship.md");
        write(&command, "body\n");
        let params = json!({ "origin": command.display().to_string(), "name": "release" });
        let Outcome::Confirm(message) =
            file_action(&roots, &state, "ext.rename", &params, false).unwrap()
        else {
            panic!("a rename must ask first");
        };
        assert!(message.contains(&command.display().to_string()));
        assert!(message.contains(
            &roots
                .claude_dir
                .join("commands/release.md")
                .display()
                .to_string()
        ));
        assert!(command.exists(), "nothing changed before the confirmation");
        let Outcome::Done(done) = file_action(&roots, &state, "ext.rename", &params, true).unwrap()
        else {
            panic!("a confirmed rename finishes");
        };
        assert!(done["message"].as_str().unwrap().starts_with("Renamed"));
        assert!(!command.exists() && roots.claude_dir.join("commands/release.md").exists());
    }
}
