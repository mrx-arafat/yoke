//! Memory, session and usage actions (memory., session., usage.) for the console, and the Memory
//! page they serve.
use super::{actions::Outcome, Console};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use yoke_core::{config, memory, store::Store};

#[path = "activity.rs"]
pub(super) mod activity;
#[path = "memory_page.rs"]
mod memory_page;

/// The Memory page (`/memory`), reached through `views::memories`.
pub(super) fn memory_page(
    store: &Store,
    c: &Console,
    params: &BTreeMap<String, String>,
) -> Result<String> {
    memory_page::page(store, c, params)
}

pub(super) fn act(
    store: &Store,
    c: &Console,
    name: &str,
    params: &Value,
    confirmed: bool,
) -> Result<Outcome> {
    match name {
        "memory.digest" => {
            let session = params
                .get("session")
                .and_then(Value::as_str)
                .context("Missing session")?;
            digest_now(store, c, session, confirmed, now()?, &mut || {
                crate::jobs::spawn_worker(&c.home)
            })
        }
        _ => bail!("Unknown action"),
    }
}

fn now() -> Result<i64> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64)
}

/// What "Digest now" sends, and to whom, in the words the confirmation shows.
pub(super) struct DigestPlan {
    pub agent: String,
    pub model: Option<String>,
    pub pending: usize,
    pub pending_chars: usize,
}

/// Why a digest cannot run for this project now; `None` when it can.
pub(super) fn digest_blocker(loaded: &config::LoadedConfig) -> Option<&'static str> {
    (!loaded.config.memory.digest).then_some(
        "Digest is off. A person turns it on by adding digest = true under [memory] in yoke.toml or ~/.yoke/config.toml and running yoke trust in their own terminal.",
    )
}

fn plan_digest(store: &Store, c: &Console, session: &str) -> Result<DigestPlan> {
    ensure!(
        !session.is_empty() && session.len() <= 512 && !session.contains('\0'),
        "Choose a session from the Digest list"
    );
    let loaded = config::load_trusted(store, &c.project, &c.home)?;
    if let Some(blocker) = digest_blocker(&loaded) {
        bail!("{blocker}");
    }
    let project = c.project.to_string_lossy();
    let state = memory::digest_state(store, &project, session)?
        .filter(|state| state.pending > 0)
        .context("This session has no pending facts in this project")?;
    let agent = loaded.config.memory.digest_agent.clone();
    Ok(DigestPlan {
        model: loaded
            .config
            .memory
            .digest_models
            .get(&agent)
            .map(str::to_owned),
        agent,
        pending: state.pending,
        pending_chars: state.pending_chars,
    })
}

/// The exact confirmation text: what leaves this computer, where it goes, and what changes.
pub(super) fn digest_message(session: &str, plan: &DigestPlan) -> String {
    let model = plan
        .model
        .as_deref()
        .map(|model| format!(" with model {model}"))
        .unwrap_or_default();
    format!(
        "Send {} pending fact{} ({} characters of recorded tool activity, secrets already masked) from session {session} to the {} CLI on this computer{model}, which sends them to its model provider. Each fact goes as recorded (id, identity hash, kind, time, session id, this project's path and detail) with Yoke's fixed summarizing instruction, in batches of at most 100 facts or 128 KiB. Facts recorded before the job runs are included; no transcript is sent. The reply becomes digest memories linked to those facts, and the facts are marked digested. It runs as a background job you can follow with yoke jobs.",
        plan.pending,
        if plan.pending == 1 { "" } else { "s" },
        plan.pending_chars,
        plan.agent
    )
}

/// "Digest now": the first call only describes what would be sent; the confirmed call queues the
/// digest job and starts the worker. Disabled digests and sessions without pending facts fail.
fn digest_now(
    store: &Store,
    c: &Console,
    session: &str,
    confirmed: bool,
    now: i64,
    start_worker: &mut dyn FnMut() -> Result<()>,
) -> Result<Outcome> {
    let plan = plan_digest(store, c, session)?;
    if !confirmed {
        return Ok(Outcome::Confirm(digest_message(session, &plan)));
    }
    let project = c.project.to_str().context("project path must be UTF-8")?;
    if !crate::memory::queue_digest(store, project, session, now)? {
        bail!(
            "A digest of this session started less than 30 seconds ago; wait for it, then refresh"
        );
    }
    start_worker()?;
    Ok(Outcome::Done(json!({
        "message": format!("Digest queued for {} pending fact{}; new memories appear here when the {} CLI answers.", plan.pending, if plan.pending == 1 { "" } else { "s" }, plan.agent)
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A console over a temporary home and project; `enabled` trusts `digest = true` for it.
    pub(super) fn console(enabled: bool) -> (tempfile::TempDir, Store, Console) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (home, project) = (root.join("home"), root.join("project"));
        std::fs::create_dir_all(home.join(".yoke")).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        let store = Store::open(home.join(".yoke/state.db")).unwrap();
        if enabled {
            std::fs::write(
                project.join("yoke.toml"),
                "[memory]\ndigest = true\ndigest_agent = \"codex\"\n",
            )
            .unwrap();
            yoke_core::trust::prepare_project(&store, &project)
                .unwrap()
                .commit(&store)
                .unwrap();
        }
        let c = Console {
            home,
            project,
            database: root.join("unused.db"),
            token: "token".into(),
            csrf: "csrf".into(),
            host: "127.0.0.1:1".into(),
        };
        (temp, store, c)
    }

    #[test]
    fn digest_now_says_what_it_sends_and_queues_only_after_confirmation() {
        let (_temp, store, c) = console(true);
        let project = c.project.to_string_lossy().into_owned();
        memory::record_fact(&store, "s1", &project, "file_write", "Edit: abcd", 10).unwrap();
        memory::record_fact(&store, "s1", &project, "denial", "blocked", 11).unwrap();
        let started = std::cell::Cell::new(0);
        let mut start = || {
            started.set(started.get() + 1);
            Ok(())
        };
        let Outcome::Confirm(message) =
            digest_now(&store, &c, "s1", false, 100, &mut start).unwrap()
        else {
            panic!("the first call must ask for confirmation");
        };
        assert!(
            message.contains("Send 2 pending facts (17 characters"),
            "{message}"
        );
        assert!(
            message.contains("to the codex CLI on this computer"),
            "{message}"
        );
        assert!(message.contains("no transcript is sent"), "{message}");
        assert!(crate::memory::digest_job(&store, &project, "s1")
            .unwrap()
            .is_none());
        assert_eq!(started.get(), 0);

        let Outcome::Done(data) = digest_now(&store, &c, "s1", true, 100, &mut start).unwrap()
        else {
            panic!("the confirmed call must finish");
        };
        assert!(data["message"]
            .as_str()
            .unwrap()
            .contains("Digest queued for 2"));
        let job = crate::memory::digest_job(&store, &project, "s1")
            .unwrap()
            .unwrap();
        assert_eq!((job.state.as_str(), started.get()), ("queued", 1));
        // A second confirmation inside the claim window queues nothing new.
        assert!(digest_now(&store, &c, "s1", true, 110, &mut start).is_err());
        assert!(digest_now(&store, &c, "other", false, 120, &mut start).is_err());
    }

    #[test]
    fn digest_now_refuses_while_digest_is_off() {
        let (_temp, store, c) = console(false);
        let project = c.project.to_string_lossy().into_owned();
        memory::record_fact(&store, "s1", &project, "denial", "blocked", 10).unwrap();
        let error = digest_now(&store, &c, "s1", true, 100, &mut || Ok(()))
            .err()
            .unwrap();
        assert!(error.to_string().contains("Digest is off"), "{error}");
        assert!(crate::memory::digest_job(&store, &project, "s1")
            .unwrap()
            .is_none());
    }
}
