//! Memory, session and usage actions (memory., session., usage.) for the console.
//! Owner: parity-memory (Pilot parity fleet). Stub wired by the orchestrator.
use super::{actions::Outcome, Console};
use anyhow::Result;
use serde_json::Value;
use yoke_core::store::Store;

pub(super) fn act(
    _store: &Store,
    _c: &Console,
    _name: &str,
    _params: &Value,
    _confirmed: bool,
) -> Result<Outcome> {
    anyhow::bail!("Unknown action")
}
