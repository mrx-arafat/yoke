//! Latest agent quota windows: percent used and reset time as the agent last reported them.
//! Yoke records and shows these figures; it never computes, predicts or enforces a quota.
use std::fs::OpenOptions;
use std::io::Read;
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
const MAX_FILE_BYTES: u64 = 8 * 1024;
const MAX_WINDOWS: usize = 8;

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
    /// Unix seconds when a source last confirmed these figures.
    pub observed_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// Shortest window first.
    pub windows: Vec<Window>,
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
        self.windows.sort_by_key(|w| w.window_minutes);
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
        let mut confirmed = false;
        for w in &incoming.windows {
            confirmed |= self.absorb_window(w.clone());
        }
        if confirmed {
            self.observed_at = self.observed_at.max(incoming.observed_at);
            self.plan = incoming.plan.clone().or(self.plan.take());
        }
        self.windows.sort_by_key(|w| w.window_minutes);
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
                && self.windows.iter().all(|w| w.window_minutes > 0
                    && w.resets_at >= 0
                    && w.used_percent.is_finite()
                    && (0.0..=100.0).contains(&w.used_percent)),
            "quota record is out of range"
        );
        Ok(self)
    }

    /// `5h 8% · 7d 63% (resets Mon 10:00)`: every window's percent used, then the reset time
    /// of the window closest to its limit, which is the one that will block work first.
    pub fn status_field(&self, now: i64, offset_secs: i64) -> Option<String> {
        let binding = self
            .windows
            .iter()
            .fold(None::<&Window>, |best, w| match best {
                Some(b) if b.used_percent >= w.used_percent => Some(b),
                _ => Some(w),
            })?;
        let percents = self
            .windows
            .iter()
            .map(|w| format!("{} {}%", w.label(), percent(w.used_percent)))
            .collect::<Vec<_>>()
            .join(" · ");
        Some(format!(
            "{percents} (resets {})",
            short_time(binding.resets_at, now, offset_secs)
        ))
    }
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
    let directory = path.parent().context("quota file has no directory")?;
    if !std::fs::symlink_metadata(directory).is_ok_and(|meta| meta.is_dir()) {
        return Ok(false);
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
    fn claude_and_codex_payloads_become_labelled_windows_and_bad_windows_are_dropped() {
        let quota = claude((8.0, 1_792_179_600), (63.0, 1_792_609_200), NOW);
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
