//! Explicit Codex child usage belongs to the hook's logical parent session.

use super::*;
use serde::de::{IgnoredAny, MapAccess, Visitor};

struct FirstKind<'a>(&'a mut Option<Value>);

impl<'de> Visitor<'de> for FirstKind<'_> {
    type Value = ();

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a transcript record")
    }

    fn visit_map<A: MapAccess<'de>>(
        self,
        mut fields: A,
    ) -> std::result::Result<Self::Value, A::Error> {
        while let Some(key) = fields.next_key::<String>()? {
            if key == "type" {
                // Classification only: the snapshot parser still validates the whole row.
                // Stop here so a legacy first tool-output row can be larger than the bound.
                *self.0 = Some(fields.next_value()?);
                // Returning success here would make serde require the closing brace.
                // Capture the kind out of band and stop this classification-only parse.
                return Err(serde::de::Error::custom("record kind captured"));
            }
            fields.next_value::<IgnoredAny>()?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(super) struct Stream {
    thread: String,
    session: String,
    prefix: String,
    pub(super) cursor_key: String,
}

fn digest(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

/// Inspect a bounded first record without following links or reopening the snapshot.
pub(super) fn identify(
    file: &mut File,
    session: &str,
    limit: u64,
) -> Result<(Option<Stream>, u64)> {
    file.seek(SeekFrom::Start(0))?;
    let mut kind = None;
    let probe = {
        let reader = BufReader::new(file.by_ref().take(limit));
        let mut parser = serde_json::Deserializer::from_reader(reader);
        serde::de::Deserializer::deserialize_map(&mut parser, FirstKind(&mut kind))
    };
    if kind.is_none() {
        match probe {
            Ok(()) => return Ok((None, 0)),
            Err(error) if error.is_eof() && file.metadata()?.len() <= limit => return Ok((None, 0)),
            Err(error) => return Err(error).context("cannot identify usage transcript within the read limit; keep the rollout and report incomplete usage"),
        }
    }
    if kind.as_ref().and_then(Value::as_str) != Some("session_meta") {
        return Ok((None, 0));
    }
    file.seek(SeekFrom::Start(0))?;
    let mut reader = BufReader::new(file.by_ref().take(limit.saturating_add(1)));
    let mut bytes = Vec::new();
    let mut read = 0;
    loop {
        bytes.clear();
        let n = reader.read_until(b'\n', &mut bytes)?;
        read += n as u64;
        ensure!(read <= limit,
            "usage transcript header exceeds the read limit; keep the rollout and report incomplete usage");
        if n == 0 {
            return Ok((None, read));
        }
        if !bytes.iter().all(u8::is_ascii_whitespace) {
            break;
        }
    }
    let row: Line = match serde_json::from_slice(&bytes) {
        Ok(row) => row,
        Err(_) if !bytes.ends_with(b"\n") => return Ok((None, read)),
        Err(error) => {
            return Err(error).context(
                "malformed usage transcript header; keep the rollout and report the error",
            )
        }
    };
    if row.kind != "session_meta" {
        return Ok((None, read));
    }
    let payload = row.payload.unwrap_or_default();
    let thread = payload.id.as_str().context(
        "usage transcript metadata lacks a thread ID; keep the rollout and report incomplete usage",
    )?;
    identifier(thread)?;
    if thread == session {
        ensure!(payload.session_id.is_null() || payload.session_id.as_str() == Some(session),
            "transcript session does not match requested session; use the rollout belonging to this session");
        return Ok((None, read));
    }
    let parent = payload.parent_thread_id.as_str().context(
        "child rollout lacks its parent thread ID; keep the rollout and report incomplete usage",
    )?;
    identifier(parent)?;
    ensure!(payload.session_id.as_str() == Some(session)
        && parent != thread
        && payload.source["subagent"]["thread_spawn"]["parent_thread_id"].as_str() == Some(parent),
        "transcript session does not match requested session; use a rollout with an explicit matching parent session");
    let encoded = serde_json::to_vec(&(session, thread))?;
    let identity = digest(&encoded);
    Ok((
        Some(Stream {
            thread: thread.to_owned(),
            session: session.to_owned(),
            prefix: format!("child-{identity}-"),
            cursor_key: format!("usage.child-cursor.{identity}"),
        }),
        read,
    ))
}

impl Stream {
    pub(super) fn pattern(&self) -> String {
        format!("{}*", self.prefix)
    }

    pub(super) fn cursor(&self, db: &rusqlite::Connection, owner: &str) -> Result<Option<Cursor>> {
        let exists: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM usage_samples WHERE agent='codex' AND session_id=?1 AND record_key GLOB ?2)",
            params![owner, self.pattern()], |row| row.get(0))?;
        if exists {
            load_cursor(db, &self.cursor_key)
        } else {
            Ok(None)
        }
    }

    pub(super) fn sample(
        &self,
        payload: &Payload,
        ts: &Value,
        model: &str,
    ) -> Result<Option<(String, Sample)>> {
        ensure!(payload.session_id.as_str() == Some(&self.session),
            "transcript session does not match requested session; keep the rollout and report its conflicting usage identity");
        let thread = payload
            .thread_id
            .as_str()
            .context("Codex usage lacks thread ID; keep the rollout and report incomplete usage")?;
        identifier(thread)?;
        if thread != self.thread {
            // The logical session was checked above. Copied ancestor/sibling responses
            // belong to their own physical streams, never this child's accounting.
            return Ok(None);
        }
        let response = payload.response_id.as_str().context(
            "Codex usage lacks response ID; keep the rollout and report incomplete usage",
        )?;
        identifier(response)?;
        let usage = &payload.usage;
        let mut counts = [
            count(usage, "input_tokens", false)?,
            count(usage, "output_tokens", false)?,
            count(usage, "cached_input_tokens", true)?,
            count(usage, "cache_write_input_tokens", true)?,
        ];
        let cached = counts[2]
            .checked_add(counts[3])
            .context("cache counters overflow")?;
        ensure!(cached <= counts[0], "cached tokens exceed Codex input tokens; keep the rollout and report invalid usage counters");
        counts[0] -= cached;
        Ok(Some((
            format!("{}{}", self.prefix, digest(response.as_bytes())),
            Sample {
                model: model.to_owned(),
                counts,
                cost: None,
                ts: timestamp(ts),
            },
        )))
    }
}

pub(super) fn check_existing(
    db: &rusqlite::Connection,
    session: &str,
    key: &str,
    sample: &Sample,
) -> Result<()> {
    let before: Option<[i64; 4]> = db.query_row(
        "SELECT input_tokens,output_tokens,cache_read_tokens,cache_write_tokens FROM usage_samples WHERE agent='codex' AND session_id=?1 AND record_key=?2",
        params![session, key], |r| Ok([r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?])).optional()?;
    ensure!(
        before.is_none_or(|counts| counts == sample.counts),
        "conflicting Codex response usage; keep the rollout and report the response ID"
    );
    Ok(())
}

fn previous(db: &rusqlite::Connection, key: &str) -> Result<Option<PartialImport>> {
    Ok(db
        .query_row("SELECT value FROM kv WHERE key=?1", [key], |r| {
            r.get::<_, String>(0)
        })
        .optional()?
        .and_then(|v| serde_json::from_str(&v).ok()))
}

fn merged(before: Option<PartialImport>, snapshot: &Snapshot) -> Option<PartialImport> {
    let known = || snapshot.partial.iter().chain(before.iter());
    known().next().map(|first| PartialImport {
        transcript_bytes: snapshot.size,
        skipped_bytes: known().map(|p| p.skipped_bytes).sum(),
        limit_bytes: first.limit_bytes,
    })
}

/// Keep per-stream coverage private and expose one conservative parent warning.
pub(super) fn coverage(
    db: &rusqlite::Connection,
    agent: &str,
    session: &str,
    snapshot: &Snapshot,
) -> Result<Option<PartialImport>> {
    let public = partial_key(agent, session);
    let prefix = format!("usage.coverage.{}.", digest(session.as_bytes()));
    let native = format!("{prefix}parent");
    let total_key = format!("usage.coverage-total.{}", digest(session.as_bytes()));
    let mut all = if agent == "codex" {
        previous(db, &total_key)?
    } else {
        None
    };
    if snapshot.child.is_none() && all.is_none() {
        return Ok(if snapshot.base.is_none() {
            snapshot.partial.clone()
        } else {
            merged(previous(db, &public)?, snapshot)
        });
    }
    if all.is_none() {
        if let Some(old) = previous(db, &public)? {
            put_kv(db, &native, &serde_json::to_string(&old)?)?;
            all = Some(old);
        }
    }
    let own = snapshot
        .child
        .as_ref()
        .map_or(native, |s| format!("{prefix}{}", s.prefix));
    let before = previous(db, &own)?;
    let marker = if snapshot.base.is_none() {
        snapshot.partial.clone()
    } else {
        merged(before.clone(), snapshot)
    };
    let own_marker = marker.unwrap_or(PartialImport {
        transcript_bytes: snapshot.size,
        skipped_bytes: 0,
        limit_bytes: MAX_BYTES,
    });
    put_kv(db, &own, &serde_json::to_string(&own_marker)?)?;
    let mut all = all.unwrap_or(PartialImport {
        transcript_bytes: 0,
        skipped_bytes: 0,
        limit_bytes: MAX_BYTES,
    });
    // Adjust one stream in the same transaction; never scan every child on a hook.
    all.transcript_bytes = all
        .transcript_bytes
        .checked_sub(before.as_ref().map_or(0, |m| m.transcript_bytes))
        .and_then(|n| n.checked_add(own_marker.transcript_bytes))
        .context("inconsistent usage coverage size; keep state and report the error")?;
    all.skipped_bytes = all
        .skipped_bytes
        .checked_sub(before.as_ref().map_or(0, |m| m.skipped_bytes))
        .and_then(|n| n.checked_add(own_marker.skipped_bytes))
        .context("inconsistent usage coverage size; keep state and report the error")?;
    put_kv(db, &total_key, &serde_json::to_string(&all)?)?;
    Ok((all.skipped_bytes > 0).then_some(all))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_full_parent_import_cannot_clear_a_childs_partial_coverage() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path().join("state.db"))?;
        let path = temp.path().join("child.jsonl");
        let meta = json!({"type":"session_meta","payload":{
            "id":"child","session_id":"parent","parent_thread_id":"parent",
            "source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent"}}}
        }});
        let record = |id: usize| {
            json!({"type":"token_usage_record","payload":{
                "session_id":"parent","thread_id":"child","response_id":format!("r{id}"),
                "usage":{"input_tokens":10,"output_tokens":1}
            }})
        };
        let rows = format!(
            "{meta}\n{}\n",
            (0..8)
                .map(|i| record(i).to_string())
                .collect::<Vec<_>>()
                .join("\n")
        );
        std::fs::write(&path, rows)?;
        // One header and three records of total work, so the older records are skipped.
        let limit =
            meta.to_string().len() as u64 + 1 + 3 * (record(0).to_string().len() as u64 + 1);
        ingest_bounded(&store, "codex", "parent", "/project", &path, limit)?;
        let key = ("codex".to_owned(), "parent".to_owned());
        let before = partial_imports(&store)?[&key].skipped_bytes;
        assert!(before > 0);
        let parent = temp.path().join("parent.jsonl");
        std::fs::write(&parent, "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":5,\"output_tokens\":1}}}}\n")?;
        ingest(&store, "codex", "parent", "/project", &parent)?;
        assert_eq!(partial_imports(&store)?[&key].skipped_bytes, before);
        // A complete replay of the child recovers its skipped records and clears the warning.
        let new_path = temp.path().join("replacement.jsonl");
        let quota_row = json!({"type":"event_msg","timestamp":"2026-10-16T12:00:00Z","payload":{
            "type":"token_count","info":null,"rate_limits":{"plan_type":"pro","primary":{
                "used_percent":34.0,"window_minutes":10080,"resets_at":1_792_649_595
            },"secondary":null}
        }});
        std::fs::write(&new_path, format!("{meta}\n{}\n{quota_row}\n", record(9)))?;
        std::fs::rename(&new_path, &path)?;
        ingest(&store, "codex", "parent", "/project", &path)?;
        assert!(!partial_imports(&store)?.contains_key(&key));
        let total = summaries(&store, Some("/project"), Some("parent"))?
            .iter()
            .map(|r| r.input_tokens)
            .sum::<u64>();
        assert_eq!(total, 15);
        let quota =
            crate::quota::load(&store, "codex")?.context("child quota must still be recorded")?;
        assert_eq!(quota.windows[0].used_percent, 34.0);
        Ok(())
    }
}
