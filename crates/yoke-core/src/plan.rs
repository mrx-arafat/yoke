//! Database-owned workflow state and human-approved plan checks.
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};

use crate::store::Store;

mod acceptance;
mod checks;
mod feedback;
mod files;
mod storage;
pub use acceptance::{
    accept_requirement, complete_task, task, Actor, Completion, Progress, Requirement,
    RequirementProgress, RequirementStatus, Task, TaskProgress, TaskState,
};
pub use checks::{check, record_review, record_review_with_fingerprint};
pub use feedback::{anchors, close, comment, reply, validate, Closure, Feedback, Validation};
pub use files::{content_fingerprint, fingerprint};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanHeader {
    pub title: String,
    #[serde(default)]
    pub review: bool,
    #[serde(default, rename = "check")]
    pub checks: Vec<PlanCheck>,
    /// Acceptance conditions, each satisfied by blocking checks or by human evidence.
    #[serde(default, rename = "requirement", skip_serializing_if = "Vec::is_empty")]
    pub requirements: Vec<Requirement>,
    #[serde(default, rename = "task", skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<Task>,
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
    /// Commit recorded when the plan started; absent for plans started before it was tracked.
    pub base_ref: Option<String>,
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
    /// SHA-256 of each artifact when the check captured it, keyed by artifact path.
    pub artifact_hashes: BTreeMap<String, String>,
}

/// How a recorded artifact compares with the file now on disk.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EvidenceStatus {
    Verified,
    /// The file exists but its content or type differs from what the check captured.
    Tampered,
    Missing,
    /// The check ran before content hashes were recorded.
    Unrecorded,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvidenceReport {
    pub check_run_id: i64,
    pub path: String,
    pub status: EvidenceStatus,
}

#[derive(Debug, Serialize)]
pub struct PlanDetail {
    pub plan: PlanRecord,
    pub content: String,
    pub transitions: Vec<TransitionRecord>,
    pub checks: Vec<CheckRun>,
    pub evidence: Vec<EvidenceReport>,
    pub progress: Progress,
    pub feedback: Vec<Feedback>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreparedApproval {
    pub plan: PlanRecord,
    pub header: String,
    pub digest: String,
    /// Open advice and comments shown to the approving human; blocking findings refuse earlier.
    pub open_feedback: Vec<Feedback>,
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
        requirements: Vec::new(),
        tasks: Vec::new(),
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
    let evidence = verify(&checks);
    let progress = acceptance::progress(store, &plan, &content, &checks)?;
    let feedback = feedback::list(store, &plan, &content)?;
    Ok(PlanDetail {
        plan,
        content,
        transitions,
        checks,
        evidence,
        progress,
        feedback,
    })
}

/// Compare every artifact a plan's checks captured with the file now on disk.
pub fn verify_evidence(store: &Store, project: &Path, slug: &str) -> Result<Vec<EvidenceReport>> {
    let plan = get(store, project, slug)?;
    Ok(verify(&storage::checks(store, plan.id)?))
}

/// Compare bytes about to be served with the hash recorded when the check captured them.
pub fn evidence_matches(run: &CheckRun, path: &str, bytes: &[u8]) -> EvidenceStatus {
    match run.artifact_hashes.get(path) {
        None => EvidenceStatus::Unrecorded,
        Some(hash) if *hash == files::hash(bytes) => EvidenceStatus::Verified,
        Some(_) => EvidenceStatus::Tampered,
    }
}

fn verify(checks: &[CheckRun]) -> Vec<EvidenceReport> {
    checks
        .iter()
        .flat_map(|run| {
            run.artifacts.iter().map(|path| EvidenceReport {
                check_run_id: run.id,
                path: path.clone(),
                status: files::evidence_status(path, run.artifact_hashes.get(path)),
            })
        })
        .collect()
}

/// Capture the exact header the caller will display for human confirmation.
pub fn prepare_approval(store: &Store, project: &Path, slug: &str) -> Result<PreparedApproval> {
    let plan = get(store, project, slug)?;
    ensure!(
        !matches!(plan.state.as_str(), "checking" | "done" | "cancelled"),
        "cannot approve a checking, completed, or cancelled plan"
    );
    let content = files::read(&plan)?;
    let (header, _) = files::parse(&content)?;
    feedback::ensure_unblocked(store.connection(), &plan)?;
    let open_feedback = feedback::list(store, &plan, &content)?
        .into_iter()
        .filter(|item| item.state.as_deref() == Some("open"))
        .collect();
    Ok(PreparedApproval {
        digest: files::hash(header.as_bytes()),
        header,
        plan,
        open_feedback,
    })
}

/// Freeze the displayed header; the caller must authenticate and confirm the human action.
pub fn approve(store: &Store, prepared: &PreparedApproval) -> Result<PlanRecord> {
    ensure!(
        !matches!(
            prepared.plan.state.as_str(),
            "checking" | "done" | "cancelled"
        ),
        "cannot approve a checking, completed, or cancelled plan"
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
    storage::approve(store, &prepared.plan, &header, |tx| {
        feedback::ensure_unblocked(tx, &prepared.plan)
    })
}

pub fn start(store: &Store, project: &Path, slug: &str, session: &str) -> Result<PlanRecord> {
    ensure!(!session.trim().is_empty(), "session is required");
    let plan = get(store, project, slug)?;
    ensure!(plan.state == "approved", "only an approved plan can start");
    files::approved(&plan)?;
    let base_ref = files::head(Path::new(&plan.project))?;
    storage::start(store, &plan, session, &base_ref)
}

pub fn ask(store: &Store, project: &Path, slug: &str, question: &str) -> Result<PlanRecord> {
    ensure!(!question.trim().is_empty(), "question must not be empty");
    let plan = get(store, project, slug)?;
    ensure!(plan.state == "building", "only a building plan can wait");
    storage::transition(store, &plan, "waiting", "agent", Some(question), None, None)
}

/// Human action only: CLI confirmation, authenticated console, or a real user prompt.
/// A plan left in `checking` resumes only when no live process owns its check; the
/// interrupted check is recorded as invalid evidence.
pub fn resume(store: &Store, project: &Path, slug: &str) -> Result<PlanRecord> {
    let plan = get(store, project, slug)?;
    ensure!(
        matches!(plan.state.as_str(), "waiting" | "stalled" | "checking"),
        "only a waiting, stalled, or interrupted checking plan can resume"
    );
    files::approved(&plan)?;
    if plan.state == "checking" {
        let _owner = files::claim_check(&plan)?
            .context("a live process still owns this plan's check; wait for it to finish")?;
        return storage::recover(store, &plan, &mut checks::interrupted_run(&plan)?);
    }
    storage::transition(store, &plan, "building", "human", None, None, None)
}

/// Human action only: abandon an unfinished plan, record why, and release its session.
pub fn cancel(store: &Store, project: &Path, slug: &str, reason: &str) -> Result<PlanRecord> {
    ensure!(!reason.trim().is_empty(), "cancel reason must not be empty");
    let plan = get(store, project, slug)?;
    ensure!(
        !matches!(plan.state.as_str(), "done" | "cancelled"),
        "only an unfinished plan can be cancelled"
    );
    storage::cancel(store, &plan, reason)
}

/// Human action only: hand a started plan to a replacement session. The approved header,
/// base commit and check evidence are untouched, and the previous session is released.
pub fn reassign(store: &Store, project: &Path, slug: &str, session: &str) -> Result<PlanRecord> {
    ensure!(!session.trim().is_empty(), "session is required");
    let plan = get(store, project, slug)?;
    ensure!(
        matches!(plan.state.as_str(), "building" | "waiting" | "stalled"),
        "only a building, waiting, or stalled plan can be reassigned; recover a checking plan first"
    );
    storage::reassign(store, &plan, session)
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
