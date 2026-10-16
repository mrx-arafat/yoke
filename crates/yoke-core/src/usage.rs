//! Bounded local transcript accounting. Costs are estimates, never subscription bills.
use std::collections::BTreeMap;
use std::fs::{Metadata, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    quota::{self, Quota},
    redact::redact,
    store::Store,
};

/// The most transcript bytes one import counts: the newest ones.
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

/// The last turn's Codex counters in the order of the totals, only when every present
/// counter is a non-negative integer; the field is a restart hint, never an accounting source.
fn last_turn(info: &Value) -> Option<[i64; 4]> {
    let last = info.get("last_token_usage")?.as_object()?;
    let get = |key: &str| match last.get(key) {
        None => Some(0),
        Some(value) => value.as_i64().filter(|n| *n >= 0),
    };
    Some([
        get("input_tokens")?,
        get("output_tokens")?,
        get("cached_input_tokens")?,
        get("cache_write_input_tokens")?,
    ])
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

/// The newest quota reading in a transcript, dated by its line's timestamp.
struct QuotaSeen {
    quota: Quota,
    ts: String,
}

/// A transcript larger than the import bound is counted from its newest bytes only. The
/// usage in the skipped older part is not counted, and this says how much was skipped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartialImport {
    pub transcript_bytes: u64,
    pub skipped_bytes: u64,
    pub limit_bytes: u64,
}

fn partial_key(agent: &str, session: &str) -> String {
    format!("{PARTIAL_PREFIX}{agent}.{session}")
}

const PARTIAL_PREFIX: &str = "usage.partial.";

struct Snapshot {
    observed: Fingerprint,
    samples: BTreeMap<String, Sample>,
    quota: Option<QuotaSeen>,
    partial: Option<PartialImport>,
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
    rate_limits: Value,
}

/// Import a complete bounded JSONL snapshot atomically. No message content is persisted.
/// A transcript over the bound is counted from its newest bytes, and the skipped part is
/// recorded as a [`PartialImport`]. Valid snapshots without usage are no-ops, returning no
/// newly imported summaries.
/// A snapshot is only committed while the transcript is still the file it was read from,
/// so an importer that lost a race can never overwrite a newer or replaced transcript.
pub fn ingest(
    store: &Store,
    agent: &str,
    session: &str,
    project: &str,
    transcript: &Path,
) -> Result<Vec<UsageSummary>> {
    ingest_bounded(store, agent, session, project, transcript, MAX_BYTES)
}

fn ingest_bounded(
    store: &Store,
    agent: &str,
    session: &str,
    project: &str,
    transcript: &Path,
    limit: u64,
) -> Result<Vec<UsageSummary>> {
    ensure!(
        matches!(agent, "claude" | "codex"),
        "unsupported transcript agent"
    );
    let session = identifier(session)?;
    let project = identifier(project)?;
    for _ in 0..MAX_ATTEMPTS {
        let snapshot = read_snapshot(agent, &session, transcript, limit)?;
        if snapshot.samples.is_empty() {
            // Early lifecycle events may precede token counters. Do not erase an
            // earlier import when the current snapshot contains no accounting data.
            if let Some(seen) = &snapshot.quota {
                store.transaction(|tx| record_quota(tx, seen))?;
            }
            return Ok(Vec::new());
        }
        if commit_snapshot(store, agent, &session, &project, transcript, &snapshot)? {
            return summaries(store, Some(&project), Some(&session));
        }
    }
    bail!("usage transcript kept changing during import")
}

fn read_snapshot(agent: &str, session: &str, transcript: &Path, limit: u64) -> Result<Snapshot> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(transcript)
        .context("open usage transcript")?;
    let metadata = file.metadata()?;
    ensure!(metadata.is_file(), "transcript must be a regular file");
    let observed = fingerprint(&metadata);
    // Past the bound only the newest bytes are read, from the first line that starts inside
    // them; `take` stops at the size seen now, so a growing file is caught by the commit check.
    let size = metadata.len();
    let start = size.saturating_sub(limit);
    if start > 0 {
        file.seek(SeekFrom::Start(start - 1))?;
    }
    let mut reader = BufReader::new(file.take(size - start.saturating_sub(1)));
    let mut bytes = Vec::new();
    let mut skipped = 0;
    if start > 0 {
        // The byte before `start` ends a line, or the line it belongs to is dropped.
        skipped = start - 1 + reader.read_until(b'\n', &mut bytes)? as u64;
    }
    let mut baselined = skipped == 0;
    let mut samples = BTreeMap::<String, Sample>::new();
    let mut previous = [0i64; 4];
    let mut quota = None;
    let mut model = "unknown".to_owned();
    let mut ordinal = 0u64;
    loop {
        bytes.clear();
        if reader.read_until(b'\n', &mut bytes)? == 0 {
            break;
        }
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
            // Read before the counters are skipped: a repeated or counter-less event still
            // carries the account's current limits.
            if let (Some(found), Some(ts)) = (
                Quota::from_codex(&payload.rate_limits, 0),
                timestamp(&row.timestamp),
            ) {
                quota = Some(QuotaSeen { quota: found, ts });
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
            // Counters are cumulative, so with the older part skipped the first count only sets
            // the baseline; counting it would date everything before it to this moment.
            if !baselined {
                baselined = true;
                previous = current;
                continue;
            }
            // Codex repeats a token count without new usage.
            if current == previous {
                continue;
            }
            // The total restarts from zero on a new conversation segment, such as after a
            // thread settings change. A restart shows as a total that decreased, or as a total equal to
            // the last turn's usage while an earlier total existed. Either starts a new
            // segment counted from zero: samples before it stay, and only usage since the
            // restart is added, so re-importing the file yields the same samples.
            let restarted = current.iter().zip(previous).any(|(n, before)| *n < before)
                || (previous != [0; 4] && last_turn(&payload.info) == Some(current));
            if restarted {
                previous = [0; 4];
            }
            let mut delta = [0; 4];
            for index in 0..4 {
                delta[index] = current[index] - previous[index];
            }
            previous = current;
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
    Ok(Snapshot {
        observed,
        samples,
        quota,
        partial: (skipped > 0).then_some(PartialImport {
            transcript_bytes: size,
            skipped_bytes: skipped,
            limit_bytes: limit,
        }),
    })
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
        if let Some(seen) = &snapshot.quota {
            record_quota(tx, seen)?;
        }
        // A complete import clears an earlier partial marker, as when a transcript is replaced.
        let key = partial_key(agent, session);
        match &snapshot.partial {
            Some(partial) => tx.execute("INSERT INTO kv(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key, serde_json::to_string(partial)?])?,
            None => tx.execute("DELETE FROM kv WHERE key=?1", [key])?,
        };
        Ok(true)
    })
}

/// Merge a transcript's newest quota reading into the state. SQLite dates it so the one
/// RFC 3339 reader already used for samples decides what the timestamp means.
fn record_quota(tx: &rusqlite::Connection, seen: &QuotaSeen) -> Result<()> {
    let observed_at: Option<i64> =
        tx.query_row("SELECT unixepoch(?1)", [&seen.ts], |row| row.get(0))?;
    if let Some(observed_at) = observed_at {
        quota::record_in(
            tx,
            &Quota {
                observed_at,
                ..seen.quota.clone()
            },
        )?;
    }
    Ok(())
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

/// Schema version that added `usage_samples.ts`, which time-bounded totals and session spans need.
pub const TIME_SCHEMA: usize = 12;

/// Totals for one agent.
#[derive(Debug, Serialize)]
pub struct AgentTotals {
    pub agent: String,
    #[serde(flatten)]
    pub totals: UsageTotals,
}

/// Sessions whose transcript was only partly counted, by agent and session id.
pub fn partial_imports(store: &Store) -> Result<BTreeMap<(String, String), PartialImport>> {
    let mut query = store
        .connection()
        .prepare("SELECT key,value FROM kv WHERE key LIKE 'usage.partial.%' LIMIT 1000")?;
    let rows = query.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut found = BTreeMap::new();
    for row in rows {
        let (key, value) = row?;
        // A marker that does not parse is ignored: it can only hide a note, never usage.
        if let (Some((agent, session)), Ok(partial)) = (
            key[PARTIAL_PREFIX.len()..].split_once('.'),
            serde_json::from_str(&value),
        ) {
            found.insert((agent.to_owned(), session.to_owned()), partial);
        }
    }
    Ok(found)
}

/// One session across every model it used.
#[derive(Debug, Serialize)]
pub struct SessionUsage {
    pub agent: String,
    pub session_id: String,
    pub project: String,
    pub models: Vec<String>,
    /// The earlier of the recorded session start and the first usage sample, because the
    /// start is recorded late when Yoke was installed mid-session; None when nothing is dated.
    pub started_at: Option<i64>,
    /// Time of the newest dated usage sample.
    pub last_at: Option<i64>,
    /// Set when only the newest part of the transcript was counted.
    pub partial: Option<PartialImport>,
    #[serde(flatten)]
    pub totals: UsageTotals,
}

/// Per-agent totals of samples at or after `since` (Unix seconds); undated samples are left out.
pub fn totals_by_agent(store: &Store, since: i64) -> Result<Vec<AgentTotals>> {
    let mut query = store.connection().prepare(&format!(
        "SELECT agent,{TOTALS} FROM usage_samples WHERE ts>=?1 GROUP BY agent ORDER BY agent"
    ))?;
    let rows = query.query_map([since], |row| {
        Ok(AgentTotals {
            agent: row.get(0)?,
            totals: read_totals(row, 1)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// The sessions with the newest usage first; sessions without dated samples come last.
pub fn recent_sessions(store: &Store, limit: usize) -> Result<Vec<SessionUsage>> {
    let mut query = store.connection().prepare(&format!(
        "SELECT u.agent,u.session_id,u.project,json_group_array(DISTINCT u.model),coalesce(min(s.started_at,min(u.ts)),s.started_at,min(u.ts)),max(u.ts),{TOTALS}
         FROM usage_samples u LEFT JOIN sessions s ON s.id=u.session_id
         GROUP BY u.agent,u.session_id ORDER BY max(u.ts) IS NULL,max(u.ts) DESC,u.session_id LIMIT ?1"
    ))?;
    let partial = partial_imports(store)?;
    let rows = query.query_map([limit.min(1000) as i64], |row| {
        let models: String = row.get(3)?;
        let mut models: Vec<String> = serde_json::from_str(&models).unwrap_or_default();
        models.sort();
        let (agent, session_id): (String, String) = (row.get(0)?, row.get(1)?);
        Ok(SessionUsage {
            partial: partial.get(&(agent.clone(), session_id.clone())).cloned(),
            agent,
            session_id,
            project: row.get(2)?,
            models,
            started_at: row.get(4)?,
            last_at: row.get(5)?,
            totals: read_totals(row, 6)?,
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
        let older = read_snapshot("claude", "s", &path, MAX_BYTES).unwrap();
        std::fs::write(
            &path,
            format!("{}\n{}\n", message("m1", 10), message("m2", 20)),
        )
        .unwrap();
        let newer = read_snapshot("claude", "s", &path, MAX_BYTES).unwrap();
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

    #[test]
    fn codex_counter_resets_start_a_new_segment_without_freezing_or_double_counting() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("state.db")).unwrap();
        let path = dir.path().join("fixture.jsonl");
        // Same field shape as a real rollout: a cumulative total plus the last turn's usage,
        // as [input, output, cached] where input includes the cached tokens.
        let event = |total: [u64; 3], last: [u64; 3]| {
            let usage = |[input, output, cached]: [u64; 3]| json!({"input_tokens":input,"cached_input_tokens":cached,"output_tokens":output,"reasoning_output_tokens":0,"total_tokens":input+output});
            json!({"type":"event_msg","timestamp":"2026-10-14T10:00:00Z","payload":{"type":"token_count","info":{"total_token_usage":usage(total),"last_token_usage":usage(last),"model_context_window":272000}}}).to_string()
        };
        let totals = |store: &Store| {
            let t = report(store, None, Some("s"), &UsageRange::default(), Period::Day)
                .unwrap()
                .totals;
            (
                t.samples,
                t.input_tokens,
                t.output_tokens,
                t.cache_read_tokens,
            )
        };
        let before_reset = format!(
            "{}\n{}\n{}\n",
            event([100, 10, 20], [100, 10, 20]),
            event([100, 10, 20], [100, 10, 20]),
            event([300, 30, 60], [200, 20, 40])
        );
        std::fs::write(&path, &before_reset).unwrap();
        ingest(&store, "codex", "s", "/project", &path).unwrap();
        assert_eq!(totals(&store), (2, 240, 30, 60));
        // The total restarts at zero on a new conversation segment: once as
        // a decrease (repeated, as agents repeat token counts), then again without a decrease
        // but equal to the last turn's usage, where the cached tokens outgrow the delta.
        let after_reset = format!(
            "{before_reset}{}\n{}\n{}\n{}\n",
            event([50, 5, 10], [50, 5, 10]),
            event([50, 5, 10], [50, 5, 10]),
            event([120, 12, 30], [70, 7, 20]),
            event([130, 13, 100], [130, 13, 100])
        );
        std::fs::write(&path, &after_reset).unwrap();
        let imported = ingest(&store, "codex", "s", "/project", &path).map(|rows| rows.len());
        assert_eq!(
            (imported.map_err(|error| error.to_string()), totals(&store)),
            (Ok(1), (5, 360, 55, 190))
        );
        // Re-importing the same file keeps the same samples and never double counts.
        ingest(&store, "codex", "s", "/project", &path).unwrap();
        assert_eq!(totals(&store), (5, 360, 55, 190));
        // Accounting continues in the new segment after the reset.
        std::fs::write(
            &path,
            format!("{after_reset}{}\n", event([150, 15, 110], [20, 2, 10])),
        )
        .unwrap();
        ingest(&store, "codex", "s", "/project", &path).unwrap();
        assert_eq!(totals(&store), (6, 370, 57, 200));
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
    fn codex_count(ts: &str, input: u64, used_percent: f64, info: bool) -> Value {
        let mut row = json!({"timestamp":ts,"type":"event_msg","payload":{"type":"token_count","info":null,
            "rate_limits":{"plan_type":"pro","primary":{"used_percent":used_percent,"window_minutes":10080,"resets_at":1_792_649_595},"secondary":null}}});
        if info {
            row["payload"]["info"] =
                json!({"total_token_usage":{"input_tokens":input,"output_tokens":1}});
        }
        row
    }

    #[test]
    fn codex_quota_is_recorded_by_the_import_and_an_older_session_never_regresses_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("state.db")).unwrap();
        let path = dir.path().join("fixture.jsonl");
        let ingest_rows = |session: &str, rows: &[Value]| {
            let lines: String = rows.iter().map(|row| format!("{row}\n")).collect();
            std::fs::write(&path, lines).unwrap();
            ingest(&store, "codex", session, "/project", &path).unwrap()
        };
        assert!(crate::quota::load(&store, "codex").unwrap().is_none());

        ingest_rows(
            "new",
            &[codex_count("2026-10-16T12:00:00Z", 10, 34.0, true)],
        );
        let stored = crate::quota::load(&store, "codex").unwrap().unwrap();
        assert_eq!(stored.observed_at, 1_792_152_000);
        assert_eq!(stored.plan.as_deref(), Some("pro"));
        assert_eq!(
            (
                stored.windows[0].window_minutes,
                stored.windows[0].used_percent
            ),
            (10_080, 34.0)
        );

        ingest_rows(
            "old",
            &[codex_count("2026-10-15T12:00:00Z", 10, 20.0, true)],
        );
        assert_eq!(
            crate::quota::load(&store, "codex").unwrap().unwrap(),
            stored
        );

        // A counter-less event still carries the quota, though it has no usage to import.
        let quota_only = ingest_rows(
            "quiet",
            &[codex_count("2026-10-16T13:00:00Z", 0, 40.0, false)],
        );
        assert!(quota_only.is_empty());
        let latest = crate::quota::load(&store, "codex").unwrap().unwrap();
        assert_eq!(
            (latest.windows[0].used_percent, latest.observed_at),
            (40.0, 1_792_155_600)
        );
    }

    #[test]
    fn agent_totals_and_recent_sessions_follow_sample_times() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("state.db")).unwrap();
        let path = dir.path().join("fixture.jsonl");
        let message = |session: &str, id: &str, model: &str, input: u64, ts: &str| json!({"timestamp":ts,"type":"assistant","sessionId":session,"message":{"id":id,"model":model,"usage":{"input_tokens":input,"output_tokens":100,"service_tier":"standard","inference_geo":"global","speed":"standard"}}});
        let import = |agent: &str, session: &str, rows: Vec<Value>| {
            let lines: String = rows.iter().map(|row| format!("{row}\n")).collect();
            std::fs::write(&path, lines).unwrap();
            ingest(&store, agent, session, "/work/app", &path).unwrap();
        };
        import(
            "claude",
            "recent",
            vec![
                message(
                    "recent",
                    "m1",
                    "claude-haiku-5-5",
                    1000,
                    "2026-10-16T10:00:00Z",
                ),
                message(
                    "recent",
                    "m2",
                    "claude-haiku-5-5",
                    500,
                    "2026-10-16T11:30:00Z",
                ),
            ],
        );
        import(
            "claude",
            "older",
            vec![message(
                "older",
                "m3",
                "unknown",
                70,
                "2026-10-10T08:00:00Z",
            )],
        );
        import(
            "codex",
            "cx",
            vec![codex_count("2026-10-16T09:00:00Z", 40, 33.0, true)],
        );

        // A session row written after its first sample (Yoke installed mid-session) must not
        // push the start past the usage; one written before it keeps the earlier time.
        for (id, started) in [("recent", 1_792_150_000), ("cx", 1_792_140_000)] {
            store
                .connection()
                .execute(
                    "INSERT INTO sessions(id,agent,project,started_at) VALUES (?1,'x','/work/app',?2)",
                    params![id, started],
                )
                .unwrap();
        }
        let since = 1_792_108_800; // 2026-10-16T00:00:00Z
        let totals = totals_by_agent(&store, since).unwrap();
        let summary: Vec<_> = totals
            .iter()
            .map(|t| {
                (
                    t.agent.as_str(),
                    t.totals.input_tokens,
                    t.totals.output_tokens,
                )
            })
            .collect();
        assert_eq!(summary, [("claude", 1500, 200), ("codex", 40, 1)]);
        assert_eq!(
            totals[0].totals.estimated_cost_usd.as_deref(),
            Some("0.000250000")
        );

        let sessions = recent_sessions(&store, 2).unwrap();
        let ids: Vec<_> = sessions.iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(ids, ["recent", "cx"]);
        assert_eq!(
            (
                sessions[0].started_at,
                sessions[0].last_at,
                sessions[0].models.clone()
            ),
            (
                Some(1_792_144_800),
                Some(1_792_150_200),
                vec!["claude-haiku-5-5".to_owned()]
            )
        );
        assert_eq!(sessions[0].totals.input_tokens, 1500);
        assert_eq!(sessions[1].started_at, Some(1_792_140_000));
    }
    #[test]
    fn a_transcript_over_the_bound_counts_its_newest_part_and_records_what_was_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("state.db")).unwrap();
        let path = dir.path().join("fixture.jsonl");
        let claude: Vec<String> = (1..=10)
            .map(|i| {
                json!({"type":"assistant","sessionId":"s","message":{"id":format!("m{i:02}"),"model":"unknown","usage":{"input_tokens":1000 + i,"output_tokens":1}}}).to_string()
            })
            .collect();
        let line = claude[0].len() as u64 + 1;
        let write = |lines: &[String]| {
            std::fs::write(
                &path,
                lines.iter().map(|l| format!("{l}\n")).collect::<String>(),
            )
            .unwrap();
        };
        // The bound ends inside the 7th line, so the newest three lines are counted.
        let limit = 3 * line + 10;
        write(&claude);
        let counted = ingest_bounded(&store, "claude", "s", "/project", &path, limit).unwrap();
        assert_eq!(
            (counted[0].input_tokens, counted[0].output_tokens),
            (1008 + 1009 + 1010, 3)
        );
        let marker = PartialImport {
            transcript_bytes: 10 * line,
            skipped_bytes: 7 * line,
            limit_bytes: limit,
        };
        assert_eq!(
            partial_imports(&store).unwrap()[&("claude".to_owned(), "s".to_owned())],
            marker
        );
        assert_eq!(recent_sessions(&store, 5).unwrap()[0].partial, Some(marker));

        // Once the transcript fits again the whole of it is counted and the marker is gone.
        write(&claude[..2]);
        let whole = ingest_bounded(&store, "claude", "s", "/project", &path, limit).unwrap();
        assert_eq!(whole[0].input_tokens, 1001 + 1002);
        assert!(partial_imports(&store).unwrap().is_empty());

        // Codex counters are cumulative: the first counted event is only the baseline, so the
        // skipped part is not dated to it.
        let codex: Vec<String> = (1..=6)
            .map(|i| {
                codex_count(&format!("2026-10-16T09:0{i}:00Z"), 100 * i, 1.0, true).to_string()
            })
            .collect();
        let line = codex[0].len() as u64 + 1;
        write(&codex);
        let counted =
            ingest_bounded(&store, "codex", "c", "/project", &path, 3 * line + 10).unwrap();
        assert_eq!(
            (counted[0].input_tokens, counted[0].output_tokens),
            (200, 0)
        );
        assert_eq!(partial_imports(&store).unwrap().len(), 1);
    }
}
