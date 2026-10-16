//! Read-only status refresh; external wrapping is explicit and bounded.
use std::io::{IsTerminal, Read};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{ensure, Context, Result};
use serde_json::Value;
use yoke_core::engine::execute_bounded;
use yoke_core::store::{StatusSnapshot, Store};

const INPUT_CAP: usize = 64 * 1024;

fn clean(text: &str, limit: usize) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(limit)
        .collect()
}

fn context_percentage(payload: &Value) -> Option<f64> {
    let window = &payload["context_window"];
    if let Some(percent) = window["used_percentage"].as_f64() {
        return (percent.is_finite() && (0.0..=100.0).contains(&percent)).then_some(percent);
    }
    let size = window["context_window_size"].as_u64()?;
    if size == 0 {
        return None;
    }
    let usage = &window["current_usage"];
    let used = usage["input_tokens"]
        .as_u64()?
        .checked_add(usage["cache_creation_input_tokens"].as_u64()?)?
        .checked_add(usage["cache_read_input_tokens"].as_u64()?)?;
    (used <= size).then_some(used as f64 * 100.0 / size as f64)
}

fn render(snapshot: Option<&StatusSnapshot>, payload: &Value) -> String {
    let mode = snapshot
        .and_then(|s| s.mode.as_deref())
        .unwrap_or("unrecorded");
    let mut fields = vec![format!("yoke {}", clean(mode, 24))];
    if let Some(snapshot) = snapshot {
        if let Some(plan) = snapshot.plan_slug.as_deref() {
            fields.push(format!(
                "{}:{}",
                clean(plan, 64),
                clean(snapshot.plan_state.as_deref().unwrap_or("unknown"), 24)
            ));
        }
        if snapshot.plan_state.as_deref() == Some("stalled") {
            fields.push("! stalled".into());
        } else if snapshot.recent_deny {
            fields.push("! recent deny".into());
        }
        if let Some(latest) = snapshot.latest_version.as_deref() {
            if let (Ok(latest_version), Ok(current)) = (
                crate::update::version(latest),
                crate::update::version(env!("CARGO_PKG_VERSION")),
            ) {
                if latest_version > current {
                    fields.push(format!("update {latest}"));
                }
            }
        }
    }
    if let Some(percent) = context_percentage(payload) {
        fields.push(format!("ctx {percent:.0}%"));
    }
    fields.join(" | ")
}

fn refresh(home: &Path, input: &[u8], wrap: Option<&str>) -> Result<String> {
    ensure!(input.len() <= INPUT_CAP, "statusline input exceeds 64 KiB");
    let payload: Value = if input.iter().all(u8::is_ascii_whitespace) {
        Value::Null
    } else {
        serde_json::from_slice(input).context("invalid statusline JSON")?
    };
    let db = home.join(".yoke/state.db");
    let store = match std::fs::metadata(&db) {
        Ok(_) => {
            let store = Store::open_read_only(&db).context("read statusline database")?;
            // A file with no committed schema (e.g. zero bytes) has no state yet, like a missing one.
            (store.schema_version()?.0 > 0).then_some(store)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let cwd = std::env::current_dir()?;
    let session = payload["session_id"].as_str();
    // Session ids identify the project even when the status callback runs in a subdirectory.
    let project = if session.is_some() {
        None
    } else {
        payload["workspace"]["project_dir"]
            .as_str()
            .or_else(|| payload["cwd"].as_str())
            .or_else(|| cwd.to_str())
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_secs()
        .try_into()?;
    let snapshot = store
        .as_ref()
        .map(|store| store.status_snapshot(session, project, now))
        .transpose()?;
    let mut line = render(snapshot.as_ref(), &payload);
    if let Some(wrap) = wrap {
        ensure!(
            std::env::var_os("YOKE_STATUSLINE_WRAPPED").is_none(),
            "recursive statusline wrapper refused"
        );
        ensure!(
            !wrap.trim().is_empty() && wrap.len() <= 4096 && !wrap.contains('\0'),
            "invalid statusline wrapper"
        );
        let output = execute_bounded(
            Command::new("/bin/sh")
                .arg("-c")
                .arg(wrap)
                .env("YOKE_OFF", "1")
                .env("YOKE_STATUSLINE_WRAPPED", "1"),
            input,
            Duration::from_millis(250),
            4096,
        )?;
        ensure!(
            output.success && !output.timed_out,
            "statusline wrapper failed or exceeded 250 ms"
        );
        ensure!(
            !output.stdout_truncated,
            "statusline wrapper output exceeds 4096 bytes"
        );
        let wrapped = clean(&String::from_utf8_lossy(&output.stdout), 512);
        if !wrapped.trim().is_empty() {
            line.push_str(" | ");
            line.push_str(wrapped.trim());
        }
    }
    Ok(line)
}

/// Consume the callback's EOF-terminated input, read existing state, print one line.
pub fn run(wrap: Option<&str>) -> Result<()> {
    let mut input = Vec::new();
    if !std::io::stdin().is_terminal() {
        std::io::stdin()
            .take((INPUT_CAP + 1) as u64)
            .read_to_end(&mut input)?;
    }
    let home = crate::commands::home()?;
    println!("{}", refresh(&home, &input, wrap)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The wrapper has a fixed 250 ms budget that a loaded host can miss once; a wrapper that
    /// really cannot finish fails every attempt.
    fn refresh_wrapped(home: &Path, input: &[u8], wrap: &str) -> String {
        let mut last = None;
        for _ in 0..10 {
            match refresh(home, input, Some(wrap)) {
                Ok(line) => return line,
                Err(error) if error.to_string().contains("exceeded 250 ms") => last = Some(error),
                Err(error) => panic!("{error:#}"),
            }
        }
        panic!("the wrapper missed its 250 ms budget 10 times: {last:?}");
    }

    #[test]
    fn absent_state_does_not_create_database_and_wrapper_receives_identical_stdin() {
        let home = tempfile::tempdir().unwrap();
        let input = br#"{ "context_window": {"used_percentage": 42} }"#;
        let line = refresh_wrapped(home.path(), input, "cat");
        assert_eq!(
            line,
            format!(
                "yoke unrecorded | ctx 42% | {}",
                String::from_utf8_lossy(input)
            )
        );
        assert!(!home.path().join(".yoke").exists());
        assert!(refresh(home.path(), b"{", None).is_err());
        assert!(refresh(home.path(), b"{}", Some("sleep 2")).is_err());
    }

    #[test]
    fn existing_wal_state_is_read_without_changes_and_missing_shm_is_not_created() {
        fn files(directory: &Path) -> std::collections::BTreeMap<std::ffi::OsString, Vec<u8>> {
            std::fs::read_dir(directory)
                .unwrap()
                .map(|entry| {
                    let entry = entry.unwrap();
                    (entry.file_name(), std::fs::read(entry.path()).unwrap())
                })
                .collect()
        }
        let home = tempfile::tempdir().unwrap();
        let directory = home.path().join(".yoke");
        std::fs::create_dir(&directory).unwrap();
        // Hooks truncate the WAL when the last connection closes, so build the state in a
        // scratch directory, copy the live main file with its sidecars while a second
        // connection still holds the WAL open, and read that copy with no connection open.
        let scratch = home.path().join("scratch");
        std::fs::create_dir(&scratch).unwrap();
        let store = Store::open(scratch.join("state.db")).unwrap();
        store
            .set_kv(crate::update::LATEST_VERSION_KEY, "v999.0.0")
            .unwrap();
        let holder = rusqlite::Connection::open(scratch.join("state.db")).unwrap();
        let rows: i64 = holder
            .query_row("SELECT count(*) FROM kv", [], |row| row.get(0))
            .unwrap();
        assert!(rows > 0);
        for name in ["state.db", "state.db-wal", "state.db-shm"] {
            std::fs::copy(scratch.join(name), directory.join(name)).unwrap();
        }
        drop(store);
        drop(holder);
        let before = files(&directory);
        assert!(before.contains_key(std::ffi::OsStr::new("state.db-shm")));
        assert!(!before[std::ffi::OsStr::new("state.db-wal")].is_empty());
        assert_eq!(
            refresh(home.path(), b"{}", None).unwrap(),
            "yoke unrecorded | update v999.0.0"
        );
        assert_eq!(files(&directory), before);

        // Only this disposable fixture's sidecar is removed. A callback must
        // surface unavailable state rather than recreate the WAL index.
        std::fs::remove_file(directory.join("state.db-shm")).unwrap();
        let before = files(&directory);
        assert!(refresh(home.path(), b"{}", None).is_err());
        assert_eq!(files(&directory), before);
    }

    #[test]
    fn renders_snapshot_alerts_context_and_only_newer_valid_updates() {
        let mut snapshot = StatusSnapshot {
            mode: Some("enforce".into()),
            plan_slug: Some("demo\nplan".into()),
            plan_state: Some("stalled".into()),
            recent_deny: true,
            latest_version: Some("v999.0.0".into()),
        };
        let line = render(
            Some(&snapshot),
            &json!({"context_window":{"used_percentage":75}}),
        );
        assert_eq!(
            line,
            "yoke enforce | demoplan:stalled | ! stalled | update v999.0.0 | ctx 75%"
        );
        snapshot.latest_version = Some("../bad".into());
        assert!(!render(Some(&snapshot), &json!({})).contains("update"));
        assert!(context_percentage(&json!({"context_window":{"used_percentage":101}})).is_none());
    }
}
