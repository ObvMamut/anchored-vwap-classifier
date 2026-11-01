//! Roll diagnostics. The series is a single stitched series, so a roll shows up
//! only as a step between two consecutive sessions.
#![allow(clippy::indexing_slicing)]

use chrono::{Datelike, NaiveDate, TimeDelta};
use serde::Serialize;
use vwap_core::DataConfig;

use crate::calendar::expiries;
use crate::clean::CleanBars;
use crate::time::{MINUTE_US, epoch_days};

/// A boundary between two sessions: the bar index of the first bar after a break.
#[derive(Debug, Clone, Copy)]
pub struct Boundary {
    pub idx: usize,
    pub ny_date: i32,
    pub step: f64,
}

pub fn boundaries(b: &CleanBars) -> Vec<Boundary> {
    (1..b.len())
        .filter(|&k| b.ts_us[k] - b.ts_us[k - 1] >= 55 * MINUTE_US)
        .map(|k| Boundary {
            idx: k,
            ny_date: b.ny_date[k],
            step: b.open[k] - b.close[k - 1],
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct OffsetStep {
    pub offset_days: i64,
    pub boundary_date: String,
    pub step: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RollRow {
    pub expiry: String,
    pub predicted_roll_date: String,
    pub predicted_step: f64,
    pub observed_boundary_date: String,
    pub observed_offset_days: i64,
    pub observed_step: f64,
    pub confirmed: bool,
    pub steps_by_offset: Vec<OffsetStep>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RollReport {
    pub roll_offset_days: i64,
    pub typical_abs_step_median: f64,
    pub typical_abs_step_threshold: f64,
    pub confirmed: usize,
    pub total: usize,
    pub observed_offset_histogram: std::collections::BTreeMap<i64, usize>,
    pub rows: Vec<RollRow>,
}

fn date_of(days: i32) -> NaiveDate {
    NaiveDate::from_ymd_opt(1970, 1, 1)
        .and_then(|e| e.checked_add_signed(TimeDelta::days(i64::from(days))))
        .unwrap_or_default()
}

fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let pos = (q * (sorted.len() - 1) as f64).round() as usize;
    sorted[pos.min(sorted.len() - 1)]
}

/// Computes the roll table. Also returns the bar index of the boundary used as
/// the roll point of each expiry (observed when confirmed, otherwise predicted).
pub fn roll_report(b: &CleanBars, cfg: &DataConfig) -> (RollReport, Vec<usize>) {
    let bounds = boundaries(b);
    let mut abs: Vec<f64> = bounds.iter().map(|x| x.step.abs()).collect();
    abs.sort_by(|a, c| a.partial_cmp(c).unwrap_or(std::cmp::Ordering::Equal));
    let median = percentile(&abs, 0.5);
    let threshold = percentile(&abs, cfg.roll_confirm_percentile);

    let (first, last) = match (b.ny_date.first(), b.ny_date.last()) {
        (Some(&f), Some(&l)) => (date_of(f), date_of(l)),
        _ => {
            return (
                RollReport {
                    roll_offset_days: cfg.roll_offset_days,
                    typical_abs_step_median: 0.0,
                    typical_abs_step_threshold: 0.0,
                    confirmed: 0,
                    total: 0,
                    observed_offset_histogram: Default::default(),
                    rows: vec![],
                },
                vec![],
            );
        }
    };
    let mut rows = Vec::new();
    let mut roll_idx = Vec::new();
    let mut hist = std::collections::BTreeMap::new();
    for e in expiries(first.year(), last.year()) {
        let window_start = e - TimeDelta::days(cfg.roll_search_window_days);
        if e > last || window_start < first {
            continue;
        }
        let in_window: Vec<&Boundary> = bounds
            .iter()
            .filter(|x| x.ny_date >= epoch_days(window_start) && x.ny_date <= epoch_days(e))
            .collect();
        let Some(best) = in_window.iter().max_by(|a, c| {
            a.step
                .abs()
                .partial_cmp(&c.step.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        }) else {
            continue;
        };
        let first_on_or_after = |d: NaiveDate| bounds.iter().find(|x| x.ny_date >= epoch_days(d));
        let predicted_date = e - TimeDelta::days(cfg.roll_offset_days);
        let predicted = first_on_or_after(predicted_date);
        let steps_by_offset = (0..=14)
            .filter_map(|o| {
                first_on_or_after(e - TimeDelta::days(o)).map(|x| OffsetStep {
                    offset_days: o,
                    boundary_date: date_of(x.ny_date).to_string(),
                    step: x.step,
                })
            })
            .collect();
        let observed_offset = (e - date_of(best.ny_date)).num_days();
        let confirmed = best.step.abs() > threshold;
        if confirmed {
            *hist.entry(observed_offset).or_insert(0usize) += 1;
        }
        let used = if confirmed {
            Some(best.idx)
        } else {
            predicted.map(|x| x.idx)
        };
        if let Some(i) = used {
            roll_idx.push(i);
        }
        rows.push(RollRow {
            expiry: e.to_string(),
            predicted_roll_date: predicted_date.to_string(),
            predicted_step: predicted.map_or(0.0, |x| x.step),
            observed_boundary_date: date_of(best.ny_date).to_string(),
            observed_offset_days: observed_offset,
            observed_step: best.step,
            confirmed,
            steps_by_offset,
        });
    }
    let confirmed = rows.iter().filter(|r| r.confirmed).count();
    (
        RollReport {
            roll_offset_days: cfg.roll_offset_days,
            typical_abs_step_median: median,
            typical_abs_step_threshold: threshold,
            confirmed,
            total: rows.len(),
            observed_offset_histogram: hist,
            rows,
        },
        roll_idx,
    )
}
