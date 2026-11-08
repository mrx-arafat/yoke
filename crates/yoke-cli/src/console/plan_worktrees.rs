//! Plan worktrees on the Plans page: status, diff, sync (squash merge) and discard.
//! Owner: parity-plans. Loaded as a child of `dashboard`.
//!
//! A plan "lives in" a linked worktree when its recorded project is that worktree's path. The
//! browser names a worktree only by an opaque id that must match one `git worktree list` reports.
use super::super::{
    actions::Outcome,
    projects,
    render::{self, escape},
    Console,
};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use std::path::Path;
use yoke_core::{
    plan::{self, PlanRecord},
    store::Store,
    worktree::{self, Report, Worktree},
};

/// Diffs longer than this are cut at a line boundary on screen.
const DIFF_DISPLAY_LIMIT: usize = 300_000;

struct Row {
    worktree: Worktree,
    plans: Vec<PlanRecord>,
}

fn id(path: &Path) -> String {
    projects::project_id(&path.to_string_lossy())
}

/// Linked worktrees of the console project's repository that hold at least one plan.
fn rows(store: &Store, c: &Console) -> Result<Vec<Row>> {
    let mut rows = Vec::new();
    for item in worktree::list(&c.project)? {
        if item.is_main {
            continue;
        }
        let plans = plan::list(store, &item.path).unwrap_or_default();
        if !plans.is_empty() {
            rows.push(Row {
                worktree: item,
                plans,
            });
        }
    }
    Ok(rows)
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(10)]
}

/// The plans' own files are ignored by Git in most projects; they may go with a finished worktree.
fn allowed(row: &Row) -> Vec<String> {
    row.plans
        .iter()
        .filter_map(|p| Path::new(&p.file).strip_prefix(&row.worktree.path).ok())
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

fn plan_badges(plans: &[PlanRecord]) -> String {
    plans
        .iter()
        .map(|p| {
            format!(
                r#"<span class="badge {}">{} &middot; {}</span>"#,
                escape(&p.state),
                escape(&p.slug),
                escape(&p.state)
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Reasons a worktree may not be synced or discarded that come from its plans, not from Git.
fn plan_blockers(plans: &[PlanRecord]) -> (Vec<String>, Vec<String>) {
    let mut both: Vec<String> = plans
        .iter()
        .filter(|p| !matches!(p.state.as_str(), "done" | "cancelled"))
        .map(|p| {
            format!(
                "Plan {} is still {}; finish or cancel it first.",
                p.slug, p.state
            )
        })
        .collect();
    let discard = both.clone();
    if !plans.iter().any(|p| p.state == "done") {
        both.push("No plan in this worktree is done; sync follows a completed plan.".to_owned());
    }
    (both, discard)
}

/// Worktrees with plans, each with a one-line status and a link to review it.
pub(in super::super) fn panel(store: &Store, c: &Console) -> Result<String> {
    // A project that is not a Git repository simply has no worktrees to show.
    let Ok(rows) = rows(store, c) else {
        return Ok(String::new());
    };
    if rows.is_empty() {
        return Ok(String::new());
    }
    let mut out = String::from(
        r#"<section class="panel" aria-label="Plan worktrees"><div class="list-caption"><h2>Plan worktrees</h2></div><p class="muted">Plans whose work lives in a linked Git worktree. Review its changes, squash-merge them into the main checkout's branch, then discard it.</p><ul class="dash-list">"#,
    );
    for row in &rows {
        let summary = match worktree::report(&c.project, &row.worktree.path, &c.project, &[]) {
            Ok(r) => format!(
                "{} ahead of {}, {} behind &middot; {} uncommitted",
                r.ahead,
                escape(r.base.branch.as_deref().unwrap_or("detached HEAD")),
                r.behind,
                r.changes_total
            ),
            Err(error) => format!("Status unavailable: {}", escape(&format!("{error:#}"))),
        };
        out.push_str(&format!(
            r#"<li><a data-row data-nav href="{}">{}</a> {}<br><span class="muted">{} &middot; {}</span></li>"#,
            render::href("plans", &format!("worktree={}", id(&row.worktree.path))),
            escape(row.worktree.branch.as_deref().unwrap_or("detached HEAD")),
            plan_badges(&row.plans),
            escape(&row.worktree.path.to_string_lossy()),
            summary
        ));
    }
    out.push_str("</ul></section>");
    Ok(out)
}

fn diff_html(text: &str) -> String {
    if text.trim().is_empty() {
        return "<p class=\"muted\">No changes.</p>".to_owned();
    }
    let mut out = String::from("<div class=\"diff wt-diff\" data-mode=\"unified\">");
    let mut used = 0;
    for line in text.lines() {
        used += line.len() + 1;
        if used > DIFF_DISPLAY_LIMIT {
            out.push_str("<p class=\"muted\">The diff is cut off here; read the rest with git diff in a terminal.</p>");
            break;
        }
        let class = if ["diff ", "index ", "+++", "---"]
            .iter()
            .any(|p| line.starts_with(p))
        {
            "wt-file"
        } else if line.starts_with("@@") {
            "wt-hunk"
        } else if line.starts_with('+') {
            "diff-add"
        } else if line.starts_with('-') {
            "diff-remove"
        } else {
            ""
        };
        out.push_str(&format!(
            "<span class=\"diff-line {class}\">{}</span>",
            escape(line)
        ));
    }
    out.push_str("</div>");
    out
}

fn blockers_html(blockers: &[String]) -> String {
    let items: String = blockers
        .iter()
        .map(|b| format!("<li>{}</li>", escape(b)))
        .collect();
    format!("<p>Not available yet:</p><ul>{items}</ul>")
}

fn changes_html(report: &Report) -> String {
    if report.changes.is_empty() {
        return "<p class=\"muted\">No uncommitted changes.</p>".to_owned();
    }
    let rows: String = report
        .changes
        .iter()
        .map(|c| {
            format!(
                "<tr><td><code>{}</code></td><td>{}</td></tr>",
                escape(c.code.trim()),
                escape(&c.path)
            )
        })
        .collect();
    let more = if report.changes_total > report.changes.len() {
        format!(
            "<p class=\"muted\">{} more not listed.</p>",
            report.changes_total - report.changes.len()
        )
    } else {
        String::new()
    };
    format!("<div class=\"table-scroll\" tabindex=\"0\" role=\"region\" aria-label=\"Uncommitted changes\"><table><thead><tr><th scope=\"col\">Status</th><th scope=\"col\">Path</th></tr></thead><tbody>{rows}</tbody></table></div>{more}")
}

fn find<'a>(rows: &'a [Row], wanted: &str) -> Result<&'a Row> {
    rows.iter()
        .find(|row| id(&row.worktree.path) == wanted)
        .context("Unknown worktree. Open it from the Plans page.")
}

/// The review page for one worktree.
pub(in super::super) fn detail(store: &Store, c: &Console, wanted: &str) -> Result<String> {
    let rows = rows(store, c)?;
    let row = find(&rows, wanted)?;
    let report = worktree::report(&c.project, &row.worktree.path, &c.project, &allowed(row))?;
    let text = worktree::diff(&c.project, &row.worktree.path)?;
    let (mut sync_blockers, mut discard_blockers) = plan_blockers(&row.plans);
    sync_blockers.extend(report.sync_blockers.iter().cloned());
    discard_blockers.extend(report.discard_blockers.iter().cloned());
    let base = report.base.branch.as_deref().unwrap_or("detached HEAD");
    let branch = report.target.branch.as_deref().unwrap_or("detached HEAD");
    let mut out = format!(
        r#"<p><a data-nav href="{}">All plans</a></p><section class="panel"><div class="list-caption"><h2>{}</h2><span>{}</span></div><dl class="facts oversight"><dt>Folder</dt><dd>{}</dd><dt>Base</dt><dd>{} in {}</dd><dt>Commits</dt><dd>{} ahead, {} behind</dd><dt>HEAD</dt><dd><code>{}</code></dd></dl>"#,
        render::link(c, "plans"),
        escape(branch),
        plan_badges(&row.plans),
        escape(&report.target.path.to_string_lossy()),
        escape(base),
        escape(&report.base.path.to_string_lossy()),
        report.ahead,
        report.behind,
        escape(short(&report.target.head))
    );
    out.push_str(&format!(
        "<h3>Uncommitted changes</h3>{}</section>",
        changes_html(&report)
    ));
    out.push_str(&format!(
        "<section class=\"panel\"><h2>Changes against {}</h2>{}{}",
        escape(base),
        diff_html(&text.committed),
        if text.truncated {
            "<p class=\"muted\">Git output was cut off at 4 MiB.</p>"
        } else {
            ""
        }
    ));
    if !text.uncommitted.trim().is_empty() || !text.untracked.is_empty() {
        out.push_str("<h3>Not committed yet</h3>");
        out.push_str(&diff_html(&text.uncommitted));
        if !text.untracked.is_empty() {
            let names: Vec<String> = text.untracked.iter().map(|n| escape(n)).collect();
            out.push_str(&format!(
                "<p class=\"muted\">Untracked files: {}</p>",
                names.join(", ")
            ));
        }
    }
    out.push_str("</section>");

    let params = json!({"id": id(&row.worktree.path), "head": report.target.head}).to_string();
    let slugs: Vec<&str> = row.plans.iter().map(|p| p.slug.as_str()).collect();
    out.push_str("<section class=\"panel\"><h2>Sync into the main checkout</h2>");
    out.push_str(&format!("<p>Squash-merges {} into {} in the main checkout and commits it there. It does not push, and repository hooks do not run.</p>", escape(branch), escape(base)));
    if sync_blockers.is_empty() {
        out.push_str(&format!(
            r#"<div class="action-zone"><label>Commit message<textarea data-field="message" rows="3" maxlength="4000">{}</textarea></label><div class="actions"><button type="button" data-plans-run="plans.worktree.sync" data-params="{}" data-confirm-label="Squash merge and commit">Review sync</button></div></div>"#,
            escape(&format!("Squash merge {branch}\n\nPlans: {}", slugs.join(", "))),
            escape(&params)
        ));
    } else {
        out.push_str(&blockers_html(&sync_blockers));
    }
    out.push_str("</section><section class=\"panel\"><h2>Discard this worktree</h2>");
    out.push_str(&format!("<p>Removes the folder with Git's own non-forcing removal. It refuses while anything here is uncommitted or not yet in {}. The branch {} is kept.</p>", escape(base), escape(branch)));
    if discard_blockers.is_empty() {
        out.push_str(&format!(
            r#"<div class="action-zone"><div class="actions"><button type="button" class="danger-button" data-plans-run="plans.worktree.discard" data-params="{}" data-after="plans" data-confirm-label="Discard worktree">Review discard</button></div></div>"#,
            escape(&params)
        ));
    } else {
        out.push_str(&blockers_html(&discard_blockers));
    }
    out.push_str("</section></main>");
    Ok(out)
}

fn text<'a>(params: &'a Value, key: &str) -> Result<&'a str> {
    params
        .get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("Missing {key}"))
}

/// `plans.worktree.sync` with `{id, head, message}` and `plans.worktree.discard` with `{id, head}`.
pub(in super::super) fn act(
    store: &Store,
    c: &Console,
    name: &str,
    params: &Value,
    confirmed: bool,
) -> Result<Outcome> {
    let rows = rows(store, c)?;
    let row = find(&rows, text(params, "id")?)?;
    let head = text(params, "head")?;
    let report = worktree::report(&c.project, &row.worktree.path, &c.project, &allowed(row))?;
    ensure!(
        report.target.head == head,
        "The worktree changed since you opened it; reload the page and review it again."
    );
    let (sync_plan, discard_plan) = plan_blockers(&row.plans);
    let base = report.base.branch.clone().unwrap_or_default();
    let branch = report.target.branch.clone().unwrap_or_default();
    match name {
        "plans.worktree.sync" => {
            let message = text(params, "message")?;
            let mut blockers = sync_plan;
            blockers.extend(report.sync_blockers.iter().cloned());
            ensure!(blockers.is_empty(), "{}", blockers.join(" "));
            if !confirmed {
                return Ok(Outcome::Confirm(format!(
                    "Squash-merge {branch} into {base} in {} and commit it with this message? It changes the main checkout's {base} branch, does not push, and runs no hooks.\n\n{message}",
                    report.base.path.display()
                )));
            }
            let done = worktree::sync(&c.project, &row.worktree.path, head, message, &c.project)?;
            Ok(Outcome::Done(json!({"message": format!(
                "Synced {} into {} as commit {}. Discard the worktree when you are finished with it.",
                done.branch, done.base_branch, short(&done.commit)
            )})))
        }
        "plans.worktree.discard" => {
            let mut blockers = discard_plan;
            blockers.extend(report.discard_blockers.iter().cloned());
            ensure!(blockers.is_empty(), "{}", blockers.join(" "));
            if !confirmed {
                return Ok(Outcome::Confirm(format!(
                    "Remove the worktree folder {}? Git deletes it with everything ignored inside it, and this cannot be undone. Branch {branch} is kept, and its changes are already in {base}.",
                    report.target.path.display()
                )));
            }
            worktree::discard(
                &c.project,
                &row.worktree.path,
                head,
                &c.project,
                &allowed(row),
            )?;
            Ok(Outcome::Done(
                json!({"message": format!("Worktree removed. Branch {branch} is kept.")}),
            ))
        }
        _ => bail!("Unknown action"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = yoke_core::process::git()
            .unwrap()
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}");
        String::from_utf8(out.stdout).unwrap()
    }

    #[test]
    fn worktree_controls_follow_plan_state_and_ask_before_changing_anything() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (main, linked) = (root.join("main"), root.join("feature"));
        std::fs::create_dir(&main).unwrap();
        git(&main, &["init", "-q", "-b", "main"]);
        git(&main, &["config", "user.name", "Fixture"]);
        git(&main, &["config", "user.email", "fixture@example.invalid"]);
        std::fs::write(main.join(".gitignore"), ".yoke/\n").unwrap();
        std::fs::write(main.join("base.txt"), "base\n").unwrap();
        git(&main, &["add", "."]);
        git(&main, &["commit", "-qm", "base"]);
        git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature",
                linked.to_str().unwrap(),
            ],
        );
        std::fs::write(linked.join("feature.txt"), "<b>feature</b>\n").unwrap();
        git(&linked, &["add", "feature.txt"]);
        git(&linked, &["commit", "-qm", "feature"]);
        let database = root.join("state.db");
        let store = Store::open(&database).unwrap();
        plan::create(&store, &linked, ".yoke/plans", "wt-plan", "WT").unwrap();
        let c = Console {
            home: root.clone(),
            project: main.clone(),
            database: database.clone(),
            token: "token".into(),
            csrf: "csrf".into(),
            host: "127.0.0.1:1".into(),
        };
        let wanted = id(&linked);
        let head = git(&linked, &["rev-parse", "HEAD"]).trim().to_owned();
        let call = |name: &str, extra: Value, confirmed: bool| {
            let mut params = json!({"id": wanted, "head": head});
            params
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            act(&store, &c, name, &params, confirmed)
        };

        let panel = panel(&store, &c).unwrap();
        assert!(
            panel.contains(&format!("plans?worktree={wanted}")),
            "{panel}"
        );
        assert!(panel.contains("1 ahead of main"), "{panel}");
        let html = detail(&store, &c, &wanted).unwrap();
        assert!(html.contains("&lt;b&gt;feature&lt;/b&gt;") && !html.contains("<b>feature"));
        assert!(
            html.contains("Plan wt-plan is still draft") && !html.contains("plans.worktree.sync")
        );
        assert!(
            call("plans.worktree.sync", json!({"message": "m"}), true).is_err(),
            "draft plan blocks sync"
        );

        let connection = rusqlite::Connection::open(&database).unwrap();
        connection
            .execute("UPDATE plans SET state='done' WHERE slug='wt-plan'", [])
            .unwrap();
        assert!(detail(&store, &c, &wanted)
            .unwrap()
            .contains("plans.worktree.sync"));
        assert!(
            call("plans.worktree.discard", json!({}), true).is_err(),
            "unmerged work blocks discard even for a done plan"
        );

        let sync = json!({"message": "Squash feature"});
        assert!(matches!(
            call("plans.worktree.sync", sync.clone(), false).unwrap(),
            Outcome::Confirm(_)
        ));
        assert!(!main.join("feature.txt").exists(), "asking must not merge");
        assert!(call(
            "plans.worktree.sync",
            json!({"message": "m", "head": "stale"}),
            true
        )
        .is_err());
        assert!(matches!(
            call("plans.worktree.sync", sync, true).unwrap(),
            Outcome::Done(_)
        ));
        assert_eq!(
            std::fs::read_to_string(main.join("feature.txt")).unwrap(),
            "<b>feature</b>\n"
        );

        assert!(matches!(
            call("plans.worktree.discard", json!({}), false).unwrap(),
            Outcome::Confirm(_)
        ));
        assert!(linked.exists(), "asking must not remove");
        assert!(matches!(
            call("plans.worktree.discard", json!({}), true).unwrap(),
            Outcome::Done(_)
        ));
        assert!(!linked.exists());
        assert!(git(&main, &["branch", "--list", "feature"]).contains("feature"));
    }
}
