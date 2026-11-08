use super::{content, random_token, render::escape, Console};
use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::Read,
    sync::{Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};
use yoke_core::{
    memory::{self, MemorySnapshot},
    plan::{self, PreparedApproval},
    store::Store,
};

/// Console action state, shared by every request thread.
#[derive(Default)]
pub(super) struct Actions(Mutex<State>);

#[derive(Default)]
struct State {
    approvals: HashMap<String, (Instant, PreparedApproval)>,
    forgotten: HashMap<String, (Instant, MemorySnapshot)>,
    /// Delegated actions waiting for the human to confirm: ticket -> (created, action, params).
    confirmations: HashMap<String, (Instant, String, Value)>,
}

/// What a page module's `act` returns for a delegated action (`<prefix>.<verb>`).
#[allow(dead_code)] // Constructed by page modules as they gain actions.
pub(super) enum Outcome {
    /// Finished; the value becomes the response's `data` (include a `message` for the toast).
    Done(Value),
    /// Needs the human to confirm first; the message says exactly what will happen.
    /// The browser gets `data.confirm = {ticket, message}` and confirms with action `confirm`.
    Confirm(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Action {
    action: String,
    slug: Option<String>,
    id: Option<i64>,
    value: Option<bool>,
    ticket: Option<String>,
    ids: Option<Vec<i64>>,
    /// Free-form arguments for delegated page actions.
    #[serde(default)]
    params: Option<Value>,
}

impl Actions {
    pub(super) fn apply(&self, store: &Store, c: &Console, action: Action) -> Result<Value> {
        if action.action == "confirm" || action.action.contains('.') {
            return self.delegate(store, c, action);
        }
        self.lock().apply(store, c, action)
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Route `<prefix>.<verb>` to the page module that owns the prefix. The page action runs
    /// without the lock, so a slow one (a push, a reviewer CLI) never blocks another request.
    fn delegate(&self, store: &Store, c: &Console, action: Action) -> Result<Value> {
        let (name, params, confirmed) = if action.action == "confirm" {
            let ticket = action.ticket.context("Missing confirmation ticket")?;
            let mut state = self.lock();
            state.expire();
            let (_, name, params) = state
                .confirmations
                .remove(&ticket)
                .context("This confirmation expired or was already used; start the action again")?;
            (name, params, true)
        } else {
            (action.action, action.params.unwrap_or(Value::Null), false)
        };
        let outcome = match name.split('.').next().unwrap_or_default() {
            "changes" | "note" => super::changes::act(store, c, &name, &params, confirmed)?,
            "plans" | "prd" | "docs" => super::dashboard::act(store, c, &name, &params, confirmed)?,
            "ext" => super::extensions::act(store, c, &name, &params, confirmed)?,
            "settings" => super::settings::act(store, c, &name, &params, confirmed)?,
            "memory" | "session" | "usage" => {
                super::insights::act(store, c, &name, &params, confirmed)?
            }
            "system" => super::system::act(store, c, &name, &params, confirmed)?,
            _ => anyhow::bail!("Unknown action"),
        };
        match outcome {
            Outcome::Done(data) => Ok(json!({ "data": data })),
            Outcome::Confirm(message) => {
                let mut state = self.lock();
                state.expire();
                ensure!(
                    state.confirmations.len() < 32,
                    "Too many pending confirmations; wait for expiry."
                );
                let ticket = random_token()?;
                state
                    .confirmations
                    .insert(ticket.clone(), (Instant::now(), name, params));
                Ok(json!({ "data": { "confirm": { "ticket": ticket, "message": message } } }))
            }
        }
    }
}

impl State {
    fn expire(&mut self) {
        self.approvals
            .retain(|_, (at, _)| at.elapsed() < Duration::from_secs(300));
        self.forgotten
            .retain(|_, (at, _)| at.elapsed() < Duration::from_secs(30));
        self.confirmations
            .retain(|_, (at, ..)| at.elapsed() < Duration::from_secs(300));
    }

    fn apply(&mut self, store: &Store, c: &Console, action: Action) -> Result<Value> {
        self.expire();
        match action.action.as_str() {
            "prepare-approval" => {
                ensure!(
                    self.approvals.len() < 32,
                    "Too many pending confirmations; wait for expiry."
                );
                let prepared = plan::prepare_approval(
                    store,
                    &c.project,
                    action.slug.as_deref().context("Missing plan")?,
                )?;
                let ticket = random_token()?;
                let feedback: String = prepared
                    .open_feedback
                    .iter()
                    .map(|item| {
                        format!(
                            "<li>#{} <code>{}</code> {}</li>",
                            item.id,
                            escape(&item.anchor),
                            escape(&item.text)
                        )
                    })
                    .collect();
                let feedback = if feedback.is_empty() {
                    feedback
                } else {
                    format!("<h4>Open advice and comments</h4><ul>{feedback}</ul>")
                };
                let html = format!(
                    r#"<div class="confirmation"><h3>Approve this exact contract?</h3><p>This freezes the displayed header. A changed header or plan revision cancels approval. Approval cannot be undone.</p><pre><code>{}</code></pre><p class="muted">SHA-256: <code>{}</code></p>{feedback}<div class="actions"><button data-action="approve" data-ticket="{ticket}">Confirm approval</button><button class="quiet" data-cancel>Cancel</button></div></div>"#,
                    content::highlight(&prepared.header),
                    escape(&prepared.digest)
                );
                self.approvals.insert(ticket, (Instant::now(), prepared));
                Ok(json!({"data":{"confirmation":html}}))
            }
            "approve" => {
                let (_, prepared) = self
                    .approvals
                    .remove(action.ticket.as_deref().context("Missing confirmation")?)
                    .context(
                        "Approval expired or already used; review the current header again.",
                    )?;
                plan::approve(store, &prepared)?;
                Ok(json!({"data":{"message":"Plan approved. The reviewed header is frozen."}}))
            }
            "resume" => {
                plan::resume(
                    store,
                    &c.project,
                    action.slug.as_deref().context("Missing plan")?,
                )?;
                Ok(json!({"data":{"message":"Plan resumed."}}))
            }
            "acknowledge" | "acknowledge-all" => {
                // Acknowledge exactly what was shown, never items that arrived after rendering.
                let ids = match action.action.as_str() {
                    "acknowledge" => vec![action.id.context("Missing notification ID")?],
                    _ => action.ids.context("Missing notification IDs")?,
                };
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_secs() as i64;
                let changed = yoke_core::notifications::acknowledge(
                    store,
                    &c.project,
                    &yoke_core::notifications::Target::Ids(ids),
                    now,
                )?;
                let message = match changed {
                    0 => "Already acknowledged.".to_owned(),
                    1 => "Notification acknowledged.".to_owned(),
                    n => format!("{n} notifications acknowledged."),
                };
                Ok(json!({"data":{"message":message}}))
            }
            "undo-forget" => {
                let ticket = action.ticket.as_deref().context("Missing undo token")?;
                let (_, snapshot) = self
                    .forgotten
                    .get(ticket)
                    .context("Undo expired or already used.")?;
                memory::restore_snapshot(store, snapshot)?;
                self.forgotten.remove(ticket);
                Ok(json!({"data":{"message":"Memory restored, including its source provenance."}}))
            }
            "pin" | "share" | "forget" => {
                let id = action.id.context("Missing memory ID")?;
                let m = memory::get(store, id)?.context("Memory no longer exists.")?;
                ensure!(
                    m.project == c.project.to_string_lossy(),
                    "Memory is outside this project."
                );
                match action.action.as_str() {
                    "pin" => {
                        memory::pin(store, id, action.value.context("Missing pin value")?)?;
                        Ok(json!({"data":{"message":"Memory updated."}}))
                    }
                    "share" => {
                        memory::share(store, id)?;
                        Ok(
                            json!({"data":{"message":"Memory shared. A human must run yoke trust in the terminal before the changed project knowledge takes effect."}}),
                        )
                    }
                    _ => {
                        ensure!(
                            self.forgotten.len() < 32,
                            "Too many pending undo operations; wait 30 seconds."
                        );
                        let snapshot = memory::forget_with_snapshot(store, id)?
                            .context("Memory no longer exists.")?;
                        let message = if snapshot.memory.shared {
                            "Memory forgotten. Undo is available for 30 seconds. The shared entry remains in .yoke/knowledge.md; remove it by hand and run yoke trust in your own terminal to stop sharing it."
                        } else {
                            "Memory forgotten. Undo is available for 30 seconds."
                        };
                        let ticket = random_token()?;
                        self.forgotten
                            .insert(ticket.clone(), (Instant::now(), snapshot));
                        Ok(json!({"data":{"message":message,"undo":ticket}}))
                    }
                }
            }
            _ => anyhow::bail!("Unknown action"),
        }
    }
}

pub(super) fn artifact(
    store: &Store,
    c: &Console,
    params: &std::collections::BTreeMap<String, String>,
) -> Result<(Vec<u8>, &'static str)> {
    let detail = plan::detail(
        store,
        &c.project,
        params.get("slug").context("Missing plan")?,
    )?;
    let check_id = params
        .get("check")
        .context("Missing check")?
        .parse::<i64>()?;
    let index = params
        .get("index")
        .context("Missing screenshot index")?
        .parse::<usize>()?;
    let run = detail
        .checks
        .iter()
        .find(|v| v.id == check_id)
        .context("Unknown check")?;
    let path = run.artifacts.get(index).context("Unknown screenshot")?;
    let canonical = std::path::Path::new(path)
        .canonicalize()
        .context("Screenshot no longer exists on disk.")?;
    ensure!(
        canonical.starts_with(&c.project),
        "Screenshot is outside this project."
    );
    let ext = canonical
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    ensure!(
        ["png", "jpg", "jpeg", "webp"].contains(&ext.as_str()),
        "Unsupported screenshot type."
    );
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    #[cfg(not(unix))]
    anyhow::bail!("Safe screenshot opening is unavailable on this platform.");
    let mut file = options.open(&canonical)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() <= 10 * 1024 * 1024,
        "Screenshot must be a regular file no larger than 10 MiB."
    );
    // Recheck the opened inode against the resolved path to reject replacement races.
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let current = std::fs::metadata(&canonical)?;
        ensure!(
            metadata.dev() == current.dev() && metadata.ino() == current.ino(),
            "Screenshot changed while opening."
        );
    }
    ensure!(
        canonical.canonicalize()?.starts_with(&c.project),
        "Screenshot moved outside this project."
    );
    let mut bytes = Vec::new();
    file.by_ref()
        .take(10 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 10 * 1024 * 1024,
        "Screenshot grew beyond its size limit."
    );
    // Hash the exact bytes being served so a replaced file is never shown as evidence.
    ensure!(
        plan::evidence_matches(run, path, &bytes) != plan::EvidenceStatus::Tampered,
        "Screenshot changed after its check captured it; it is not served as evidence."
    );
    let kind = if ext == "png" && bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if ["jpg", "jpeg"].contains(&ext.as_str()) && bytes.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if ext == "webp" && bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else {
        anyhow::bail!("Screenshot content does not match its image type.")
    };
    Ok((bytes, kind))
}
