//! Pre-approval reviewer findings and anchored human feedback; only a human closes either.
use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};
use serde::Serialize;

use super::{files, get, now, storage, Actor, PlanRecord};
use crate::redact::redact_payload;
use crate::review::PlanSeverity;
use crate::store::Store;

#[derive(Debug, Clone, Serialize)]
pub struct Feedback {
    pub id: i64,
    /// Set on replies; replies carry no state of their own.
    pub parent_id: Option<i64>,
    pub ts: i64,
    /// `human`, `agent`, or `reviewer` for plan-validation findings.
    pub author: String,
    /// `blocking` or `advice` on validation findings; absent on comments and replies.
    pub severity: Option<String>,
    pub anchor: String,
    /// SHA-256 of the plan file the item was written against.
    pub digest: String,
    pub text: String,
    /// `open`, `resolved` or `acknowledged` on top-level items.
    pub state: Option<String>,
    pub resolved_by: Option<String>,
    pub resolved_at: Option<i64>,
    pub note: Option<String>,
    /// Whether the plan file is unchanged since this item was written.
    pub current: bool,
    /// Whether the anchor still exists in the current plan file.
    pub anchored: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Closure {
    Resolved,
    Acknowledged,
}

impl Closure {
    fn as_str(self) -> &'static str {
        match self {
            Self::Resolved => "resolved",
            Self::Acknowledged => "acknowledged",
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Validation {
    pub plan: PlanRecord,
    pub digest: String,
    pub findings: Vec<Feedback>,
    pub blocking: usize,
    pub ms: u64,
}

const COLUMNS: &str =
    "id,parent_id,ts,author,severity,anchor,digest,text,state,resolved_by,resolved_at,note";

fn row(row: &Row<'_>) -> rusqlite::Result<Feedback> {
    Ok(Feedback {
        id: row.get(0)?,
        parent_id: row.get(1)?,
        ts: row.get(2)?,
        author: row.get(3)?,
        severity: row.get(4)?,
        anchor: row.get(5)?,
        digest: row.get(6)?,
        text: row.get(7)?,
        state: row.get(8)?,
        resolved_by: row.get(9)?,
        resolved_at: row.get(10)?,
        note: row.get(11)?,
        current: false,
        anchored: false,
    })
}

/// GitHub-style heading slug restricted to ASCII so anchors can be typed.
fn slug(heading: &str) -> String {
    let mut out = String::new();
    for c in heading.chars().map(|c| c.to_ascii_lowercase()) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if matches!(c, ' ' | '-' | '_') && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_owned()
}

/// Every stable anchor in a plan: `plan`, header ids, and Markdown heading slugs.
pub fn anchors(content: &str) -> Result<BTreeSet<String>> {
    let (raw, header) = files::parse(content)?;
    let mut anchors = BTreeSet::from(["plan".to_owned()]);
    anchors.extend(
        header
            .requirements
            .iter()
            .map(|item| format!("requirement:{}", item.id)),
    );
    anchors.extend(header.tasks.iter().map(|item| format!("task:{}", item.id)));
    anchors.extend(
        header
            .checks
            .iter()
            .map(|item| format!("check:{}", item.name)),
    );
    let opening = if content.starts_with("+++\r\n") { 5 } else { 4 };
    let body = content[opening + raw.len()..]
        .split_once('\n')
        .map_or("", |(_, body)| body);
    let mut fenced = false;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if line.len() - trimmed.len() > 3 {
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        let level = trimmed.bytes().take_while(|byte| *byte == b'#').count();
        if fenced || !(1..=6).contains(&level) {
            continue;
        }
        let text = &trimmed[level..];
        if !(text.is_empty() || text.starts_with([' ', '\t'])) {
            continue;
        }
        let slug = slug(text.trim().trim_end_matches('#'));
        if !slug.is_empty() {
            anchors.insert(format!("heading:{slug}"));
        }
    }
    Ok(anchors)
}

/// All feedback on a plan, oldest first, marked against the current plan file.
pub(super) fn list(store: &Store, plan: &PlanRecord, content: &str) -> Result<Vec<Feedback>> {
    let mut statement = store.connection().prepare(&format!(
        "SELECT {COLUMNS} FROM plan_feedback WHERE plan_id=?1 ORDER BY id"
    ))?;
    let items = statement
        .query_map([plan.id], row)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(mark(items, content))
}

/// Mark each item against the current plan file's digest and anchors.
fn mark(mut items: Vec<Feedback>, content: &str) -> Vec<Feedback> {
    let digest = files::hash(content.as_bytes());
    let anchors = anchors(content).unwrap_or_default();
    for item in &mut items {
        item.current = item.digest == digest;
        item.anchored = anchors.contains(&item.anchor);
    }
    items
}

fn marked(item: Feedback, content: &str) -> Feedback {
    mark(vec![item], content).remove(0)
}

fn text(input: &str) -> Result<String> {
    let text = redact_payload(input.trim(), 4096);
    ensure!(!text.trim().is_empty(), "feedback text must not be empty");
    Ok(text)
}

fn by_id(connection: &Connection, plan: i64, id: i64) -> Result<Option<Feedback>> {
    Ok(connection
        .query_row(
            &format!("SELECT {COLUMNS} FROM plan_feedback WHERE id=?1 AND plan_id=?2"),
            params![id, plan],
            row,
        )
        .optional()?)
}

struct NewItem<'a> {
    parent: Option<i64>,
    author: &'a str,
    severity: Option<&'a str>,
    anchor: &'a str,
    digest: &'a str,
    text: String,
}

fn insert(tx: &Transaction<'_>, plan: &PlanRecord, ts: i64, item: NewItem<'_>) -> Result<Feedback> {
    tx.execute(
        "INSERT INTO plan_feedback(plan_id,parent_id,ts,author,severity,anchor,digest,text,state) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![plan.id, item.parent, ts, item.author, item.severity, item.anchor, item.digest, item.text, item.parent.is_none().then_some("open")],
    )?;
    let id = tx.last_insert_rowid();
    storage::fact(tx, plan, ts, "plan_feedback", &serde_json::json!({"plan": plan.slug, "id": id, "author": item.author, "severity": item.severity, "anchor": item.anchor}).to_string())?;
    by_id(tx, plan.id, id)?.context("feedback disappeared")
}

/// Anchor feedback to a stable plan location; unknown anchors are rejected.
pub fn comment(
    store: &Store,
    project: &Path,
    slug: &str,
    anchor: &str,
    body: &str,
    author: Actor,
) -> Result<Feedback> {
    let plan = get(store, project, slug)?;
    let content = files::read(&plan)?;
    let known = anchors(&content)?;
    ensure!(
        known.contains(anchor),
        "unknown anchor {anchor:?}; use one of: {}",
        known.into_iter().collect::<Vec<_>>().join(", ")
    );
    let item = NewItem {
        parent: None,
        author: author.as_str(),
        severity: None,
        anchor,
        digest: &files::hash(content.as_bytes()),
        text: text(body)?,
    };
    let ts = now()?;
    let item = store.transaction(|tx| insert(tx, &plan, ts, item))?;
    Ok(marked(item, &content))
}

/// Reply to a finding or comment. Anyone may reply; replies never change its state.
pub fn reply(
    store: &Store,
    project: &Path,
    slug: &str,
    parent: i64,
    body: &str,
    author: Actor,
) -> Result<Feedback> {
    let plan = get(store, project, slug)?;
    let item = by_id(store.connection(), plan.id, parent)?
        .with_context(|| format!("plan {slug} has no feedback #{parent}"))?;
    ensure!(
        item.parent_id.is_none(),
        "reply to the original item #{}, not to a reply",
        item.parent_id.unwrap_or_default()
    );
    let content = files::read(&plan)?;
    let reply = NewItem {
        parent: Some(parent),
        author: author.as_str(),
        severity: None,
        anchor: &item.anchor,
        digest: &files::hash(content.as_bytes()),
        text: text(body)?,
    };
    let ts = now()?;
    let item = store.transaction(|tx| insert(tx, &plan, ts, reply))?;
    Ok(marked(item, &content))
}

/// Human action only: resolve or acknowledge an open finding or comment. Repeating is a no-op.
pub fn close(
    store: &Store,
    project: &Path,
    slug: &str,
    id: i64,
    closure: Closure,
    note: Option<&str>,
) -> Result<Feedback> {
    let plan = get(store, project, slug)?;
    let note = note.map(text).transpose()?;
    let ts = now()?;
    let item = store.transaction(|tx| {
        let item = by_id(tx, plan.id, id)?
            .with_context(|| format!("plan {slug} has no feedback #{id}"))?;
        let state = item
            .state
            .as_deref()
            .context("replies have no state; resolve the original item")?;
        if state == closure.as_str() {
            return Ok(item);
        }
        ensure!(state == "open", "feedback #{id} is already {state}");
        tx.execute(
            "UPDATE plan_feedback SET state=?1,resolved_by='human',resolved_at=?2,note=?3 WHERE id=?4 AND state='open'",
            params![closure.as_str(), ts, note, id],
        )?;
        storage::fact(tx, &plan, ts, "plan_feedback", &serde_json::json!({"plan": plan.slug, "id": id, "state": closure.as_str(), "actor": "human"}).to_string())?;
        by_id(tx, plan.id, id)?.context("feedback disappeared")
    })?;
    Ok(match files::read(&plan) {
        Ok(content) => marked(item, &content),
        Err(_) => item,
    })
}

/// Refuse approval while any blocking finding is open, whatever revision it reviewed,
/// so editing the plan cannot hide a finding from the approving human.
pub(super) fn ensure_unblocked(connection: &Connection, plan: &PlanRecord) -> Result<()> {
    let mut statement = connection.prepare(
        "SELECT id,anchor,text FROM plan_feedback WHERE plan_id=?1 AND severity='blocking' AND state='open' ORDER BY id",
    )?;
    let open: Vec<(i64, String, String)> = statement
        .query_map([plan.id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    if open.is_empty() {
        return Ok(());
    }
    let lines: Vec<String> = open
        .iter()
        .map(|(id, anchor, text)| {
            let short: String = text.chars().take(200).collect();
            format!("#{id} {anchor}: {short}")
        })
        .collect();
    bail!(
        "{} blocking validation finding(s) are open:\n{}\nA human resolves each with yoke plan resolve {slug} <id> or accepts the risk with yoke plan acknowledge {slug} <id>",
        open.len(),
        lines.join("\n"),
        slug = plan.slug
    )
}

/// Ask the configured independent reviewer to validate the plan before approval and persist
/// its findings against the exact plan text it reviewed. Reviewer failures persist nothing.
pub fn validate(
    store: &Store,
    project: &Path,
    slug: &str,
    command: &mut Command,
    timeout: Duration,
) -> Result<Validation> {
    let plan = get(store, project, slug)?;
    ensure!(
        !matches!(plan.state.as_str(), "checking" | "done" | "cancelled"),
        "only a plan that can still be approved can be validated"
    );
    let content = files::read(&plan)?;
    let known = anchors(&content)?;
    let digest = files::hash(content.as_bytes());
    let review = crate::review::validate_plan(&plan.project, command, timeout, &content, &known)?;
    let items = review
        .findings
        .iter()
        .map(|finding| {
            Ok(NewItem {
                parent: None,
                author: "reviewer",
                severity: Some(match finding.severity {
                    PlanSeverity::Blocking => "blocking",
                    PlanSeverity::Advice => "advice",
                }),
                anchor: &finding.anchor,
                digest: &digest,
                text: text(&finding.summary)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let ts = now()?;
    let findings = store.transaction(|tx| {
        items
            .into_iter()
            .map(|item| insert(tx, &plan, ts, item))
            .collect::<Result<Vec<_>>>()
    })?;
    let findings = mark(findings, &content);
    Ok(Validation {
        blocking: review
            .findings
            .iter()
            .filter(|finding| finding.severity == PlanSeverity::Blocking)
            .count(),
        plan,
        digest,
        findings,
        ms: review.ms,
    })
}
