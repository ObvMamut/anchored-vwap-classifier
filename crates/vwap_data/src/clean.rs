//! Cleaning of the raw series: timestamp conversion, ordering, de duplication,
//! validity checks and per bar session columns.
//!
//! Numeric kernels index columns of equal length with indices produced by
//! iterating those same columns, so indexing cannot go out of bounds.
#![allow(clippy::indexing_slicing)]

use vwap_core::{DataConfig, WeightColumn};

use crate::error::DataError;
use crate::load::RawBars;
use crate::time::{ConvertStats, epoch_days, globex_anchor_date, labels_to_utc, ny_clock};

/// Bit flags stored per bar.
pub mod flag {
    pub const ZERO_WEIGHT: u32 = 1;
    pub const GAP_BEFORE: u32 = 2;
    pub const OUTLIER_RETURN: u32 = 4;
    pub const OUTLIER_RANGE: u32 = 8;
    pub const THIN_SESSION: u32 = 16;
    pub const ROLL_BUFFER: u32 = 32;
    pub const EARLY_CLOSE_SESSION: u32 = 64;
    pub const OFF_GRID: u32 = 128;
}

/// Cleaned 1 minute bars as equal length columns, ascending in UTC time.
#[derive(Debug, Clone, Default)]
pub struct CleanBars {
    pub ts_us: Vec<i64>,
    pub open: Vec<f64>,
    pub high: Vec<f64>,
    pub low: Vec<f64>,
    pub close: Vec<f64>,
    pub volume: Vec<f64>,
    pub tick_volume: Vec<f64>,
    /// The column chosen by `weight_column`.
    pub weight: Vec<f64>,
    /// New York calendar date, days since the Unix epoch.
    pub ny_date: Vec<i32>,
    /// New York minute of day.
    pub ny_minute: Vec<u16>,
    /// Anchor date (18:00 New York) of the containing Globex session.
    pub globex_session: Vec<i32>,
    pub flags: Vec<u32>,
}

impl CleanBars {
    pub fn len(&self) -> usize {
        self.ts_us.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ts_us.is_empty()
    }
}

/// Counts of what the cleaning step did.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct CleanCounts {
    pub rows_in: usize,
    pub nonexistent_local_dropped: usize,
    pub ambiguous_local_resolved: usize,
    pub identical_duplicates_dropped: usize,
    pub invalid_ohlc_dropped: usize,
    pub off_grid_prices: usize,
    pub rows_out: usize,
}

pub fn clean(raw: &RawBars, cfg: &DataConfig) -> Result<(CleanBars, CleanCounts), DataError> {
    let n = raw.len();
    let mut counts = CleanCounts {
        rows_in: n,
        ..CleanCounts::default()
    };
    let (utc, stats): (Vec<Option<i64>>, ConvertStats) =
        labels_to_utc(&raw.ts_us, cfg.timestamp_kind, cfg.server_offset_hours);
    counts.nonexistent_local_dropped = stats.nonexistent_local;
    counts.ambiguous_local_resolved = stats.ambiguous_local;

    let mut order: Vec<usize> = (0..n).filter(|&i| utc[i].is_some()).collect();
    order.sort_by_key(|&i| utc[i].unwrap_or(i64::MAX));

    let mut out = CleanBars::default();
    let mut last_idx: Option<usize> = None;
    let mut last_ts = i64::MIN;
    for &i in &order {
        let ts = utc[i].unwrap_or(i64::MAX);
        if ts == last_ts {
            let j = last_idx.unwrap_or(i);
            let same = raw.open[i] == raw.open[j]
                && raw.high[i] == raw.high[j]
                && raw.low[i] == raw.low[j]
                && raw.close[i] == raw.close[j]
                && raw.volume[i] == raw.volume[j]
                && raw.tick_volume[i] == raw.tick_volume[j];
            if same {
                counts.identical_duplicates_dropped += 1;
                continue;
            }
            return Err(DataError::Validation(format!(
                "conflicting duplicate rows at utc microsecond {ts}"
            )));
        }
        let (o, h, l, c) = (raw.open[i], raw.high[i], raw.low[i], raw.close[i]);
        let v = raw.volume[i];
        let tv = raw.tick_volume[i];
        let valid = [o, h, l, c].iter().all(|x| x.is_finite() && *x > 0.0)
            && h >= l
            && o >= l
            && o <= h
            && c >= l
            && c <= h
            && v.is_finite()
            && v >= 0.0
            && tv.is_finite()
            && tv >= 0.0;
        if !valid {
            if cfg.drop_invalid_bars {
                counts.invalid_ohlc_dropped += 1;
                last_ts = ts;
                last_idx = Some(i);
                continue;
            }
            return Err(DataError::Validation(format!(
                "invalid OHLC or volume at utc microsecond {ts}"
            )));
        }
        let on_grid = [o, h, l, c].iter().all(|&x| {
            let q = x / cfg.price_grid;
            (q - q.round()).abs() < 1e-6
        });
        let clock = ny_clock(ts);
        let weight = match cfg.weight_column {
            WeightColumn::Volume => v,
            WeightColumn::TickVolume => tv,
        };
        let mut f = 0u32;
        if !on_grid {
            f |= flag::OFF_GRID;
            counts.off_grid_prices += 1;
        }
        if weight == 0.0 {
            f |= flag::ZERO_WEIGHT;
        }
        out.ts_us.push(ts);
        out.open.push(o);
        out.high.push(h);
        out.low.push(l);
        out.close.push(c);
        out.volume.push(v);
        out.tick_volume.push(tv);
        out.weight.push(weight);
        out.ny_date.push(epoch_days(clock.date));
        out.ny_minute.push(u16::try_from(clock.minute).unwrap_or(0));
        out.globex_session
            .push(epoch_days(globex_anchor_date(clock)));
        out.flags.push(f);
        last_ts = ts;
        last_idx = Some(i);
    }
    counts.rows_out = out.len();
    Ok((out, counts))
}
