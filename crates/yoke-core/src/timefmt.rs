//! Calendar time formatting shared by every text output.
//! Text shows local time as `YYYY-MM-DD HH:MM`; raw epoch seconds stay in `--json`.

/// Seconds east of UTC that the host's time zone applies at `at`; UTC when it cannot be read.
pub fn utc_offset_secs(at: i64) -> i64 {
    // Every release target has a 64-bit `time_t` and `tm_gmtoff`.
    // SAFETY: localtime_r only reads `at` and writes the `tm` passed to it.
    unsafe {
        let mut local: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&at, &mut local).is_null() {
            return 0;
        }
        local.tm_gmtoff
    }
}

/// Calendar fields of a Unix time shifted by a zone offset.
struct Civil {
    year: i64,
    month: usize,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
    /// 0 is Sunday.
    weekday: usize,
    /// Days since 1970-01-01 in that zone.
    days: i64,
}

fn civil(epoch: i64, offset_secs: i64) -> Civil {
    let local = epoch.saturating_add(offset_secs);
    let days = local.div_euclid(86_400);
    let seconds = local.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    Civil {
        year: year_of_era + era * 400 + i64::from(month <= 2),
        month: month as usize,
        day: day_of_year - (153 * shifted_month + 2) / 5 + 1,
        hour: seconds / 3_600,
        minute: seconds % 3_600 / 60,
        second: seconds % 60,
        weekday: (days + 4).rem_euclid(7) as usize,
        days,
    }
}

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Short reset label: `15:00` today, `Mon 10:00` within the next six days, else `Oct 23 10:00`.
pub fn short_time(epoch: i64, now: i64, offset_secs: i64) -> String {
    let (at, today) = (civil(epoch, offset_secs), civil(now, offset_secs));
    let clock = format!("{:02}:{:02}", at.hour, at.minute);
    match at.days - today.days {
        0 => clock,
        1..=6 => format!("{} {clock}", WEEKDAYS[at.weekday]),
        _ => format!("{} {} {clock}", MONTHS[at.month - 1], at.day),
    }
}

/// `2026-10-16 15:00` in the given zone.
pub fn full_time(epoch: i64, offset_secs: i64) -> String {
    let at = civil(epoch, offset_secs);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        at.year, at.month, at.day, at.hour, at.minute
    )
}

/// `2026-10-16 15:00:07` in the given zone, for logs where order within a minute matters.
pub fn full_time_secs(epoch: i64, offset_secs: i64) -> String {
    format!(
        "{}:{:02}",
        full_time(epoch, offset_secs),
        civil(epoch, offset_secs).second
    )
}

/// `2026-10-16 15:00` in the host's local zone at that instant.
pub fn local_time(epoch: i64) -> String {
    full_time(epoch, utc_offset_secs(epoch))
}

/// `2026-10-16 15:00:07` in the host's local zone at that instant.
pub fn local_time_secs(epoch: i64) -> String {
    full_time_secs(epoch, utc_offset_secs(epoch))
}

/// `UTC+06:00` for 21600 seconds east.
pub fn zone_label(offset_secs: i64) -> String {
    if offset_secs == 0 {
        return "UTC".into();
    }
    let sign = if offset_secs < 0 { '-' } else { '+' };
    let minutes = offset_secs.abs() / 60;
    format!("UTC{sign}{:02}:{:02}", minutes / 60, minutes % 60)
}

/// Unix seconds of the local midnight that starts the day containing `now`.
pub fn day_start(now: i64, offset_secs: i64) -> i64 {
    civil(now, offset_secs).days * 86_400 - offset_secs
}

/// `3h 12m`, `2d 4h`, `45m`, or `<1m`.
pub fn duration_text(secs: i64) -> String {
    let secs = secs.max(0);
    match (secs / 86_400, secs % 86_400 / 3_600, secs % 3_600 / 60) {
        (0, 0, 0) => "<1m".into(),
        (0, 0, m) => format!("{m}m"),
        (0, h, m) => format!("{h}h {m}m"),
        (d, h, _) => format!("{d}d {h}h"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-16 10:00:00 UTC, a Friday.
    const NOW: i64 = 1_792_144_800;

    #[test]
    fn times_format_in_the_given_zone() {
        assert_eq!(full_time(NOW, 0), "2026-10-16 10:00");
        assert_eq!(full_time(NOW, 6 * 3_600), "2026-10-16 16:00");
        assert_eq!(full_time_secs(NOW + 7, 6 * 3_600), "2026-10-16 16:00:07");
        assert_eq!(short_time(NOW + 20 * 86_400, NOW, 0), "Nov 5 10:00");
        assert_eq!(day_start(NOW, 6 * 3_600), NOW - 16 * 3_600);
        assert_eq!(zone_label(6 * 3_600), "UTC+06:00");
        assert_eq!(duration_text(3 * 3_600 + 12 * 60), "3h 12m");
    }
}
