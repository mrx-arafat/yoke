use super::{content, random_token, render::escape, Console};
use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::Read,
    time::{Duration, Instant},
};
use yoke_core::{
    memory::{self, MemorySnapshot},
    plan::{self, PreparedApproval},
    store::Store,
};

#[derive(Default)]
pub(super) struct Actions {
    approvals: HashMap<String, (Instant, PreparedApproval)>,
    forgotten: HashMap<String, (Instant, MemorySnapshot)>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Action {
    action: String,
    slug: Option<String>,
    id: Option<i64>,
    value: Option<bool>,
    ticket: Option<String>,
}

impl Actions {
    pub(super) fn apply(&mut self, store: &Store, c: &Console, action: Action) -> Result<Value> {
        self.approvals
            .retain(|_, (at, _)| at.elapsed() < Duration::from_secs(300));
        self.forgotten
            .retain(|_, (at, _)| at.elapsed() < Duration::from_secs(30));
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
                let html = format!(
                    r#"<div class="confirmation"><h3>Approve this exact contract?</h3><p>This freezes the displayed header. A changed header or plan revision cancels approval. Approval cannot be undone.</p><pre><code>{}</code></pre><p class="muted">SHA-256: <code>{}</code></p><div class="actions"><button data-action="approve" data-ticket="{ticket}">Confirm approval</button><button class="quiet" data-cancel>Cancel</button></div></div>"#,
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
                        let ticket = random_token()?;
                        self.forgotten
                            .insert(ticket.clone(), (Instant::now(), snapshot));
                        Ok(
                            json!({"data":{"message":"Memory forgotten. Undo is available for 30 seconds.","undo":ticket}}),
                        )
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
    let path = detail
        .checks
        .iter()
        .find(|v| v.id == check_id)
        .context("Unknown check")?
        .artifacts
        .get(index)
        .context("Unknown screenshot")?;
    let canonical = std::path::Path::new(path).canonicalize()?;
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
