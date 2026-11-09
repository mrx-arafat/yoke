//! Usage extras for the console: cost by model, today and the daily average, a straight-line
//! forecast for the current month, commands that rewrite gates changed, and which recorded
//! sessions landed Git commits. Read-only. Costs are estimates from published API prices, and a
//! sample without a known price is counted, never guessed.

use anyhow::Result;
use rusqlite::params;

use super::{bound, epoch_day, read_totals, scope, UsageRange, UsageTotals, TOTALS};
use crate::{store::Store, timefmt};

/// Totals of one model in a scope.
#[derive(Debug, PartialEq, Eq)]
pub struct ModelUsage {
    pub model: String,
    pub totals: UsageTotals,
}

/// Every model used in the scope, most expensive priced spend first, at most 100.
pub fn by_model(
    store: &Store,
    project: Option<&str>,
    range: &UsageRange,
) -> Result<Vec<ModelUsage>> {
    let (filter, bind) = scope(project, None, range);
    let mut query = store.connection().prepare(&format!(
        "SELECT model,{TOTALS} FROM usage_samples WHERE {filter} GROUP BY model ORDER BY coalesce(SUM(cost_nano_usd),0) DESC,model LIMIT 100"
    ))?;
    let rows = query.query_map(&*bound(&bind), |row| {
        Ok(ModelUsage {
            model: row.get(0)?,
            totals: read_totals(row, 1)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Samples, input plus output tokens, and priced spend of one period, ready for arithmetic.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Spend {
    pub samples: u64,
    pub tokens: u64,
    /// Sum of the samples with a known price, in billionths of a US dollar.
    pub known_nano_usd: i64,
    pub unknown_pricing_samples: u64,
}

fn spend(store: &Store, project: Option<&str>, range: &UsageRange) -> Result<Spend> {
    let (filter, bind) = scope(project, None, range);
    Ok(store.connection().query_row(
        &format!("SELECT count(*),coalesce(SUM(input_tokens+output_tokens),0),coalesce(SUM(cost_nano_usd),0),coalesce(SUM(cost_nano_usd IS NULL),0) FROM usage_samples WHERE {filter}"),
        &*bound(&bind),
        |row| {
            Ok(Spend {
                samples: row.get(0)?,
                tokens: row.get(1)?,
                known_nano_usd: row.get(2)?,
                unknown_pricing_samples: row.get(3)?,
            })
        },
    )?)
}

/// Today, the average day of a recent window, and the current month with its forecast, all in
/// calendar days of one UTC offset.
#[derive(Debug, PartialEq, Eq)]
pub struct Pace {
    /// `YYYY-MM-DD` of today.
    pub today_label: String,
    pub today: Spend,
    /// Calendar days averaged over, today included; idle days count as zero.
    pub window_days: i64,
    pub window: Spend,
    /// `YYYY-MM` of the current month.
    pub month_label: String,
    pub month_to_date: Spend,
    /// Today's day of the month, counted as a whole day.
    pub day_of_month: i64,
    pub days_in_month: i64,
}

impl Pace {
    pub fn average_tokens(&self) -> u64 {
        self.window.tokens / self.window_days.max(1) as u64
    }

    pub fn average_known_nano_usd(&self) -> i64 {
        self.window.known_nano_usd / self.window_days.max(1)
    }

    /// Month-to-date priced spend per elapsed day, times the days in the month.
    pub fn forecast_known_nano_usd(&self) -> i64 {
        let per_day = self.month_to_date.known_nano_usd as i128 / self.day_of_month.max(1) as i128;
        (per_day * self.days_in_month as i128).min(i64::MAX as i128) as i64
    }
}

fn day_label(at: i64, offset_secs: i64) -> String {
    timefmt::full_time(at, offset_secs)[..10].to_owned()
}

fn days_in_month(year: i64, month: i64) -> i64 {
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// `now` is Unix seconds; `window_days` (at least 1) ends with today.
pub fn pace(
    store: &Store,
    project: Option<&str>,
    now: i64,
    offset_minutes: i32,
    window_days: i64,
) -> Result<Pace> {
    let offset_secs = i64::from(offset_minutes) * 60;
    let window_days = window_days.clamp(1, 366);
    let today_label = day_label(now, offset_secs);
    let first = day_label(
        timefmt::day_start(now, offset_secs) - (window_days - 1) * 86_400,
        offset_secs,
    );
    let year = today_label[..4].parse::<i64>()?;
    let month = today_label[5..7].parse::<i64>()?;
    let day_of_month = today_label[8..10].parse::<i64>()?;
    let month_start = format!("{}-01", &today_label[..7]);
    let range = |from: &str| UsageRange::new(Some(from), Some(&today_label), offset_minutes);
    Ok(Pace {
        today: spend(store, project, &range(&today_label)?)?,
        window_days,
        window: spend(store, project, &range(&first)?)?,
        month_label: today_label[..7].to_owned(),
        month_to_date: spend(store, project, &range(&month_start)?)?,
        day_of_month,
        days_in_month: days_in_month(year, month),
        today_label,
    })
}

/// Unix-second bounds of a range: inclusive start, exclusive end.
fn bounds(range: &UsageRange) -> (Option<i64>, Option<i64>) {
    let offset = i64::from(range.offset_minutes()) * 60;
    (
        range.from().map(|day| epoch_day(day) * 86_400 - offset),
        range.to().map(|day| (epoch_day(day) + 1) * 86_400 - offset),
    )
}

/// Commands that rewrite gates changed and that ran in enforce mode, by gate, most first. Only
/// what Yoke recorded: it cannot know what a rewrite saved.
pub fn rewrites(store: &Store, project: &str, range: &UsageRange) -> Result<Vec<(String, i64)>> {
    let (start, end) = bounds(range);
    let mut query = store.connection().prepare(
        "SELECT json_extract(g.value,'$.name') AS gate,count(*) FROM decisions d, json_each(d.gates) g
         WHERE d.project=?1 AND d.outcome='rewrite' AND (?2 IS NULL OR d.ts>=?2) AND (?3 IS NULL OR d.ts<?3)
         AND json_extract(g.value,'$.result')='rewrite' GROUP BY gate ORDER BY count(*) DESC,gate LIMIT 50",
    )?;
    let rows = query.query_map(params![project, start, end], |row| {
        Ok((
            row.get::<_, Option<String>>(0)?.unwrap_or_default(),
            row.get::<_, i64>(1)?,
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// One recorded session's dated usage: when it ran and what it cost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionWindow {
    pub session_id: String,
    pub first_at: i64,
    pub last_at: i64,
    pub known_nano_usd: i64,
    pub unknown_pricing_samples: u64,
}

const SESSION_LIMIT: i64 = 5000;

/// Sessions with dated samples in the scope, at most 5000.
pub fn session_windows(
    store: &Store,
    project: &str,
    range: &UsageRange,
) -> Result<Vec<SessionWindow>> {
    let (filter, mut bind) = scope(Some(project), None, range);
    bind.push((":limit", rusqlite::types::Value::Integer(SESSION_LIMIT)));
    let mut query = store.connection().prepare(&format!(
        "SELECT session_id,min(ts),max(ts),coalesce(SUM(cost_nano_usd),0),coalesce(SUM(cost_nano_usd IS NULL),0)
         FROM usage_samples WHERE {filter} AND ts IS NOT NULL GROUP BY agent,session_id ORDER BY min(ts),session_id LIMIT :limit"
    ))?;
    let rows = query.query_map(&*bound(&bind), |row| {
        Ok(SessionWindow {
            session_id: row.get(0)?,
            first_at: row.get(1)?,
            last_at: row.get(2)?,
            known_nano_usd: row.get(3)?,
            unknown_pricing_samples: row.get(4)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// A commit counts for a session when it was authored between the session's first usage sample
/// and one hour after its last, the same window Pilot Shell uses.
pub const LANDING_GRACE_SECS: i64 = 3600;

/// What the sessions in a scope shipped.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Correlation {
    pub sessions: usize,
    /// Sessions with at least one commit in their window.
    pub landed_sessions: usize,
    /// Distinct commits inside any session's window.
    pub commits: usize,
    pub total_known_nano_usd: i64,
    pub landed_known_nano_usd: i64,
    pub unknown_pricing_samples: u64,
}

impl Correlation {
    /// Priced spend of the sessions that landed code per commit they landed.
    pub fn known_nano_usd_per_commit(&self) -> Option<i64> {
        (self.commits > 0).then(|| self.landed_known_nano_usd / self.commits as i64)
    }
}

/// Match commit author times to session windows. Overlapping sessions can both claim a commit;
/// it is still counted once.
pub fn correlate(windows: &[SessionWindow], commit_times: &[i64]) -> Correlation {
    let mut claimed = vec![false; commit_times.len()];
    let mut result = Correlation {
        sessions: windows.len(),
        ..Correlation::default()
    };
    for window in windows {
        result.total_known_nano_usd += window.known_nano_usd;
        result.unknown_pricing_samples += window.unknown_pricing_samples;
        let end = window.last_at.saturating_add(LANDING_GRACE_SECS);
        let mut landed = false;
        for (index, at) in commit_times.iter().enumerate() {
            if (window.first_at..=end).contains(at) {
                landed = true;
                claimed[index] = true;
            }
        }
        if landed {
            result.landed_sessions += 1;
            result.landed_known_nano_usd += window.known_nano_usd;
        }
    }
    result.commits = claimed.into_iter().filter(|claimed| *claimed).count();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(store: &Store, session: &str, key: &str, model: &str, cost: Option<i64>, ts: i64) {
        store
            .connection()
            .execute(
                "INSERT INTO usage_samples(agent,session_id,project,record_key,model,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,cost_nano_usd,ts) VALUES ('claude',?1,'/p',?2,?3,10,5,0,0,?4,?5)",
                params![session, key, model, cost, ts],
            )
            .unwrap();
    }

    #[test]
    fn pace_averages_calendar_days_and_forecasts_the_month_in_a_straight_line() {
        let store = Store::open(":memory:").unwrap();
        // 2026-11-10 12:00 UTC is day 10 of a 30-day month.
        let now = 1_794_312_000;
        assert_eq!(day_label(now, 0), "2026-11-10");
        sample(&store, "a", "1", "opus", Some(2_000_000_000), now - 3600);
        sample(&store, "a", "2", "opus", None, now - 3 * 86_400);
        sample(
            &store,
            "b",
            "3",
            "haiku",
            Some(1_000_000_000),
            now - 9 * 86_400,
        );
        sample(
            &store,
            "b",
            "4",
            "haiku",
            Some(7_000_000_000),
            now - 40 * 86_400,
        );
        let pace = pace(&store, Some("/p"), now, 0, 14).unwrap();
        assert_eq!(pace.today.samples, 1);
        assert_eq!(pace.window.samples, 3);
        assert_eq!(pace.window.unknown_pricing_samples, 1);
        assert_eq!(pace.average_known_nano_usd(), 3_000_000_000 / 14);
        assert_eq!(pace.average_tokens(), 45 / 14);
        assert_eq!((pace.day_of_month, pace.days_in_month), (10, 30));
        // $3 over 10 days of a 30-day month.
        assert_eq!(pace.forecast_known_nano_usd(), 9_000_000_000);

        let models = by_model(&store, Some("/p"), &UsageRange::default()).unwrap();
        assert_eq!(
            models.iter().map(|m| m.model.as_str()).collect::<Vec<_>>(),
            ["haiku", "opus"]
        );
        assert_eq!(models[1].totals.unknown_pricing_samples, 1);
    }

    #[test]
    fn commits_inside_a_session_window_count_once_and_credit_every_session_that_ran() {
        let window = |id: &str, first, last, cost| SessionWindow {
            session_id: id.into(),
            first_at: first,
            last_at: last,
            known_nano_usd: cost,
            unknown_pricing_samples: 0,
        };
        let windows = [
            window("a", 1_000, 2_000, 10),
            window("b", 1_500, 1_600, 20),
            window("c", 9_000, 9_100, 40),
        ];
        // 1_550 is inside a and b; 5_600 is exactly a's last sample plus the grace hour; 8_999
        // is before c and after everything else.
        let result = correlate(&windows, &[1_550, 2_000 + LANDING_GRACE_SECS, 8_999]);
        assert_eq!(
            result,
            Correlation {
                sessions: 3,
                landed_sessions: 2,
                commits: 2,
                total_known_nano_usd: 70,
                landed_known_nano_usd: 30,
                unknown_pricing_samples: 0,
            }
        );
        assert_eq!(result.known_nano_usd_per_commit(), Some(15));
    }

    #[test]
    fn rewrites_count_only_enforced_rewrites_by_gate() {
        let store = Store::open(":memory:").unwrap();
        store.connection().execute_batch(
            r#"INSERT INTO sessions(id,agent,project,started_at) VALUES ('s','claude','/p',1);
             INSERT INTO decisions(ts,session_id,agent,project,event,mode,outcome,gates,payload) VALUES
             (100,'s','claude','/p','pre_tool_use','enforce','rewrite','[{"name":"rtk","result":"rewrite","ms":1,"exit_code":null}]','{}'),
             (101,'s','claude','/p','pre_tool_use','enforce','rewrite','[{"name":"lint","result":"pass","ms":1,"exit_code":null},{"name":"rtk","result":"rewrite","ms":1,"exit_code":null}]','{}'),
             (102,'s','claude','/p','pre_tool_use','observe','allow','[{"name":"rtk","result":"rewrite","ms":1,"exit_code":null}]','{}'),
             (103,'s','claude','/q','pre_tool_use','enforce','rewrite','[{"name":"rtk","result":"rewrite","ms":1,"exit_code":null}]','{}');"#,
        ).unwrap();
        assert_eq!(
            rewrites(&store, "/p", &UsageRange::default()).unwrap(),
            vec![("rtk".to_owned(), 2)]
        );
    }
}
