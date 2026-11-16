//! Clock and calendar helpers.
//!
//! Stored times are UTC seconds. A [`Tz`] turns them into local wall-clock
//! time, and local days into UTC ranges. Three kinds exist: the system zone
//! (asked from Windows, so daylight-saving changes are right for any date), a
//! fixed offset (demo data, tests) and a table of transitions (tests).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// A local day, counted in days since 1970-01-01 of the local calendar.
pub type Day = i32;

#[derive(Clone, Debug)]
pub enum Tz {
    /// Always this many seconds east of UTC.
    Fixed(i32),
    /// `base` before the first transition; from each `(utc, offset)` on, that offset.
    Table {
        base: i32,
        transitions: Arc<Vec<(i64, i32)>>,
    },
    /// The time zone Windows is set to.
    System,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Local {
    pub day: Day,
    pub year: i32,
    /// 1..=12
    pub month: u32,
    /// 1..=31
    pub dom: u32,
    /// 0 = Monday .. 6 = Sunday
    pub weekday: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

static SYSTEM_CACHE: Mutex<Option<HashMap<i64, i32>>> = Mutex::new(None);

fn system_offset(utc: i64) -> i32 {
    // Offsets only change on quarter-hour boundaries.
    let key = utc.div_euclid(900);
    if let Ok(mut guard) = SYSTEM_CACHE.lock() {
        let map = guard.get_or_insert_with(HashMap::new);
        if let Some(v) = map.get(&key) {
            return *v;
        }
        if map.len() > 200_000 {
            map.clear();
        }
        let v = crate::sys::utc_offset_at(key * 900);
        map.insert(key, v);
        return v;
    }
    crate::sys::utc_offset_at(utc)
}

/// Forget cached system offsets (after the user changed the time zone).
pub fn reset_system_tz() {
    if let Ok(mut guard) = SYSTEM_CACHE.lock() {
        *guard = None;
    }
}

impl Tz {
    /// A zone with daylight saving for tests: `std` seconds east of UTC, and
    /// `dst` between the two UTC instants.
    pub fn with_dst(std: i32, dst: i32, dst_from_utc: i64, dst_until_utc: i64) -> Tz {
        Tz::Table {
            base: std,
            transitions: Arc::new(vec![(dst_from_utc, dst), (dst_until_utc, std)]),
        }
    }

    /// Seconds east of UTC at this instant.
    pub fn offset(&self, utc: i64) -> i32 {
        match self {
            Tz::Fixed(o) => *o,
            Tz::Table { base, transitions } => transitions
                .iter()
                .rev()
                .find(|(t, _)| utc >= *t)
                .map(|(_, o)| *o)
                .unwrap_or(*base),
            Tz::System => system_offset(utc),
        }
    }

    /// The local day this instant falls on.
    pub fn day_of(&self, utc: i64) -> Day {
        (utc + self.offset(utc) as i64).div_euclid(86_400) as Day
    }

    /// Seconds since local midnight (wall clock), 0..86400.
    pub fn second_of_day(&self, utc: i64) -> u32 {
        (utc + self.offset(utc) as i64).rem_euclid(86_400) as u32
    }

    pub fn local(&self, utc: i64) -> Local {
        let day = self.day_of(utc);
        let (year, month, dom) = civil_from_days(day);
        let s = self.second_of_day(utc);
        Local {
            day,
            year,
            month,
            dom,
            weekday: weekday(day),
            hour: s / 3600,
            minute: s / 60 % 60,
            second: s % 60,
        }
    }

    /// The first UTC instant of a local day. Correct across daylight-saving
    /// changes: a day can be 23 or 25 hours long.
    pub fn day_start(&self, day: Day) -> i64 {
        let nominal = day as i64 * 86_400;
        // The first instant t with day_of(t) >= day lies within a day of nominal.
        let (mut lo, mut hi) = (nominal - 100_000, nominal + 100_000);
        while hi - lo > 1 {
            let mid = lo + (hi - lo) / 2;
            if self.day_of(mid) >= day {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        hi
    }

    /// `[start, end)` of a local day in UTC.
    pub fn day_bounds(&self, day: Day) -> (i64, i64) {
        (self.day_start(day), self.day_start(day + 1))
    }

    /// The UTC instant of a local wall-clock time on a local day. Times that
    /// do not exist (spring forward) land just after the gap.
    pub fn at(&self, day: Day, hour: u32, minute: u32) -> i64 {
        let wall = day as i64 * 86_400 + hour as i64 * 3600 + minute as i64 * 60;
        let guess = wall - self.offset(wall - self.offset(wall) as i64) as i64;
        let (start, end) = self.day_bounds(day);
        guess.clamp(start, end - 1)
    }
}

/// Now, in UTC seconds.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Year, month, day of a day number (proleptic Gregorian).
pub fn civil_from_days(day: Day) -> (i32, u32, u32) {
    let z = day as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((y + i64::from(m <= 2)) as i32, m, d)
}

pub fn days_from_civil(year: i32, month: u32, dom: u32) -> Day {
    let y = year as i64 - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = month as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + dom as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) as Day
}

/// 0 = Monday .. 6 = Sunday.
pub fn weekday(day: Day) -> u32 {
    (day as i64 + 3).rem_euclid(7) as u32
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    (days_from_civil(ny, nm, 1) - days_from_civil(year, month, 1)) as u32
}

/// The first day of the week containing `day`.
pub fn week_start(day: Day, monday_first: bool) -> Day {
    let wd = weekday(day) as i32;
    if monday_first {
        day - wd
    } else {
        day - (wd + 1) % 7
    }
}

/// The first day of the month containing `day`.
pub fn month_start(day: Day) -> Day {
    let (y, m, _) = civil_from_days(day);
    days_from_civil(y, m, 1)
}

pub const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
pub const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
pub const MONTHS_LONG: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// `2026-11-12`
pub fn iso_date(day: Day) -> String {
    let (y, m, d) = civil_from_days(day);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Parses `2026-11-12`.
pub fn parse_iso_date(text: &str) -> Option<Day> {
    let mut parts = text.trim().split('-');
    let y: i32 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&m) || d == 0 || d > days_in_month(y, m) {
        return None;
    }
    Some(days_from_civil(y, m, d))
}

/// `Thu 12 Nov 2026`
pub fn fmt_day(day: Day) -> String {
    let (y, m, d) = civil_from_days(day);
    format!(
        "{} {d} {} {y}",
        WEEKDAYS[weekday(day) as usize],
        MONTHS[m as usize - 1]
    )
}

/// `12 Nov`
pub fn fmt_day_short(day: Day) -> String {
    let (_, m, d) = civil_from_days(day);
    format!("{d} {}", MONTHS[m as usize - 1])
}

/// `14:05`
pub fn fmt_hm(utc: i64, tz: &Tz) -> String {
    let s = tz.second_of_day(utc);
    format!("{:02}:{:02}", s / 3600, s / 60 % 60)
}

/// `2026-11-12 14:05:09` in local time.
pub fn fmt_datetime(utc: i64, tz: &Tz) -> String {
    let l = tz.local(utc);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        l.year, l.month, l.dom, l.hour, l.minute, l.second
    )
}

/// `3h 12m`, `12m`, `45s`. Never negative.
pub fn fmt_dur(secs: i64) -> String {
    let s = secs.max(0);
    if s >= 3600 {
        format!("{}h {:02}m", s / 3600, s / 60 % 60)
    } else if s >= 60 {
        format!("{}m", s / 60)
    } else {
        format!("{s}s")
    }
}

/// `3:12:05` for running clocks.
pub fn fmt_clock(secs: i64) -> String {
    let s = secs.max(0);
    format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

/// Hours with two decimals, for timesheets: `3.25`.
pub fn fmt_hours(secs: i64) -> String {
    format!("{:.2}", secs.max(0) as f64 / 3600.0)
}

/// Parses `14:05` or `9:30` into (hour, minute).
pub fn parse_hm(text: &str) -> Option<(u32, u32)> {
    let (h, m) = text.trim().split_once(':')?;
    let h: u32 = h.trim().parse().ok()?;
    let m: u32 = m.trim().parse().ok()?;
    (h < 24 && m < 60).then_some((h, m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_round_trip() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        for day in (-800_000..800_000).step_by(997) {
            let (y, m, d) = civil_from_days(day);
            assert_eq!(days_from_civil(y, m, d), day);
        }
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2026, 2), 28);
        assert_eq!(days_in_month(2026, 12), 31);
        // 12 November 2026 is a Thursday.
        let d = days_from_civil(2026, 11, 12);
        assert_eq!(weekday(d), 3);
        assert_eq!(fmt_day(d), "Thu 12 Nov 2026");
        assert_eq!(iso_date(d), "2026-11-12");
        assert_eq!(parse_iso_date("2026-11-12"), Some(d));
        assert_eq!(parse_iso_date("2026-02-30"), None);
        assert_eq!(parse_iso_date("nope"), None);
        assert_eq!(week_start(d, true), d - 3);
        assert_eq!(week_start(d, false), d - 4);
        assert_eq!(month_start(d), d - 11);
    }

    #[test]
    fn fixed_zone_days() {
        let tz = Tz::Fixed(3 * 3600);
        let day = days_from_civil(2026, 11, 12);
        let (a, b) = tz.day_bounds(day);
        assert_eq!(b - a, 86_400);
        assert_eq!(a, day as i64 * 86_400 - 3 * 3600);
        assert_eq!(tz.day_of(a), day);
        assert_eq!(tz.day_of(a - 1), day - 1);
        assert_eq!(tz.at(day, 14, 5), a + 14 * 3600 + 5 * 60);
        assert_eq!(fmt_hm(tz.at(day, 14, 5), &tz), "14:05");
        let l = tz.local(tz.at(day, 23, 59));
        assert_eq!(
            (l.year, l.month, l.dom, l.hour, l.minute),
            (2026, 11, 12, 23, 59)
        );
    }

    #[test]
    fn dst_days_are_23_and_25_hours() {
        // Central European rules for 2026: forward on 29 March 01:00 UTC,
        // back on 25 October 01:00 UTC.
        let fwd = days_from_civil(2026, 3, 29) as i64 * 86_400 + 3600;
        let back = days_from_civil(2026, 10, 25) as i64 * 86_400 + 3600;
        let tz = Tz::with_dst(3600, 7200, fwd, back);
        let spring = days_from_civil(2026, 3, 29);
        let (a, b) = tz.day_bounds(spring);
        assert_eq!(b - a, 23 * 3600);
        assert_eq!(tz.local(a).hour, 0);
        assert_eq!(tz.day_of(b), spring + 1);
        let autumn = days_from_civil(2026, 10, 25);
        let (a, b) = tz.day_bounds(autumn);
        assert_eq!(b - a, 25 * 3600);
        // 02:30 exists twice in autumn and never in spring; both resolve inside the day.
        let t = tz.at(autumn, 2, 30);
        assert_eq!(tz.day_of(t), autumn);
        let t = tz.at(spring, 2, 30);
        assert_eq!(tz.day_of(t), spring);
        // An ordinary summer day.
        let (a, b) = tz.day_bounds(days_from_civil(2026, 7, 1));
        assert_eq!(b - a, 86_400);
        assert_eq!(tz.offset(a), 7200);
    }

    #[test]
    fn durations() {
        assert_eq!(fmt_dur(45), "45s");
        assert_eq!(fmt_dur(125), "2m");
        assert_eq!(fmt_dur(3 * 3600 + 12 * 60 + 5), "3h 12m");
        assert_eq!(fmt_dur(-4), "0s");
        assert_eq!(fmt_clock(3 * 3600 + 12 * 60 + 5), "3:12:05");
        assert_eq!(fmt_hours(3 * 3600 + 900), "3.25");
        assert_eq!(parse_hm("9:30"), Some((9, 30)));
        assert_eq!(parse_hm("24:00"), None);
        assert_eq!(parse_hm("x"), None);
    }

    #[test]
    fn system_zone_is_sane() {
        let tz = Tz::System;
        let now = now();
        assert!(tz.offset(now).abs() <= 16 * 3600);
        let day = tz.day_of(now);
        let (a, b) = tz.day_bounds(day);
        assert!(a <= now && now < b);
        assert!((b - a - 86_400).abs() <= 2 * 3600);
    }
}
