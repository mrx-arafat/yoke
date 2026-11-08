//! Team remote section of the Extensions page. Opt-in: nothing here talks to a remote until the
//! human presses Connect, Pull or Push, and a push lists the exact files and the remote first.
use super::super::{
    actions::Outcome,
    render::{self, encode, escape},
};
use super::{agent_label, category, text, ROW_LIMIT};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};
use yoke_core::{
    extension_files as files,
    extensions::{Kind, Report, Roots, Scope},
    redact::redact,
    team_remote::{self as team, ChangeKind, FileCompare, FileState, Remote, RowKind, SharePlan},
};

/// The push the human last reviewed; confirming pushes exactly this, or nothing.
static REVIEWED: Mutex<Option<SharePlan>> = Mutex::new(None);
const SCOPES: [Scope; 2] = [Scope::User, Scope::Project];

fn with_query(base: &str, extra: &str) -> String {
    if base.is_empty() {
        extra.to_owned()
    } else {
        format!("{base}&amp;{extra}")
    }
}

pub(super) fn panel(
    roots: &Roots,
    state_dir: &Path,
    report: &Report,
    params: &BTreeMap<String, String>,
    base_query: &str,
) -> String {
    let mut out = String::from(
        r#"<section class="panel ext-remote" aria-labelledby="ext-remote"><h2 id="ext-remote">Team remote</h2>"#,
    );
    match team::load(state_dir) {
        Err(error) => out.push_str(&format!(
            r#"<p role="alert">{}</p>"#,
            escape(&format!("{error:#}"))
        )),
        Ok(None) => out.push_str(CONNECT_FORM),
        Ok(Some(state)) => {
            let pulled = fs::metadata(team::clone_dir(state_dir).join(".git/FETCH_HEAD"))
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|age| format!(r#"<time data-time="{0}">{0}</time>"#, age.as_secs()))
                .unwrap_or_else(|| "not yet".into());
            out.push_str(&format!(
                r#"<dl class="facts"><dt>Address</dt><dd><code>{}</code></dd><dt>Branch</dt><dd>{}</dd><dt>Last pull</dt><dd>{pulled}</dd></dl><p class="muted">Pull fetches the remote into Yoke's own clone; it changes none of your extensions. Installing and pushing each ask you to confirm the exact files first.</p><div class="actions"><button type="button" data-ext-remote-pull>Pull from remote</button><button type="button" class="quiet" data-ext-remote-disconnect>Disconnect</button></div>"#,
                escape(&redact(&state.url)),
                escape(state.branch.as_deref().unwrap_or("main, master or the only branch")),
            ));
            out.push_str(&remote_list(roots, state_dir, params, base_query));
            out.push_str(&share_form(roots, report, &redact(&state.url)));
        }
    }
    out.push_str("</section>");
    out
}

const CONNECT_FORM: &str = r#"<p class="muted">Share skills, rules, commands and agents through a Git repository you choose. This is off until you connect. Connecting runs <code>git fetch</code> against the address and sends nothing. Yoke keeps its own clone under <code>~/.yoke/team-remote</code> and the address in <code>~/.yoke/team-remote.json</code>, outside your trusted policy. It never stores a password or token and never asks for one: sign in once in a terminal with an SSH key or a Git credential helper.</p><form class="filters ext-remote-form" data-ext-remote-connect><label class="grow">Repository address<input name="url" required maxlength="2048" placeholder="git@github.com:your-team/extensions.git" autocomplete="off" spellcheck="false"></label><label>Branch (optional)<input name="branch" maxlength="255" placeholder="main" autocomplete="off" spellcheck="false"></label><button type="submit">Connect</button></form>"#;

fn local_status(roots: &Roots, remote: &Remote, scope: Scope) -> &'static str {
    let Some(unit) = files::unit_path(roots, remote.agent, remote.kind, scope, &remote.name) else {
        return "not available";
    };
    if fs::symlink_metadata(&unit).is_err() {
        return "not installed";
    }
    match team::compare(remote, Some(&unit)) {
        Ok(list) if list.iter().all(|file| file.state == FileState::Same) => "same",
        Ok(_) => "differs",
        Err(_) => "unreadable",
    }
}

fn remote_list(
    roots: &Roots,
    state_dir: &Path,
    params: &BTreeMap<String, String>,
    base_query: &str,
) -> String {
    let listing = match team::list(state_dir) {
        Ok(listing) => listing,
        Err(error) => return format!(r#"<p role="alert">{}</p>"#, escape(&format!("{error:#}"))),
    };
    let mut out = String::new();
    if let Some(id) = params.get("remote").filter(|id| !id.is_empty()) {
        out.push_str(&match listing.items.iter().find(|item| &item.id() == id) {
            Some(remote) => compare_view(roots, remote, base_query),
            None => r#"<p role="alert">That extension is no longer on the remote. Pull again.</p>"#
                .into(),
        });
    }
    if listing.items.is_empty() {
        out.push_str(r#"<p class="muted">Nothing pulled yet, or the remote holds no extensions. Press Pull; the first push creates the layout.</p>"#);
    } else {
        out.push_str(r#"<h3>On the remote</h3><div class="table-scroll" tabindex="0" role="region" aria-label="Extensions on the team remote"><table><thead><tr><th scope="col">Name</th><th scope="col">Category</th><th scope="col">User copy</th><th scope="col">Project copy</th><th scope="col">Actions</th></tr></thead><tbody>"#);
        for remote in listing.items.iter().take(ROW_LIMIT) {
            let id = remote.id();
            let mut buttons = format!(
                r#"<a data-nav href="{}">Compare</a>"#,
                render::href(
                    "extensions",
                    &with_query(base_query, &format!("remote={}", encode(&id)))
                )
            );
            let mut cells = String::new();
            for scope in SCOPES {
                let status = local_status(roots, remote, scope);
                cells.push_str(&format!("<td>{status}</td>"));
                if status != "not available" {
                    buttons.push_str(&format!(
                        r#" <button type="button" class="quiet" data-ext-install="{}" data-ext-scope="{}">Install to {}</button>"#,
                        escape(&id),
                        files::scope_label(scope),
                        files::scope_label(scope)
                    ));
                }
            }
            out.push_str(&format!(
                r#"<tr><th scope="row">{}<br><span class="muted">{}</span></th><td>{} / {}</td>{cells}<td class="ext-row-actions">{buttons}</td></tr>"#,
                escape(&remote.name),
                escape(&id),
                category(remote.kind),
                agent_label(remote.agent),
            ));
        }
        out.push_str("</tbody></table></div>");
    }
    if !listing.skipped.is_empty() {
        out.push_str(r#"<details><summary>Left out of the list</summary><ul>"#);
        for line in &listing.skipped {
            out.push_str(&format!("<li>{}</li>", escape(line)));
        }
        out.push_str("</ul></details>");
    }
    out
}

fn compare_view(roots: &Roots, remote: &Remote, base_query: &str) -> String {
    let mut out = format!(
        r#"<div class="ext-compare"><h3>Compare {}</h3><p><a data-nav href="{}">Close comparison</a></p>"#,
        escape(&remote.id()),
        render::href("extensions", base_query)
    );
    let mut installed = false;
    for scope in SCOPES {
        let Some(unit) = files::unit_path(roots, remote.agent, remote.kind, scope, &remote.name)
            .filter(|unit| fs::symlink_metadata(unit).is_ok())
        else {
            continue;
        };
        installed = true;
        out.push_str(&format!(
            r#"<h4>Your {} copy against the remote</h4><p class="muted"><code>{}</code></p>"#,
            files::scope_label(scope),
            escape(&redact(&unit.display().to_string()))
        ));
        out.push_str(&file_blocks(team::compare(remote, Some(&unit))));
    }
    if !installed {
        out.push_str(r#"<h4>Not installed here; the remote's content</h4>"#);
        out.push_str(&file_blocks(team::compare(remote, None)));
    }
    out.push_str("</div>");
    out
}

fn file_blocks(compared: Result<Vec<FileCompare>>) -> String {
    let list = match compared {
        Ok(list) => list,
        Err(error) => return format!(r#"<p role="alert">{}</p>"#, escape(&format!("{error:#}"))),
    };
    let mut out = String::new();
    for file in list {
        let state = match file.state {
            FileState::Same => "identical",
            FileState::Differs => "differs",
            FileState::LocalOnly => "only on this machine",
            FileState::RemoteOnly => "only on the remote",
        };
        out.push_str(&format!(
            r#"<h5><code>{}</code> <span class="badge">{state}</span></h5>"#,
            escape(&file.path)
        ));
        if let Some(note) = file.note {
            out.push_str(&format!(r#"<p class="muted">{note}</p>"#));
        }
        match (file.state, &file.local, &file.remote) {
            (FileState::Differs, Some(local), Some(remote)) => {
                out.push_str(&split_table(&file.path, &redact(local), &redact(remote)));
            }
            (FileState::RemoteOnly, _, Some(text)) | (FileState::LocalOnly, Some(text), _) => out
                .push_str(&format!(
                    r#"<pre class="ext-code"><code>{}</code></pre>"#,
                    escape(&redact(text))
                )),
            _ => {}
        }
    }
    out
}

fn split_table(path: &str, local: &str, remote: &str) -> String {
    let Some(rows) = team::side_by_side(local, remote) else {
        return r#"<p class="muted">Too long to compare line by line. Open both files in your editor.</p>"#.into();
    };
    let mut out = format!(
        r#"<div class="ext-diff-scroll" tabindex="0" role="region" aria-label="Side-by-side comparison of {}"><table class="ext-diff"><thead><tr><th scope="col" colspan="2">This machine</th><th scope="col"><span class="sr-only">Change</span></th><th scope="col" colspan="2">Remote</th></tr></thead><tbody>"#,
        escape(path)
    );
    for row in rows.iter().take(1500) {
        let (class, sign, word) = match row.kind {
            RowKind::Same => ("same", "", ""),
            RowKind::Changed => ("changed", "~", "changed"),
            RowKind::Removed => ("removed", "-", "only on this machine"),
            RowKind::Added => ("added", "+", "only on the remote"),
        };
        let side = |cell: &Option<(usize, String)>| match cell {
            Some((number, line)) => format!(
                r#"<td class="num">{number}</td><td class="text"><code>{}</code></td>"#,
                escape(line)
            ),
            None => r#"<td class="num"></td><td class="text empty"></td>"#.into(),
        };
        let marker = if sign.is_empty() {
            String::new()
        } else {
            format!(r#"{sign}<span class="sr-only"> {word}</span>"#)
        };
        out.push_str(&format!(
            r#"<tr class="{class}">{}<td class="sign">{marker}</td>{}</tr>"#,
            side(&row.left),
            side(&row.right)
        ));
    }
    out.push_str("</tbody></table></div>");
    out
}

fn share_form(roots: &Roots, report: &Report, url: &str) -> String {
    let mut rows = String::new();
    for item in &report.items {
        if !matches!(
            item.kind,
            Kind::Skill | Kind::Rule | Kind::Command | Kind::Agent
        ) || !matches!(item.scope, Scope::User | Scope::Project)
        {
            continue;
        }
        let Some(shared) = files::target_for(roots, item)
            .ok()
            .and_then(|target| team::shared_from(&target).ok())
        else {
            continue;
        };
        rows.push_str(&format!(
            r#"<li><label><input type="checkbox" name="share" value="{}"> <strong>{}</strong> <span class="muted">{} / {} scope</span></label></li>"#,
            escape(&item.origin.display().to_string()),
            escape(&shared.id()),
            category(item.kind),
            files::scope_label(item.scope),
        ));
    }
    if rows.is_empty() {
        return r#"<h3>Share with the team</h3><p class="muted">No user or project skills, rules, commands or agents can be shared yet.</p>"#.into();
    }
    format!(
        r#"<h3>Share with the team</h3><p class="muted">Pick what to send to <code>{}</code>. Pushing is the only step that sends your files: you see every file and the branch first, Yoke never forces, and a rejected push changes nothing.</p><form class="ext-remote-form" data-ext-share-form><ul class="ext-share">{rows}</ul><div class="actions"><button type="submit">Review and push</button></div></form>"#,
        escape(url)
    )
}

fn now() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

fn describe_push(plan: &SharePlan) -> String {
    let mut message = format!(
        "Send these files to {} on branch {}:\n",
        plan.url, plan.branch
    );
    for change in &plan.changes {
        let verb = match change.kind {
            ChangeKind::Add => "Add",
            ChangeKind::Modify => "Change",
            ChangeKind::Remove => "Remove",
        };
        message.push_str(&format!("{verb} {}\n", change.path));
    }
    message.push_str("Yoke commits them in its own clone and pushes with your existing Git sign-in. It never forces: if the remote moved, the push is rejected and nothing changes. Nothing leaves this machine until you confirm.");
    message
}

pub(super) fn act(
    roots: &Roots,
    state: &Path,
    name: &str,
    params: &Value,
    confirmed: bool,
) -> Result<Outcome> {
    match name {
        "ext.remote-connect" => {
            let branch = params["branch"].as_str().unwrap_or_default();
            let synced = team::connect(state, text(params, "url")?, Some(branch))?;
            Ok(Outcome::Done(
                json!({ "message": pulled_message(state, &synced) }),
            ))
        }
        "ext.remote-pull" => {
            let synced = team::pull(state)?;
            Ok(Outcome::Done(
                json!({ "message": pulled_message(state, &synced) }),
            ))
        }
        "ext.remote-disconnect" => {
            if !confirmed {
                return Ok(Outcome::Confirm(format!(
                    "Disconnect from the team remote.\nYoke forgets the address and deletes its own clone in {}.\nExtensions you installed stay where they are, and nothing is sent anywhere.",
                    team::clone_dir(state).display()
                )));
            }
            team::disconnect(state)?;
            Ok(Outcome::Done(
                json!({ "message": "Disconnected from the team remote" }),
            ))
        }
        "ext.remote-install" => install(roots, state, params, confirmed),
        "ext.remote-push" => push(roots, state, params, confirmed),
        _ => bail!("Unknown action"),
    }
}

fn pulled_message(state: &Path, synced: &team::Synced) -> String {
    if synced.empty {
        return format!(
            "Connected. The remote has no commits yet; the first push creates branch {}",
            synced.branch
        );
    }
    let count = team::list(state).map_or(0, |listing| listing.items.len());
    format!(
        "Pulled branch {}: {count} extension(s) on the remote",
        synced.branch
    )
}

fn install(roots: &Roots, state: &Path, params: &Value, confirmed: bool) -> Result<Outcome> {
    let id = text(params, "id")?;
    let scope = files::parse_scope(text(params, "scope")?).context("Pick user or project")?;
    let listing = team::list(state)?;
    let remote = listing
        .items
        .iter()
        .find(|item| item.id() == id)
        .context("That extension is no longer on the remote; pull again")?;
    let plan = team::plan_install(roots, remote, scope)?;
    if !confirmed {
        let mut message = format!(
            "Install {} from the team remote into the {} scope.\nFrom: {}\nTo: {}\nFiles: {}\n",
            remote.id(),
            files::scope_label(scope),
            plan.from.display(),
            plan.to.display(),
            plan.files.join(", ")
        );
        if plan.replaces {
            message.push_str("A copy already exists there. It moves into the Yoke trash (~/.yoke/extension-trash) first; nothing is erased.\n");
        }
        message.push_str("Skills can contain scripts: use Compare to read them before installing.");
        return Ok(Outcome::Confirm(message));
    }
    let trashed = team::apply_install(&plan, state, now()?)?;
    let note = trashed.map_or(String::new(), |trash| {
        format!("; the old copy is in {}", trash.display())
    });
    Ok(Outcome::Done(
        json!({ "message": format!("Installed {} to {}{note}", remote.id(), plan.to.display()) }),
    ))
}

fn push(roots: &Roots, state: &Path, params: &Value, confirmed: bool) -> Result<Outcome> {
    let origins = params["origins"]
        .as_array()
        .context("Pick at least one extension")?;
    ensure!(
        (1..=50).contains(&origins.len()),
        "Pick between 1 and 50 extensions to share"
    );
    let mut items = Vec::new();
    for origin in origins {
        let origin = origin.as_str().context("Bad selection")?;
        items.push(team::shared_from(&files::find(roots, Path::new(origin))?)?);
    }
    let mut reviewed = REVIEWED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !confirmed {
        team::pull(state)?;
        let plan = team::plan_push(state, &items)?;
        let message = describe_push(&plan);
        *reviewed = Some(plan);
        return Ok(Outcome::Confirm(message));
    }
    let plan = reviewed
        .take()
        .context("Review the list again before pushing")?;
    let message = team::push(state, &items, &plan)?;
    Ok(Outcome::Done(json!({ "message": message })))
}
