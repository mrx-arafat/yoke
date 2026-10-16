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
    List,
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
    },
    /// Review and freeze exact check commands in a human terminal.
    Approve {
        /// Plan to approve.
        slug: String,
    },
    /// Bind an approved plan to an existing recorded agent session.
    Start {
        /// Plan to start.
        slug: String,
        /// Recorded session id that will build the plan.
        #[arg(long)]
        session: String,
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
    },
    /// Human-only: resume a waiting or stalled plan, or recover one whose check was interrupted.
    Resume {
        /// Plan to resume.
        slug: String,
    },
    /// Human-only: cancel an unfinished plan and release its session.
    Cancel {
        /// Plan to cancel.
        slug: String,
        /// Audit reason recorded with the cancellation.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Human-only: move a building, waiting, or stalled plan to a replacement session.
    Reassign {
        /// Plan to hand over.
        slug: String,
        /// Recorded session id that takes over the plan.
        #[arg(long)]
        session: String,
    },
    /// Run frozen checks and required independent review on the actual tree.
    Check {
        /// Plan to check.
        slug: String,
        /// Agent that built the change; the other agent reviews it.
        #[arg(long, value_enum)]
        agent: AgentArg,
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
    },
}

pub fn run(command: PlanCommand) -> Result<()> {
    match &command {
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
                print!("{}", render_new(&row, plan::has_head(project)));
            }
        }
        PlanCommand::Status {
            slug: None,
            json: true,
        } => println!("{}", serde_json::to_string(&plan::list(&store, project)?)?),
        PlanCommand::List | PlanCommand::Status { slug: None, .. } => {
            let plans = plan::list(&store, project)?;
            println!("plans[{}]{{slug,state,file}}:", plans.len());
            for row in plans {
                println!(
                    "  {},{},{}",
                    commands::quote(&row.slug),
                    row.state,
                    commands::quote(&row.file)
                );
            }
        }
        PlanCommand::Status {
            slug: Some(slug),
            json,
        } => {
            let detail = plan::detail(&store, project, &slug)?;
            if json {
                println!("{}", serde_json::to_string(&detail)?);
            } else {
                status(&detail);
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
                &mut review::command(reviewer, loaded.config.review.models.get(reviewer.as_str())),
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
                },
        } => {
            let definition = plan::task(&store, project, &slug, &task)?;
            let actor = if definition.owner == plan::Actor::Human {
                commands::require_human("plan task done")?;
                println!(
                    "plan: {}\ntask: {}\ntitle: {}",
                    commands::quote(&slug),
                    commands::quote(&task),
                    commands::quote(&definition.title)
                );
                if !commands::confirm(&format!("done {task}"))? {
                    println!("task: cancelled");
                    return Ok(());
                }
                plan::Actor::Human
            } else {
                plan::Actor::Agent
            };
            let progress =
                plan::complete_task(&store, project, &slug, &task, evidence.as_deref(), actor)?;
            println!("{}", serde_json::to_string(&progress)?);
        }
        PlanCommand::Accept {
            slug,
            requirement,
            evidence,
        } => {
            let detail = plan::detail(&store, project, &slug)?;
            let item = detail
                .progress
                .requirements
                .iter()
                .find(|item| item.requirement.id == requirement)
                .with_context(|| format!("plan {slug} has no requirement {requirement:?}"))?;
            println!(
                "plan: {}\nrequirement: {}\ntext: {}\nevidence: {}",
                commands::quote(&slug),
                commands::quote(&requirement),
                commands::quote(&item.requirement.text),
                commands::quote(&evidence)
            );
            if commands::confirm(&format!("accept {requirement}"))? {
                let accepted =
                    plan::accept_requirement(&store, project, &slug, &requirement, &evidence)?;
                println!("{}", serde_json::to_string(&accepted)?);
            } else {
                println!("accept: cancelled");
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
        PlanCommand::Resolve { slug, id, note } => {
            close(&store, project, &slug, id, plan::Closure::Resolved, note)?
        }
        PlanCommand::Acknowledge { slug, id, note } => close(
            &store,
            project,
            &slug,
            id,
            plan::Closure::Acknowledged,
            note,
        )?,
        PlanCommand::Approve { slug } => {
            let prepared = plan::prepare_approval(&store, project, &slug)?;
            println!(
                "plan: {}\nheader: {}\ndigest: {}",
                commands::quote(&slug),
                commands::quote(&prepared.header),
                prepared.digest
            );
            print_feedback("open_feedback", &prepared.open_feedback);
            if commands::confirm(&format!("approve {slug}"))? {
                println!(
                    "{}",
                    serde_json::to_string(&plan::approve(&store, &prepared)?)?
                );
            } else {
                println!("approval: cancelled");
            }
        }
        PlanCommand::Start { slug, session } => {
            println!(
                "{}",
                serde_json::to_string(&plan::start(&store, project, &slug, &session)?)?
            );
            if loaded.config.mode == Mode::Observe {
                eprintln!("{OBSERVE_NOTE}");
            }
        }
        PlanCommand::Ask {
            question,
            slug,
            session,
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
            println!(
                "{}",
                serde_json::to_string(&plan::ask(&store, project, &slug, &question)?)?
            );
        }
        PlanCommand::Resume { slug } => {
            println!(
                "{}",
                serde_json::to_string(&plan::get(&store, project, &slug)?)?
            );
            if commands::confirm(&format!("resume {slug}"))? {
                println!(
                    "{}",
                    serde_json::to_string(&plan::resume(&store, project, &slug)?)?
                );
            } else {
                println!("resume: cancelled");
            }
        }
        PlanCommand::Cancel { slug, reason } => {
            println!(
                "{}",
                serde_json::to_string(&plan::get(&store, project, &slug)?)?
            );
            if commands::confirm(&format!("cancel {slug}"))? {
                let reason = reason.as_deref().unwrap_or("cancelled by human");
                println!(
                    "{}",
                    serde_json::to_string(&plan::cancel(&store, project, &slug, reason)?)?
                );
            } else {
                println!("cancel: aborted");
            }
        }
        PlanCommand::Reassign { slug, session } => {
            println!(
                "{}",
                serde_json::to_string(&plan::get(&store, project, &slug)?)?
            );
            if commands::confirm(&format!("reassign {slug}"))? {
                println!(
                    "{}",
                    serde_json::to_string(&plan::reassign(&store, project, &slug, &session)?)?
                );
            } else {
                println!("reassign: aborted");
            }
        }
        PlanCommand::Check { slug, agent } => {
            let reviewer = review::Reviewer::select(
                &loaded.config.review.reviewer,
                yoke_core::event::Agent::from(agent).as_str(),
            )?;
            let model = loaded.config.review.models.get(reviewer.as_str());
            let summary = plan::check(&store, project, &slug, Some((reviewer, model)))?;
            println!("{}", serde_json::to_string(&summary)?);
            ensure!(
                summary.passed,
                "plan checks did not pass; inspect yoke plan status {slug}"
            );
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
    ensure!(
        persisted_pass,
        "review did not pass; inspect yoke plan status {slug}"
    );
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
) -> Result<()> {
    let detail = plan::detail(store, project, slug)?;
    let item = detail
        .feedback
        .iter()
        .find(|item| item.id == id)
        .with_context(|| format!("plan {slug} has no feedback #{id}"))?;
    print_feedback("feedback", std::slice::from_ref(item));
    let verb = match closure {
        plan::Closure::Resolved => "resolve",
        plan::Closure::Acknowledged => "acknowledge",
    };
    if commands::confirm(&format!("{verb} {id}"))? {
        let item = plan::close(store, project, slug, id, closure, note.as_deref())?;
        println!("{}", serde_json::to_string(&item)?);
    } else {
        println!("{verb}: cancelled");
    }
    Ok(())
}

/// Longest feedback text a table row shows before pointing at `--json`.
const TEXT_LIMIT: usize = 200;

fn short(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.char_indices().nth(TEXT_LIMIT) {
        Some((end, _)) => commands::quote(&format!(
            "{}... ({} chars)",
            &flat[..end],
            flat.chars().count()
        )),
        None => commands::quote(&flat),
    }
}

fn print_feedback(name: &str, items: &[plan::Feedback]) {
    print!("{}", feedback_table(name, items));
}

/// Feedback rows; a reply shows the item it answers in its state column.
fn feedback_table(name: &str, items: &[plan::Feedback]) -> String {
    if items.is_empty() {
        return format!("{name}: none\n");
    }
    let mut text = format!(
        "{name}[{}]{{id,author,severity,state,anchor,text}}:\n",
        items.len()
    );
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
        text.push_str(&format!(
            "  {},{},{severity},{state},{},{}\n",
            item.id,
            item.author,
            commands::quote(&anchor),
            short(&item.text)
        ));
    }
    text
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
fn status(detail: &plan::PlanDetail) {
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
    println!("  revision: {}", plan.revision);
    println!("  file: {}", commands::quote(&plan.file));
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
        println!("tasks[{}]{{id,owner,state,title}}:", progress.tasks.len());
        for item in &progress.tasks {
            println!(
                "  {},{},{},{}",
                commands::quote(&item.task.id),
                item.task.owner.as_str(),
                serde_json::to_value(item.state)
                    .unwrap_or_default()
                    .as_str()
                    .unwrap_or(""),
                short(&item.task.title)
            );
        }
    }
    if progress.requirements.is_empty() {
        println!("requirements: none declared");
    } else {
        println!(
            "requirements[{}]{{id,satisfied_by,text}}:",
            progress.requirements.len()
        );
        for item in &progress.requirements {
            println!(
                "  {},{},{}",
                commands::quote(&item.requirement.id),
                serde_json::to_value(item.status)
                    .unwrap_or_default()
                    .as_str()
                    .unwrap_or(""),
                short(&item.requirement.text)
            );
        }
    }
    print_feedback("open_feedback", &thread);
    let shown = detail.checks.len().min(10);
    if shown == 0 {
        println!("checks: none run");
    } else {
        println!(
            "checks[{shown}]{{id,name,result,blocking}}: newest {shown} of {}",
            detail.checks.len()
        );
        for run in detail.checks.iter().rev().take(shown) {
            println!(
                "  {},{},{},{}",
                run.id,
                commands::quote(&run.name),
                run.result,
                run.blocking
            );
        }
    }
    if !detail.evidence.is_empty() {
        println!("evidence[{}]{{check,path,status}}:", detail.evidence.len());
        for item in &detail.evidence {
            println!(
                "  {},{},{}",
                item.check_run_id,
                commands::quote(&item.path),
                serde_json::to_value(item.status)
                    .unwrap_or_default()
                    .as_str()
                    .unwrap_or("")
            );
        }
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
            text.contains("  2,reviewer,reply,reply_to_1,\"plan\","),
            "{text}"
        );
        assert!(
            text.contains("yoke plan resolve format-policy <id>"),
            "{text}"
        );
        assert!(text.contains("--json` for the full finding text"), "{text}");
        assert!(!text.starts_with('{'), "{text}");
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
