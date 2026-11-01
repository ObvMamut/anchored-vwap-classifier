//! The registered features. Each is a function of the `CausalView`, the touch event
//! (all of whose fields are known at the close of the touch bar) and finished sessions.
#![allow(clippy::indexing_slicing)]

use vwap_core::FeaturesConfig;
use vwap_events::{CoarseBars, Event};

use crate::calendar::CalendarTable;
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

const NAMES: [&str; 27] = [
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
    "conf_count",
    "cal_cpi_day",
    "cal_cpi_until",
    "cal_cpi_since",
    "cal_fomc_day",
    "cal_fomc_until",
    "cal_fomc_since",
    "cal_nfp_day",
    "cal_nfp_until",
    "cal_nfp_since",
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
    cfg: &FeaturesConfig,
) -> [f64; 18] {
    let nan = f64::NAN;
    let t = v.t();
    let s = f64::from(e.side);
    let sigma = e.atr;
    let w = v.width() as usize;

    // vwap slope aligned with the approach side over `slope_minutes`
    let l_slope = (cfg.slope_minutes / w).max(1);
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

    // approach speed over `speed_bars` bars
    let speed = match (
        t.checked_sub(cfg.speed_bars + 1),
        signed_dist(v, s, t.wrapping_sub(1)),
    ) {
        (Some(j), Some(d1)) if v.in_touch_session(j) => match signed_dist(v, s, j) {
            Some(d0) => (d0 - d1) / (cfg.speed_bars as f64 * sigma),
            None => nan,
        },
        _ => nan,
    };

    // relative volume against the same clock slot in earlier sessions
    let rel_volume = {
        let mut hist = Vec::new();
        if let (Some(pos), Some(m), Some(vol)) =
            (sessions.position_of_bar(t), v.minute(t), v.weight(t))
        {
            for k in (0..pos).rev().take(cfg.rel_volume_sessions) {
                let (_, a, z) = sessions.spans[k];
                // earlier sessions are entirely before the touch bar
                if let Some(x) = (a..z)
                    .find(|&i| v.minute(i) == Some(m))
                    .and_then(|i| v.weight(i))
                {
                    hist.push(x);
                }
            }
            if hist.len() >= cfg.rel_volume_min_sessions {
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
    let after_open =
        (u32::from(cfg.rth_open_minute)..cfg.globex_open_minute).contains(&close_minute);
    let lv = levels.for_event(e.trading_date, after_open);
    let rej = |l: f64| s * (l - e.close) / sigma;
    let (dpdh, dpdl, dpdc, donh, donl) = match lv {
        Some(l) => (rej(l.pdh), rej(l.pdl), rej(l.pdc), rej(l.onh), rej(l.onl)),
        None => (nan, nan, nan, nan, nan),
    };
    let near = [dpdh, dpdl, dpdc, donh, donl]
        .iter()
        .filter(|x| x.is_finite())
        .any(|x| x.abs() <= cfg.confluence_band_sigma);
    let conf_flag = if lv.is_some() {
        f64::from(u8::from(near))
    } else {
        nan
    };
    // number of the five levels within the confluence band of the touch close
    let conf_count = if lv.is_some() {
        [dpdh, dpdl, dpdc, donh, donl]
            .iter()
            .filter(|x| x.is_finite() && x.abs() <= cfg.confluence_band_sigma)
            .count() as f64
    } else {
        nan
    };

    // time blocks by the New York clock of the touch close
    let edges = cfg.time_block_edges;
    let time_block = if (edges[0]..edges[1]).contains(&close_minute) {
        0.0
    } else if (edges[1]..edges[2]).contains(&close_minute) {
        1.0
    } else if (edges[2]..edges[3]).contains(&close_minute) {
        2.0
    } else {
        3.0
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
        sigma / e.close * cfg.vol_ratio_scale,
        band_width,
        conf_flag,
        dpdh,
        dpdl,
        dpdc,
        donh,
        time_block,
        conf_count,
    ]
}

/// Builds the feature table for `events` (all on the same coarse bars).
pub fn build_features(
    events: &[Event],
    bars: &CoarseBars,
    levels: &LevelTable,
    sessions: &SessionIndex,
    calendar: &CalendarTable,
    cfg: &FeaturesConfig,
) -> FeatureTable {
    let mut cols = vec![Vec::with_capacity(events.len()); NAMES.len()];
    for e in events {
        let v = CausalView::new(bars, e.bar);
        let row = feature_row(e, &v, levels, sessions, cfg);
        let cal = calendar.features(e.close_us, e.session);
        for (c, x) in cols.iter_mut().zip(row.into_iter().chain(cal)) {
            c.push(x);
        }
    }
    FeatureTable {
        names: NAMES.to_vec(),
        cols,
    }
}
