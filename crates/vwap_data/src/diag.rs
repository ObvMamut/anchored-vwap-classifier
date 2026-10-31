//! First look diagnostics on the raw series: ordering, price grid, volume,
//! session hours and gaps.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, Timelike, Utc};
use chrono_tz::America::New_York;
use serde::Serialize;

use crate::load::RawBars;

#[derive(Debug, Serialize)]
pub struct RawDiagnostics {
    pub rows: usize,
    pub source_dtype: String,
    pub ascending_steps: usize,
    pub descending_steps: usize,
    pub equal_steps: usize,
    pub first_utc: String,
    pub last_utc: String,
    pub share_on_quarter_grid: f64,
    pub share_on_tenth_grid: f64,
    pub volume_zero_share: f64,
    pub tick_volume_zero_share: f64,
    pub volume_zero_share_by_year: BTreeMap<i32, f64>,
    pub mean_volume_nonzero: f64,
    pub mean_tick_volume: f64,
    pub bars_by_ny_hour: BTreeMap<u32, usize>,
    pub bars_by_ny_weekday: BTreeMap<u32, usize>,
    pub gaps_over_30_min_by_ny_start: BTreeMap<String, usize>,
    pub bars_per_year: BTreeMap<i32, usize>,
}

fn on_grid(x: f64, step: f64) -> bool {
    let q = x / step;
    (q - q.round()).abs() < 1e-6
}

pub fn utc(ts_us: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_micros(ts_us).unwrap_or_default()
}

/// Diagnostics on the raw series in file order.
pub fn raw_diagnostics(raw: &RawBars) -> RawDiagnostics {
    let n = raw.len();
    let (mut asc, mut desc, mut eq) = (0, 0, 0);
    for w in raw.ts_us.windows(2) {
        if let [a, b] = w {
            if b > a {
                asc += 1;
            } else if b < a {
                desc += 1;
            } else {
                eq += 1;
            }
        }
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| raw.ts_us.get(i).copied().unwrap_or(0));
    let ts: Vec<i64> = order
        .iter()
        .map(|&i| raw.ts_us.get(i).copied().unwrap_or(0))
        .collect();

    let all_prices = [&raw.open, &raw.high, &raw.low, &raw.close];
    let total_prices = (n * 4).max(1) as f64;
    let quarter = all_prices
        .iter()
        .map(|c| c.iter().filter(|&&x| on_grid(x, 0.25)).count())
        .sum::<usize>() as f64
        / total_prices;
    let tenth = all_prices
        .iter()
        .map(|c| c.iter().filter(|&&x| on_grid(x, 0.1)).count())
        .sum::<usize>() as f64
        / total_prices;

    let vol_zero = raw.volume.iter().filter(|&&v| v == 0.0).count();
    let tv_zero = raw.tick_volume.iter().filter(|&&v| v == 0.0).count();
    let nz: Vec<f64> = raw.volume.iter().copied().filter(|&v| v > 0.0).collect();

    let mut zero_by_year: BTreeMap<i32, (usize, usize)> = BTreeMap::new();
    let mut bars_per_year: BTreeMap<i32, usize> = BTreeMap::new();
    let mut by_hour: BTreeMap<u32, usize> = BTreeMap::new();
    let mut by_wd: BTreeMap<u32, usize> = BTreeMap::new();
    for (&t, &v) in raw.ts_us.iter().zip(&raw.volume) {
        let dt = utc(t);
        let e = zero_by_year.entry(dt.year()).or_default();
        e.1 += 1;
        if v == 0.0 {
            e.0 += 1;
        }
        *bars_per_year.entry(dt.year()).or_default() += 1;
        let ny = dt.with_timezone(&New_York);
        *by_hour.entry(ny.hour()).or_default() += 1;
        *by_wd
            .entry(ny.weekday().num_days_from_monday())
            .or_default() += 1;
    }

    let mut gaps: BTreeMap<String, usize> = BTreeMap::new();
    for w in ts.windows(2) {
        if let [a, b] = w {
            let mins = (b - a) / 60_000_000;
            if mins > 30 {
                let ny = utc(*a).with_timezone(&New_York);
                let key = format!(
                    "{} {:02}:{:02} for {}m",
                    ny.format("%a"),
                    ny.hour(),
                    ny.minute(),
                    mins.min(10_000)
                );
                *gaps.entry(key).or_default() += 1;
            }
        }
    }
    // keep only the most common gap signatures
    let mut gap_vec: Vec<(String, usize)> = gaps.into_iter().collect();
    gap_vec.sort_by_key(|g| std::cmp::Reverse(g.1));
    gap_vec.truncate(25);

    RawDiagnostics {
        rows: n,
        source_dtype: raw.source_dtype.clone(),
        ascending_steps: asc,
        descending_steps: desc,
        equal_steps: eq,
        first_utc: utc(ts.first().copied().unwrap_or(0)).to_rfc3339(),
        last_utc: utc(ts.last().copied().unwrap_or(0)).to_rfc3339(),
        share_on_quarter_grid: quarter,
        share_on_tenth_grid: tenth,
        volume_zero_share: vol_zero as f64 / n.max(1) as f64,
        tick_volume_zero_share: tv_zero as f64 / n.max(1) as f64,
        volume_zero_share_by_year: zero_by_year
            .into_iter()
            .map(|(y, (z, t))| (y, z as f64 / t.max(1) as f64))
            .collect(),
        mean_volume_nonzero: nz.iter().sum::<f64>() / nz.len().max(1) as f64,
        mean_tick_volume: raw.tick_volume.iter().sum::<f64>() / n.max(1) as f64,
        bars_by_ny_hour: by_hour,
        bars_by_ny_weekday: by_wd,
        gaps_over_30_min_by_ny_start: gap_vec.into_iter().collect(),
        bars_per_year,
    }
}
