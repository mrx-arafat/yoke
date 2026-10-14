//! Requirements and tasks declared in the approved header, with completion as runtime data.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::{files, get, now, storage, CheckRun, PlanHeader, PlanRecord};
use crate::redact::redact_payload;
use crate::store::Store;

/// The run name that records requirement and task acceptance for one check attempt.
pub(super) const RUN: &str = "acceptance";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requirement {
    pub id: String,
    pub text: String,
    /// Blocking checks (or `review`) whose passing run satisfies this requirement.
    #[serde(default)]
    pub checks: Vec<String>,
    /// Satisfied only by evidence a human records with `yoke plan accept`.
    #[serde(default)]
    pub human: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub requirements: Vec<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub owner: Actor,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Actor {
    #[default]
    Agent,
    Human,
}

impl Actor {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Human => "human",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Completion {
    pub ts: i64,
    pub actor: String,
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    Done,
    Ready,
    /// At least one dependency is not done yet.
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RequirementStatus {
    /// Every linked check passed in the latest valid check attempt.
    Check,
    /// A human recorded evidence for the approved requirement text.
    Human,
    Unsatisfied,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskProgress {
    #[serde(flatten)]
    pub task: Task,
    pub state: TaskState,
    pub completion: Option<Completion>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RequirementProgress {
    #[serde(flatten)]
    pub requirement: Requirement,
    pub status: RequirementStatus,
    pub completion: Option<Completion>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Progress {
    pub tasks: Vec<TaskProgress>,
    pub requirements: Vec<RequirementProgress>,
}

impl Progress {
    /// Human-readable reasons the plan cannot be accepted yet; empty when it can.
    pub fn unmet(&self, slug: &str) -> Vec<String> {
        let requirements = self
            .requirements
            .iter()
            .filter(|item| item.status == RequirementStatus::Unsatisfied)
            .map(|item| match item.requirement.human {
                true => format!("requirement {} needs human evidence: a human runs yoke plan accept {slug} {} --evidence '<what was verified>'", item.requirement.id, item.requirement.id),
                false => format!("requirement {} needs passing checks: {}", item.requirement.id, item.requirement.checks.join(", ")),
            });
        let tasks = self
            .tasks
            .iter()
            .filter(|item| item.state != TaskState::Done)
            .map(|item| match item.task.owner {
                Actor::Human => format!("task {} is not done; a human runs yoke plan task done {slug} {}", item.task.id, item.task.id),
                Actor::Agent => format!("task {} is not done; run yoke plan task done {slug} {} --evidence '<what changed>'", item.task.id, item.task.id),
            });
        requirements.chain(tasks).collect()
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

/// Reject malformed ids, dangling references and dependency cycles at parse time.
pub(super) fn validate(header: &PlanHeader) -> Result<()> {
    if header.requirements.is_empty() && header.tasks.is_empty() {
        return Ok(());
    }
    ensure!(
        header.checks.iter().all(|check| check.name != RUN),
        "check name {RUN} is reserved when a plan declares requirements or tasks"
    );
    let mut linkable: BTreeSet<&str> = header
        .checks
        .iter()
        .filter(|check| check.blocking)
        .map(|check| check.name.as_str())
        .collect();
    if header.review {
        linkable.insert("review");
    }
    let mut requirements = BTreeSet::new();
    for requirement in &header.requirements {
        let id = &requirement.id;
        ensure!(
            valid_id(id) && requirements.insert(id.as_str()),
            "requirement ids must be unique, 1-64 lowercase letters, digits, or hyphens: {id:?}"
        );
        ensure!(
            !requirement.text.trim().is_empty(),
            "requirement {id} needs text"
        );
        ensure!(
            requirement.human == requirement.checks.is_empty(),
            "requirement {id} must name blocking checks or set human = true, not both"
        );
        for check in &requirement.checks {
            ensure!(
                linkable.contains(check.as_str()),
                "requirement {id} links {check:?}, which is not a blocking check or required review"
            );
        }
    }
    let mut tasks = BTreeSet::new();
    for task in &header.tasks {
        let id = &task.id;
        ensure!(
            valid_id(id) && tasks.insert(id.as_str()),
            "task ids must be unique, 1-64 lowercase letters, digits, or hyphens: {id:?}"
        );
        ensure!(!task.title.trim().is_empty(), "task {id} needs a title");
        for requirement in &task.requirements {
            ensure!(
                requirements.contains(requirement.as_str()),
                "task {id} references unknown requirement {requirement:?}"
            );
        }
    }
    for task in &header.tasks {
        for dependency in &task.depends_on {
            ensure!(
                tasks.contains(dependency.as_str()) && dependency != &task.id,
                "task {} depends on unknown or self task {dependency:?}",
                task.id
            );
        }
    }
    // Kahn's algorithm: a task never released by its dependencies sits on a cycle.
    let mut waiting: BTreeMap<&str, usize> = BTreeMap::new();
    let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for task in &header.tasks {
        let unique: BTreeSet<&str> = task.depends_on.iter().map(String::as_str).collect();
        waiting.insert(&task.id, unique.len());
        for dependency in unique {
            dependents.entry(dependency).or_default().push(&task.id);
        }
    }
    let mut ready: Vec<&str> = waiting
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(id, _)| *id)
        .collect();
    let mut released = 0;
    while let Some(id) = ready.pop() {
        released += 1;
        for dependent in dependents.get(id).into_iter().flatten() {
            let count = waiting.get_mut(dependent).expect("validated task");
            *count -= 1;
            if *count == 0 {
                ready.push(dependent);
            }
        }
    }
    ensure!(
        released == header.tasks.len(),
        "task dependencies contain a cycle"
    );
    Ok(())
}

fn definition<T: Serialize>(item: &T) -> Result<String> {
    Ok(files::hash(serde_json::to_string(item)?.as_bytes()))
}

type Completions = BTreeMap<(String, String), Completion>;

/// Completions keyed by kind and item id, only for the exact definitions now approved.
fn completions(store: &Store, plan_id: i64, header: &PlanHeader) -> Result<Completions> {
    let mut wanted = BTreeMap::new();
    for task in &header.tasks {
        wanted.insert(("task".to_owned(), task.id.clone()), definition(task)?);
    }
    for requirement in &header.requirements {
        wanted.insert(
            ("requirement".to_owned(), requirement.id.clone()),
            definition(requirement)?,
        );
    }
    let mut statement = store.connection().prepare(
        "SELECT kind,item,definition,ts,actor,evidence FROM plan_completions WHERE plan_id=?1",
    )?;
    let rows = statement.query_map([plan_id], |row| {
        Ok((
            (row.get::<_, String>(0)?, row.get::<_, String>(1)?),
            row.get::<_, String>(2)?,
            Completion {
                ts: row.get(3)?,
                actor: row.get(4)?,
                evidence: row.get(5)?,
            },
        ))
    })?;
    let mut found = Completions::new();
    for row in rows {
        let (key, digest, completion) = row?;
        if wanted.get(&key) == Some(&digest) {
            found.insert(key, completion);
        }
    }
    Ok(found)
}

fn evaluate(header: &PlanHeader, done: &Completions, passed: &BTreeSet<String>) -> Progress {
    let finished = |id: &str| done.contains_key(&("task".to_owned(), id.to_owned()));
    let tasks = header
        .tasks
        .iter()
        .map(|task| TaskProgress {
            state: if finished(&task.id) {
                TaskState::Done
            } else if task.depends_on.iter().all(|id| finished(id)) {
                TaskState::Ready
            } else {
                TaskState::Blocked
            },
            completion: done.get(&("task".to_owned(), task.id.clone())).cloned(),
            task: task.clone(),
        })
        .collect();
    let requirements = header
        .requirements
        .iter()
        .map(|requirement| {
            let completion = done
                .get(&("requirement".to_owned(), requirement.id.clone()))
                .cloned();
            let status = if requirement.human {
                match completion {
                    Some(_) => RequirementStatus::Human,
                    None => RequirementStatus::Unsatisfied,
                }
            } else if requirement.checks.iter().all(|name| passed.contains(name)) {
                RequirementStatus::Check
            } else {
                RequirementStatus::Unsatisfied
            };
            RequirementProgress {
                requirement: requirement.clone(),
                status,
                completion,
            }
        })
        .collect();
    Progress {
        tasks,
        requirements,
    }
}

/// Header whose tasks bind the plan: the frozen approval, or the editable draft before it.
fn binding_header(plan: &PlanRecord, content: &str) -> Option<(String, PlanHeader)> {
    match &plan.approved_header {
        Some(raw) => files::parse(&format!("+++\n{raw}+++\n"))
            .ok()
            .map(|(_, header)| (raw.clone(), header)),
        None => files::parse(content).ok(),
    }
}

/// Progress against the binding header; check-linked requirements use the latest attempt
/// recorded for that same approved header.
pub(super) fn progress(
    store: &Store,
    plan: &PlanRecord,
    content: &str,
    runs: &[CheckRun],
) -> Result<Progress> {
    let Some((raw, header)) = binding_header(plan, content) else {
        return Ok(Progress::default());
    };
    let digest = files::hash(raw.as_bytes());
    let passed = runs
        .iter()
        .rev()
        .find(|run| run.name == RUN)
        .filter(|run| run.findings["header"] == digest.as_str())
        .and_then(|run| run.findings["passed_checks"].as_array())
        .map(|names| {
            names
                .iter()
                .filter_map(|name| name.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    Ok(evaluate(
        &header,
        &completions(store, plan.id, &header)?,
        &passed,
    ))
}

/// Evaluate one check attempt: `None` when the header declares neither requirements nor tasks.
/// Returns whether everything is accepted, the evidence JSON, and unmet reasons.
pub(super) fn attempt(
    store: &Store,
    plan: &PlanRecord,
    header: &PlanHeader,
    runs: &[CheckRun],
    valid: bool,
) -> Result<Option<(bool, serde_json::Value, String)>> {
    if header.requirements.is_empty() && header.tasks.is_empty() {
        return Ok(None);
    }
    let raw = plan
        .approved_header
        .as_deref()
        .context("plan has not been approved")?;
    let passed: BTreeSet<String> = match valid {
        true => runs
            .iter()
            .filter(|run| run.result == "pass")
            .map(|run| run.name.clone())
            .collect(),
        false => BTreeSet::new(),
    };
    let progress = evaluate(header, &completions(store, plan.id, header)?, &passed);
    let unmet = progress.unmet(&plan.slug);
    let summary = match unmet.is_empty() {
        true => format!(
            "{} requirements satisfied and {} tasks done",
            progress.requirements.len(),
            progress.tasks.len()
        ),
        false => unmet.join("\n"),
    };
    let findings = serde_json::json!({
        "header": files::hash(raw.as_bytes()),
        "passed_checks": passed,
        "requirements": progress.requirements.iter().map(|item| serde_json::json!({"id": item.requirement.id, "status": item.status})).collect::<Vec<_>>(),
        "tasks": progress.tasks.iter().map(|item| serde_json::json!({"id": item.task.id, "state": item.state})).collect::<Vec<_>>(),
    });
    Ok(Some((unmet.is_empty(), findings, summary)))
}

/// The approved definition of one task, for callers deciding whether a human must act.
pub fn task(store: &Store, project: &Path, slug: &str, id: &str) -> Result<Task> {
    let plan = get(store, project, slug)?;
    let header = files::approved(&plan)?;
    header
        .tasks
        .into_iter()
        .find(|task| task.id == id)
        .with_context(|| format!("plan {slug} has no approved task {id:?}"))
}

fn open_for_progress(plan: &PlanRecord) -> Result<PlanHeader> {
    ensure!(
        matches!(plan.state.as_str(), "building" | "waiting" | "stalled"),
        "progress is recorded only while a plan is building, waiting, or stalled"
    );
    files::approved(plan)
}

/// Mark an approved task done once its dependencies are done. `actor` is human only when the
/// caller authenticated a human; human-owned tasks refuse any other actor. Repeating is a no-op.
pub fn complete_task(
    store: &Store,
    project: &Path,
    slug: &str,
    id: &str,
    evidence: Option<&str>,
    actor: Actor,
) -> Result<TaskProgress> {
    let plan = get(store, project, slug)?;
    let header = open_for_progress(&plan)?;
    let task = header
        .tasks
        .iter()
        .find(|task| task.id == id)
        .with_context(|| format!("plan {slug} has no approved task {id:?}"))?;
    ensure!(
        task.owner == Actor::Agent || actor == Actor::Human,
        "task {id} is owned by a human; a human must run yoke plan task done {slug} {id} in their own terminal"
    );
    let done = completions(store, plan.id, &header)?;
    if !done.contains_key(&("task".to_owned(), id.to_owned())) {
        let pending: Vec<_> = task
            .depends_on
            .iter()
            .filter(|dependency| !done.contains_key(&("task".to_owned(), (*dependency).clone())))
            .map(String::as_str)
            .collect();
        ensure!(
            pending.is_empty(),
            "task {id} depends on unfinished tasks: {}",
            pending.join(", ")
        );
        record(
            store,
            &plan,
            "task",
            id,
            &definition(task)?,
            actor,
            evidence,
        )?;
    }
    let progress = evaluate(
        &header,
        &completions(store, plan.id, &header)?,
        &BTreeSet::new(),
    );
    progress
        .tasks
        .into_iter()
        .find(|item| item.task.id == id)
        .context("task disappeared")
}

/// Human action only: record evidence that a `human = true` requirement is met.
pub fn accept_requirement(
    store: &Store,
    project: &Path,
    slug: &str,
    id: &str,
    evidence: &str,
) -> Result<RequirementProgress> {
    ensure!(
        !evidence.trim().is_empty(),
        "human evidence must not be empty"
    );
    let plan = get(store, project, slug)?;
    let header = open_for_progress(&plan)?;
    let requirement = header
        .requirements
        .iter()
        .find(|requirement| requirement.id == id)
        .with_context(|| format!("plan {slug} has no approved requirement {id:?}"))?;
    if !requirement.human {
        bail!("requirement {id} is satisfied by its checks, not by human evidence");
    }
    let done = completions(store, plan.id, &header)?;
    if !done.contains_key(&("requirement".to_owned(), id.to_owned())) {
        record(
            store,
            &plan,
            "requirement",
            id,
            &definition(requirement)?,
            Actor::Human,
            Some(evidence),
        )?;
    }
    let progress = evaluate(
        &header,
        &completions(store, plan.id, &header)?,
        &BTreeSet::new(),
    );
    progress
        .requirements
        .into_iter()
        .find(|item| item.requirement.id == id)
        .context("requirement disappeared")
}

fn record(
    store: &Store,
    plan: &PlanRecord,
    kind: &str,
    item: &str,
    definition: &str,
    actor: Actor,
    evidence: Option<&str>,
) -> Result<()> {
    let ts = now()?;
    let evidence = evidence
        .map(|text| redact_payload(text.trim(), 4096))
        .filter(|text| !text.is_empty());
    store.transaction(|tx| {
        // The plan must still be the revision whose approved header was just validated.
        let current: Option<(i64, String)> = tx
            .query_row(
                "SELECT revision,state FROM plans WHERE id=?1",
                [plan.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        ensure!(
            current == Some((plan.revision, plan.state.clone())),
            "plan changed concurrently; reload before retrying"
        );
        tx.execute(
            "INSERT OR IGNORE INTO plan_completions(plan_id,kind,item,definition,ts,actor,evidence) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![plan.id, kind, item, definition, ts, actor.as_str(), evidence],
        )?;
        storage::fact(
            tx,
            plan,
            ts,
            "plan_progress",
            &serde_json::json!({"plan": plan.slug, "kind": kind, "item": item, "actor": actor.as_str()}).to_string(),
        )
    })
}
