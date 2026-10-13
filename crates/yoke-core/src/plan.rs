//! Database-owned workflow state and human-approved plan checks.
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};

use crate::store::Store;

mod checks;
mod files;
mod storage;
pub use checks::{check, record_review, record_review_with_fingerprint};
pub use files::fingerprint;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanHeader {
    pub title: String,
    #[serde(default)]
    pub review: bool,
    #[serde(default, rename = "check")]
    pub checks: Vec<PlanCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanCheck {
    pub name: String,
    #[serde(rename = "run")]
    pub command: String,
    #[serde(default = "blocking_default")]
    pub blocking: bool,
    #[serde(default = "timeout_default")]
    pub timeout: u64,
    #[serde(default)]
    pub artifacts: Vec<String>,
}
fn blocking_default() -> bool {
    true
}
fn timeout_default() -> u64 {
    30
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanRecord {
    pub id: i64,
    pub project: String,
    pub slug: String,
    pub file: String,
    pub state: String,
    pub approved_header: Option<String>,
    pub revision: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TransitionRecord {
    pub id: i64,
    pub plan_id: i64,
    pub ts: i64,
    pub from_state: String,
    pub to_state: String,
    pub actor: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckRun {
    pub id: i64,
    pub plan_id: i64,
    pub ts: i64,
    pub name: String,
    pub command: String,
    pub exit_code: Option<i32>,
    pub output_tail: String,
    pub fingerprint: String,
    pub blocking: bool,
    pub ms: u64,
    pub result: String,
    pub findings: serde_json::Value,
    pub artifacts: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct PlanDetail {
    pub plan: PlanRecord,
    pub content: String,
    pub transitions: Vec<TransitionRecord>,
    pub checks: Vec<CheckRun>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreparedApproval {
    pub plan: PlanRecord,
    pub header: String,
    pub digest: String,
}

#[derive(Debug, Serialize)]
pub struct CheckSummary {
    pub plan: PlanRecord,
    pub passed: bool,
    pub fingerprint_before: String,
    pub fingerprint_after: String,
    pub checks: Vec<CheckRun>,
}

/// Create a new file without replacing an existing plan or trusting its commands.
pub fn create(
    store: &Store,
    project: &Path,
    plans_dir: &str,
    slug: &str,
    title: &str,
) -> Result<PlanRecord> {
    files::validate_slug(slug)?;
    ensure!(!title.trim().is_empty(), "plan title must not be empty");
    let root = files::project(project)?;
    ensure!(
        storage::find(store, &root, slug)?.is_none(),
        "plan already exists"
    );
    let path = files::new_path(&root, plans_dir, slug)?;
    let header = PlanHeader {
        title: title.into(),
        review: false,
        checks: vec![PlanCheck {
            name: "diff".into(),
            command: "git diff --check".into(),
            blocking: true,
            timeout: 30,
            artifacts: Vec::new(),
        }],
    };
    let content = format!(
        "+++\n{}+++\n\n## Goal\n\nDescribe the intended result.\n",
        toml::to_string(&header)?
    );
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| format!("creating plan {}", path.display()))?;
    file.write_all(content.as_bytes())?;
    file.sync_all()?;
    storage::insert(store, &root, slug, &path)
}

pub fn list(store: &Store, project: &Path) -> Result<Vec<PlanRecord>> {
    storage::list(store, &files::project(project)?)
}

pub fn get(store: &Store, project: &Path, slug: &str) -> Result<PlanRecord> {
    files::validate_slug(slug)?;
    storage::find(store, &files::project(project)?, slug)?.context("plan not found")
}

pub fn detail(store: &Store, project: &Path, slug: &str) -> Result<PlanDetail> {
    let plan = get(store, project, slug)?;
    let content = files::read(&plan)?;
    let transitions = storage::transitions(store, plan.id)?;
    let checks = storage::checks(store, plan.id)?;
    Ok(PlanDetail {
        plan,
        content,
        transitions,
        checks,
    })
}

/// Capture the exact header the caller will display for human confirmation.
pub fn prepare_approval(store: &Store, project: &Path, slug: &str) -> Result<PreparedApproval> {
    let plan = get(store, project, slug)?;
    ensure!(
        plan.state != "checking" && plan.state != "done",
        "cannot approve a checking or completed plan"
    );
    let content = files::read(&plan)?;
    let (header, _) = files::parse(&content)?;
    Ok(PreparedApproval {
        digest: files::hash(header.as_bytes()),
        header,
        plan,
    })
}

/// Freeze the displayed header; the caller must authenticate and confirm the human action.
pub fn approve(store: &Store, prepared: &PreparedApproval) -> Result<PlanRecord> {
    ensure!(
        prepared.plan.state != "checking" && prepared.plan.state != "done",
        "cannot approve a checking or completed plan"
    );
    ensure!(
        files::hash(prepared.header.as_bytes()) == prepared.digest,
        "approval digest mismatch"
    );
    let (header, _) = files::parse(&files::read(&prepared.plan)?)?;
    ensure!(
        header == prepared.header,
        "plan header changed after it was displayed; review it again"
    );
    storage::transition(
        store,
        &prepared.plan,
        "approved",
        "human",
        None,
        Some(&header),
        None,
    )
}

pub fn start(store: &Store, project: &Path, slug: &str, session: &str) -> Result<PlanRecord> {
    ensure!(!session.trim().is_empty(), "session is required");
    let plan = get(store, project, slug)?;
    ensure!(plan.state == "approved", "only an approved plan can start");
    files::approved(&plan)?;
    storage::transition(store, &plan, "building", "agent", None, None, Some(session))
}

pub fn ask(store: &Store, project: &Path, slug: &str, question: &str) -> Result<PlanRecord> {
    ensure!(!question.trim().is_empty(), "question must not be empty");
    let plan = get(store, project, slug)?;
    ensure!(plan.state == "building", "only a building plan can wait");
    storage::transition(store, &plan, "waiting", "agent", Some(question), None, None)
}

/// Human action only: CLI confirmation, authenticated console, or a real user prompt.
pub fn resume(store: &Store, project: &Path, slug: &str) -> Result<PlanRecord> {
    let plan = get(store, project, slug)?;
    ensure!(
        matches!(plan.state.as_str(), "waiting" | "stalled"),
        "only a waiting or stalled plan can resume"
    );
    files::approved(&plan)?;
    storage::transition(store, &plan, "building", "human", None, None, None)
}

pub fn active(store: &Store, project: &Path, session: &str) -> Result<Option<PlanRecord>> {
    storage::active(store, &files::project(project)?, session)
}

fn now() -> Result<i64> {
    Ok(i64::try_from(
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
    )?)
}

#[cfg(test)]
mod tests;
