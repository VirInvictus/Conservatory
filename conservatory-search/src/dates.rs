//! Date resolution shared by `eval` and `sql_translate` so the two paths never
//! diverge on date arithmetic (the `atrium-search` pattern). A [`DateSpec`]
//! resolves to a `[start, end)` epoch-seconds (UTC) range; a comparator turns
//! that range into a match.

use chrono::{Datelike, Days, NaiveDate, NaiveTime, Utc};

use crate::ast::{Comparator, DateSpec};

/// Resolve a spec to a `[start, end)` epoch-seconds range relative to `today`.
pub fn resolve_range(spec: &DateSpec, today: NaiveDate) -> (i64, i64) {
    let (start, end) = match spec {
        DateSpec::Today => (today, next_day(today)),
        DateSpec::Yesterday => (prev_day(today), today),
        DateSpec::ThisWeek => {
            let monday = today - Days::new(today.weekday().num_days_from_monday() as u64);
            (monday, monday + Days::new(7))
        }
        DateSpec::ThisMonth => {
            let first = ymd(today.year(), today.month(), 1);
            (first, add_month(first))
        }
        DateSpec::ThisYear => (ymd(today.year(), 1, 1), ymd(today.year() + 1, 1, 1)),
        DateSpec::DaysAgo(n) => {
            let d = today - Days::new(*n as u64);
            (d, next_day(d))
        }
        DateSpec::Ymd(y, None, _) => (ymd(*y, 1, 1), ymd(*y + 1, 1, 1)),
        DateSpec::Ymd(y, Some(m), None) => {
            let first = ymd(*y, *m, 1);
            (first, add_month(first))
        }
        DateSpec::Ymd(y, Some(m), Some(d)) => {
            let day = ymd(*y, *m, *d);
            (day, next_day(day))
        }
    };
    (start_epoch(start), start_epoch(end))
}

/// Does `value` (epoch seconds) satisfy `comp` against the `[start, end)` range?
/// Precision-aware, matching CalibreQuarry: `=today` is "within today",
/// `>today` is "strictly after today", `>=today` is "today or later", etc.
pub fn matches(comp: Comparator, value: i64, start: i64, end: i64) -> bool {
    match comp {
        Comparator::Eq => value >= start && value < end,
        Comparator::Ne => value < start || value >= end,
        Comparator::Lt => value < start,
        Comparator::Le => value < end,
        Comparator::Gt => value >= end,
        Comparator::Ge => value >= start,
    }
}

fn next_day(d: NaiveDate) -> NaiveDate {
    d.checked_add_days(Days::new(1)).unwrap_or(d)
}

fn prev_day(d: NaiveDate) -> NaiveDate {
    d.checked_sub_days(Days::new(1)).unwrap_or(d)
}

/// Build a date, clamping an out-of-range day to the last valid day of a month.
fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
    let m = m.clamp(1, 12);
    for day in (1..=d.clamp(1, 31)).rev() {
        if let Some(date) = NaiveDate::from_ymd_opt(y, m, day) {
            return date;
        }
    }
    NaiveDate::from_ymd_opt(y, m, 1).unwrap_or_else(|| NaiveDate::from_ymd_opt(1970, 1, 1).unwrap())
}

fn add_month(first: NaiveDate) -> NaiveDate {
    let (y, m) = if first.month() == 12 {
        (first.year() + 1, 1)
    } else {
        (first.year(), first.month() + 1)
    };
    ymd(y, m, 1)
}

fn start_epoch(date: NaiveDate) -> i64 {
    date.and_time(NaiveTime::MIN).and_utc().timestamp()
}

/// `Utc::now` is unavailable in some contexts; the caller passes `today`. This
/// convenience is for the binary.
pub fn today_utc() -> NaiveDate {
    Utc::now().date_naive()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn year_precision_spans_the_year() {
        let (s, e) = resolve_range(&DateSpec::Ymd(2002, None, None), d(2026, 6, 20));
        assert_eq!(s, start_epoch(d(2002, 1, 1)));
        assert_eq!(e, start_epoch(d(2003, 1, 1)));
    }

    #[test]
    fn comparator_semantics() {
        let today = d(2026, 6, 20);
        let (s, e) = resolve_range(&DateSpec::Today, today);
        let noon = s + 12 * 3600;
        assert!(matches(Comparator::Eq, noon, s, e));
        assert!(!matches(Comparator::Gt, noon, s, e)); // not strictly after today
        assert!(matches(Comparator::Ge, noon, s, e));
        assert!(matches(Comparator::Gt, e + 1, s, e)); // tomorrow is after
        assert!(matches(Comparator::Lt, s - 1, s, e)); // yesterday is before
    }

    #[test]
    fn days_ago() {
        let today = d(2026, 6, 20);
        let (s, e) = resolve_range(&DateSpec::DaysAgo(3), today);
        assert_eq!(s, start_epoch(d(2026, 6, 17)));
        assert_eq!(e, start_epoch(d(2026, 6, 18)));
    }
}
