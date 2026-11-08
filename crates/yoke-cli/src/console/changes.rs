//! Changes: Git status, diffs, staging, commits, branches and stashes, plus annotations.
//! Owner: parity-changes (Pilot parity fleet).
//!
//! The browser is the human's. Local, reversible Git actions run at once; pushing and dropping a
//! stash ask for confirmation; sending the staged diff to a reviewer CLI needs two explicit clicks
//! and never runs on its own. Nothing here forces, and policy files are never touched.
use super::{actions::Outcome, render, Console};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use yoke_core::{
    config,
    git::{PullStrategy, Repo, Side},
    plan,
    review::Reviewer,
    store::Store,
};

#[path = "changes/notes.rs"]
pub(crate) mod notes;
#[cfg(test)]
#[path = "changes/tests.rs"]
mod tests;
#[path = "changes/view.rs"]
mod view;

pub(super) fn page(
    store: &Store,
    c: &Console,
    params: &BTreeMap<String, String>,
) -> Result<String> {
    let mut out = render::heading(
        "changes",
        "Review and ship the working tree: diffs, staging, commits, branches and stashes.",
    );
    let repo = match Repo::open(&c.project) {
        Ok(repo) => repo,
        Err(error) => {
            out.push_str(&render::empty(
                "This project is not a Git repository",
                &format!("{error:#}"),
                "git init",
            ));
            out.push_str("</main>");
            return Ok(out);
        }
    };
    let status = repo.status()?;
    let branches = repo.branches()?;
    let stashes = repo.stashes()?;
    let selected = view::select(
        &status,
        params.get("file").map(String::as_str),
        params.get("side").map(String::as_str),
    );
    let diff = selected.as_ref().map(|(path, side)| {
        repo.file_diff(path, *side)
            .map_err(|error| format!("{error:#}"))
    });
    let entry = selected
        .as_ref()
        .and_then(|(path, _)| status.entries.iter().find(|entry| entry.path == *path));

    let choice = notes::choose(store, c, params.get("plan").map(String::as_str))?;
    let items = match choice.plan() {
        Some(plan) => plan::detail(store, &c.project, &plan.slug)?.feedback,
        None => Vec::new(),
    };
    // Staged line numbers describe the index; they match the working tree only when nothing
    // else is unstaged, so annotation is offered only then.
    let annotatable = |side: Side, entry: &yoke_core::git::Entry| {
        side != Side::Staged || (entry.unstaged.is_none() && !entry.conflicted)
    };
    let path = match (&selected, entry) {
        (Some((path, side)), Some(entry)) if annotatable(*side, entry) => {
            view::project_path(repo.root(), &c.project, path)
        }
        _ => None,
    };
    let context = view::NoteContext {
        plan: choice.plan(),
        items: &items,
        path,
    };
    let plan_query = choice
        .plan()
        .map(|plan| format!("&amp;plan={}", render::encode(&plan.slug)))
        .unwrap_or_default();

    out.push_str(&view::bar(&status, &branches));
    out.push_str(r#"<div class="changes-layout" data-layout="flat" data-diff-view="unified" data-wrap="off"><div class="changes-side">"#);
    out.push_str(&view::files(&status, selected.as_ref(), &plan_query));
    out.push_str(&view::commit_panel(
        &status,
        &agents(store, c)
            .iter()
            .map(|(a, _)| a.clone())
            .collect::<Vec<_>>(),
    ));
    out.push_str(&view::stash_panel(&stashes));
    out.push_str(r#"</div><div class="changes-main">"#);
    out.push_str(&view::diff_panel(
        selected.as_ref(),
        entry,
        diff.as_ref(),
        &context,
    ));
    out.push_str(&view::notes_panel(&choice, &items, |slug| {
        format!("&amp;plan={}", render::encode(slug))
    }));
    out.push_str("</div></div></main>");
    Ok(out)
}

/// The reviewer CLIs the human may send a staged diff to: the configured one, or both when the
/// configuration says `auto` (no building agent is known here), if installed.
fn agents(store: &Store, c: &Console) -> Vec<(String, Option<String>)> {
    let Ok(loaded) = config::load_trusted(store, &c.project, &c.home) else {
        return Vec::new();
    };
    let review = &loaded.config.review;
    let names: &[&str] = match review.reviewer.as_str() {
        "claude" => &["claude"],
        "codex" => &["codex"],
        _ => &["claude", "codex"],
    };
    names
        .iter()
        .filter(|name| yoke_core::process::executable(name).is_ok())
        .map(|name| {
            (
                (*name).to_owned(),
                review.models.get(name).map(str::to_owned),
            )
        })
        .collect()
}

fn reviewer(store: &Store, c: &Console, agent: &str) -> Result<(Reviewer, Option<String>)> {
    let (_, model) = agents(store, c)
        .into_iter()
        .find(|(name, _)| name == agent)
        .with_context(|| format!("{agent} is not available as a reviewer here; check review.reviewer in the trusted configuration and that the CLI is on PATH"))?;
    // An explicit name never conflicts with the (unknown) building agent.
    Ok((Reviewer::select(agent, "")?, model))
}

pub(super) fn str_param<'a>(params: &'a Value, key: &str) -> Result<&'a str> {
    params
        .get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("Missing {key}"))
}

fn optional_str<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
}

fn paths_param(params: &Value) -> Result<Vec<String>> {
    params
        .get("paths")
        .and_then(Value::as_array)
        .context("Missing paths")?
        .iter()
        .map(|path| {
            path.as_str()
                .map(str::to_owned)
                .context("A path must be text")
        })
        .collect()
}

fn done(message: impl Into<String>) -> Result<Outcome> {
    Ok(Outcome::Done(json!({"message": message.into()})))
}

/// First line of git's own words, for the toast.
fn first_line(output: &str) -> String {
    output
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_owned()
}

pub(super) fn act(
    store: &Store,
    c: &Console,
    name: &str,
    params: &Value,
    confirmed: bool,
) -> Result<Outcome> {
    match name {
        "note.add" => return notes::add(store, c, params),
        "note.export" => return notes::export(store, c, params),
        _ => {}
    }
    let repo = Repo::open(&c.project)?;
    match name {
        "changes.stage" => {
            repo.stage(&paths_param(params)?)?;
            done("Staged.")
        }
        "changes.unstage" => {
            repo.unstage(&paths_param(params)?)?;
            done("Unstaged.")
        }
        "changes.stage-all" => {
            repo.stage_all()?;
            done("Everything is staged.")
        }
        "changes.unstage-all" => {
            repo.unstage_all()?;
            done("Everything is unstaged.")
        }
        "changes.commit" => {
            let body = params.get("body").and_then(Value::as_str).unwrap_or("");
            let committed = repo.commit(str_param(params, "subject")?, body)?;
            done(format!(
                "Committed {}: {}",
                committed.hash, committed.subject
            ))
        }
        "changes.switch" => {
            let target = str_param(params, "name")?;
            repo.switch(target)?;
            done(format!("Switched to {target}."))
        }
        "changes.create-branch" => {
            let branch = str_param(params, "name")?;
            repo.create_branch(branch, optional_str(params, "start"))?;
            done(format!("Created and switched to {branch}."))
        }
        "changes.fetch" => {
            repo.fetch()?;
            done("Fetched all remotes.")
        }
        "changes.pull" => {
            let strategy = PullStrategy::parse(str_param(params, "strategy")?)
                .context("Choose fast-forward only, merge or rebase")?;
            let output = repo.pull(strategy)?;
            done(format!("Pull finished. {}", first_line(&output)))
        }
        "changes.push" => {
            let plan = repo.push_plan()?;
            ensure!(
                plan.branch == str_param(params, "branch")?
                    && plan.head == str_param(params, "head")?,
                "The branch changed since this page loaded; refresh and review the push again"
            );
            if !confirmed {
                return Ok(Outcome::Confirm(describe_push(&plan)));
            }
            repo.push(&plan)?;
            done(format!("Pushed {} to {}.", plan.branch, plan.remote))
        }
        "changes.stash-save" => {
            let message = params.get("message").and_then(Value::as_str).unwrap_or("");
            let untracked = params
                .get("untracked")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            done(repo.stash_save(message, untracked)?)
        }
        "changes.stash-apply" => {
            repo.stash_apply(str_param(params, "id")?)?;
            done("Stash applied; it is still in the list.")
        }
        "changes.stash-pop" => {
            repo.stash_pop(str_param(params, "id")?)?;
            done("Stash applied and removed from the list.")
        }
        "changes.stash-drop" => {
            let id = str_param(params, "id")?;
            let stash = repo
                .stashes()?
                .into_iter()
                .find(|stash| stash.id == id)
                .context("That stash no longer exists; refresh the Changes page")?;
            if !confirmed {
                return Ok(Outcome::Confirm(format!(
                    "Drop the stash \"{}\"? Its saved changes are deleted and cannot be restored from this console.",
                    stash.message
                )));
            }
            repo.stash_drop(id)?;
            done("Stash dropped.")
        }
        "changes.draft-preview" => {
            let agent = str_param(params, "agent")?;
            reviewer(store, c, agent)?;
            let staged = repo.staged_for_message()?;
            Ok(Outcome::Done(json!({"preview": {
                "agent": agent,
                "files": staged.files,
                "skipped": staged.skipped,
                "bytes": staged.bytes,
                "digest": staged.digest,
            }})))
        }
        "changes.draft-message" => {
            let agent = str_param(params, "agent")?;
            let (who, model) = reviewer(store, c, agent)?;
            let staged = repo.staged_for_message()?;
            ensure!(
                staged.digest == str_param(params, "digest")?,
                "The staged changes differ from what you were shown; draft again to see the new list"
            );
            let draft = repo.draft_commit_message(who, model.as_deref(), &staged)?;
            Ok(Outcome::Done(json!({
                "message": "Draft ready. Read and edit it before committing.",
                "subject": draft.subject,
                "body": draft.body,
            })))
        }
        _ => bail!("Unknown action"),
    }
}

fn describe_push(plan: &yoke_core::git::PushPlan) -> String {
    let mut text = format!(
        "Push {} commit(s) from {} to {} ({}){}. This sends them to the remote; a push cannot be undone from this console and is never forced.",
        plan.ahead,
        plan.branch,
        plan.remote,
        plan.url,
        if plan.set_upstream { " and set it as the upstream" } else { "" }
    );
    for commit in &plan.commits {
        text.push_str("\n- ");
        text.push_str(commit);
    }
    if plan.ahead as usize > plan.commits.len() {
        text.push_str("\n- and more");
    }
    text
}
