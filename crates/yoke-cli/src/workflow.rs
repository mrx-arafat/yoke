use anyhow::{ensure, Context, Result};
use clap::Subcommand;
use yoke_core::config::Mode;
use yoke_core::{plan, review};

use crate::{commands, AgentArg};

#[derive(Subcommand)]
pub enum PlanCommand {
    /// Create an editable draft; approval does not happen automatically.
    New {
        /// Plan slug, also the draft file name under plans.dir.
        slug: String,
        /// Title written into the draft header.
        #[arg(long)]
        title: Option<String>,
        /// Print the full plan record as JSON.
        #[arg(long)]
        json: bool,
    },
    /// List this project's persisted plans.
    List {
        /// Print full plan records and their bound sessions as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Show a plan's tasks, requirements, feedback and evidence, or list plans.
    Status {
        /// Plan to show; omit it to list every plan.
        slug: Option<String>,
        /// Print every field and the plan body as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Ask the independent reviewer to validate a plan before approval.
    Validate {
        /// Plan to validate.
        slug: String,
        /// Agent that will build the plan; the other agent reviews it.
        #[arg(long, value_enum)]
        agent: AgentArg,
        /// Print the full validation record as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Record task progress on an approved plan.
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
    /// Human-only: record evidence that a `human = true` requirement is met.
    Accept {
        /// Plan slug.
        slug: String,
        /// Requirement id declared with human = true.
        requirement: String,
        /// What was checked and how.
        #[arg(long)]
        evidence: String,
        /// Print the full requirement record as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Anchor feedback to `plan`, `requirement:<id>`, `task:<id>`, `check:<name>` or `heading:<slug>`.
    Comment {
        /// Plan slug.
        slug: String,
        /// Where the feedback applies, for example task:<id> or check:<name>.
        #[arg(long)]
        anchor: String,
        /// Comment text.
        #[arg(long)]
        text: String,
        /// Print the full feedback record as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Reply to a finding or comment; only a human can close it.
    Reply {
        /// Plan slug.
        slug: String,
        /// Finding or comment id from yoke plan status.
        id: i64,
        /// Reply text.
        #[arg(long)]
        text: String,
        /// Print the full feedback record as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Human-only: mark a finding or comment resolved.
    Resolve {
        /// Plan slug.
        slug: String,
        /// Finding or comment id from yoke plan status.
        id: i64,
        /// Note recorded with the resolution.
        #[arg(long)]
        note: Option<String>,
        /// Print the full feedback record as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Human-only: accept the risk of a finding or comment without changing the plan.
    Acknowledge {
        /// Plan slug.
        slug: String,
        /// Finding or comment id from yoke plan status.
        id: i64,
        /// Note recorded with the acknowledgement.
        #[arg(long)]
        note: Option<String>,
        /// Print the full feedback record as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Review and freeze exact check commands in a human terminal.
    Approve {
        /// Plan to approve.
        slug: String,
        /// Print the approved plan record as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Bind an approved plan to an existing recorded agent session.
    Start {
        /// Plan to start.
        slug: String,
        /// Recorded session id that will build the plan; the session column of yoke log lists them.
        #[arg(long)]
        session: String,
        /// Print the full plan record as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Record a question and wait for a human response.
    Ask {
        /// Question for the human; the plan waits until a human resumes it.
        question: String,
        /// Plan asking the question.
        #[arg(long)]
        slug: Option<String>,
        /// Recorded session id of the active plan, used instead of --slug.
        #[arg(long)]
        session: Option<String>,
        /// Print the full plan record as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Human-only: resume a waiting or stalled plan, or recover one whose check was interrupted.
    Resume {
        /// Plan to resume.
        slug: String,
        /// Print the resumed plan record as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Human-only: cancel an unfinished plan and release its session.
    Cancel {
        /// Plan to cancel.
        slug: String,
        /// Audit reason recorded with the cancellation.
        #[arg(long)]
        reason: Option<String>,
        /// Print the cancelled plan record as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Human-only: move a building, waiting, or stalled plan to a replacement session.
    Reassign {
        /// Plan to hand over.
        slug: String,
        /// Recorded session id that takes over the plan.
        #[arg(long)]
        session: String,
        /// Print the reassigned plan record as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Run frozen checks and required independent review on the actual tree.
    Check {
        /// Plan to check.
        slug: String,
        /// Agent that built the change; the other agent reviews it.
        #[arg(long, value_enum)]
        agent: AgentArg,
        /// Print the full check summary as JSON, including fingerprints and output tails.
        #[arg(long)]
        json: bool,
    },
    /// Show the exact diff the independent review sees; read-only.
    Diff {
        /// Plan whose recorded base the diff starts from.
        slug: String,
        /// Print the diff and its base as JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum TaskCommand {
    /// Mark a task done after its dependencies; human-owned tasks need a human terminal.
    Done {
        /// Plan slug.
        slug: String,
        /// Task id from the plan header.
        task: String,
        /// What was done and how it was verified.
        #[arg(long)]
        evidence: Option<String>,
        /// Print the full task record as JSON.
        #[arg(long)]
        json: bool,
    },
}

pub fn run(command: PlanCommand) -> Result<()> {
    match &command {
        PlanCommand::Check { agent, .. } | PlanCommand::Validate { agent, .. } => {
            commands::verify_building_agent(*agent)?;
        }
        PlanCommand::Approve { .. } => commands::require_human("plan approve")?,
        PlanCommand::Accept { .. } => commands::require_human("plan accept")?,
        PlanCommand::Resolve { .. } => commands::require_human("plan resolve")?,
        PlanCommand::Acknowledge { .. } => commands::require_human("plan acknowledge")?,
        PlanCommand::Resume { .. } => commands::require_human("plan resume")?,
        PlanCommand::Cancel { .. } => commands::require_human("plan cancel")?,
        PlanCommand::Reassign { .. } => commands::require_human("plan reassign")?,
        _ => {}
    }
    let (store, loaded) = commands::load_project()?;
    let project = &loaded.project_root;
    match command {
        PlanCommand::New { slug, title, json } => {
            let row = plan::create(
                &store,
                project,
                &loaded.config.plans.dir,
                &slug,
                title.as_deref().unwrap_or(&slug),
            )?;
            if json {
                println!("{}", serde_json::to_string(&row)?);
            } else {
                let has_head = plan::has_head(project);
                let mut output = render_new(&row, has_head);
                if !has_head {
                    let git = yoke_core::process::git()?
                        .current_dir(project)
                        .args(["rev-parse", "--is-bare-repository"])
                        .output()?;
                    let advice = if !git.status.success() {
                        "This directory is not a Git repository; run `git init` and make an initial commit first (set git user.name and user.email before committing)"
                    } else if git.stdout.starts_with(b"true") {
                        "This is a bare Git repository; use a working clone or worktree and make an initial commit there before reviewing a plan"
                    } else {
                        plan::NO_HEAD_ADVICE
                    };
                    output = output.replace(plan::NO_HEAD_ADVICE, advice);
                }
                print!("{output}");
            }
        }
        PlanCommand::Status {
            slug: None,
            json: true,
        }
        | PlanCommand::List { json: true } => {
            let sessions = plan::bound_sessions(&store, project)?;
            let rows = plan::list(&store, project)?
                .iter()
                .map(|row| record_with_session(row, &sessions))
                .collect::<Result<Vec<_>>>()?;
            println!("{}", serde_json::to_string(&rows)?);
        }
        PlanCommand::List { json: false } | PlanCommand::Status { slug: None, .. } => {
            let home = commands::home()?;
            let plans = plan::list(&store, project)?;
            let sessions = plan::bound_sessions(&store, project)?;
            let rows = plans
                .iter()
                .map(|row| {
                    vec![
                        row.slug.clone(),
                        row.state.clone(),
                        sessions
                            .get(&row.id)
                            .cloned()
                            .unwrap_or_else(|| "none".into()),
                        crate::table::path(std::path::Path::new(&row.file), project, &home),
                    ]
                })
                .collect::<Vec<_>>();
            print!(
                "{}",
                crate::table::render(
                    "plans",
                    &[
                        ("SLUG", 4, 28),
                        ("STATE", 8, 9),
                        ("SESSION", 7, 24),
                        ("FILE", 4, 180)
                    ],
                    &rows
                )
            );
        }
        PlanCommand::Status {
            slug: Some(slug),
            json,
        } => {
            let detail = plan::detail(&store, project, &slug)?;
            if json {
                let mut value = serde_json::to_value(&detail)?;
                value["session"] = serde_json::to_value(
                    plan::bound_sessions(&store, project)?.get(&detail.plan.id),
                )?;
                println!("{}", serde_json::to_string(&value)?);
            } else {
                status(
                    &detail,
                    plan::bound_sessions(&store, project)?
                        .get(&detail.plan.id)
                        .map(String::as_str),
                    &commands::home()?,
                );
            }
        }
        PlanCommand::Validate { slug, agent, json } => {
            let reviewer = review::Reviewer::select(
                &loaded.config.review.reviewer,
                yoke_core::event::Agent::from(agent).as_str(),
            )?;
            let validation = plan::validate(
                &store,
                project,
                &slug,
                &mut review::command(reviewer, loaded.config.review.models.get(reviewer.as_str()))?,
                std::time::Duration::from_secs(600),
            )?;
            if json {
                println!("{}", serde_json::to_string(&validation)?);
            } else {
                print!("{}", render_validation(&validation));
            }
        }
        PlanCommand::Task {
            command:
                TaskCommand::Done {
                    slug,
                    task,
                    evidence,
                    json,
                },
        } => {
            let definition = plan::task(&store, project, &slug, &task)?;
            let actor = if definition.owner == plan::Actor::Human {
                commands::require_human("plan task done")?;
                commands::preview(
                    &format!(
                        "plan: {}\ntask: {}\ntitle: {}\n",
                        commands::quote(&slug),
                        commands::quote(&task),
                        commands::quote(&definition.title)
                    ),
                    json,
                );
                if !commands::confirm(&format!("done {task}"), json)? {
                    print_cancelled("task: cancelled", json);
                    return Ok(());
                }
                plan::Actor::Human
            } else {
                plan::Actor::Agent
            };
            let progress =
                plan::complete_task(&store, project, &slug, &task, evidence.as_deref(), actor)?;
            if json {
                println!("{}", serde_json::to_string(&progress)?);
            } else {
                print!("{}", render_task(&slug, &progress));
            }
        }
        PlanCommand::Accept {
            slug,
            requirement,
            evidence,
            json,
        } => {
            let detail = plan::detail(&store, project, &slug)?;
            let item = detail
                .progress
                .requirements
                .iter()
                .find(|item| item.requirement.id == requirement)
                .with_context(|| format!("plan {slug} has no requirement {requirement:?}"))?;
            commands::preview(
                &format!(
                    "plan: {}\nrequirement: {}\ntext: {}\nevidence: {}\n",
                    commands::quote(&slug),
                    commands::quote(&requirement),
                    commands::quote(&item.requirement.text),
                    commands::quote(&evidence)
                ),
                json,
            );
            if commands::confirm(&format!("accept {requirement}"), json)? {
                let accepted =
                    plan::accept_requirement(&store, project, &slug, &requirement, &evidence)?;
                if json {
                    println!("{}", serde_json::to_string(&accepted)?);
                } else {
                    print!("{}", render_requirement(&slug, &accepted));
                }
            } else {
                print_cancelled("accept: cancelled", json);
            }
        }
        PlanCommand::Comment {
            slug,
            anchor,
            text,
            json,
        } => {
            let item = plan::comment(
                &store,
                project,
                &slug,
                &anchor,
                &text,
                actor("plan comment"),
            )?;
            print_added(&item, json)?;
        }
        PlanCommand::Reply {
            slug,
            id,
            text,
            json,
        } => {
            let item = plan::reply(&store, project, &slug, id, &text, actor("plan reply"))?;
            print_added(&item, json)?;
        }
        PlanCommand::Resolve {
            slug,
            id,
            note,
            json,
        } => close(
            &store,
            project,
            &slug,
            id,
            plan::Closure::Resolved,
            note,
            json,
        )?,
        PlanCommand::Acknowledge {
            slug,
            id,
            note,
            json,
        } => close(
            &store,
            project,
            &slug,
            id,
            plan::Closure::Acknowledged,
            note,
            json,
        )?,
        PlanCommand::Approve { slug, json } => {
            let prepared = plan::prepare_approval(&store, project, &slug)?;
            commands::preview(
                &format!(
                    "plan: {}\n{}digest: {}\n{}",
                    commands::quote(&slug),
                    render_approval_header(&prepared.header),
                    prepared.digest,
                    feedback_table("open_feedback", &prepared.open_feedback)
                ),
                json,
            );
            if commands::confirm(&format!("approve {slug}"), json)? {
                let approved = plan::approve(&store, &prepared)?;
                let help = [format!("Run `yoke plan start {slug} --session <session-id>`; the session column of `yoke log` lists recorded session ids")];
                print_record(&approved, &[], &help, json)?;
            } else {
                print_cancelled("approval: cancelled", json);
            }
        }
        PlanCommand::Start {
            slug,
            session,
            json,
        } => {
            let started = plan::start(&store, project, &slug, &session)?;
            let help = [format!(
                "Build the change, then run `yoke plan check {slug} --agent <claude|codex>`"
            )];
            print_record(
                &started,
                &[("session", commands::quote(&session))],
                &help,
                json,
            )?;
            if loaded.config.mode == Mode::Observe {
                eprintln!("{OBSERVE_NOTE}");
            }
        }
        PlanCommand::Ask {
            question,
            slug,
            session,
            json,
        } => {
            let slug = match slug {
                Some(slug) => slug,
                None => {
                    plan::active(
                        &store,
                        project,
                        session.as_deref().context("provide --slug or --session")?,
                    )?
                    .context("session has no active plan")?
                    .slug
                }
            };
            let waiting = plan::ask(&store, project, &slug, &question)?;
            let help = [format!(
                "A human answers and runs `yoke plan resume {slug}` in their own terminal"
            )];
            print_record(&waiting, &[("question", short(&question))], &help, json)?;
        }
        PlanCommand::Resume { slug, json } => {
            print_preview(&plan::get(&store, project, &slug)?, json);
            if commands::confirm(&format!("resume {slug}"), json)? {
                let resumed = plan::resume(&store, project, &slug)?;
                let help = [format!(
                    "Continue the work, then run `yoke plan check {slug} --agent <claude|codex>`"
                )];
                print_record(&resumed, &[], &help, json)?;
            } else {
                print_cancelled("resume: cancelled", json);
            }
        }
        PlanCommand::Cancel { slug, reason, json } => {
            print_preview(&plan::get(&store, project, &slug)?, json);
            if commands::confirm(&format!("cancel {slug}"), json)? {
                let reason = reason.as_deref().unwrap_or("cancelled by human");
                let cancelled = plan::cancel(&store, project, &slug, reason)?;
                print_record(&cancelled, &[("reason", short(reason))], &[], json)?;
            } else {
                print_cancelled("cancel: aborted", json);
            }
        }
        PlanCommand::Reassign {
            slug,
            session,
            json,
        } => {
            print_preview(&plan::get(&store, project, &slug)?, json);
            if commands::confirm(&format!("reassign {slug}"), json)? {
                let moved = plan::reassign(&store, project, &slug, &session)?;
                print_record(&moved, &[("session", commands::quote(&session))], &[], json)?;
            } else {
                print_cancelled("reassign: aborted", json);
            }
        }
        PlanCommand::Check { slug, agent, json } => {
            let record = plan::get(&store, project, &slug)?;
            let header = record
                .approved_header
                .as_deref()
                .unwrap_or_default()
                .parse::<toml_edit::DocumentMut>()?;
            let reviewer = if header.get("review").and_then(|item| item.as_bool()) == Some(true) {
                let reviewer = review::Reviewer::select(
                    &loaded.config.review.reviewer,
                    yoke_core::event::Agent::from(agent).as_str(),
                )?;
                Some((reviewer, loaded.config.review.models.get(reviewer.as_str())))
            } else {
                None
            };
            let summary = plan::check(&store, project, &slug, reviewer)?;
            if json {
                println!("{}", serde_json::to_string(&summary)?);
            } else {
                print!("{}", render_check(&summary));
            }
            if !summary.passed {
                return Err(commands::ReportedFailure(format!(
                    "plan checks did not pass; inspect yoke plan status {slug}"
                ))
                .into());
            }
        }
        PlanCommand::Diff { slug, json } => {
            ensure!(plan::has_head(project), "{}", plan::NO_HEAD_ADVICE);
            crate::diff::run(&store, project, &slug, json)?
        }
    }
    Ok(())
}

pub fn review(
    slug: Option<String>,
    session: Option<String>,
    agent: AgentArg,
    json: bool,
) -> Result<()> {
    commands::verify_building_agent(agent)?;
    let (store, loaded) = commands::load_project()?;
    let slug = match slug {
        Some(slug) => slug,
        None => {
            plan::active(
                &store,
                &loaded.project_root,
                session.as_deref().context(
                    "provide a plan slug or --session so review evidence has a persisted owner",
                )?,
            )?
            .context("session has no active plan for review evidence")?
            .slug
        }
    };
    // Verify evidence owner before paying for a reviewer process.
    let record = plan::get(&store, &loaded.project_root, &slug)?;
    plan::ensure_started(&record)?;
    let reviewer = review::Reviewer::select(
        &loaded.config.review.reviewer,
        yoke_core::event::Agent::from(agent).as_str(),
    )?;
    let before = plan::fingerprint(&loaded.project_root)?;
    let mut report = review::run(
        &loaded.project_root,
        reviewer,
        loaded.config.review.models.get(reviewer.as_str()),
        record.base_ref.as_deref(),
    )?;
    let persisted_pass = plan::record_review_with_fingerprint(
        &store,
        &loaded.project_root,
        &slug,
        &report,
        &before,
    )?;
    if report.passed && !persisted_pass {
        report.passed = false;
        report.error =
            Some("review evidence was invalidated; inspect the stored check result".into());
    }
    if json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        print!("{}", render_review(&slug, &report));
    }
    if !persisted_pass {
        return Err(commands::ReportedFailure(format!(
            "review did not pass; inspect yoke plan status {slug}"
        ))
        .into());
    }
    Ok(())
}

/// Label an author by the same human-terminal test the human-only commands use.
fn actor(action: &str) -> plan::Actor {
    match commands::require_human(action) {
        Ok(()) => plan::Actor::Human,
        Err(_) => plan::Actor::Agent,
    }
}

fn close(
    store: &yoke_core::store::Store,
    project: &std::path::Path,
    slug: &str,
    id: i64,
    closure: plan::Closure,
    note: Option<String>,
    json: bool,
) -> Result<()> {
    let detail = plan::detail(store, project, slug)?;
    let item = detail
        .feedback
        .iter()
        .find(|item| item.id == id)
        .with_context(|| format!("plan {slug} has no feedback #{id}"))?;
    commands::preview(
        &feedback_table("feedback", std::slice::from_ref(item)),
        json,
    );
    let verb = match closure {
        plan::Closure::Resolved => "resolve",
        plan::Closure::Acknowledged => "acknowledge",
    };
    if commands::confirm(&format!("{verb} {id}"), json)? {
        let item = plan::close(store, project, slug, id, closure, note.as_deref())?;
        print_added(&item, json)?;
    } else {
        print_cancelled(&format!("{verb}: cancelled"), json);
    }
    Ok(())
}

/// Longest feedback text a table row shows before pointing at `--json`.
const TEXT_LIMIT: usize = 180;

fn short(text: &str) -> String {
    commands::quote(&crate::table::preview(text, TEXT_LIMIT))
}

fn print_feedback(name: &str, items: &[plan::Feedback]) {
    print!("{}", feedback_table(name, items));
}

/// Feedback rows; a reply shows the item it answers in its state column.
fn feedback_table(name: &str, items: &[plan::Feedback]) -> String {
    if items.is_empty() {
        return format!("{name}: none\n");
    }
    let mut rows = Vec::new();
    for item in items {
        let mut anchor = item.anchor.clone();
        if !item.anchored {
            anchor.push_str(" (missing)");
        }
        let (severity, state) = match item.parent_id {
            Some(parent) => ("reply".to_owned(), format!("reply_to_{parent}")),
            None => (
                item.severity.as_deref().unwrap_or("comment").to_owned(),
                item.state.as_deref().unwrap_or("reply").to_owned(),
            ),
        };
        rows.push(vec![
            item.id.to_string(),
            item.author.clone(),
            severity,
            state,
            anchor,
            item.text.clone(),
        ]);
    }
    crate::table::render(
        name,
        &[
            ("ID", 2, 8),
            ("AUTHOR", 6, 8),
            ("SEVERITY", 8, 8),
            ("STATE", 5, 20),
            ("ANCHOR", 6, 28),
            ("TEXT", 4, 180),
        ],
        &rows,
    )
}

fn help_lines(help: &[String]) -> String {
    let mut text = format!("help[{}]:\n", help.len());
    for line in help {
        text.push_str(&format!("  {line}\n"));
    }
    text
}

/// Printed on stderr by `plan start` so the JSON on stdout stays one value.
const OBSERVE_NOTE: &str = "yoke: mode is observe, so Stop only records what it would block. Set mode = \"enforce\" in yoke.toml and run `yoke trust` in your own terminal to hold Stop until `yoke plan check` passes";

/// Text view of a new draft; `--json` carries the full record.
fn render_new(record: &plan::PlanRecord, has_head: bool) -> String {
    let slug = &record.slug;
    let mut help = vec![
        "Replace the placeholder Goal and the example check with commands that prove the work".to_owned(),
        format!("Run `yoke plan validate {slug} --agent <claude|codex>` to review the plan, then a human runs `yoke plan approve {slug}` in their own terminal"),
    ];
    if !has_head {
        help.push(plan::NO_HEAD_ADVICE.to_owned());
    }
    format!(
        "plan: {}\nstate: {}\nfile: {}\n{}",
        commands::quote(slug),
        record.state,
        commands::quote(&record.file),
        help_lines(&help)
    )
}

/// Longest tail of a failed check's output a table row shows.
const OUTPUT_TAIL: usize = 160;

fn record_with_session(
    record: &plan::PlanRecord,
    sessions: &std::collections::BTreeMap<i64, String>,
) -> Result<serde_json::Value> {
    let mut value = serde_json::to_value(record)?;
    value["session"] = serde_json::to_value(sessions.get(&record.id))?;
    Ok(value)
}

fn render_approval_header(header: &str) -> String {
    let mut escaped = 0;
    let mut text = String::from("header:\n");
    // Only LF is structural. CR and Unicode line separators must remain visible data.
    for line in header.split('\n') {
        text.push_str("  ");
        for c in line.chars() {
            if c.is_ascii_graphic() || c == ' ' {
                // A literal backslash must not impersonate an escaped control character.
                if c == '\\' {
                    text.push('\\');
                }
                text.push(c);
            } else {
                escaped += 1;
                text.extend(c.escape_default());
            }
        }
        text.push('\n');
    }
    text.push_str(&format!("escaped_characters: {escaped}\n"));
    text
}

/// A plan after a state change: the full record with `--json`, a compact text view otherwise.
fn print_record(
    record: &plan::PlanRecord,
    facts: &[(&str, String)],
    help: &[String],
    json: bool,
) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string(record)?);
    } else {
        print!("{}", render_record(record, facts, help));
    }
    Ok(())
}

/// The plan a human is about to change, shown before the confirmation prompt.
fn print_preview(record: &plan::PlanRecord, json: bool) {
    commands::preview(&render_record(record, &[], &[]), json);
}

fn print_cancelled(text: &str, json: bool) {
    if json {
        println!("{}", serde_json::json!({"cancelled": true}));
    } else {
        println!("{text}");
    }
}

/// `facts` are extra `key: value` lines whose values are already quoted where they need to be.
fn render_record(record: &plan::PlanRecord, facts: &[(&str, String)], help: &[String]) -> String {
    let mut text = format!(
        "plan: {}\nstate: {}\n",
        commands::quote(&record.slug),
        record.state
    );
    for (key, value) in facts {
        text.push_str(&format!("{key}: {value}\n"));
    }
    text.push_str(&format!("file: {}\n", commands::quote(&record.file)));
    if let Some(base) = &record.base_ref {
        text.push_str(&format!("base: {base}\n"));
    }
    if !help.is_empty() {
        text.push_str(&help_lines(help));
    }
    text
}

/// The serialized name of a state enum, such as `done` or `human`.
fn word<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Why a check did not pass: its exit code and the end of its output on one line.
fn check_detail(run: &plan::CheckRun) -> String {
    if run.result == "pass" {
        return String::new();
    }
    let flat = run
        .output_tail
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let start = flat
        .char_indices()
        .rev()
        .nth(OUTPUT_TAIL - 1)
        .map_or(0, |(at, _)| at);
    let tail = format!("{}{}", if start > 0 { "..." } else { "" }, &flat[start..]);
    match (run.exit_code, tail.is_empty()) {
        (Some(code), false) => format!("exit {code}: {tail}"),
        (Some(code), true) => format!("exit {code}"),
        (None, _) => tail,
    }
}

/// Text view of a finished check run; `--json` carries fingerprints and the stored output tails.
fn render_check(summary: &plan::CheckSummary) -> String {
    let slug = &summary.plan.slug;
    let mut text = format!(
        "plan: {}\nstate: {}\nresult: {}\n",
        commands::quote(slug),
        summary.plan.state,
        if summary.passed { "pass" } else { "fail" }
    );
    if summary.checks.is_empty() {
        text.push_str("checks: none declared\n");
    } else {
        let rows = summary
            .checks
            .iter()
            .map(|run| {
                vec![
                    run.name.clone(),
                    run.result.clone(),
                    run.blocking.to_string(),
                    run.ms.to_string(),
                    check_detail(run),
                ]
            })
            .collect::<Vec<_>>();
        text.push_str(&crate::table::render(
            "checks",
            &[
                ("NAME", 4, 28),
                ("RESULT", 6, 12),
                ("BLOCKING", 8, 8),
                ("MS", 2, 10),
                ("DETAIL", 6, 180),
            ],
            &rows,
        ));
    }
    if !summary.passed {
        text.push_str(&help_lines(&[
            format!("Fix the blocking failures, then run `yoke plan check {slug} --agent <claude|codex>` again"),
            format!("Run `yoke plan status {slug} --json` for each check's full output"),
        ]));
    }
    text
}

/// `by` and `evidence` lines of a recorded completion.
fn completion_lines(completion: Option<&plan::Completion>) -> String {
    let Some(completion) = completion else {
        return String::new();
    };
    let mut text = format!("by: {}\n", completion.actor);
    if let Some(evidence) = &completion.evidence {
        text.push_str(&format!("evidence: {}\n", short(evidence)));
    }
    text
}

/// Text view of a completed task; `--json` carries the full record.
fn render_task(slug: &str, progress: &plan::TaskProgress) -> String {
    format!(
        "plan: {}\ntask: {}\nstate: {}\ntitle: {}\n{}{}",
        commands::quote(slug),
        commands::quote(&progress.task.id),
        word(&progress.state),
        short(&progress.task.title),
        completion_lines(progress.completion.as_ref()),
        help_lines(&[format!(
            "Run `yoke plan status {slug}` for the remaining tasks and requirements"
        )])
    )
}

/// Text view of a requirement a human accepted; `--json` carries the full record.
fn render_requirement(slug: &str, progress: &plan::RequirementProgress) -> String {
    format!(
        "plan: {}\nrequirement: {}\nstatus: {}\ntext: {}\n{}{}",
        commands::quote(slug),
        commands::quote(&progress.requirement.id),
        word(&progress.status),
        short(&progress.requirement.text),
        completion_lines(progress.completion.as_ref()),
        help_lines(&[format!(
            "Run `yoke plan status {slug}` for the remaining tasks and requirements"
        )])
    )
}

/// Text view of a plan validation; `--json` carries the digest and untruncated findings.
fn render_validation(validation: &plan::Validation) -> String {
    let slug = &validation.plan.slug;
    let mut text = format!(
        "plan: {}\nfindings: {} ({} blocking)\nms: {}\n{}",
        commands::quote(slug),
        validation.findings.len(),
        validation.blocking,
        validation.ms,
        feedback_table("validation", &validation.findings)
    );
    let mut help = Vec::new();
    if validation.blocking > 0 {
        help.push(format!("Edit the plan to address each blocking finding, then run `yoke plan validate {slug} --agent <claude|codex>` again"));
        help.push(format!("A human runs `yoke plan resolve {slug} <id>` or `yoke plan acknowledge {slug} <id>` before `yoke plan approve {slug}`"));
    }
    if validation
        .findings
        .iter()
        .any(|item| item.text.chars().count() > TEXT_LIMIT)
    {
        help.push(format!("Run `yoke plan validate {slug} --agent <claude|codex> --json` for the full finding text"));
    }
    if !help.is_empty() {
        text.push_str(&help_lines(&help));
    }
    text
}

/// A new feedback item as one table row, or the full record with `--json`.
fn print_added(item: &plan::Feedback, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string(item)?);
    } else {
        print_feedback("feedback", std::slice::from_ref(item));
    }
    Ok(())
}

/// Verdict and findings of a standalone review; the reviewer's raw output stays out of a pass.
fn render_review(slug: &str, report: &review::ReviewReport) -> String {
    let mut text = format!(
        "review:\n  plan: {}\n  reviewer: {}\n  result: {}\n  ms: {}\n",
        commands::quote(slug),
        report.agent.as_deref().unwrap_or("custom"),
        if report.passed { "pass" } else { "fail" },
        report.ms
    );
    if let Some(error) = &report.error {
        text.push_str(&format!("  error: {}\n", short(error)));
    }
    if report.findings.is_empty() {
        text.push_str("findings: none\n");
    } else {
        text.push_str(&format!(
            "findings[{}]{{severity,file,line,summary}}:\n",
            report.findings.len()
        ));
        for finding in &report.findings {
            let severity = match finding.severity {
                review::Severity::Blocking => "blocking",
                review::Severity::Advisory => "advisory",
            };
            text.push_str(&format!(
                "  {severity},{},{},{}\n",
                commands::quote(&finding.file),
                finding.line,
                short(&finding.summary)
            ));
        }
    }
    if !report.passed && report.error.is_some() {
        let flat = report
            .output_tail
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let start = flat.len().saturating_sub(400);
        let start = (start..=flat.len())
            .find(|at| flat.is_char_boundary(*at))
            .unwrap_or(0);
        text.push_str(&format!(
            "output_tail: {}\n",
            commands::quote(&flat[start..])
        ));
    }
    text.push_str(&help_lines(&[format!(
        "Run `yoke plan status {slug}` for the stored result, or add --json for the full report"
    )]));
    text
}

/// AXI text view of one plan; `--json` carries every field and the plan body.
fn status(detail: &plan::PlanDetail, session: Option<&str>, home: &std::path::Path) {
    let plan = &detail.plan;
    let progress = &detail.progress;
    let done = progress
        .tasks
        .iter()
        .filter(|item| item.state == plan::TaskState::Done)
        .count();
    let satisfied = progress
        .requirements
        .iter()
        .filter(|item| item.status != plan::RequirementStatus::Unsatisfied)
        .count();
    let open: Vec<_> = detail
        .feedback
        .iter()
        .filter(|item| item.state.as_deref() == Some("open"))
        .cloned()
        .collect();
    let blocking = open
        .iter()
        .filter(|item| item.severity.as_deref() == Some("blocking"))
        .count();
    // Each open item is followed by the replies it received.
    let thread: Vec<_> = open
        .iter()
        .flat_map(|item| {
            std::iter::once(item).chain(
                detail
                    .feedback
                    .iter()
                    .filter(|reply| reply.parent_id == Some(item.id)),
            )
        })
        .cloned()
        .collect();
    let replies = detail
        .feedback
        .iter()
        .filter(|item| item.parent_id.is_some())
        .count();
    println!("plan:");
    println!("  slug: {}", commands::quote(&plan.slug));
    println!("  state: {}", plan.state);
    println!(
        "  session: {}",
        session
            .map(commands::quote)
            .unwrap_or_else(|| "none".into())
    );
    println!("  revision: {}", plan.revision);
    println!(
        "  file: {}",
        commands::quote(&crate::table::path(
            std::path::Path::new(&plan.file),
            std::path::Path::new(&plan.project),
            home
        ))
    );
    println!("  tasks: {done}/{} done", progress.tasks.len());
    println!(
        "  requirements: {satisfied}/{} satisfied",
        progress.requirements.len()
    );
    println!(
        "  feedback: {} open ({blocking} blocking) of {}{}",
        open.len(),
        detail.feedback.len() - replies,
        match replies {
            0 => String::new(),
            1 => ", 1 reply".to_owned(),
            n => format!(", {n} replies"),
        }
    );
    if progress.tasks.is_empty() {
        println!("tasks: none declared");
    } else {
        let rows = progress
            .tasks
            .iter()
            .map(|item| {
                vec![
                    item.task.id.clone(),
                    item.task.owner.as_str().into(),
                    word(&item.state),
                    item.task.title.clone(),
                ]
            })
            .collect::<Vec<_>>();
        print!(
            "{}",
            crate::table::render(
                "tasks",
                &[
                    ("ID", 2, 24),
                    ("OWNER", 5, 5),
                    ("STATE", 5, 12),
                    ("TITLE", 5, 180)
                ],
                &rows
            )
        );
    }
    if progress.requirements.is_empty() {
        println!("requirements: none declared");
    } else {
        let rows = progress
            .requirements
            .iter()
            .map(|item| {
                vec![
                    item.requirement.id.clone(),
                    word(&item.status),
                    item.requirement.text.clone(),
                ]
            })
            .collect::<Vec<_>>();
        print!(
            "{}",
            crate::table::render(
                "requirements",
                &[("ID", 2, 24), ("STATUS", 6, 16), ("TEXT", 4, 180)],
                &rows
            )
        );
    }
    print_feedback("open_feedback", &thread);
    let shown = detail.checks.len().min(10);
    if shown == 0 {
        println!("checks: none run");
    } else {
        println!("check history: newest {shown} of {}", detail.checks.len());
        let rows = detail
            .checks
            .iter()
            .rev()
            .take(shown)
            .map(|run| {
                vec![
                    run.id.to_string(),
                    run.name.clone(),
                    run.result.clone(),
                    run.blocking.to_string(),
                ]
            })
            .collect::<Vec<_>>();
        print!(
            "{}",
            crate::table::render(
                "checks",
                &[
                    ("ID", 2, 8),
                    ("NAME", 4, 40),
                    ("RESULT", 6, 12),
                    ("BLOCKING", 8, 8)
                ],
                &rows
            )
        );
    }
    if !detail.evidence.is_empty() {
        let rows = detail
            .evidence
            .iter()
            .map(|item| {
                vec![
                    item.check_run_id.to_string(),
                    crate::table::path(
                        std::path::Path::new(&item.path),
                        std::path::Path::new(&plan.project),
                        home,
                    ),
                    word(&item.status),
                ]
            })
            .collect::<Vec<_>>();
        print!(
            "{}",
            crate::table::render(
                "evidence",
                &[("CHECK", 5, 8), ("PATH", 4, 180), ("STATUS", 8, 10)],
                &rows
            )
        );
    }
    let slug = &plan.slug;
    let mut help = Vec::new();
    match plan.state.as_str() {
        "draft" => {
            help.push(format!("Run `yoke plan validate {slug} --agent <claude|codex>` to review the plan before approval"));
            help.push(format!(
                "A human runs `yoke plan approve {slug}` in their own terminal"
            ));
        }
        "approved" => help.push(format!(
            "Run `yoke plan start {slug} --session <session-id>`"
        )),
        "building" => {
            if let Some(task) = progress
                .tasks
                .iter()
                .find(|item| item.state == plan::TaskState::Ready)
            {
                help.push(format!(
                    "Run `yoke plan task done {slug} {} --evidence '<what changed>'`",
                    task.task.id
                ));
            }
            help.push(format!(
                "Run `yoke plan check {slug} --agent <claude|codex>`"
            ));
        }
        "waiting" | "stalled" | "checking" => help.push(format!(
            "A human runs `yoke plan resume {slug}` in their own terminal"
        )),
        _ => {}
    }
    if blocking > 0 {
        help.push(format!(
            "A human runs `yoke plan resolve {slug} <id>` or `yoke plan acknowledge {slug} <id>`"
        ));
    }
    let cut = thread
        .iter()
        .any(|item| item.text.chars().count() > TEXT_LIMIT);
    if detail.checks.len() > shown || detail.feedback.len() > thread.len() || cut {
        help.push(format!("Run `yoke plan status {slug} --json` for every check run, feedback item and the plan body"));
    }
    if !help.is_empty() {
        println!("help[{}]:", help.len());
        for line in help {
            println!("  {line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_header_preserves_lines_and_exposes_hidden_characters() {
        let shown = render_approval_header(
            "title = 'safe'\nrun = 'echo \u{202e}\t\r\u{1b}'\nliteral = '\\u{202e}'",
        );
        assert!(
            shown.contains("header:\n  title = 'safe'\n  run = 'echo \\u{202e}\\t\\r\\u{1b}'\n"),
            "{shown}"
        );
        assert!(shown.contains("literal = '\\\\u{202e}'"), "{shown}");
        assert!(shown.ends_with("escaped_characters: 4\n"), "{shown}");
        assert!(shown
            .chars()
            .all(|c| c == '\n' || c.is_ascii_graphic() || c == ' '));
    }

    fn record() -> plan::PlanRecord {
        plan::PlanRecord {
            id: 1,
            project: "/p".into(),
            slug: "format-policy".into(),
            file: "/p/.yoke/plans/format-policy.md".into(),
            state: "draft".into(),
            approved_header: None,
            revision: 0,
            created_at: 1,
            updated_at: 1,
            base_ref: None,
        }
    }

    fn feedback(id: i64, parent: Option<i64>, severity: Option<&str>) -> plan::Feedback {
        plan::Feedback {
            id,
            parent_id: parent,
            ts: 1,
            author: "reviewer".into(),
            severity: severity.map(Into::into),
            anchor: "plan".into(),
            digest: String::new(),
            text: "x".repeat(300),
            state: parent.is_none().then(|| "open".to_owned()),
            resolved_by: None,
            resolved_at: None,
            note: None,
            current: true,
            anchored: true,
        }
    }

    #[test]
    fn new_and_validate_print_compact_text_with_next_steps() {
        let new = render_new(&record(), false);
        assert!(new.starts_with("plan: \"format-policy\"\nstate: draft\nfile: \"/p/.yoke/plans/"));
        assert!(new.contains("yoke plan approve format-policy"), "{new}");
        assert!(new.contains(plan::NO_HEAD_ADVICE), "{new}");
        assert!(!render_new(&record(), true).contains(plan::NO_HEAD_ADVICE));

        let validation = plan::Validation {
            plan: record(),
            digest: String::new(),
            findings: vec![
                feedback(1, None, Some("blocking")),
                feedback(2, Some(1), None),
            ],
            blocking: 1,
            ms: 7,
        };
        let text = render_validation(&validation);
        assert!(text.contains("findings: 2 (1 blocking)"), "{text}");
        assert!(
            text.lines().any(|line| line.split_whitespace().take(5).eq([
                "2",
                "reviewer",
                "reply",
                "reply_to_1",
                "plan"
            ])),
            "{text}"
        );
        assert!(
            text.contains("yoke plan resolve format-policy <id>"),
            "{text}"
        );
        assert!(text.contains("--json` for the full finding text"), "{text}");
        assert!(!text.starts_with('{'), "{text}");
    }

    fn run(
        name: &str,
        result: &str,
        exit: Option<i32>,
        tail: &str,
        blocking: bool,
    ) -> plan::CheckRun {
        plan::CheckRun {
            id: 1,
            plan_id: 1,
            ts: 1,
            name: name.into(),
            command: "true".into(),
            exit_code: exit,
            output_tail: tail.into(),
            fingerprint: String::new(),
            blocking,
            ms: 5,
            result: result.into(),
            findings: serde_json::Value::Null,
            artifacts: Vec::new(),
            artifact_hashes: Default::default(),
        }
    }

    #[test]
    fn state_changes_and_checks_print_compact_text_with_the_next_step() {
        let mut building = record();
        building.state = "building".into();
        building.base_ref = Some("abc123".into());
        let help = ["Run `yoke plan check format-policy --agent <claude|codex>`".to_owned()];
        assert_eq!(
            render_record(&building, &[("session", commands::quote("s-1"))], &help),
            "plan: \"format-policy\"\nstate: building\nsession: \"s-1\"\nfile: \"/p/.yoke/plans/format-policy.md\"\nbase: abc123\nhelp[1]:\n  Run `yoke plan check format-policy --agent <claude|codex>`\n"
        );

        let mut summary = plan::CheckSummary {
            plan: building,
            passed: false,
            fingerprint_before: "a".into(),
            fingerprint_after: "a".into(),
            checks: vec![
                run("diff", "pass", Some(0), "", true),
                run("tests", "fail", Some(1), "3 failed\nleft == right", true),
                run("lint", "fail", Some(1), "warn", false),
            ],
        };
        let checked = render_check(&summary);
        assert!(
            checked
                .starts_with("plan: \"format-policy\"\nstate: building\nresult: fail\nchecks: 3\n"),
            "{checked}"
        );
        for row in [
            "NAME RESULT BLOCKING MS DETAIL",
            "diff pass true 5 -",
            "tests fail true 5 exit 1: 3 failed left == right",
            "lint fail false 5 exit 1: warn",
        ] {
            assert!(
                checked
                    .lines()
                    .any(|line| line.split_whitespace().collect::<Vec<_>>().join(" ") == row),
                "{row}: {checked}"
            );
        }
        assert!(
            checked.contains("Fix the blocking failures")
                && checked.contains("--json` for each check's full output"),
            "{checked}"
        );
        summary.passed = true;
        summary.plan.state = "done".into();
        let passed = render_check(&summary);
        assert!(passed.contains("state: done\nresult: pass\n"), "{passed}");
        assert!(!passed.contains("help"), "{passed}");
    }

    #[test]
    fn task_and_requirement_progress_print_who_recorded_what() {
        let task = plan::TaskProgress {
            task: plan::Task {
                id: "capture".into(),
                title: "Capture the shots".into(),
                requirements: Vec::new(),
                depends_on: Vec::new(),
                owner: plan::Actor::Agent,
            },
            state: plan::TaskState::Done,
            completion: Some(plan::Completion {
                ts: 1,
                actor: "agent".into(),
                evidence: Some("wrote shots/a.png".into()),
            }),
        };
        assert_eq!(
            render_task("fix", &task),
            "plan: \"fix\"\ntask: \"capture\"\nstate: done\ntitle: \"Capture the shots\"\nby: agent\nevidence: \"wrote shots/a.png\"\nhelp[1]:\n  Run `yoke plan status fix` for the remaining tasks and requirements\n"
        );
        let requirement = plan::RequirementProgress {
            requirement: plan::Requirement {
                id: "signed-off".into(),
                text: "A person signed off".into(),
                checks: Vec::new(),
                human: true,
            },
            status: plan::RequirementStatus::Human,
            completion: Some(plan::Completion {
                ts: 1,
                actor: "human".into(),
                evidence: Some("reviewed".into()),
            }),
        };
        assert_eq!(
            render_requirement("fix", &requirement),
            "plan: \"fix\"\nrequirement: \"signed-off\"\nstatus: human\ntext: \"A person signed off\"\nby: human\nevidence: \"reviewed\"\nhelp[1]:\n  Run `yoke plan status fix` for the remaining tasks and requirements\n"
        );
    }

    #[test]
    fn review_text_shows_the_verdict_and_keeps_reviewer_noise_out_of_a_pass() {
        let mut report = review::ReviewReport {
            findings: vec![],
            passed: true,
            error: None,
            exit_code: Some(0),
            output_tail: "OpenAI Codex banner\nwarning: bubblewrap".into(),
            ms: 12,
            agent: Some("codex".into()),
            model: None,
        };
        let text = render_review("fix", &report);
        assert!(
            text.contains("  reviewer: codex\n  result: pass\n"),
            "{text}"
        );
        assert!(text.contains("findings: none"), "{text}");
        assert!(!text.contains("bubblewrap"), "{text}");

        report.passed = false;
        report.error = Some("reviewer exited 1".into());
        let text = render_review("fix", &report);
        assert!(
            text.contains("  result: fail\n  ms: 12\n  error: \"reviewer exited 1\""),
            "{text}"
        );
        assert!(
            text.contains("output_tail: \"OpenAI Codex banner warning: bubblewrap\""),
            "{text}"
        );
    }
}
