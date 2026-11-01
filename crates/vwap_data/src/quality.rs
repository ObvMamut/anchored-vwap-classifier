//! Quality checks on cleaned bars: gaps, outliers, session shape, bar label
//! convention and holiday rules. Flags are written onto the bars.
#![allow(clippy::indexing_slicing)]

use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate, TimeDelta};
use serde::Serialize;
use vwap_core::{BarLabel, DataConfig};

use crate::calendar::holiday_rules;
use crate::clean::{CleanBars, flag};
use crate::error::DataError;
use crate::time::{MINUTE_US, epoch_days};

#[derive(Debug, Default, Serialize)]
pub struct GapReport {
    pub minor_gaps_2_to_ok: usize,
    pub scheduled_maintenance: usize,
    pub scheduled_weekend: usize,
    pub extended_closures: usize,
    pub unexpected_short: usize,
    pub unexpected_long: usize,
    pub unexpected_long_by_year: BTreeMap<i32, usize>,
    pub longest_unexpected: Vec<(String, i64)>,
    pub extended_closure_dates: Vec<(String, i64)>,
}

#[derive(Debug, Default, Serialize)]
pub struct OutlierReport {
    pub return_outliers: usize,
    pub range_outliers: usize,
    pub return_outliers_by_year: BTreeMap<i32, usize>,
}

#[derive(Debug, Default, Serialize)]
pub struct SessionReport {
    pub sessions: usize,
    pub thin_sessions: usize,
    pub early_close_sessions: usize,
    pub median_bars: f64,
    pub bars_histogram: BTreeMap<String, usize>,
    pub thin_by_year: BTreeMap<i32, usize>,
}

#[derive(Debug, Default, Serialize)]
pub struct LabelCheck {
    pub gaps_prev_bar_minute_1019: usize,
    pub gaps_prev_bar_minute_1020: usize,
    pub first_bar_minute_1080: usize,
    pub first_bar_minute_1081: usize,
    pub detected: String,
    pub bars_in_17h_hour: usize,
}

#[derive(Debug, Default, Serialize)]
pub struct HolidayCheck {
    pub full_holidays_with_bars: Vec<(String, String, usize, u16)>,
    pub early_closes: Vec<(String, String, usize, u16)>,
}

#[derive(Debug, Default, Serialize)]
pub struct QualityReport {
    pub gaps: GapReport,
    pub outliers: OutlierReport,
    pub sessions: SessionReport,
    pub label_check: LabelCheck,
    pub holidays: HolidayCheck,
    pub zero_weight_bars: usize,
    pub zero_weight_by_year: BTreeMap<i32, usize>,
    pub flag_counts: BTreeMap<String, usize>,
}

fn year_of(days: i32) -> i32 {
    NaiveDate::from_ymd_opt(1970, 1, 1)
        .and_then(|e| e.checked_add_signed(TimeDelta::days(i64::from(days))))
        .map_or(0, |d| d.year())
}

fn date_str(days: i32) -> String {
    NaiveDate::from_ymd_opt(1970, 1, 1)
        .and_then(|e| e.checked_add_signed(TimeDelta::days(i64::from(days))))
        .map_or_else(String::new, |d| d.to_string())
}

fn median_in_place(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let m = v.len() / 2;
    if v.len() % 2 == 1 {
        v[m]
    } else {
        (v[m - 1] + v[m]) / 2.0
    }
}

pub fn analyze(b: &mut CleanBars, cfg: &DataConfig) -> Result<QualityReport, DataError> {
    let mut rep = QualityReport::default();
    let n = b.len();

    // Gaps and label convention evidence.
    let mut label = LabelCheck::default();
    for k in 1..n {
        let dt = (b.ts_us[k] - b.ts_us[k - 1]) / MINUTE_US;
        if dt <= 1 {
            continue;
        }
        let prev_min = b.ny_minute[k - 1];
        let scheduled = dt >= 55 && (prev_min == 1019 || prev_min == 1020);
        if scheduled {
            if prev_min == 1019 {
                label.gaps_prev_bar_minute_1019 += 1;
            } else {
                label.gaps_prev_bar_minute_1020 += 1;
            }
            match b.ny_minute[k] {
                1080 => label.first_bar_minute_1080 += 1,
                1081 => label.first_bar_minute_1081 += 1,
                _ => {}
            }
            if dt < 120 {
                rep.gaps.scheduled_maintenance += 1;
            } else if (2800..=3100).contains(&dt) {
                rep.gaps.scheduled_weekend += 1;
            } else {
                rep.gaps.extended_closures += 1;
                rep.gaps
                    .extended_closure_dates
                    .push((date_str(b.ny_date[k - 1]), dt));
            }
        } else if dt <= i64::from(cfg.max_gap_minutes_ok) {
            rep.gaps.minor_gaps_2_to_ok += 1;
        } else if dt <= i64::from(cfg.max_gap_minutes_event) {
            rep.gaps.unexpected_short += 1;
        } else {
            rep.gaps.unexpected_long += 1;
            *rep.gaps
                .unexpected_long_by_year
                .entry(year_of(b.ny_date[k]))
                .or_default() += 1;
            rep.gaps.longest_unexpected.push((
                format!("{} minute {}", date_str(b.ny_date[k - 1]), prev_min),
                dt,
            ));
            b.flags[k] |= flag::GAP_BEFORE;
        }
    }
    rep.gaps
        .longest_unexpected
        .sort_by_key(|g| std::cmp::Reverse(g.1));
    rep.gaps.longest_unexpected.truncate(30);
    rep.gaps.extended_closure_dates.truncate(60);
    label.bars_in_17h_hour = b
        .ny_minute
        .iter()
        .filter(|&&m| (1020..1080).contains(&m))
        .count();
    let open_votes = label.gaps_prev_bar_minute_1019;
    let close_votes = label.gaps_prev_bar_minute_1020;
    label.detected = if open_votes > close_votes {
        "open"
    } else {
        "close"
    }
    .into();
    let configured = match cfg.bar_label {
        BarLabel::Open => "open",
        BarLabel::Close => "close",
    };
    if label.detected != configured {
        return Err(DataError::Validation(format!(
            "bar_label is {configured} in config but the session boundaries indicate {}",
            label.detected
        )));
    }
    rep.label_check = label;

    // Outliers from the previous block of bars (causal).
    let w = cfg.outlier_window_bars;
    let mut start = 0usize;
    let mut prev_stats: Option<(f64, f64, f64)> = None; // median, mad, median range
    while start < n {
        let end = (start + w).min(n);
        if let Some((med, mad, med_range)) = prev_stats {
            for k in start.max(1)..end {
                let contiguous =
                    b.ts_us[k] - b.ts_us[k - 1] <= i64::from(cfg.max_gap_minutes_ok) * MINUTE_US;
                if contiguous && mad > 0.0 {
                    let r = (b.close[k] / b.close[k - 1]).ln();
                    let z = (r - med) / (1.4826 * mad);
                    if z.abs() > cfg.outlier_z {
                        b.flags[k] |= flag::OUTLIER_RETURN;
                        rep.outliers.return_outliers += 1;
                        *rep.outliers
                            .return_outliers_by_year
                            .entry(year_of(b.ny_date[k]))
                            .or_default() += 1;
                    }
                }
                if med_range > 0.0
                    && (b.high[k] - b.low[k]) > cfg.max_range_median_multiple * med_range
                {
                    b.flags[k] |= flag::OUTLIER_RANGE;
                    rep.outliers.range_outliers += 1;
                }
            }
        }
        let mut rets: Vec<f64> = (start.max(1)..end)
            .filter(|&k| {
                b.ts_us[k] - b.ts_us[k - 1] <= i64::from(cfg.max_gap_minutes_ok) * MINUTE_US
            })
            .map(|k| (b.close[k] / b.close[k - 1]).ln())
            .collect();
        let med = median_in_place(&mut rets);
        let mut devs: Vec<f64> = rets.iter().map(|r| (r - med).abs()).collect();
        let mad = median_in_place(&mut devs);
        let mut ranges: Vec<f64> = (start..end).map(|k| b.high[k] - b.low[k]).collect();
        let med_range = median_in_place(&mut ranges);
        prev_stats = Some((med, mad, med_range));
        start = end;
    }

    // Sessions.
    let mut sess: BTreeMap<i32, (usize, usize, usize, u16)> = BTreeMap::new(); // first idx, last idx, count, last minute
    for k in 0..n {
        let e = sess.entry(b.globex_session[k]).or_insert((k, k, 0, 0));
        e.1 = k;
        e.2 += 1;
        e.3 = b.ny_minute[k];
    }
    let mut counts: Vec<f64> = sess.values().map(|v| v.2 as f64).collect();
    let flat_min = u16::try_from(
        cfg.flat_time_globex
            .0
            .format("%H")
            .to_string()
            .parse::<u32>()
            .unwrap_or(16)
            * 60
            + cfg
                .flat_time_globex
                .0
                .format("%M")
                .to_string()
                .parse::<u32>()
                .unwrap_or(55),
    )
    .unwrap_or(1015);
    rep.sessions.sessions = sess.len();
    rep.sessions.median_bars = median_in_place(&mut counts);
    for (&date, &(first, last, count, last_min)) in &sess {
        let bucket = match count {
            0..=299 => "under 300",
            300..=899 => "300 to 899",
            900..=1241 => "900 to 1241",
            1242..=1379 => "1242 to 1379",
            1380 => "1380",
            _ => "over 1380",
        };
        *rep.sessions
            .bars_histogram
            .entry(bucket.into())
            .or_default() += 1;
        let thin = (count as f64) < cfg.min_bars_fraction * cfg.expected_session_bars;
        let early = last_min < flat_min && last_min >= 600;
        for f in &mut b.flags[first..=last] {
            if thin {
                *f |= flag::THIN_SESSION;
            }
            if early {
                *f |= flag::EARLY_CLOSE_SESSION;
            }
        }
        if thin {
            rep.sessions.thin_sessions += 1;
            *rep.sessions.thin_by_year.entry(year_of(date)).or_default() += 1;
        }
        if early {
            rep.sessions.early_close_sessions += 1;
        }
    }

    // Holiday rules versus observed data.
    if let (Some(&f), Some(&l)) = (b.ny_date.first(), b.ny_date.last()) {
        for year in year_of(f)..=year_of(l) {
            let rules = holiday_rules(year);
            for (d, name) in rules.full_holidays.iter().chain(rules.early_closes.iter()) {
                let days = epoch_days(*d);
                if days < f || days > l {
                    continue;
                }
                let idx: Vec<usize> = (0..n).filter(|&k| b.ny_date[k] == days).collect();
                // restrict to the daytime so the evening reopening is not counted
                let day_bars: Vec<usize> = idx
                    .into_iter()
                    .filter(|&k| (570..1020).contains(&b.ny_minute[k]))
                    .collect();
                let last_min = day_bars.last().map_or(0, |&k| b.ny_minute[k]);
                let rec = (d.to_string(), (*name).to_string(), day_bars.len(), last_min);
                if rules.full_holidays.iter().any(|(x, _)| x == d) {
                    if !day_bars.is_empty() {
                        rep.holidays.full_holidays_with_bars.push(rec);
                    }
                } else {
                    rep.holidays.early_closes.push(rec);
                }
            }
        }
    }

    for k in 0..n {
        if b.flags[k] & flag::ZERO_WEIGHT != 0 {
            rep.zero_weight_bars += 1;
            *rep.zero_weight_by_year
                .entry(year_of(b.ny_date[k]))
                .or_default() += 1;
        }
    }
    rep.flag_counts = count_flags(b);
    Ok(rep)
}

/// Counts of each flag bit over all bars.
pub fn count_flags(b: &CleanBars) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for (name, bit) in [
        ("zero_weight", flag::ZERO_WEIGHT),
        ("gap_before", flag::GAP_BEFORE),
        ("outlier_return", flag::OUTLIER_RETURN),
        ("outlier_range", flag::OUTLIER_RANGE),
        ("thin_session", flag::THIN_SESSION),
        ("roll_buffer", flag::ROLL_BUFFER),
        ("early_close_session", flag::EARLY_CLOSE_SESSION),
        ("off_grid", flag::OFF_GRID),
    ] {
        m.insert(
            name.to_string(),
            b.flags.iter().filter(|&&f| f & bit != 0).count(),
        );
    }
    m
}

/// Marks the bars of the first `buffer_sessions` Globex sessions at or after each roll boundary.
pub fn flag_roll_buffers(b: &mut CleanBars, roll_idx: &[usize], buffer_sessions: u32) {
    for &i in roll_idx {
        let Some(&s0) = b.globex_session.get(i) else {
            continue;
        };
        let mut sessions_seen = 0u32;
        let mut cur = s0;
        for k in i..b.len() {
            if b.globex_session[k] != cur {
                sessions_seen += 1;
                cur = b.globex_session[k];
            }
            if sessions_seen >= buffer_sessions.max(1) {
                break;
            }
            b.flags[k] |= flag::ROLL_BUFFER;
        }
    }
}
