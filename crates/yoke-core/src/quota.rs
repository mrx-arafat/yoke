//! Reported quota windows and bounded local history, separate from token/cost accounting.
//! Projections describe recent observed burn, never a provider guarantee or an enforced budget.
use std::fs::OpenOptions;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::store::Store;
use crate::timefmt::short_time;

/// Unchanged figures are still rewritten after this long, so `observed_at` keeps meaning
/// "last confirmed" without a write on every statusline redraw.
const REFRESH_SECS: i64 = 60;
const MAX_FILE_BYTES: u64 = 128 * 1024;
const MAX_WINDOWS: usize = 8;
const HISTORY_PER_WINDOW: usize = 64;
const TREND_SECS: i64 = 3600;
pub const FRESH_SECS: i64 = 300;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub observed_at: i64,
    #[serde(flatten)]
    pub window: Window,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Trend {
    pub percent_per_hour: Option<f64>,
    /// Only populated when recent burn would exhaust the window before its reset.
    pub exhausts_at: Option<i64>,
    pub span_secs: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub window_minutes: u32,
    pub used_percent: f64,
    /// Unix seconds.
    pub resets_at: i64,
}

impl Window {
    /// `5h`, `7d`, or the exact length for any other window.
    pub fn label(&self) -> String {
        let minutes = self.window_minutes;
        if minutes.is_multiple_of(1440) {
            format!("{}d", minutes / 1440)
        } else if minutes.is_multiple_of(60) {
            format!("{}h", minutes / 60)
        } else {
            format!("{minutes}m")
        }
    }

    pub fn remaining_percent(&self) -> f64 {
        (100.0 - self.used_percent).max(0.0)
    }

    /// The window already reset, so its figure describes the previous window.
    pub fn expired(&self, now: i64) -> bool {
        self.resets_at <= now
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Quota {
    pub agent: String,
    /// Newest source observation, in Unix seconds; history retains per-window times.
    pub observed_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// Shortest window first.
    pub windows: Vec<Window>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<Observation>,
}

fn window(used: &Value, resets: &Value, window_minutes: u32) -> Option<Window> {
    Some(Window {
        window_minutes,
        used_percent: used
            .as_f64()
            .filter(|p| p.is_finite() && (0.0..=100.0).contains(p))?,
        resets_at: resets.as_i64().filter(|t| *t >= 0)?,
    })
}

impl Quota {
    fn empty(agent: &str, observed_at: i64) -> Self {
        Self {
            agent: agent.into(),
            observed_at,
            plan: None,
            windows: Vec::new(),
            history: Vec::new(),
        }
    }

    /// From a Claude Code statusline payload: `rate_limits.five_hour` and `.seven_day`.
    /// Absent or malformed windows are left out; `None` when no window is usable.
    pub fn from_claude(payload: &Value, observed_at: i64) -> Option<Self> {
        let limits = &payload["rate_limits"];
        let mut quota = Self::empty("claude", observed_at);
        for (key, minutes) in [("five_hour", 300), ("seven_day", 10_080)] {
            let slot = &limits[key];
            if let Some(w) = window(&slot["used_percentage"], &slot["resets_at"], minutes) {
                quota.absorb_window(w);
            }
        }
        quota.finish()
    }

    /// From a Codex rollout `token_count` event's `rate_limits`: `primary` and `secondary`
    /// carry their own `window_minutes`, so a plan with one window yields one.
    pub fn from_codex(limits: &Value, observed_at: i64) -> Option<Self> {
        let mut quota = Self::empty("codex", observed_at);
        for key in ["primary", "secondary"] {
            let slot = &limits[key];
            let minutes = slot["window_minutes"]
                .as_u64()
                .and_then(|m| u32::try_from(m).ok())
                .filter(|m| *m > 0);
            if let Some(w) =
                minutes.and_then(|m| window(&slot["used_percent"], &slot["resets_at"], m))
            {
                quota.absorb_window(w);
            }
        }
        quota.plan = limits["plan_type"]
            .as_str()
            .map(|plan| {
                plan.chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                    .take(32)
                    .collect::<String>()
            })
            .filter(|plan| !plan.is_empty());
        quota.finish()
    }

    fn finish(mut self) -> Option<Self> {
        self.windows.retain(|w| valid_reset(w, self.observed_at));
        self.windows.sort_by_key(|w| w.window_minutes);
        for w in self.windows.clone() {
            self.remember(Observation {
                observed_at: self.observed_at,
                window: w,
            });
        }
        (!self.windows.is_empty()).then_some(self)
    }

    /// Keeps the newer reading of a window: a later reset wins, and within one reset the higher
    /// percent wins, because usage only grows until the window resets. Returns whether the
    /// reading was kept or confirmed the stored one, rather than being older than it.
    fn absorb_window(&mut self, incoming: Window) -> bool {
        match self
            .windows
            .iter_mut()
            .find(|w| w.window_minutes == incoming.window_minutes)
        {
            None => {
                self.windows.push(incoming);
                true
            }
            Some(stored) if incoming == *stored => true,
            Some(stored)
                if incoming.resets_at > stored.resets_at
                    || (incoming.resets_at == stored.resets_at
                        && incoming.used_percent > stored.used_percent) =>
            {
                *stored = incoming;
                true
            }
            Some(_) => false,
        }
    }

    /// Fold in another reading in any order; only readings that are not older than the stored
    /// ones advance `observed_at`.
    pub fn absorb(&mut self, incoming: &Quota) {
        if self.agent != incoming.agent {
            return;
        }
        // Old records have no history. Seed once; never re-date one window when only another
        // window was confirmed by a partial payload.
        if self.history.is_empty() {
            for window in self.windows.clone() {
                self.remember(Observation {
                    observed_at: self.observed_at,
                    window,
                });
            }
        }
        let mut confirmed = false;
        for w in &incoming.windows {
            if !valid_reset(w, incoming.observed_at) {
                continue;
            }
            confirmed |= self.absorb_window(w.clone());
        }
        if confirmed {
            self.observed_at = self.observed_at.max(incoming.observed_at);
            self.plan = incoming.plan.clone().or(self.plan.take());
        }
        self.windows.sort_by_key(|w| w.window_minutes);
        for sample in &incoming.history {
            self.remember(sample.clone());
        }
        // Also accepts legacy records with no history.
        if incoming.history.is_empty() {
            for window in &incoming.windows {
                self.remember(Observation {
                    observed_at: incoming.observed_at,
                    window: window.clone(),
                });
            }
        }
    }

    fn remember(&mut self, sample: Observation) {
        let minutes = sample.window.window_minutes;
        if !self.windows.iter().any(|w| w.window_minutes == minutes) {
            return;
        }
        let indices: Vec<_> = self
            .history
            .iter()
            .enumerate()
            .filter(|(_, s)| s.window.window_minutes == minutes)
            .map(|(i, _)| i)
            .collect();
        if let Some(&last) = indices.last() {
            let previous = &self.history[last];
            if sample.observed_at == previous.observed_at
                && sample.window.resets_at == previous.window.resets_at
                && sample.window.used_percent > previous.window.used_percent
            {
                self.history[last] = sample;
                return;
            }
            if sample.observed_at <= previous.observed_at
                || sample.window.resets_at < previous.window.resets_at
                || (sample.window.resets_at == previous.window.resets_at
                    && sample.window.used_percent < previous.window.used_percent)
            {
                return;
            }
            // Keep a baseline plus a current point, then at most one point per minute.
            if indices.len() >= 2 {
                let baseline = &self.history[indices[indices.len() - 2]];
                if sample.window.resets_at == baseline.window.resets_at
                    && sample.observed_at - baseline.observed_at < REFRESH_SECS
                {
                    self.history.remove(last);
                }
            }
        }
        self.history.push(sample);
        let count = self
            .history
            .iter()
            .filter(|s| s.window.window_minutes == minutes)
            .count();
        if count > HISTORY_PER_WINDOW {
            if let Some(first) = self
                .history
                .iter()
                .position(|s| s.window.window_minutes == minutes)
            {
                self.history.remove(first);
            }
        }
    }

    pub fn latest(&self, window: &Window) -> i64 {
        self.history
            .iter()
            .rev()
            .find(|s| s.window == *window)
            .map_or(self.observed_at, |s| s.observed_at)
    }

    pub fn fresh(&self, window: &Window, now: i64) -> bool {
        // Conflicting out-of-order sources can raise the retained peak without a usable
        // chronological endpoint. Do not borrow another window's observation time for it.
        if !self.history.is_empty() && !self.history.iter().any(|s| s.window == *window) {
            return false;
        }
        let at = self.latest(window);
        !window.expired(now) && (0..=FRESH_SECS).contains(&now.saturating_sub(at))
    }

    /// Average percentage points/hour over up to one recent hour, with a minute of evidence.
    pub fn trend(&self, window: &Window, now: i64) -> Trend {
        if !self.fresh(window, now) {
            return Trend::default();
        }
        let at = self.latest(window);
        let Some(base) = self.history.iter().find(|s| {
            s.window.window_minutes == window.window_minutes
                && s.window.resets_at == window.resets_at
                && (REFRESH_SECS..=TREND_SECS).contains(&at.saturating_sub(s.observed_at))
                && s.window.used_percent <= window.used_percent
        }) else {
            return Trend::default();
        };
        let span = at - base.observed_at;
        let rate = (window.used_percent - base.window.used_percent) * 3600.0 / span as f64;
        let exhausts_at = if rate > 0.0 && window.used_percent < 100.0 {
            let seconds = window.remaining_percent() * 3600.0 / rate;
            // Bound the float before converting or adding untrusted timestamps.
            (seconds < (window.resets_at - at) as f64)
                .then(|| at.saturating_add(seconds.ceil() as i64))
                .filter(|t| *t < window.resets_at)
        } else {
            None
        };
        Trend {
            percent_per_hour: Some(rate),
            exhausts_at,
            span_secs: Some(span),
        }
    }

    pub fn warning(&self, window: &Window, now: i64, threshold: u8) -> Option<String> {
        if !self.fresh(window, now) {
            return None;
        }
        if window.used_percent >= f64::from(threshold) {
            Some(format!(
                "{} {}% used ({}% left)",
                window.label(),
                percent(window.used_percent),
                percent(window.remaining_percent())
            ))
        } else {
            self.trend(window, now).exhausts_at.map(|at| {
                if at <= now {
                    return format!(
                        "{} may already have reached its limit at recent burn",
                        window.label()
                    );
                }
                format!(
                    "{} may reach its limit in {} at recent burn",
                    window.label(),
                    crate::timefmt::duration_text((at - now).max(0))
                )
            })
        }
    }

    /// The same windows and plan, whatever `observed_at` says.
    fn same_figures(&self, other: &Quota) -> bool {
        self.windows == other.windows && self.plan == other.plan
    }

    /// Stored quota is user-writable data: reject anything the constructors could not produce.
    fn checked(self, agent: &str) -> Result<Self> {
        ensure!(self.agent == agent, "quota record belongs to another agent");
        ensure!(
            !self.windows.is_empty()
                && self.windows.len() <= MAX_WINDOWS
                && self.observed_at >= 0
                && self.history.len() <= MAX_WINDOWS * HISTORY_PER_WINDOW
                && self
                    .history
                    .iter()
                    .all(|s| s.observed_at <= self.observed_at
                        && valid_reset(&s.window, s.observed_at)
                        && s.window.used_percent.is_finite()
                        && (0.0..=100.0).contains(&s.window.used_percent)
                        && self
                            .windows
                            .iter()
                            .any(|w| w.window_minutes == s.window.window_minutes))
                && self.windows.iter().all(|w| w.window_minutes > 0
                    && w.resets_at >= 0
                    && valid_reset(w, self.observed_at)
                    && w.used_percent.is_finite()
                    && (0.0..=100.0).contains(&w.used_percent)),
            "quota record is out of range"
        );
        for window in &self.windows {
            let samples: Vec<_> = self
                .history
                .iter()
                .filter(|s| s.window.window_minutes == window.window_minutes)
                .collect();
            ensure!(
                samples.len() <= HISTORY_PER_WINDOW
                    && samples
                        .windows(2)
                        .all(|pair| pair[0].observed_at < pair[1].observed_at
                            && (pair[0].window.resets_at < pair[1].window.resets_at
                                || pair[0].window.resets_at == pair[1].window.resets_at
                                    && pair[0].window.used_percent <= pair[1].window.used_percent)),
                "quota history is not monotonic"
            );
        }
        Ok(self)
    }

    /// `5h 8% · 7d 63% (resets Mon 10:00)`: active percentages and the most-used window's reset.
    pub fn status_field(&self, now: i64, offset_secs: i64) -> Option<String> {
        let binding =
            self.windows
                .iter()
                .filter(|w| !w.expired(now))
                .fold(None::<&Window>, |best, w| match best {
                    Some(b) if b.used_percent >= w.used_percent => Some(b),
                    _ => Some(w),
                })?;
        let percents = self
            .windows
            .iter()
            .filter(|w| !w.expired(now))
            .map(|w| format!("{} {}%", w.label(), percent(w.used_percent)))
            .collect::<Vec<_>>()
            .join(" · ");
        Some(format!(
            "{percents} (resets {})",
            short_time(binding.resets_at, now, offset_secs)
        ))
    }
}

// Permit five minutes of source clock skew, never an indefinitely pinned quota window.
fn valid_reset(window: &Window, observed_at: i64) -> bool {
    observed_at >= 0
        && window.window_minutes > 0
        && window.resets_at >= 0
        && window.resets_at
            <= observed_at.saturating_add(i64::from(window.window_minutes) * 60 + 300)
}

/// `8` for a whole percent, `8.5` otherwise.
pub fn percent(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

/// Where the statusline records Claude's quota; it is a file because only the statusline
/// receives it, and the statusline never writes the state database.
pub fn claude_file(home: &Path) -> PathBuf {
    home.join(".yoke/quota-claude.json")
}

/// Read a quota file. A missing file is `None`; a symlink, a non-regular file, an oversized
/// or malformed one, or one for another agent is an error.
pub fn read_file(path: &Path, agent: &str) -> Result<Option<Quota>> {
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("open quota file"),
    };
    ensure!(
        file.metadata()?.is_file(),
        "quota file must be a regular file"
    );
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_FILE_BYTES,
        "quota file is too large"
    );
    let quota: Quota = serde_json::from_slice(&bytes).context("malformed quota file")?;
    quota.checked(agent).map(Some)
}

/// Merge a reading into the quota file with one atomic rename. It never creates the state
/// directory, and skips the write when nothing changed recently. Returns whether it wrote.
pub fn record_file(path: &Path, incoming: &Quota, now: i64) -> Result<bool> {
    incoming.clone().checked(&incoming.agent)?;
    let directory = path.parent().context("quota file has no directory")?;
    if !std::fs::symlink_metadata(directory).is_ok_and(|meta| meta.is_dir()) {
        return Ok(false);
    }
    // Status callbacks from concurrent sessions share an account. Atomic rename alone would
    // let a late writer erase a newer reading and its history; never wait on a paint.
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(directory.join(".quota.lock"))?;
    ensure!(
        lock.metadata()?.is_file(),
        "quota lock must be a regular file"
    );
    // SAFETY: flock borrows a live file descriptor and does not access Rust memory.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock {
            return Ok(false);
        }
        return Err(error).context("lock quota file");
    }
    // A damaged file is replaced rather than blocking every later reading.
    let current = read_file(path, &incoming.agent).unwrap_or(None);
    let mut merged = current.clone().unwrap_or_else(|| incoming.clone());
    merged.absorb(incoming);
    if let Some(current) = &current {
        if current.same_figures(&merged) && now.saturating_sub(current.observed_at) < REFRESH_SECS {
            return Ok(false);
        }
    }
    let mut staged = tempfile::Builder::new()
        .prefix(".quota-")
        .tempfile_in(directory)
        .context("stage quota file")?;
    serde_json::to_writer(&mut staged, &merged)?;
    staged.persist(path).map_err(|error| error.error)?;
    Ok(true)
}

fn key(agent: &str) -> String {
    format!("quota.{agent}")
}

/// The quota stored in the state database for an agent whose figures arrive with hooks.
pub fn load(store: &Store, agent: &str) -> Result<Option<Quota>> {
    load_from(store.connection(), agent)
}

fn load_from(connection: &Connection, agent: &str) -> Result<Option<Quota>> {
    let text: Option<String> = connection
        .query_row("SELECT value FROM kv WHERE key=?1", [key(agent)], |row| {
            row.get(0)
        })
        .optional()?;
    text.map(|text| {
        serde_json::from_str::<Quota>(&text)
            .context("malformed stored quota")?
            .checked(agent)
    })
    .transpose()
}

/// Merge a reading into the state database inside the caller's transaction.
pub(crate) fn record_in(connection: &Connection, incoming: &Quota) -> Result<()> {
    incoming.clone().checked(&incoming.agent)?;
    let current = load_from(connection, &incoming.agent).unwrap_or(None);
    let mut merged = current.clone().unwrap_or_else(|| incoming.clone());
    merged.absorb(incoming);
    if current.as_ref() != Some(&merged) {
        connection.execute(
            "INSERT INTO kv(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key(&incoming.agent), serde_json::to_string(&merged)?],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn reset_times_cannot_outlive_the_observed_window_and_clock_slack() {
        let at = 10_000;
        let payload = |reset| serde_json::json!({"rate_limits":{"five_hour":{"used_percentage":5,"resets_at":reset}}});
        assert!(super::Quota::from_claude(&payload(at + 18_300), at).is_some());
        assert!(super::Quota::from_claude(&payload(at + 18_301), at).is_none());
        let forged = super::Quota {
            agent: "claude".into(),
            observed_at: at,
            plan: None,
            history: Vec::new(),
            windows: vec![super::Window {
                window_minutes: 300,
                used_percent: 5.0,
                resets_at: i64::MAX,
            }],
        };
        assert!(forged.checked("claude").is_err());
    }
    use super::*;
    use serde_json::json;

    /// 2026-10-16 10:00:00 UTC, a Friday.
    const NOW: i64 = 1_792_144_800;

    fn claude(five: (f64, i64), seven: (f64, i64), at: i64) -> Quota {
        Quota::from_claude(
            &json!({"rate_limits":{
                "five_hour":{"used_percentage":five.0,"resets_at":five.1},
                "seven_day":{"used_percentage":seven.0,"resets_at":seven.1}}}),
            at,
        )
        .unwrap()
    }

    #[test]
    fn recent_burn_projects_only_fresh_same_reset_readings() {
        let mut quota = claude((40.0, NOW + 7200), (20.0, NOW + 86400), NOW);
        quota.absorb(&claude((50.0, NOW + 7200), (20.0, NOW + 86400), NOW + 600));
        let trend = quota.trend(&quota.windows[0], NOW + 600);
        assert_eq!(trend.percent_per_hour, Some(60.0));
        assert_eq!(trend.exhausts_at, Some(NOW + 3600));
        assert!(quota.warning(&quota.windows[0], NOW + 600, 85).is_some());
        assert!(quota
            .trend(&quota.windows[0], NOW + 901)
            .percent_per_hour
            .is_none());
        assert!(quota
            .trend(&quota.windows[0], NOW + 599)
            .percent_per_hour
            .is_none());
        assert!(quota
            .trend(&quota.windows[1], NOW + 600)
            .exhausts_at
            .is_none());
        quota.absorb(&claude((1.0, NOW + 9000), (20.0, NOW + 86400), NOW + 700));
        assert!(quota
            .trend(&quota.windows[0], NOW + 700)
            .exhausts_at
            .is_none());
        assert!(quota.warning(&quota.windows[0], NOW + 700, 85).is_none());
    }

    #[test]
    fn quota_history_is_bounded_and_survives_file_and_database_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("state.db")).unwrap();
        let path = dir.path().join("quota.json");
        let mut quota = claude((1.0, NOW + 18000), (20.0, NOW + 86400), NOW);
        for minute in 1..200 {
            quota.absorb(&claude(
                (1.0 + minute as f64 / 4.0, NOW + 18000),
                (20.0, NOW + 86400),
                NOW + minute * 60,
            ));
        }
        assert!(quota.history.len() <= 128);
        record_file(&path, &quota, NOW + 12000).unwrap();
        assert_eq!(read_file(&path, "claude").unwrap().unwrap(), quota);
        store.transaction(|tx| record_in(tx, &quota)).unwrap();
        assert_eq!(load(&store, "claude").unwrap().unwrap(), quota);
        let trend = quota.trend(&quota.windows[0], NOW + 199 * 60);
        assert_eq!(trend.percent_per_hour, Some(15.0));
    }

    #[test]
    fn expired_quota_never_looks_current_or_warns() {
        let quota = claude((99.0, NOW - 1), (20.0, NOW + 86400), NOW);
        assert!(!quota.status_field(NOW, 0).unwrap().contains("99%"));
        assert!(quota.warning(&quota.windows[0], NOW, 85).is_none());
    }

    #[test]
    fn partial_confirmations_do_not_redate_other_windows_and_same_second_updates_keep_the_peak() {
        let mut quota = claude((10.0, NOW + 7200), (20.0, NOW + 86400), NOW);
        let partial = Quota::from_claude(
            &json!({"rate_limits":{"five_hour":{"used_percentage":20,"resets_at":NOW + 7200}}}),
            NOW + 600,
        )
        .unwrap();
        quota.absorb(&partial);
        assert!(quota.fresh(&quota.windows[0], NOW + 600));
        assert!(!quota.fresh(&quota.windows[1], NOW + 600));
        let peak = Quota::from_claude(
            &json!({"rate_limits":{"five_hour":{"used_percentage":25,"resets_at":NOW + 7200}}}),
            NOW + 600,
        )
        .unwrap();
        quota.absorb(&peak);
        assert_eq!(
            quota
                .history
                .iter()
                .rev()
                .find(|s| s.window.window_minutes == 300)
                .unwrap()
                .window
                .used_percent,
            25.0
        );
        assert_eq!(
            quota.trend(&quota.windows[0], NOW + 600).percent_per_hour,
            Some(90.0)
        );
        let conflicting = Quota::from_claude(
            &json!({"rate_limits":{"five_hour":{"used_percentage":30,"resets_at":NOW + 7200}}}),
            NOW + 500,
        )
        .unwrap();
        quota.absorb(&conflicting);
        assert!(!quota.fresh(&quota.windows[0], NOW + 600));
        assert!(quota
            .trend(&quota.windows[0], NOW + 600)
            .percent_per_hour
            .is_none());
    }

    #[test]
    fn quota_file_skips_a_busy_writer_without_losing_the_last_record() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("quota.json");
        let reading = claude((10.0, NOW + 7200), (20.0, NOW + 86400), NOW);
        record_file(&path, &reading, NOW).unwrap();
        let lock = std::fs::File::open(directory.path().join(".quota.lock")).unwrap();
        // SAFETY: the descriptor is live and owned by this test.
        assert_eq!(
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        assert!(!record_file(&path, &reading, NOW + 600).unwrap());
        assert_eq!(read_file(&path, "claude").unwrap().unwrap(), reading);
    }

    #[test]
    fn legacy_records_gain_history_without_migrating_or_losing_their_baseline() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path().join("state.db")).unwrap();
        let path = directory.path().join("quota.json");
        let mut legacy =
            serde_json::to_value(claude((10.0, NOW + 7200), (20.0, NOW + 86400), NOW)).unwrap();
        legacy.as_object_mut().unwrap().remove("history");
        store.set_kv("quota.claude", &legacy.to_string()).unwrap();
        std::fs::write(&path, legacy.to_string()).unwrap();
        assert!(load(&store, "claude").unwrap().unwrap().history.is_empty());
        let reading = claude((20.0, NOW + 7200), (20.0, NOW + 86400), NOW + 600);
        record_file(&path, &reading, NOW + 600).unwrap();
        store.transaction(|tx| record_in(tx, &reading)).unwrap();
        let stored = load(&store, "claude").unwrap().unwrap();
        assert_eq!(read_file(&path, "claude").unwrap().unwrap(), stored);
        assert_eq!(
            stored.trend(&stored.windows[0], NOW + 600).percent_per_hour,
            Some(60.0)
        );
    }

    #[test]
    fn claude_and_codex_payloads_become_labelled_windows_and_bad_windows_are_dropped() {
        let quota = claude((8.0, NOW + 3_600), (63.0, 1_792_609_200), NOW);
        let labels: Vec<_> = quota.windows.iter().map(Window::label).collect();
        assert_eq!(labels, ["5h", "7d"]);
        assert_eq!(quota.windows[1].remaining_percent(), 37.0);

        let codex = Quota::from_codex(
            &json!({"plan_type":"pro","primary":{"used_percent":33.0,"window_minutes":10080,"resets_at":1_792_649_595},"secondary":null}),
            NOW,
        )
        .unwrap();
        assert_eq!(
            (codex.windows.len(), codex.plan.as_deref()),
            (1, Some("pro"))
        );
        assert_eq!(codex.windows[0].label(), "7d");

        let hostile = json!({"rate_limits":{"five_hour":{"used_percentage":101,"resets_at":5},"seven_day":{"used_percentage":"x","resets_at":5}}});
        assert!(Quota::from_claude(&hostile, NOW).is_none());
        assert!(Quota::from_claude(&json!({}), NOW).is_none());
    }

    #[test]
    fn readings_merge_in_any_order_and_a_stale_reading_never_regresses_a_window() {
        let newer = claude((40.0, 2_000), (63.0, 9_000), NOW);
        let stale = claude((10.0, 2_000), (50.0, 9_000), NOW + 5);
        let mut merged = newer.clone();
        merged.absorb(&stale);
        assert_eq!(merged, newer, "a lower percent in the same window is older");

        let mut other = stale.clone();
        other.absorb(&newer);
        assert_eq!(other.windows, newer.windows);

        let next_window = claude((2.0, 20_000), (63.0, 9_000), NOW + 9);
        merged.absorb(&next_window);
        assert_eq!(
            (merged.windows[0].used_percent, merged.windows[0].resets_at),
            (2.0, 20_000)
        );
        assert_eq!(merged.observed_at, NOW + 9);
    }

    #[test]
    fn status_field_shows_every_percent_and_the_reset_of_the_window_nearest_its_limit() {
        let quota = claude((8.0, NOW + 3_600), (63.0, NOW + 3 * 86_400), NOW);
        assert_eq!(
            quota.status_field(NOW, 0).as_deref(),
            Some("5h 8% · 7d 63% (resets Mon 10:00)")
        );
        let nearly_out = claude((92.5, NOW + 3_600), (63.0, NOW + 3 * 86_400), NOW);
        assert_eq!(
            nearly_out.status_field(NOW, 0).as_deref(),
            Some("5h 92.5% · 7d 63% (resets 11:00)")
        );
    }

    #[test]
    fn the_file_is_atomic_private_never_creates_the_state_directory_and_skips_unchanged_writes() {
        let home = tempfile::tempdir().unwrap();
        let path = claude_file(home.path());
        let reading = claude((8.0, 2_000), (63.0, 9_000), NOW);
        assert!(!record_file(&path, &reading, NOW).unwrap());
        assert!(!home.path().join(".yoke").exists());

        std::fs::create_dir(home.path().join(".yoke")).unwrap();
        assert!(record_file(&path, &reading, NOW).unwrap());
        assert!(
            !record_file(&path, &reading, NOW + 30).unwrap(),
            "unchanged and fresh"
        );
        let later = Quota {
            observed_at: NOW + 61,
            ..reading.clone()
        };
        assert!(
            record_file(&path, &later, NOW + 61).unwrap(),
            "refreshed after a minute"
        );
        assert_eq!(
            read_file(&path, "claude").unwrap().unwrap().observed_at,
            NOW + 61
        );

        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        std::fs::write(&path, "{broken").unwrap();
        assert!(read_file(&path, "claude").is_err());
        assert!(
            record_file(&path, &reading, NOW + 90).unwrap(),
            "a damaged file is replaced"
        );
        assert!(
            read_file(&path, "codex").is_err(),
            "another agent's record is refused"
        );
    }
}
