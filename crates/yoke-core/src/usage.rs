//! Bounded local transcript accounting. Costs are estimates, never subscription bills.
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use anyhow::{ensure, Context, Result};
use rusqlite::params;
use serde::Serialize;
use serde_json::Value;

use crate::{redact::redact, store::Store};

const MAX_BYTES: u64 = 32 * 1024 * 1024;
const MAX_LINE: usize = 1024 * 1024;

#[derive(Debug, Serialize)]
pub struct UsageSummary {
    pub agent: String,
    pub session_id: String,
    pub project: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub estimated_cost_usd: Option<String>,
    pub unknown_pricing_samples: u64,
}

struct Sample {
    model: String,
    counts: [i64; 4],
    cost: Option<i64>,
}

fn count(value: &Value, key: &str, optional: bool) -> Result<i64> {
    if optional && value.get(key).is_none() {
        return Ok(0);
    }
    value[key]
        .as_i64()
        .filter(|n| *n >= 0)
        .with_context(|| format!("invalid transcript counter {key}"))
}

fn identifier(value: &str) -> Result<String> {
    ensure!(
        !value.is_empty() && value.len() <= 4096 && !value.chars().any(char::is_control),
        "invalid usage identifier"
    );
    Ok(redact(value))
}

// Verified 2026-10-13: https://platform.claude.com/docs/en/about-claude/pricing
// Nanodollars per token, standard global inference only. Unlisted models and
// missing tier/TTL information deliberately retain unknown cost.
fn claude_cost(model: &str, usage: &Value, counts: [i64; 4]) -> Option<i64> {
    if usage["service_tier"].as_str()? != "standard"
        || usage["inference_geo"].as_str()? != "global"
        || usage["speed"].as_str()? != "standard"
    {
        return None;
    }
    let prompt = counts[0].checked_add(counts[2])?.checked_add(counts[3])?;
    let rates = match model {
        "claude-opus-5-5" => [4000, 20000, 200, 5000, 8000],
        "claude-sonnet-5-5" => [2000, 10000, 100, 2500, 4000],
        "claude-haiku-5-5" if prompt <= 100_000 => [100, 500, 10, 125, 200],
        "claude-haiku-5-5" => [500, 2500, 50, 625, 1000],
        "claude-sonnet-4-6" => [3000, 15000, 300, 3750, 6000],
        _ => return None,
    };
    let (short, long) = if counts[3] == 0 {
        (0, 0)
    } else {
        let cache = &usage["cache_creation"];
        (
            count(cache, "ephemeral_5m_input_tokens", false).ok()?,
            count(cache, "ephemeral_1h_input_tokens", false).ok()?,
        )
    };
    if short.checked_add(long)? != counts[3] {
        return None;
    }
    [counts[0], counts[1], counts[2], short, long]
        .into_iter()
        .zip(rates)
        .try_fold(0i64, |sum, (tokens, rate)| {
            sum.checked_add(tokens.checked_mul(rate)?)
        })
}

/// Import a complete bounded JSONL snapshot atomically. No message content is persisted.
/// Valid snapshots without usage are no-ops, returning no newly imported summaries.
pub fn ingest(
    store: &Store,
    agent: &str,
    session: &str,
    project: &str,
    transcript: &Path,
) -> Result<Vec<UsageSummary>> {
    ensure!(
        matches!(agent, "claude" | "codex"),
        "unsupported transcript agent"
    );
    let session = identifier(session)?;
    let project = identifier(project)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(transcript)
        .context("open usage transcript")?;
    ensure!(
        file.metadata()?.is_file(),
        "transcript must be a regular file"
    );
    let mut reader = BufReader::new(file.take(MAX_BYTES + 1));
    let mut bytes = Vec::new();
    let mut total = 0u64;
    let mut samples = BTreeMap::<String, Sample>::new();
    let mut previous = [0i64; 4];
    let mut model = "unknown".to_owned();
    let mut ordinal = 0u64;
    loop {
        bytes.clear();
        let read = reader
            .by_ref()
            .take((MAX_LINE + 1) as u64)
            .read_until(b'\n', &mut bytes)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        ensure!(
            total <= MAX_BYTES && read <= MAX_LINE,
            "usage transcript exceeds bounded import limits"
        );
        if bytes.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let row: Value =
            serde_json::from_slice(&bytes).context("malformed usage transcript JSON")?;
        if agent == "claude" {
            if row["type"] != "assistant" || row["message"]["usage"].is_null() {
                continue;
            }
            if let Some(found) = row["sessionId"].as_str() {
                ensure!(
                    identifier(found)? == session,
                    "transcript session does not match requested session"
                );
            }
            let message = &row["message"];
            let key = identifier(message["id"].as_str().context("usage message lacks id")?)?;
            let model = identifier(message["model"].as_str().unwrap_or("unknown"))?;
            let usage = &message["usage"];
            let counts = [
                count(usage, "input_tokens", false)?,
                count(usage, "output_tokens", false)?,
                count(usage, "cache_read_input_tokens", true)?,
                count(usage, "cache_creation_input_tokens", true)?,
            ];
            let cost = claude_cost(&model, usage, counts);
            // Streaming copies share a message id. The final snapshot replaces it.
            samples.insert(
                key,
                Sample {
                    model,
                    counts,
                    cost,
                },
            );
        } else {
            if row["type"] == "session_meta" {
                if let Some(found) = row["payload"]["id"].as_str() {
                    ensure!(
                        identifier(found)? == session,
                        "transcript session does not match requested session"
                    );
                }
            }
            if row["type"] == "turn_context" {
                model = identifier(row["payload"]["model"].as_str().unwrap_or("unknown"))?;
            }
            if row["type"] != "event_msg" || row["payload"]["type"] != "token_count" {
                continue;
            }
            let usage = &row["payload"]["info"]["total_token_usage"];
            if usage.is_null() {
                continue;
            }
            let current = [
                count(usage, "input_tokens", false)?,
                count(usage, "output_tokens", false)?,
                count(usage, "cached_input_tokens", true)?,
                count(usage, "cache_write_input_tokens", true)?,
            ];
            ensure!(
                current.iter().zip(previous).all(|(n, before)| *n >= before),
                "Codex cumulative counters decreased; refusing ambiguous accounting"
            );
            let mut delta = [0; 4];
            for index in 0..4 {
                delta[index] = current[index] - previous[index];
            }
            previous = current;
            if delta == [0; 4] {
                continue;
            }
            let cached = delta[2]
                .checked_add(delta[3])
                .context("cache counters overflow")?;
            ensure!(
                cached <= delta[0],
                "cached tokens exceed Codex input tokens"
            );
            // OpenAI input totals include both cache reads and cache writes.
            // https://developers.openai.com/api/docs/guides/prompt-caching
            delta[0] -= cached;
            ordinal += 1;
            // Local Codex token_count lacks authoritative service-tier metadata.
            // Keep tokens, explicitly leave cost unknown.
            samples.insert(
                format!("total-{ordinal}"),
                Sample {
                    model: model.clone(),
                    counts: delta,
                    cost: None,
                },
            );
        }
    }
    if samples.is_empty() {
        // Early lifecycle events may precede token counters. Do not erase an
        // earlier import when the current snapshot contains no accounting data.
        return Ok(Vec::new());
    }
    store.transaction(|tx| {
        // Replace only this explicitly imported session, retaining other sessions.
        tx.execute("DELETE FROM usage_samples WHERE agent=?1 AND session_id=?2", params![agent, session])?;
        for (key, sample) in &samples {
            tx.execute("INSERT INTO usage_samples(agent,session_id,project,record_key,model,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,cost_nano_usd) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![agent, session, project, key, sample.model, sample.counts[0], sample.counts[1], sample.counts[2], sample.counts[3], sample.cost])?;
        }
        Ok(())
    })?;
    summaries(store, Some(&project), Some(&session))
}

/// Group by session and exact model; a partially priced group has unknown total cost.
pub fn summaries(
    store: &Store,
    project: Option<&str>,
    session: Option<&str>,
) -> Result<Vec<UsageSummary>> {
    let mut query = store.connection().prepare("SELECT agent,session_id,project,model,SUM(input_tokens),SUM(output_tokens),SUM(cache_read_tokens),SUM(cache_write_tokens),SUM(cost_nano_usd),SUM(cost_nano_usd IS NULL) FROM usage_samples WHERE (?1 IS NULL OR project=?1) AND (?2 IS NULL OR session_id=?2) GROUP BY agent,session_id,project,model ORDER BY session_id,model LIMIT 1000")?;
    let rows = query.query_map(params![project.map(redact), session.map(redact)], |row| {
        let cost: Option<i64> = row.get(8)?;
        let unknown: u64 = row.get(9)?;
        Ok(UsageSummary {
            agent: row.get(0)?,
            session_id: row.get(1)?,
            project: row.get(2)?,
            model: row.get(3)?,
            input_tokens: row.get(4)?,
            output_tokens: row.get(5)?,
            cache_read_tokens: row.get(6)?,
            cache_write_tokens: row.get(7)?,
            estimated_cost_usd: cost
                .filter(|_| unknown == 0)
                .map(|n| format!("{}.{:09}", n / 1_000_000_000, n % 1_000_000_000)),
            unknown_pricing_samples: unknown,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn import_deduplicates_prices_known_samples_and_rejects_bad_snapshot_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("state.db")).unwrap();
        let path = dir.path().join("fixture.jsonl");
        let row = json!({"type":"assistant","sessionId":"s","message":{"id":"m","model":"claude-haiku-5-5","content":"must never be stored", "usage":{"input_tokens":1000,"output_tokens":100,"cache_read_input_tokens":1000,"service_tier":"standard","inference_geo":"global","speed":"standard"}}});
        std::fs::write(&path, format!("{row}\n{row}\n")).unwrap();
        let result = ingest(&store, "claude", "s", "/project", &path).unwrap();
        assert_eq!(result[0].input_tokens, 1000);
        assert_eq!(result[0].estimated_cost_usd.as_deref(), Some("0.000160000"));
        assert_eq!(
            ingest(&store, "claude", "s", "/project", &path).unwrap()[0].output_tokens,
            100
        );
        std::fs::write(&path, format!("{row}\n{{broken\n")).unwrap();
        assert!(ingest(&store, "claude", "s", "/project", &path).is_err());
        assert_eq!(summaries(&store, None, None).unwrap()[0].input_tokens, 1000);
    }

    #[test]
    fn early_transcripts_are_noops_without_erasing_prior_usage_or_hiding_invalid_input() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("state.db")).unwrap();
        let path = dir.path().join("fixture.jsonl");
        let sample = json!({"type":"assistant","sessionId":"s","message":{"id":"m","model":"unknown","usage":{"input_tokens":7,"output_tokens":3}}});
        std::fs::write(&path, format!("{sample}\n")).unwrap();
        ingest(&store, "claude", "s", "/project", &path).unwrap();
        for agent in ["claude", "codex"] {
            for early in [
                "",
                " \n",
                "{\"type\":\"user\"}\n",
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":null}}\n",
            ] {
                std::fs::write(&path, early).unwrap();
                assert!(ingest(&store, agent, "s", "/project", &path)
                    .unwrap()
                    .is_empty());
            }
            for invalid in ["{bad\n".to_owned(), " ".repeat(MAX_LINE + 1)] {
                std::fs::write(&path, invalid).unwrap();
                assert!(ingest(&store, agent, "s", "/project", &path).is_err());
            }
        }
        let persisted = summaries(&store, None, Some("s")).unwrap();
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].input_tokens, 7);
        assert_eq!(persisted[0].output_tokens, 3);
    }

    #[test]
    fn codex_cumulative_samples_do_not_double_count_or_guess_prices() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("state.db")).unwrap();
        let path = dir.path().join("fixture.jsonl");
        let first = json!({"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"cache_write_input_tokens":10,"output_tokens":10}}}});
        let last = json!({"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":150,"cached_input_tokens":30,"cache_write_input_tokens":20,"output_tokens":20}}}});
        std::fs::write(&path, format!("{first}\n{first}\n{last}\n")).unwrap();
        let result = ingest(&store, "codex", "s", "/project", &path).unwrap();
        assert_eq!(result[0].input_tokens, 100);
        assert_eq!(result[0].cache_read_tokens, 30);
        assert_eq!(result[0].cache_write_tokens, 20);
        assert_eq!(result[0].output_tokens, 20);
        assert_eq!(result[0].unknown_pricing_samples, 2);
        assert!(result[0].estimated_cost_usd.is_none());
    }
}
