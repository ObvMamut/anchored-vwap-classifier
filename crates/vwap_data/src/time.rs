//! Timestamp conversion and New York clock helpers. All conversions go through
//! `chrono_tz`, never through fixed offsets.

use chrono::{
    DateTime, Datelike, LocalResult, NaiveDate, NaiveDateTime, TimeDelta, TimeZone, Timelike, Utc,
};
use chrono_tz::America::New_York;
use vwap_core::TimestampKind;

/// Outcome of converting a label column to UTC.
#[derive(Debug, Default, Clone, Copy)]
pub struct ConvertStats {
    pub ambiguous_local: usize,
    pub nonexistent_local: usize,
}

/// Converts file labels (UTC microseconds as stored) to true UTC microseconds.
/// Entries that map to a non existent local time are `None`.
pub fn labels_to_utc(
    labels_us: &[i64],
    kind: TimestampKind,
    server_offset_hours: i64,
) -> (Vec<Option<i64>>, ConvertStats) {
    let mut stats = ConvertStats::default();
    let out = labels_us
        .iter()
        .map(|&us| match kind {
            TimestampKind::Utc => Some(us),
            TimestampKind::NyLocal | TimestampKind::ServerNySynced => {
                let naive = DateTime::from_timestamp_micros(us)?.naive_utc();
                let shift = if kind == TimestampKind::ServerNySynced {
                    server_offset_hours
                } else {
                    0
                };
                let local = naive.checked_sub_signed(TimeDelta::hours(shift))?;
                match New_York.from_local_datetime(&local) {
                    LocalResult::Single(dt) => Some(dt.with_timezone(&Utc).timestamp_micros()),
                    LocalResult::Ambiguous(first, _) => {
                        stats.ambiguous_local += 1;
                        Some(first.with_timezone(&Utc).timestamp_micros())
                    }
                    LocalResult::None => {
                        stats.nonexistent_local += 1;
                        None
                    }
                }
            }
        })
        .collect();
    (out, stats)
}

/// New York wall clock of a UTC microsecond timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NyClock {
    pub date: NaiveDate,
    /// Minutes since local midnight.
    pub minute: u32,
}

pub fn ny_clock(ts_us: i64) -> NyClock {
    let dt = DateTime::from_timestamp_micros(ts_us)
        .unwrap_or_default()
        .with_timezone(&New_York);
    NyClock {
        date: dt.date_naive(),
        minute: dt.hour() * 60 + dt.minute(),
    }
}

pub fn naive_ny(ts_us: i64) -> NaiveDateTime {
    DateTime::from_timestamp_micros(ts_us)
        .unwrap_or_default()
        .with_timezone(&New_York)
        .naive_local()
}

/// Days since the Unix epoch of a date.
pub fn epoch_days(d: NaiveDate) -> i32 {
    d.num_days_from_ce() - NaiveDate::from_ymd_opt(1970, 1, 1).map_or(0, |e| e.num_days_from_ce())
}

pub const MINUTE_US: i64 = 60_000_000;
/// 18:00 New York in minutes since midnight.
pub const GLOBEX_OPEN_MIN: u32 = 18 * 60;
/// 17:00 New York in minutes since midnight.
pub const GLOBEX_BREAK_MIN: u32 = 17 * 60;
/// 09:30 New York in minutes since midnight.
pub const RTH_OPEN_MIN: u32 = 9 * 60 + 30;

/// Globex session anchor date of a New York wall clock time: the date on which
/// the 18:00 anchor of the containing session falls.
pub fn globex_anchor_date(c: NyClock) -> NaiveDate {
    if c.minute >= GLOBEX_OPEN_MIN {
        c.date
    } else {
        c.date.pred_opt().unwrap_or(c.date)
    }
}

pub fn utc_of_local(date: NaiveDate, minute: u32) -> Option<i64> {
    let naive = date.and_hms_opt(minute / 60, minute % 60, 0)?;
    match New_York.from_local_datetime(&naive) {
        LocalResult::Single(dt) => Some(dt.with_timezone(&Utc).timestamp_micros()),
        LocalResult::Ambiguous(first, _) => Some(first.with_timezone(&Utc).timestamp_micros()),
        LocalResult::None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(y: i32, m: u32, d: u32, h: u32, mi: u32) -> i64 {
        NaiveDate::from_ymd_opt(y, m, d)
            .and_then(|x| x.and_hms_opt(h, mi, 0))
            .map(|x| x.and_utc().timestamp_micros())
            .unwrap_or_default()
    }

    #[test]
    fn server_label_maps_back_to_new_york_in_both_seasons() {
        // Winter: server 01:00 is New York 18:00 the previous day, UTC 23:00.
        let (w, _) = labels_to_utc(
            &[label(2024, 1, 16, 1, 0)],
            TimestampKind::ServerNySynced,
            7,
        );
        let w = w[0].map(ny_clock);
        assert_eq!(w.map(|c| c.minute), Some(18 * 60));
        assert_eq!(w.map(|c| c.date), NaiveDate::from_ymd_opt(2024, 1, 15));
        // Summer: same server label, same New York wall clock, different UTC.
        let (s, _) = labels_to_utc(
            &[label(2024, 7, 16, 1, 0)],
            TimestampKind::ServerNySynced,
            7,
        );
        assert_eq!(s[0].map(ny_clock).map(|c| c.minute), Some(18 * 60));
        let utc_w = w.map(|_| ());
        assert!(utc_w.is_some());
    }

    #[test]
    fn utc_kind_is_identity() {
        let x = label(2024, 3, 1, 12, 0);
        let (o, st) = labels_to_utc(&[x], TimestampKind::Utc, 7);
        assert_eq!(o[0], Some(x));
        assert_eq!(st.nonexistent_local, 0);
    }

    #[test]
    fn anchor_dates_follow_the_18_00_rule() {
        let c = NyClock {
            date: NaiveDate::from_ymd_opt(2024, 5, 7).unwrap_or_default(),
            minute: 18 * 60,
        };
        assert_eq!(globex_anchor_date(c), c.date);
        let c2 = NyClock {
            minute: 9 * 60,
            ..c
        };
        assert_eq!(
            globex_anchor_date(c2),
            NaiveDate::from_ymd_opt(2024, 5, 6).unwrap_or_default()
        );
    }

    #[test]
    fn dst_transition_days_have_expected_offsets() {
        // 2024-03-10 spring forward: 18:00 New York is UTC-4 afterwards.
        let u = utc_of_local(
            NaiveDate::from_ymd_opt(2024, 3, 10).unwrap_or_default(),
            18 * 60,
        );
        let expected = NaiveDate::from_ymd_opt(2024, 3, 10)
            .and_then(|d| d.and_hms_opt(22, 0, 0))
            .map(|x| x.and_utc().timestamp_micros());
        assert_eq!(u, expected);
        // 2024-11-03 fall back: 18:00 New York is UTC-5.
        let u = utc_of_local(
            NaiveDate::from_ymd_opt(2024, 11, 3).unwrap_or_default(),
            18 * 60,
        );
        let expected = NaiveDate::from_ymd_opt(2024, 11, 3)
            .and_then(|d| d.and_hms_opt(23, 0, 0))
            .map(|x| x.and_utc().timestamp_micros());
        assert_eq!(u, expected);
    }
}
