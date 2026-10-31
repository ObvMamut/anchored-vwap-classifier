//! Rule based expiry dates, holidays and early closes.

use chrono::{Datelike, NaiveDate, TimeDelta, Weekday};

/// The n-th (1 based) given weekday of a month.
pub fn nth_weekday(year: i32, month: u32, weekday: Weekday, n: u32) -> Option<NaiveDate> {
    let first = NaiveDate::from_ymd_opt(year, month, 1)?;
    let offset = (7 + weekday.num_days_from_monday() - first.weekday().num_days_from_monday()) % 7;
    first.checked_add_signed(TimeDelta::days(i64::from(offset + 7 * (n - 1))))
}

fn last_weekday(year: i32, month: u32, weekday: Weekday) -> Option<NaiveDate> {
    let next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)?
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)?
    };
    let last = next.pred_opt()?;
    let back = (7 + last.weekday().num_days_from_monday() - weekday.num_days_from_monday()) % 7;
    last.checked_sub_signed(TimeDelta::days(i64::from(back)))
}

pub fn third_friday(year: i32, month: u32) -> Option<NaiveDate> {
    nth_weekday(year, month, Weekday::Fri, 3)
}

/// Quarterly expiries (third Friday of Mar, Jun, Sep, Dec) in `[from_year, to_year]`.
pub fn expiries(from_year: i32, to_year: i32) -> Vec<NaiveDate> {
    (from_year..=to_year)
        .flat_map(|y| {
            [3, 6, 9, 12]
                .into_iter()
                .filter_map(move |m| third_friday(y, m))
        })
        .collect()
}

/// Easter Sunday by the anonymous Gregorian algorithm.
pub fn easter(year: i32) -> Option<NaiveDate> {
    let a = year % 19;
    let b = year / 100;
    let c = year % 100;
    let d = b / 4;
    let e = b % 4;
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let i = c / 4;
    let k = c % 4;
    let l = (32 + 2 * e + 2 * i - h - k) % 7;
    let m = (a + 11 * h + 22 * l) / 451;
    let month = (h + l - 7 * m + 114) / 31;
    let day = (h + l - 7 * m + 114) % 31 + 1;
    NaiveDate::from_ymd_opt(year, u32::try_from(month).ok()?, u32::try_from(day).ok()?)
}

/// Rule based CME equity holidays and early close days for a year. The rule is
/// verified against the data, not trusted.
#[derive(Debug, Clone)]
pub struct HolidayRules {
    pub full_holidays: Vec<(NaiveDate, &'static str)>,
    pub early_closes: Vec<(NaiveDate, &'static str)>,
}

pub fn holiday_rules(year: i32) -> HolidayRules {
    let mut full = Vec::new();
    let mut early = Vec::new();
    let push = |v: &mut Vec<(NaiveDate, &'static str)>, d: Option<NaiveDate>, n: &'static str| {
        if let Some(d) = d {
            v.push((d, n));
        }
    };
    push(&mut full, NaiveDate::from_ymd_opt(year, 1, 1), "new_year");
    push(&mut full, nth_weekday(year, 1, Weekday::Mon, 3), "mlk");
    push(
        &mut full,
        nth_weekday(year, 2, Weekday::Mon, 3),
        "presidents",
    );
    push(
        &mut full,
        easter(year).and_then(|e| e.checked_sub_signed(TimeDelta::days(2))),
        "good_friday",
    );
    push(&mut full, last_weekday(year, 5, Weekday::Mon), "memorial");
    if year >= 2022 {
        push(
            &mut full,
            NaiveDate::from_ymd_opt(year, 6, 19),
            "juneteenth",
        );
    }
    push(
        &mut full,
        NaiveDate::from_ymd_opt(year, 7, 4),
        "independence",
    );
    push(&mut full, nth_weekday(year, 9, Weekday::Mon, 1), "labor");
    let thanksgiving = nth_weekday(year, 11, Weekday::Thu, 4);
    push(&mut full, thanksgiving, "thanksgiving");
    push(
        &mut full,
        NaiveDate::from_ymd_opt(year, 12, 25),
        "christmas",
    );
    push(
        &mut early,
        thanksgiving.and_then(|t| t.checked_add_signed(TimeDelta::days(1))),
        "day_after_thanksgiving",
    );
    push(
        &mut early,
        NaiveDate::from_ymd_opt(year, 12, 24),
        "christmas_eve",
    );
    push(&mut early, NaiveDate::from_ymd_opt(year, 7, 3), "july_3");
    HolidayRules {
        full_holidays: full,
        early_closes: early,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap_or_default()
    }

    #[test]
    fn third_fridays_match_known_nq_expiries() {
        assert_eq!(third_friday(2024, 3), Some(d(2024, 3, 15)));
        assert_eq!(third_friday(2024, 6), Some(d(2024, 6, 21)));
        assert_eq!(third_friday(2024, 9), Some(d(2024, 9, 20)));
        assert_eq!(third_friday(2024, 12), Some(d(2024, 12, 20)));
        assert_eq!(third_friday(2025, 3), Some(d(2025, 3, 21)));
    }

    #[test]
    fn easter_matches_known_dates() {
        assert_eq!(easter(2024), Some(d(2024, 3, 31)));
        assert_eq!(easter(2019), Some(d(2019, 4, 21)));
        assert_eq!(easter(2025), Some(d(2025, 4, 20)));
    }

    #[test]
    fn good_friday_and_thanksgiving_rules() {
        let r = holiday_rules(2024);
        assert!(r.full_holidays.contains(&(d(2024, 3, 29), "good_friday")));
        assert!(r.full_holidays.contains(&(d(2024, 11, 28), "thanksgiving")));
        assert!(
            r.early_closes
                .contains(&(d(2024, 11, 29), "day_after_thanksgiving"))
        );
    }

    #[test]
    fn quarterly_expiry_count() {
        assert_eq!(expiries(2017, 2025).len(), 36);
    }
}
