//! `systemMessage` is a Codex UI warning; additionalContext and plain stdout are not a HUD.
use anyhow::Result;
use serde_json::{json, Value};
use yoke_core::{notifications, quota, store::Store, timefmt};

pub(super) fn message(
    store: &Store,
    session: &str,
    threshold: u8,
    now: i64,
) -> Result<Option<String>> {
    if session.is_empty() {
        return Ok(None);
    }
    let Some(mut reading) = quota::load(store, "codex")? else {
        return Ok(None);
    };
    let warnings = notifications::quota_warning(store, &reading, threshold, now)?;
    reading.windows = reading
        .windows
        .iter()
        .filter(|w| reading.fresh(w, now))
        .cloned()
        .collect();
    let Some(field) = reading.status_field(now, timefmt::utc_offset_secs(now)) else {
        return Ok(None);
    };
    // Per-session reminders, account-wide warnings: concurrent sessions each get a HUD,
    // but crossing one threshold never creates a notification storm.
    let periodic = store.claim_interval(&format!("quota.display.codex.{session}"), now, 900)?;
    if !warnings.is_empty() {
        return Ok(Some(format!(
            "Yoke Codex quota: {field}. {}",
            warnings.join(" ")
        )));
    }
    if periodic {
        Ok(Some(format!(
            "Yoke Codex quota: {field}. Run yoke usage for recent burn and projections."
        )))
    } else {
        Ok(None)
    }
}

pub(super) fn attach(output: &mut Option<Value>, message: &str) {
    output.get_or_insert_with(|| json!({}))["systemMessage"] = json!(message);
}

#[cfg(test)]
mod tests {
    use super::*;
    use yoke_core::usage;

    #[test]
    fn quota_messages_are_visible_advisory_periodic_and_preserve_denials() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("state.db")).unwrap();
        let path = dir.path().join("rollout.jsonl");
        let at = 1_792_144_800;
        let row = json!({"timestamp":"2026-10-16T10:00:00Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":20,"window_minutes":300,"resets_at":at + 3600}}}});
        std::fs::write(&path, format!("{row}\n")).unwrap();
        usage::ingest(&store, "codex", "s", "/project", &path).unwrap();
        let first = message(&store, "s", 85, at).unwrap().unwrap();
        assert!(first.contains("5h 20%"));
        assert!(message(&store, "s", 85, at + 1).unwrap().is_none());
        assert!(message(&store, "other", 85, at + 1).unwrap().is_some());
        assert!(message(&store, "other", 85, at + 301).unwrap().is_none());
        let mut refreshed = row;
        refreshed["timestamp"] = json!("2026-10-16T10:15:00Z");
        std::fs::write(&path, format!("{refreshed}\n")).unwrap();
        usage::ingest(&store, "codex", "s", "/project", &path).unwrap();
        assert!(message(&store, "s", 85, at + 900).unwrap().is_some());
        assert!(message(&store, "s", 85, at + 901).unwrap().is_none());
        let mut denial = Some(json!({"decision":"block","reason":"check failed"}));
        attach(&mut denial, &first);
        assert_eq!(denial.as_ref().unwrap()["decision"], "block");
        assert_eq!(denial.as_ref().unwrap()["reason"], "check failed");
        assert_eq!(denial.unwrap()["systemMessage"], first);
    }
}
