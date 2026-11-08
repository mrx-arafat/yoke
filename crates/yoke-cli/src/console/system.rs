//! System: logs, backups, retention, background jobs, export and health.
//! Owner: parity-system (Pilot parity fleet). Page stub wired by the orchestrator.
use super::{actions::Outcome, render, Console};
use anyhow::Result;
use serde_json::Value;
use std::collections::BTreeMap;
use yoke_core::store::Store;

pub(super) fn page(
    _store: &Store,
    _c: &Console,
    _params: &BTreeMap<String, String>,
) -> Result<String> {
    let mut out = render::heading(
        "system",
        "Logs, backups, retention, background jobs and health.",
    );
    out.push_str(&render::empty(
        "Not built yet",
        "This page is being built.",
        "yoke console",
    ));
    out.push_str("</main>");
    Ok(out)
}

/// Handle a delegated console action. Return `Outcome::Confirm(message)` for a step that needs
/// the human to confirm; the action is then re-run with `confirmed = true` after they do.
pub(super) fn act(
    _store: &Store,
    _c: &Console,
    _name: &str,
    _params: &Value,
    _confirmed: bool,
) -> Result<Outcome> {
    anyhow::bail!("Unknown action")
}
