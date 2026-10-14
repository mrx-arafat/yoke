//! Cross-project oversight. Clients select a project by an opaque id that must match a
//! project already recorded in the database; a client-supplied path is never used.

use super::{
    render::{self, escape},
    BadRequest, Console,
};
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use yoke_core::{config, store::Store};

const DAY: i64 = 86_400;

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

pub(super) struct Scope {
    pub id: String,
    pub path: String,
    pub launch: bool,
    /// Every selectable project, launch project first, then most recently active.
    pub choices: Vec<String>,
}

impl Scope {
    /// Query suffix that keeps this scope on links; empty for the launch project.
    pub fn query(&self) -> String {
        if self.launch {
            String::new()
        } else {
            format!("&amp;project={}", self.id)
        }
    }
}

pub(super) fn project_id(path: &str) -> String {
    let digest = Sha256::digest(path.as_bytes());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Resolve the `project` query parameter; absent or empty means the launch project.
pub(super) fn scope(
    store: &Store,
    c: &Console,
    params: &BTreeMap<String, String>,
) -> Result<Scope> {
    let launch = c.project.to_string_lossy().into_owned();
    let mut choices = vec![launch.clone()];
    for row in store.projects(now() - DAY, 1000)? {
        if row.project != launch {
            choices.push(row.project);
        }
    }
    let wanted = params.get("project").filter(|v| !v.is_empty());
    let path = match wanted {
        None => launch.clone(),
        Some(id) => match choices.iter().find(|path| project_id(path) == *id) {
            Some(path) => path.clone(),
            None => {
                return Err(BadRequest(
                    "Unknown project. Choose one from the Projects page.".into(),
                )
                .into())
            }
        },
    };
    Ok(Scope {
        id: project_id(&path),
        launch: path == launch,
        path,
        choices,
    })
}

fn name(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
}

/// A `<select>` for filter forms; the option values are opaque project ids.
/// Options show the folder name; the full path is added only when two names collide.
pub(super) fn picker(scope: &Scope) -> String {
    let mut options = String::new();
    for (index, path) in scope.choices.iter().enumerate() {
        let id = project_id(path);
        let shared = scope
            .choices
            .iter()
            .filter(|other| name(other) == name(path))
            .count()
            > 1;
        options.push_str(&format!(
            "<option value=\"{}\"{} title=\"{}\">{}{}{}</option>",
            if index == 0 { "" } else { id.as_str() },
            if id == scope.id { " selected" } else { "" },
            escape(path),
            escape(name(path)),
            if index == 0 { " (launch project)" } else { "" },
            if shared {
                format!(" - {}", escape(path))
            } else {
                String::new()
            }
        ));
    }
    format!("<label>Project<select name=\"project\">{options}</select></label>")
}

/// Shown on scoped pages so a non-launch selection is never mistaken for the launch project.
pub(super) fn banner(c: &Console, scope: &Scope, page: &str) -> String {
    if scope.launch {
        return String::new();
    }
    format!(
        r#"<p class="scope">Viewing project <code>{}</code> &middot; <a data-nav href="{}">Back to launch project</a> &middot; <a data-nav href="{}">All projects</a></p>"#,
        escape(&scope.path),
        render::link(c, page),
        render::link(c, "projects")
    )
}

/// Current protection of a project: configured mode and trust of each config layer.
/// Reads only the fixed config file names under a recorded project root, never their content.
pub(super) fn protection(c: &Console, store: &Store, path: &str) -> String {
    let loaded = match config::load_trusted(store, Path::new(path), &c.home) {
        Ok(loaded) => loaded,
        Err(error) => {
            return format!(
                "Unavailable: {}",
                escape(&yoke_core::redact::redact(&format!("{error:#}")))
            )
        }
    };
    let mut parts = vec![format!(
        "Configured mode <strong>{}</strong>",
        loaded.config.mode.as_str()
    )];
    for status in loaded.snapshots.iter().filter(|s| {
        s.file
            .file_name()
            .is_some_and(|name| name == "yoke.toml" || name == "config.toml")
    }) {
        let state = match (status.trusted, status.stale) {
            (true, false) => "trusted",
            (true, true) => "trusted snapshot in use; disk file changed or missing",
            (false, _) if std::fs::symlink_metadata(&status.file).is_ok() => {
                "untrusted on disk, ignored"
            }
            (false, _) => "none",
        };
        parts.push(format!(
            "{} config: {state}",
            if status.scope == "global" {
                "Global"
            } else {
                "Project"
            }
        ));
    }
    parts.join(" &middot; ")
}

/// Copyable resume command for a recorded session. Yoke shows it and never runs it.
pub(super) fn resume_command(agent: &str, project: &str, session: &str) -> Option<String> {
    let id_ok = !session.is_empty()
        && session.len() <= 128
        && session
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b));
    if !id_ok || project.is_empty() || project.chars().any(char::is_control) {
        return None;
    }
    let quote = |value: &str| format!("'{}'", value.replace('\'', r"'\''"));
    let tool = match agent {
        "claude" => "claude --resume",
        "codex" => "codex resume",
        _ => return None,
    };
    Some(format!(
        "cd {} && {tool} {}",
        quote(project),
        quote(session)
    ))
}

pub(super) fn overview(store: &Store, c: &Console) -> Result<String> {
    let launch = c.project.to_string_lossy().into_owned();
    let mut rows = store.projects(now() - DAY, 1000)?;
    if let Some(index) = rows.iter().position(|row| row.project == launch) {
        let row = rows.remove(index);
        rows.insert(0, row);
    }
    let mut out = render::heading(
        "projects",
        "See every project this Yoke home has recorded, then open its sessions, decisions or usage.",
    );
    out.push_str("<p class=\"muted\">Projects come from recorded sessions, decisions and plans. Plans, memory and trust always show the launch project.</p>");
    if rows.is_empty() {
        out.push_str(&render::empty(
            "No projects recorded yet",
            "Start Claude or Codex with Yoke hooks; each project appears after its first recorded event.",
            "yoke install --dry-run",
        ));
    }
    for row in rows {
        let id = project_id(&row.project);
        let launch_row = row.project == launch;
        let scoped = |page: &str| {
            if launch_row {
                render::link(c, page)
            } else {
                render::href(page, &format!("project={id}"))
            }
        };
        out.push_str(&format!(
            r#"<article class="panel project-card"><div class="list-caption"><h2><a data-row data-nav href="{}">{}</a></h2>{}</div><p class="muted project-path">{}</p><dl class="facts oversight"><dt>Last activity</dt><dd>{}</dd><dt>Sessions</dt><dd>{} recorded &middot; {} without an end event</dd><dt>Denials (24 h)</dt><dd>{}</dd><dt>Open plans</dt><dd>{}</dd><dt>Protection now</dt><dd>{}</dd></dl><p class="project-links"><a data-nav href="{}">Sessions</a> <a data-nav href="{}">Decisions</a> <a data-nav href="{}">Usage</a></p></article>"#,
            scoped("sessions"),
            escape(name(&row.project)),
            if launch_row { "<span class=\"badge\">Launch project</span>" } else { "" },
            escape(&row.project),
            row.last_activity
                .map(|v| format!("<time data-time=\"{v}\">{v}</time>"))
                .unwrap_or_else(|| "No activity recorded".into()),
            row.sessions,
            row.open_sessions,
            row.recent_denials,
            row.open_plans,
            protection(c, store, &row.project),
            scoped("sessions"),
            scoped("decisions"),
            scoped("usage"),
        ));
    }
    out.push_str("<p class=\"muted\">A session without an end event is not proof that its agent is still running. Protection shows the trusted configuration now, not what applied in the past.</p></main>");
    Ok(out)
}
