//! Bounded local transcript accounting. Costs are estimates, never subscription bills.
use std::collections::BTreeMap;
use std::fs::{Metadata, OpenOptions};
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{redact::redact, store::Store};

const MAX_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ATTEMPTS: usize = 3;

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
    /// Estimate for the priced samples alone; None when no sample in the group is priced.
    pub known_cost_usd: Option<String>,
    pub unknown_pricing_samples: u64,
}

/// Token and cost totals for one group of samples.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct UsageTotals {
    pub samples: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    /// Exact estimate; None as soon as any sample in the group has an unknown price.
    pub estimated_cost_usd: Option<String>,
    /// Estimate for the priced samples alone; None when no sample in the group is priced.
    pub known_cost_usd: Option<String>,
    pub unknown_pricing_samples: u64,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct UsageBucket {
    /// `YYYY-MM-DD` or `YYYY-MM` in the range's UTC offset; None for samples without a time.
    pub period: Option<String>,
    #[serde(flatten)]
    pub totals: UsageTotals,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Period {
    Day,
    Month,
}

/// Whole calendar days in one fixed UTC offset. A bounded range excludes samples without a time.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UsageRange {
    from: Option<String>,
    to: Option<String>,
    offset_minutes: i32,
}

impl UsageRange {
    /// `from` and `to` are inclusive `YYYY-MM-DD` days; the offset is minutes east of UTC.
    pub fn new(from: Option<&str>, to: Option<&str>, offset_minutes: i32) -> Result<Self> {
        ensure!(
            (-720..=840).contains(&offset_minutes) && offset_minutes % 15 == 0,
            "UTC offset must be between -12:00 and +14:00 in 15 minute steps"
        );
        for value in [from, to].into_iter().flatten() {
            ensure!(
                calendar_day(value),
                "dates must be real days written YYYY-MM-DD"
            );
        }
        if let (Some(from), Some(to)) = (from, to) {
            ensure!(from <= to, "the start date must not be after the end date");
        }
        Ok(Self {
            from: from.map(str::to_owned),
            to: to.map(str::to_owned),
            offset_minutes,
        })
    }

    pub fn from(&self) -> Option<&str> {
        self.from.as_deref()
    }

    pub fn to(&self) -> Option<&str> {
        self.to.as_deref()
    }

    pub fn offset_minutes(&self) -> i32 {
        self.offset_minutes
    }

    pub fn is_bounded(&self) -> bool {
        self.from.is_some() || self.to.is_some()
    }
}

fn calendar_day(value: &str) -> bool {
    let part = |range: std::ops::Range<usize>| {
        value
            .get(range)
            .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|s| s.parse::<u32>().ok())
    };
    let (Some(year), Some(month), Some(day)) = (part(0..4), part(5..7), part(8..10)) else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    value.len() == 10
        && value.as_bytes()[4] == b'-'
        && value.as_bytes()[7] == b'-'
        && year >= 1
        && (1..=12).contains(&month)
        && day >= 1
        && day <= days[month as usize - 1]
}

#[derive(Debug, Serialize)]
pub struct UsageReport {
    pub totals: UsageTotals,
    /// Newest first, at most 400 periods.
    pub periods: Vec<UsageBucket>,
    pub sessions: Vec<UsageSummary>,
    /// Samples in this scope recorded without a time; any bounded range excludes them.
    pub undated_samples: u64,
}

struct Sample {
    model: String,
    counts: [i64; 4],
    cost: Option<i64>,
    ts: Option<String>,
}

/// RFC 3339 line timestamp with an explicit zone. Anything else stays "time unknown";
/// SQLite would read bare numbers as Julian days, so the shape is checked first.
fn timestamp(timestamp: &Value) -> Option<String> {
    let value = timestamp.as_str()?;
    let bytes = value.as_bytes();
    let digit = |index: usize| bytes.get(index).is_some_and(u8::is_ascii_digit);
    let shape = bytes.len() <= 40
        && [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18]
            .into_iter()
            .all(digit)
        && bytes.get(4..19).map(|s| [s[0], s[3], s[6], s[9], s[12]]) == Some(*b"--T::");
    if !shape {
        return None;
    }
    let mut rest = &bytes[19..];
    if let Some(fraction) = rest.strip_prefix(b".") {
        let end = fraction
            .iter()
            .position(|b| !b.is_ascii_digit())
            .unwrap_or(fraction.len());
        if end == 0 {
            return None;
        }
        rest = &fraction[end..];
    }
    let zone = rest == b"Z"
        || matches!(rest, [b'+' | b'-', h1, h2, b':', m1, m2]
            if [h1, h2, m1, m2].iter().all(|b| b.is_ascii_digit()));
    zone.then(|| value.to_owned())
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

/// Device, inode, size and modification/change times of the transcript a snapshot was read from.
type Fingerprint = (u64, u64, u64, i64, i64, i64, i64);

fn fingerprint(meta: &Metadata) -> Fingerprint {
    (
        meta.dev(),
        meta.ino(),
        meta.size(),
        meta.mtime(),
        meta.mtime_nsec(),
        meta.ctime(),
        meta.ctime_nsec(),
    )
}

struct Snapshot {
    observed: Fingerprint,
    samples: BTreeMap<String, Sample>,
}

/// The accounting fields of one transcript line. Everything else, including tool results and
/// file writes of any size, streams past serde without being kept, so the only size bound is
/// the whole file and no usage record is ever skipped for being large. A `message` or
/// `payload` that is not an object is malformed.
#[derive(Default, Deserialize)]
#[serde(default)]
struct Line {
    #[serde(rename = "type")]
    kind: Value,
    #[serde(rename = "sessionId")]
    session_id: Value,
    timestamp: Value,
    /// Claude: the message carrying `id`, `model` and `usage`.
    message: Option<Message>,
    /// Codex: the payload carrying the session id, model and token counts.
    payload: Option<Payload>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Message {
    id: Value,
    model: Value,
    usage: Value,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Payload {
    id: Value,
    model: Value,
    #[serde(rename = "type")]
    kind: Value,
    info: Value,
}

/// Import a complete bounded JSONL snapshot atomically. No message content is persisted.
/// Valid snapshots without usage are no-ops, returning no newly imported summaries.
/// A snapshot is only committed while the transcript is still the file it was read from,
/// so an importer that lost a race can never overwrite a newer or replaced transcript.
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
    for _ in 0..MAX_ATTEMPTS {
        let snapshot = read_snapshot(agent, &session, transcript)?;
        if snapshot.samples.is_empty() {
            // Early lifecycle events may precede token counters. Do not erase an
            // earlier import when the current snapshot contains no accounting data.
            return Ok(Vec::new());
        }
        if commit_snapshot(store, agent, &session, &project, transcript, &snapshot)? {
            return summaries(store, Some(&project), Some(&session));
        }
    }
    bail!("usage transcript kept changing during import")
}

fn read_snapshot(agent: &str, session: &str, transcript: &Path) -> Result<Snapshot> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(transcript)
        .context("open usage transcript")?;
    let metadata = file.metadata()?;
    ensure!(metadata.is_file(), "transcript must be a regular file");
    let observed = fingerprint(&metadata);
    let mut reader = BufReader::new(file.take(MAX_BYTES + 1));
    let mut bytes = Vec::new();
    let mut total = 0u64;
    let mut samples = BTreeMap::<String, Sample>::new();
    let mut previous = [0i64; 4];
    let mut model = "unknown".to_owned();
    let mut ordinal = 0u64;
    loop {
        bytes.clear();
        let read = reader.read_until(b'\n', &mut bytes)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        ensure!(
            total <= MAX_BYTES,
            "usage transcript exceeds bounded import limits"
        );
        if bytes.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let row: Line = match serde_json::from_slice(&bytes) {
            Ok(row) => row,
            // Agents append transcripts asynchronously; an unterminated last line may still
            // be in flight, and the next import reads it whole.
            Err(_) if !bytes.ends_with(b"\n") => break,
            Err(error) => return Err(error).context("malformed usage transcript JSON"),
        };
        if agent == "claude" {
            let message = row.message.unwrap_or_default();
            if row.kind != "assistant" || message.usage.is_null() {
                continue;
            }
            if let Some(found) = row.session_id.as_str() {
                ensure!(
                    identifier(found)? == session,
                    "transcript session does not match requested session"
                );
            }
            let key = identifier(message.id.as_str().context("usage message lacks id")?)?;
            let model = identifier(message.model.as_str().unwrap_or("unknown"))?;
            let usage = &message.usage;
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
                    ts: timestamp(&row.timestamp),
                },
            );
        } else {
            let payload = row.payload.unwrap_or_default();
            if row.kind == "session_meta" {
                if let Some(found) = payload.id.as_str() {
                    ensure!(
                        identifier(found)? == session,
                        "transcript session does not match requested session"
                    );
                }
            }
            if row.kind == "turn_context" {
                model = identifier(payload.model.as_str().unwrap_or("unknown"))?;
            }
            if row.kind != "event_msg" || payload.kind != "token_count" {
                continue;
            }
            let usage = &payload.info["total_token_usage"];
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
                    ts: timestamp(&row.timestamp),
                },
            );
        }
    }
    Ok(Snapshot { observed, samples })
}

/// Returns false without writing when the transcript changed after it was read.
fn commit_snapshot(
    store: &Store,
    agent: &str,
    session: &str,
    project: &str,
    transcript: &Path,
    snapshot: &Snapshot,
) -> Result<bool> {
    store.transaction(|tx| {
        // The immediate transaction serializes importers, so this check decides which
        // snapshot is still current. Any metadata failure is treated as stale: the retry
        // reopens the file and reports the real error.
        let current = std::fs::symlink_metadata(transcript).map(|meta| fingerprint(&meta));
        if current.ok() != Some(snapshot.observed) {
            return Ok(false);
        }
        // Replace only this explicitly imported session, retaining other sessions.
        tx.execute("DELETE FROM usage_samples WHERE agent=?1 AND session_id=?2", params![agent, session])?;
        for (key, sample) in &snapshot.samples {
            tx.execute("INSERT INTO usage_samples(agent,session_id,project,record_key,model,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,cost_nano_usd,ts) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,unixepoch(?11))",
                params![agent, session, project, key, sample.model, sample.counts[0], sample.counts[1], sample.counts[2], sample.counts[3], sample.cost, sample.ts])?;
        }
        Ok(true)
    })
}

/// Group by session and exact model; a partially priced group has unknown total cost.
pub fn summaries(
    store: &Store,
    project: Option<&str>,
    session: Option<&str>,
) -> Result<Vec<UsageSummary>> {
    summaries_in(store, project, session, &UsageRange::default())
}

const TOTALS: &str = "count(*),coalesce(SUM(input_tokens),0),coalesce(SUM(output_tokens),0),coalesce(SUM(cache_read_tokens),0),coalesce(SUM(cache_write_tokens),0),SUM(cost_nano_usd),coalesce(SUM(cost_nano_usd IS NULL),0)";

fn read_totals(row: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<UsageTotals> {
    let usd = |n: i64| format!("{}.{:09}", n / 1_000_000_000, n % 1_000_000_000);
    let known: Option<i64> = row.get(at + 5)?;
    let unknown: u64 = row.get(at + 6)?;
    Ok(UsageTotals {
        samples: row.get(at)?,
        input_tokens: row.get(at + 1)?,
        output_tokens: row.get(at + 2)?,
        cache_read_tokens: row.get(at + 3)?,
        cache_write_tokens: row.get(at + 4)?,
        estimated_cost_usd: (unknown == 0).then(|| usd(known.unwrap_or(0))),
        known_cost_usd: known.map(usd),
        unknown_pricing_samples: unknown,
    })
}

type Bindings = Vec<(&'static str, rusqlite::types::Value)>;

/// Names only the filters in use, so SQLite can choose the project and time index.
fn scope(project: Option<&str>, session: Option<&str>, range: &UsageRange) -> (String, Bindings) {
    use rusqlite::types::Value::{Integer, Text};
    let offset = i64::from(range.offset_minutes) * 60;
    let mut clauses = vec!["1=1"];
    let mut bind: Bindings = Vec::new();
    if let Some(project) = project {
        clauses.push("project=:project");
        bind.push((":project", Text(redact(project))));
    }
    if let Some(session) = session {
        clauses.push("session_id=:session");
        bind.push((":session", Text(redact(session))));
    }
    if let Some(day) = &range.from {
        clauses.push("ts>=:start");
        bind.push((":start", Integer(epoch_day(day) * 86_400 - offset)));
    }
    if let Some(day) = &range.to {
        clauses.push("ts<:end");
        bind.push((":end", Integer((epoch_day(day) + 1) * 86_400 - offset)));
    }
    (clauses.join(" AND "), bind)
}

/// Days from 1970-01-01 to a validated `YYYY-MM-DD` (Howard Hinnant's days_from_civil).
fn epoch_day(day: &str) -> i64 {
    let part = |range: std::ops::Range<usize>| day[range].parse::<i64>().unwrap_or(0);
    let (month, date) = (part(5..7), part(8..10));
    let year = part(0..4) - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + date - 1;
    era * 146_097 + year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year - 719_468
}

fn bound(bind: &Bindings) -> Vec<(&str, &dyn rusqlite::ToSql)> {
    bind.iter()
        .map(|(name, value)| (*name, value as &dyn rusqlite::ToSql))
        .collect()
}

fn summaries_in(
    store: &Store,
    project: Option<&str>,
    session: Option<&str>,
    range: &UsageRange,
) -> Result<Vec<UsageSummary>> {
    let (filter, bind) = scope(project, session, range);
    let sql = format!("SELECT agent,session_id,project,model,{TOTALS} FROM usage_samples WHERE {filter} GROUP BY agent,session_id,project,model ORDER BY session_id,model LIMIT 1000");
    let mut query = store.connection().prepare(&sql)?;
    let rows = query.query_map(&*bound(&bind), |row| {
        let t = read_totals(row, 4)?;
        Ok(UsageSummary {
            agent: row.get(0)?,
            session_id: row.get(1)?,
            project: row.get(2)?,
            model: row.get(3)?,
            input_tokens: t.input_tokens,
            output_tokens: t.output_tokens,
            cache_read_tokens: t.cache_read_tokens,
            cache_write_tokens: t.cache_write_tokens,
            estimated_cost_usd: t.estimated_cost_usd,
            known_cost_usd: t.known_cost_usd,
            unknown_pricing_samples: t.unknown_pricing_samples,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Totals, calendar periods and per-session rows for one scope and date range.
pub fn report(
    store: &Store,
    project: Option<&str>,
    session: Option<&str>,
    range: &UsageRange,
    period: Period,
) -> Result<UsageReport> {
    let (filter, bind) = scope(project, session, range);
    let totals = store.connection().query_row(
        &format!("SELECT {TOTALS} FROM usage_samples WHERE {filter}"),
        &*bound(&bind),
        |row| read_totals(row, 0),
    )?;
    let format = match period {
        Period::Day => "%Y-%m-%d",
        Period::Month => "%Y-%m",
    };
    let mut periods_bind = bind.clone();
    periods_bind.push((":format", rusqlite::types::Value::Text(format.into())));
    periods_bind.push((
        ":offset",
        rusqlite::types::Value::Integer(i64::from(range.offset_minutes) * 60),
    ));
    let mut query = store.connection().prepare(&format!("SELECT strftime(:format,ts+:offset,'unixepoch') AS period,{TOTALS} FROM usage_samples WHERE {filter} GROUP BY period ORDER BY period IS NULL,period DESC LIMIT 400"))?;
    let periods = query
        .query_map(&*bound(&periods_bind), |row| {
            Ok(UsageBucket {
                period: row.get(0)?,
                totals: read_totals(row, 1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let (undated_filter, undated_bind) = scope(project, session, &UsageRange::default());
    let undated_samples = store.connection().query_row(
        &format!("SELECT count(*) FROM usage_samples WHERE {undated_filter} AND ts IS NULL"),
        &*bound(&undated_bind),
        |row| row.get(0),
    )?;
    Ok(UsageReport {
        totals,
        periods,
        sessions: summaries_in(store, project, session, range)?,
        undated_samples,
    })
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
    fn a_partially_written_last_line_waits_for_the_next_import() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("state.db")).unwrap();
        let path = dir.path().join("fixture.jsonl");
        let row = |id: &str| {
            json!({"type":"assistant","sessionId":"s","message":{"id":id,"model":"unknown","usage":{"input_tokens":5,"output_tokens":1}}}).to_string()
        };
        let partial = &row("m2")[..20];
        std::fs::write(&path, format!("{}\n{partial}", row("m1"))).unwrap();
        let imported = ingest(&store, "claude", "s", "/project", &path).unwrap();
        assert_eq!(imported[0].input_tokens, 5);
        // The same bytes ending in a newline are a finished line, so they are still malformed.
        std::fs::write(&path, format!("{}\n{partial}\n", row("m1"))).unwrap();
        assert!(ingest(&store, "claude", "s", "/project", &path).is_err());
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
            std::fs::write(&path, "{bad\n").unwrap();
            assert!(ingest(&store, agent, "s", "/project", &path).is_err());
        }
        let persisted = summaries(&store, None, Some("s")).unwrap();
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].input_tokens, 7);
        assert_eq!(persisted[0].output_tokens, 3);
    }

    #[test]
    fn oversized_lines_never_fail_the_import_or_drop_a_usage_record() {
        const BIG: usize = 1024 * 1024;
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("state.db")).unwrap();
        let path = dir.path().join("fixture.jsonl");
        let row = |id: &str, input: u64, content: usize| {
            json!({"type":"assistant","sessionId":"s","message":{"id":id,"model":"unknown","content":"x".repeat(content),"usage":{"input_tokens":input,"output_tokens":1}}}).to_string()
        };
        // A huge tool result carries no accounting, while an assistant record with a huge
        // file write still carries billable usage; neither may stop or shrink the import.
        let tool_result =
            json!({"type":"user","sessionId":"s","message":{"content":"x".repeat(BIG)}})
                .to_string();
        std::fs::write(
            &path,
            format!(
                "{}\n{tool_result}\n{}\n{}\n",
                row("m1", 5, 0),
                row("m2", 7, 0),
                row("m3", 9, BIG)
            ),
        )
        .unwrap();
        let imported = ingest(&store, "claude", "s", "/project", &path).unwrap();
        assert_eq!((imported.len(), imported[0].input_tokens), (1, 21));
        // Codex rollouts carry the same oversized outputs between token counts.
        let token_count = |input: u64| {
            json!({"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":input,"output_tokens":1}}}}).to_string()
        };
        let output = json!({"type":"response_item","payload":{"type":"function_call_output","output":"x".repeat(BIG)}}).to_string();
        std::fs::write(
            &path,
            format!("{}\n{output}\n{}\n", token_count(3), token_count(10)),
        )
        .unwrap();
        assert_eq!(
            ingest(&store, "codex", "c", "/project", &path).unwrap()[0].input_tokens,
            10
        );
        // An oversized line still being written waits like any other partial last line.
        std::fs::write(
            &path,
            format!("{}\n{}", row("m1", 5, 0), &tool_result[..BIG + 10]),
        )
        .unwrap();
        assert_eq!(
            ingest(&store, "claude", "s", "/project", &path).unwrap()[0].input_tokens,
            5
        );
        std::fs::write(
            &path,
            format!("{}\n{}\n", " ".repeat(BIG + 1), row("m1", 6, 0)),
        )
        .unwrap();
        assert_eq!(
            ingest(&store, "claude", "s", "/project", &path).unwrap()[0].input_tokens,
            6
        );
    }

    #[test]
    fn stale_snapshot_never_erases_newer_usage_and_replaced_transcripts_still_win() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("state.db")).unwrap();
        let path = dir.path().join("fixture.jsonl");
        let message = |id: &str, input: u64| json!({"type":"assistant","sessionId":"s","message":{"id":id,"model":"unknown","usage":{"input_tokens":input,"output_tokens":1}}});
        let commit = |snapshot: &Snapshot| {
            commit_snapshot(&store, "claude", "s", "/project", &path, snapshot).unwrap()
        };
        std::fs::write(&path, format!("{}\n", message("m1", 10))).unwrap();
        let older = read_snapshot("claude", "s", &path).unwrap();
        std::fs::write(
            &path,
            format!("{}\n{}\n", message("m1", 10), message("m2", 20)),
        )
        .unwrap();
        let newer = read_snapshot("claude", "s", &path).unwrap();
        assert!(commit(&newer));
        assert!(!commit(&older), "the older snapshot must not be written");
        assert_eq!(
            summaries(&store, None, Some("s")).unwrap()[0].input_tokens,
            30
        );
        assert!(
            commit(&newer),
            "recommitting the same snapshot is idempotent"
        );
        assert_eq!(
            summaries(&store, None, Some("s")).unwrap()[0].input_tokens,
            30
        );

        std::fs::write(&path, format!("{}\n", message("m3", 5))).unwrap();
        let replaced = ingest(&store, "claude", "s", "/project", &path).unwrap();
        assert_eq!(replaced.len(), 1);
        assert_eq!(replaced[0].input_tokens, 5);
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

    fn claude_row(id: &str, model: &str, input: u64, ts: Option<&str>) -> Value {
        let mut row = json!({"type":"assistant","sessionId":"s","message":{"id":id,"model":model,"usage":{"input_tokens":input,"output_tokens":100,"service_tier":"standard","inference_geo":"global","speed":"standard"}}});
        if let Some(ts) = ts {
            row["timestamp"] = json!(ts);
        }
        row
    }

    fn import(store: &Store, dir: &Path, rows: &[Value]) {
        let path = dir.join("fixture.jsonl");
        let lines: String = rows.iter().map(|row| format!("{row}\n")).collect();
        std::fs::write(&path, lines).unwrap();
        ingest(store, "claude", "s", "/project", &path).unwrap();
    }

    #[test]
    fn history_splits_days_at_the_viewers_midnight_and_bounds_whole_days() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("state.db")).unwrap();
        import(
            &store,
            dir.path(),
            &[
                claude_row("before", "unknown", 1, Some("2026-10-13T17:59:59.999Z")),
                claude_row("midnight", "unknown", 10, Some("2026-10-14T00:00:00+06:00")),
                claude_row("undated", "unknown", 100, None),
                claude_row("julian", "unknown", 1000, Some("2460000")),
            ],
        );
        let days = |range: UsageRange, period| {
            report(&store, Some("/project"), None, &range, period)
                .unwrap()
                .periods
                .into_iter()
                .map(|b| (b.period, b.totals.input_tokens))
                .collect::<Vec<_>>()
        };
        let day = |s: &str| Some(s.to_owned());
        assert_eq!(
            days(UsageRange::default(), Period::Day),
            [(day("2026-10-13"), 11), (None, 1100)]
        );
        let dhaka = UsageRange::new(None, None, 360).unwrap();
        assert_eq!(
            days(dhaka.clone(), Period::Day),
            [
                (day("2026-10-14"), 10),
                (day("2026-10-13"), 1),
                (None, 1100)
            ]
        );
        assert_eq!(
            days(dhaka, Period::Month),
            [(day("2026-10"), 11), (None, 1100)]
        );
        let first = UsageRange::new(Some("2026-10-14"), Some("2026-10-14"), 360).unwrap();
        let bounded = report(&store, Some("/project"), None, &first, Period::Day).unwrap();
        assert_eq!(bounded.totals.input_tokens, 10);
        assert_eq!(bounded.undated_samples, 2);
        assert_eq!(bounded.sessions[0].input_tokens, 10);
        let before = UsageRange::new(None, Some("2026-10-13"), 360).unwrap();
        assert_eq!(days(before, Period::Day), [(day("2026-10-13"), 1)]);
        assert!(report(
            &store,
            Some("/other"),
            None,
            &UsageRange::default(),
            Period::Day
        )
        .unwrap()
        .periods
        .is_empty());
        for (from, to, offset) in [
            (Some("2026-02-29"), None, 0),
            (Some("2026-10-14"), Some("2026-10-13"), 0),
            (Some("2026-1-01"), None, 0),
            (None, None, 7),
            (None, None, 15 * 60),
        ] {
            assert!(UsageRange::new(from, to, offset).is_err());
        }
    }

    #[test]
    fn unknown_prices_stay_unknown_in_every_aggregate() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("state.db")).unwrap();
        let ts = Some("2026-10-13T10:00:00Z");
        import(
            &store,
            dir.path(),
            &[
                claude_row("priced", "claude-haiku-5-5", 1000, ts),
                claude_row("unpriced", "unknown", 1000, ts),
            ],
        );
        let usage = report(&store, None, None, &UsageRange::default(), Period::Day).unwrap();
        for totals in [&usage.totals, &usage.periods[0].totals] {
            assert_eq!(totals.samples, 2);
            assert_eq!(totals.estimated_cost_usd, None);
            assert_eq!(totals.known_cost_usd.as_deref(), Some("0.000150000"));
            assert_eq!(totals.unknown_pricing_samples, 1);
        }
        let unpriced = usage
            .sessions
            .iter()
            .find(|s| s.model == "unknown")
            .unwrap();
        assert_eq!(
            (
                unpriced.estimated_cost_usd.as_deref(),
                unpriced.known_cost_usd.as_deref()
            ),
            (None, None)
        );
        let priced = usage
            .sessions
            .iter()
            .find(|s| s.model != "unknown")
            .unwrap();
        assert_eq!(priced.estimated_cost_usd.as_deref(), Some("0.000150000"));
    }
}
