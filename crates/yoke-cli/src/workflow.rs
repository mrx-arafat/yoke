use anyhow::{ensure, Context, Result};
use clap::Subcommand;
use yoke_core::{plan, review};

use crate::{commands, AgentArg};

#[derive(Subcommand)]
pub enum PlanCommand {
    /// Create an editable draft; approval does not happen automatically.
    New {
        slug: String,
        #[arg(long)]
        title: Option<String>,
    },
    /// List this project's persisted plans.
    List,
    /// Show a plan and its transition/check evidence, or list plans.
    Status { slug: Option<String> },
    /// Review and freeze exact check commands in a human terminal.
    Approve { slug: String },
    /// Bind an approved plan to an existing recorded agent session.
    Start {
        slug: String,
        #[arg(long)]
        session: String,
    },
    /// Record a question and wait for a human response.
    Ask {
        question: String,
        #[arg(long)]
        slug: Option<String>,
        #[arg(long)]
        session: Option<String>,
    },
    /// Human-only: resume a waiting or stalled plan.
    Resume { slug: String },
    /// Run frozen checks and required independent review on the actual tree.
    Check {
        slug: String,
        #[arg(long, value_enum)]
        agent: AgentArg,
    },
}

pub fn run(command: PlanCommand) -> Result<()> {
    if matches!(
        command,
        PlanCommand::Approve { .. } | PlanCommand::Resume { .. }
    ) {
        commands::require_human(if matches!(command, PlanCommand::Approve { .. }) {
            "plan approve"
        } else {
            "plan resume"
        })?;
    }
    let (store, loaded) = commands::load_project()?;
    let project = &loaded.project_root;
    match command {
        PlanCommand::New { slug, title } => {
            let row = plan::create(
                &store,
                project,
                &loaded.config.plans.dir,
                &slug,
                title.as_deref().unwrap_or(&slug),
            )?;
            println!("{}", serde_json::to_string(&row)?);
            println!(
                "help: \"Edit the plan checks, then ask a human to run yoke plan approve <slug>.\""
            );
        }
        PlanCommand::List | PlanCommand::Status { slug: None } => {
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
        PlanCommand::Status { slug: Some(slug) } => println!(
            "{}",
            serde_json::to_string(&plan::detail(&store, project, &slug)?)?
        ),
        PlanCommand::Approve { slug } => {
            let prepared = plan::prepare_approval(&store, project, &slug)?;
            println!(
                "plan: {}\nheader: {}\ndigest: {}",
                commands::quote(&slug),
                commands::quote(&prepared.header),
                prepared.digest
            );
            if commands::confirm(&format!("approve {slug}"))? {
                println!(
                    "{}",
                    serde_json::to_string(&plan::approve(&store, &prepared)?)?
                );
            } else {
                println!("approval: cancelled");
            }
        }
        PlanCommand::Start { slug, session } => println!(
            "{}",
            serde_json::to_string(&plan::start(&store, project, &slug, &session)?)?
        ),
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
        PlanCommand::Check { slug, agent } => {
            let reviewer = review::Reviewer::select(
                &loaded.config.review.reviewer,
                yoke_core::event::Agent::from(agent).as_str(),
            )?;
            let summary = plan::check(&store, project, &slug, Some(reviewer))?;
            println!("{}", serde_json::to_string(&summary)?);
            ensure!(
                summary.passed,
                "plan checks did not pass; inspect yoke plan status {slug}"
            );
        }
    }
    Ok(())
}

pub fn review(slug: Option<String>, session: Option<String>, agent: AgentArg) -> Result<()> {
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
    plan::get(&store, &loaded.project_root, &slug)?;
    let reviewer = review::Reviewer::select(
        &loaded.config.review.reviewer,
        yoke_core::event::Agent::from(agent).as_str(),
    )?;
    let before = plan::fingerprint(&loaded.project_root)?;
    let mut report = review::run(&loaded.project_root, reviewer)?;
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
    println!("{}", serde_json::to_string(&report)?);
    ensure!(
        persisted_pass,
        "review did not pass; inspect yoke plan status {slug}"
    );
    Ok(())
}
