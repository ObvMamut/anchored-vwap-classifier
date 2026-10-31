//! The registered features. Each is a function of the `CausalView`, the touch event
//! (all of whose fields are known at the close of the touch bar) and finished sessions.
#![allow(clippy::indexing_slicing)]

use vwap_events::{CoarseBars, Event};

use crate::levels::LevelTable;
use crate::view::{CausalView, SessionIndex};

/// Features in column major order, aligned with the events they were built for.
#[derive(Debug, Clone, Default)]
pub struct FeatureTable {
    pub names: Vec<&'static str>,
    pub cols: Vec<Vec<f64>>,
}

impl FeatureTable {
    pub fn col(&self, name: &str) -> Option<&[f64]> {
        self.names
            .iter()
            .position(|&n| n == name)
            .and_then(|i| self.cols.get(i))
            .map(Vec::as_slice)
    }
}

const NAMES: [&str; 17] = [
    "vslope_align",
    "dist_travelled",
    "bars_on_side",
    "speed",
    "rel_volume",
    "ordinal",
    "raw_touches_before",
    "minute_of_day",
    "minutes_since_anchor",
    "vol_ratio",
    "band_width",
    "conf_flag",
    "dist_pdh",
    "dist_pdl",
    "dist_pdc",
    "dist_onh",
    "time_block",
];

pub fn feature_names() -> &'static [&'static str] {
    &NAMES
}

/// Signed distance to the VWAP, positive on the approach side, for bar `j`.
fn signed_dist(v: &CausalView, s: f64, j: usize) -> Option<f64> {
    Some(s * (v.close(j)? - v.vwap(j)?))
}

/// The run of consecutive bars before the touch bar that closed on the approach side.
fn run_length(v: &CausalView, s: f64) -> usize {
    let t = v.t();
    let mut n = 0;
    while n < t {
        let j = t - 1 - n;
        match signed_dist(v, s, j) {
            Some(d) if d > 0.0 && v.in_touch_session(j) => n += 1,
            _ => break,
        }
    }
    n
}

fn feature_row(
    e: &Event,
    v: &CausalView,
    levels: &LevelTable,
    sessions: &SessionIndex,
) -> [f64; 17] {
    let nan = f64::NAN;
    let t = v.t();
    let s = f64::from(e.side);
    let sigma = e.atr;
    let w = v.width() as usize;

    // vwap slope aligned with the approach side, over about 60 minutes
    let l_slope = (60 / w).max(1);
    let vslope_align = match (t.checked_sub(l_slope), v.vwap(t)) {
        (Some(j), Some(now)) if v.in_touch_session(j) => match v.vwap(j) {
            Some(then) => s * (now - then) / (l_slope as f64 * sigma),
            None => nan,
        },
        _ => nan,
    };

    let n_run = run_length(v, s);
    let dist_travelled = (1..=n_run)
        .filter_map(|k| signed_dist(v, s, t - k))
        .fold(f64::NAN, f64::max)
        / sigma;

    // approach speed over 3 bars
    let speed = match (t.checked_sub(4), signed_dist(v, s, t.wrapping_sub(1))) {
        (Some(j), Some(d1)) if v.in_touch_session(j) => match signed_dist(v, s, j) {
            Some(d0) => (d0 - d1) / (3.0 * sigma),
            None => nan,
        },
        _ => nan,
    };

    // relative volume against the same clock slot in up to 20 earlier sessions
    let rel_volume = {
        let mut hist = Vec::new();
        if let (Some(pos), Some(m), Some(vol)) =
            (sessions.position_of_bar(t), v.minute(t), v.weight(t))
        {
            for k in (0..pos).rev().take(20) {
                let (_, a, z) = sessions.spans[k];
                // earlier sessions are entirely before the touch bar
                if let Some(x) = (a..z)
                    .find(|&i| v.minute(i) == Some(m))
                    .and_then(|i| v.weight(i))
                {
                    hist.push(x);
                }
            }
            if hist.len() >= 10 {
                vol / (hist.iter().sum::<f64>() / hist.len() as f64)
            } else {
                nan
            }
        } else {
            nan
        }
    };

    let close_minute = (u32::from(v.minute(t).unwrap_or(0)) + v.width()) % 1440;
    let band_width = v.vwap_sd(t).map_or(nan, |x| x / sigma);

    // levels
    let after_open = (570..1080).contains(&close_minute);
    let lv = levels.for_event(e.trading_date, after_open);
    let rej = |l: f64| s * (l - e.close) / sigma;
    let (dpdh, dpdl, dpdc, donh, donl) = match lv {
        Some(l) => (rej(l.pdh), rej(l.pdl), rej(l.pdc), rej(l.onh), rej(l.onl)),
        None => (nan, nan, nan, nan, nan),
    };
    let near = [dpdh, dpdl, dpdc, donh, donl]
        .iter()
        .filter(|x| x.is_finite())
        .any(|x| x.abs() <= 0.5);
    let conf_flag = if lv.is_some() {
        f64::from(u8::from(near))
    } else {
        nan
    };

    // time blocks by the New York clock of the touch close
    let time_block = match close_minute {
        570..630 => 0.0,
        630..840 => 1.0,
        840..1020 => 2.0,
        _ => 3.0,
    };

    [
        vslope_align,
        dist_travelled,
        n_run as f64,
        speed,
        rel_volume,
        f64::from(e.ordinal),
        f64::from(e.raw_touches_before),
        f64::from(close_minute),
        f64::from(e.minutes_since_anchor),
        sigma / e.close * 1e4,
        band_width,
        conf_flag,
        dpdh,
        dpdl,
        dpdc,
        donh,
        time_block,
    ]
}

/// Builds the feature table for `events` (all on the same coarse bars).
pub fn build_features(
    events: &[Event],
    bars: &CoarseBars,
    levels: &LevelTable,
    sessions: &SessionIndex,
) -> FeatureTable {
    let mut cols = vec![Vec::with_capacity(events.len()); NAMES.len()];
    for e in events {
        let v = CausalView::new(bars, e.bar);
        let row = feature_row(e, &v, levels, sessions);
        for (c, x) in cols.iter_mut().zip(row) {
            c.push(x);
        }
    }
    FeatureTable {
        names: NAMES.to_vec(),
        cols,
    }
}
