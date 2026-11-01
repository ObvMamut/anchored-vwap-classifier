//! The calendar features K1 to K3 for each of CPI, FOMC and NFP (`docs/FEATURES.md`, section 6).
//!
//! Causal proof: a feature uses only the scheduled release instants, which are published weeks
//! ahead, and the close time and session of the touch bar. It reads no price and no release
//! outcome, and "until the next release" looks at a schedule, not at the future of the market.

use vwap_data::time::{epoch_days, globex_anchor_date, ny_clock};

/// Number of columns: three per release type, in the order CPI, FOMC, NFP.
pub const CALENDAR_COLUMNS: usize = 9;

const MINUTE_US: i64 = 60_000_000;

#[derive(Debug, Clone, Default)]
pub struct CalendarTable {
    enabled: bool,
    /// Per type, `(release instant in UTC microseconds, session key)`, sorted by instant.
    releases: [Vec<(i64, i32)>; 3],
    cap_minutes: f64,
}

impl CalendarTable {
    /// No snapshot: every calendar feature is NaN.
    pub fn disabled() -> Self {
        Self::default()
    }

    /// `releases[k]` are the instants of type `k` (CPI, FOMC, NFP).
    pub fn new(releases: [Vec<i64>; 3], cap_minutes: u32) -> Self {
        let tag = |v: Vec<i64>| {
            let mut out: Vec<(i64, i32)> = v
                .into_iter()
                .map(|us| (us, epoch_days(globex_anchor_date(ny_clock(us)))))
                .collect();
            out.sort_unstable();
            out
        };
        let [a, b, c] = releases;
        Self {
            enabled: true,
            releases: [tag(a), tag(b), tag(c)],
            cap_minutes: f64::from(cap_minutes),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// For each type: the release day flag, the minutes until the next release (strictly after
    /// the close) and the minutes since the last one (at or before the close), both capped.
    pub fn features(&self, close_us: i64, session: i32) -> [f64; CALENDAR_COLUMNS] {
        let mut out = [f64::NAN; CALENDAR_COLUMNS];
        if !self.enabled {
            return out;
        }
        for (k, rel) in self.releases.iter().enumerate() {
            let day = rel.iter().any(|&(_, s)| s == session);
            let i = rel.partition_point(|&(t, _)| t <= close_us);
            let until = rel.get(i).map_or(self.cap_minutes, |&(t, _)| {
                ((t - close_us) as f64 / MINUTE_US as f64).min(self.cap_minutes)
            });
            let since = i
                .checked_sub(1)
                .and_then(|j| rel.get(j))
                .map_or(self.cap_minutes, |&(t, _)| {
                    ((close_us - t) as f64 / MINUTE_US as f64).min(self.cap_minutes)
                });
            if let Some(cell) = out.get_mut(3 * k) {
                *cell = f64::from(u8::from(day));
            }
            if let Some(cell) = out.get_mut(3 * k + 1) {
                *cell = until;
            }
            if let Some(cell) = out.get_mut(3 * k + 2) {
                *cell = since;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use vwap_data::time::utc_of_local;

    fn at(y: i32, m: u32, d: u32, minute: u32) -> i64 {
        utc_of_local(NaiveDate::from_ymd_opt(y, m, d).unwrap(), minute).unwrap()
    }

    fn table() -> CalendarTable {
        CalendarTable::new(
            [
                vec![at(2025, 2, 12, 510), at(2025, 3, 12, 510)],
                vec![at(2025, 1, 29, 840)],
                vec![],
            ],
            1440,
        )
    }

    fn session_of(us: i64) -> i32 {
        epoch_days(globex_anchor_date(ny_clock(us)))
    }

    #[test]
    fn a_disabled_table_is_all_missing() {
        let t = CalendarTable::disabled();
        assert!(!t.is_enabled());
        assert!(
            t.features(at(2025, 2, 12, 600), 0)
                .iter()
                .all(|x| x.is_nan())
        );
    }

    #[test]
    fn the_release_day_flag_follows_the_globex_session() {
        let t = table();
        // 10:00 on the CPI day, and 20:00 the evening before (same Globex session as the release)
        for close in [at(2025, 2, 12, 600), at(2025, 2, 11, 20 * 60)] {
            let f = t.features(close, session_of(close));
            assert_eq!(f[0], 1.0, "{close}");
        }
        // 20:00 on the CPI day already belongs to the next session
        let close = at(2025, 2, 12, 20 * 60);
        assert_eq!(t.features(close, session_of(close))[0], 0.0);
    }

    #[test]
    fn minutes_until_and_since_are_measured_from_the_close() {
        let t = table();
        let close = at(2025, 2, 12, 600); // 90 minutes after the CPI release
        let f = t.features(close, session_of(close));
        assert_eq!(f[2], 90.0);
        // the next CPI is 28 days away, far beyond the cap
        assert_eq!(f[1], 1440.0);
        // FOMC on 29 January is long past
        assert_eq!(f[4], 1440.0);
        assert_eq!(f[5], 1440.0);
        // 45 minutes before the CPI release
        let close = at(2025, 2, 12, 465);
        let f = t.features(close, session_of(close));
        assert_eq!(f[1], 45.0);
        assert_eq!(f[2], 1440.0); // no CPI release before it in this table
    }

    #[test]
    fn a_close_at_the_release_counts_as_since_zero_and_until_the_next() {
        let t = table();
        let close = at(2025, 2, 12, 510);
        let f = t.features(close, session_of(close));
        assert_eq!(f[2], 0.0);
        assert!(f[1] > 0.0);
    }

    #[test]
    fn an_empty_type_stays_at_the_cap_and_never_flags() {
        let t = table();
        let close = at(2025, 3, 3, 600);
        let f = t.features(close, session_of(close));
        assert_eq!((f[6], f[7], f[8]), (0.0, 1440.0, 1440.0));
    }
}
