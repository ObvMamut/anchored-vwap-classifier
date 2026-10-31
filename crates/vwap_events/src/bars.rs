//! Coarse bars with the anchored VWAP and Wilder ATR.
//!
//! Everything is computed in a single forward pass, so a value at bar `i` can only
//! depend on 1 minute bars up to the close of bar `i`.
#![allow(clippy::indexing_slicing)]

use vwap_core::{DataConfig, SessionAnchor};
use vwap_data::CleanBars;
use vwap_data::clean::flag;

const MIN_US: i64 = 60_000_000;
pub const GLOBEX_ANCHOR_MIN: u32 = 18 * 60;
pub const RTH_ANCHOR_MIN: u32 = 9 * 60 + 30;

/// Bars of one width, with VWAP, its dispersion and ATR, in time order.
#[derive(Debug, Clone, Default)]
pub struct CoarseBars {
    pub width: u32,
    /// Open time of the bucket in UTC microseconds.
    pub open_us: Vec<i64>,
    pub open: Vec<f64>,
    pub high: Vec<f64>,
    pub low: Vec<f64>,
    pub close: Vec<f64>,
    pub weight: Vec<f64>,
    /// Anchored VWAP at the last minute of the bar.
    pub vwap: Vec<f64>,
    /// Weight dispersion of the typical price around the VWAP at the last minute.
    pub vwap_sd: Vec<f64>,
    /// Session key: the anchor date in days since the epoch.
    pub session: Vec<i32>,
    pub trading_date: Vec<i32>,
    /// New York minute of day at the bucket open.
    pub minute: Vec<u16>,
    pub n_minutes: Vec<u8>,
    pub flags: Vec<u32>,
    pub atr: Vec<f64>,
}

impl CoarseBars {
    pub fn len(&self) -> usize {
        self.open_us.len()
    }

    pub fn is_empty(&self) -> bool {
        self.open_us.is_empty()
    }

    pub fn partial(&self, i: usize) -> bool {
        self.n_minutes
            .get(i)
            .is_none_or(|&n| u32::from(n) < self.width)
    }
}

pub fn anchor_minute(anchor: SessionAnchor) -> u32 {
    match anchor {
        SessionAnchor::Globex => GLOBEX_ANCHOR_MIN,
        SessionAnchor::Rth => RTH_ANCHOR_MIN,
    }
}

/// Flat time as New York minute of day.
pub fn flat_minute(cfg: &DataConfig) -> u32 {
    use chrono::Timelike;
    let t = match cfg.session_anchor {
        SessionAnchor::Globex => cfg.flat_time_globex.0,
        SessionAnchor::Rth => cfg.flat_time_rth.0,
    };
    t.hour() * 60 + t.minute()
}

/// Session key and trading date of a 1 minute bar under the configured anchor.
fn session_of(b: &CleanBars, k: usize, cfg: &DataConfig) -> Option<(i32, i32)> {
    match cfg.session_anchor {
        SessionAnchor::Globex => Some((b.globex_session[k], b.globex_session[k] + 1)),
        SessionAnchor::Rth => {
            let m = u32::from(b.ny_minute[k]);
            (RTH_ANCHOR_MIN..flat_minute(cfg))
                .contains(&m)
                .then_some((b.ny_date[k], b.ny_date[k]))
        }
    }
}

pub fn build(b: &CleanBars, cfg: &DataConfig, width: u32, atr_length: usize) -> CoarseBars {
    let mut out = CoarseBars {
        width,
        ..CoarseBars::default()
    };
    let mut cur: Option<(i32, i32, u32)> = None; // session, ny_date, bucket
    let (mut s_pv, mut s_v, mut mu, mut m2) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let mut cur_session = i32::MIN;
    for k in 0..b.len() {
        let Some((session, tdate)) = session_of(b, k, cfg) else {
            continue;
        };
        if session != cur_session {
            cur_session = session;
            (s_pv, s_v, mu, m2) = (0.0, 0.0, 0.0, 0.0);
        }
        let minute = u32::from(b.ny_minute[k]);
        let bucket = minute / width;
        let w = b.weight[k];
        if w > 0.0 {
            let p = (b.high[k] + b.low[k] + b.close[k]) / 3.0;
            s_pv += p * w;
            s_v += w;
            let prev_mu = mu;
            mu += (w / s_v) * (p - mu);
            m2 += w * (p - prev_mu) * (p - mu);
        }
        let vwap = if s_v > 0.0 { s_pv / s_v } else { f64::NAN };
        let sd = if s_v > 0.0 {
            (m2 / s_v).max(0.0).sqrt()
        } else {
            f64::NAN
        };
        let key = (session, b.ny_date[k], bucket);
        if cur != Some(key) {
            cur = Some(key);
            let offset = i64::from(minute - bucket * width);
            out.open_us.push(b.ts_us[k] - offset * MIN_US);
            out.open.push(b.open[k]);
            out.high.push(b.high[k]);
            out.low.push(b.low[k]);
            out.close.push(b.close[k]);
            out.weight.push(w);
            out.vwap.push(vwap);
            out.vwap_sd.push(sd);
            out.session.push(session);
            out.trading_date.push(tdate);
            out.minute.push(u16::try_from(bucket * width).unwrap_or(0));
            out.n_minutes.push(1);
            out.flags.push(b.flags[k]);
            out.atr.push(f64::NAN);
        } else {
            let i = out.len() - 1;
            out.high[i] = out.high[i].max(b.high[k]);
            out.low[i] = out.low[i].min(b.low[k]);
            out.close[i] = b.close[k];
            out.weight[i] += w;
            out.vwap[i] = vwap;
            out.vwap_sd[i] = sd;
            out.n_minutes[i] = out.n_minutes[i].saturating_add(1);
            out.flags[i] |= b.flags[k];
        }
    }
    wilder_atr(&mut out, atr_length);
    out
}

/// Wilder ATR. The first value is the mean of the first `n` true ranges. On the first bar
/// after a roll flag starts, the true range uses the bar range only.
fn wilder_atr(b: &mut CoarseBars, n: usize) {
    let mut sum = 0.0;
    let mut atr = f64::NAN;
    for i in 0..b.len() {
        let range = b.high[i] - b.low[i];
        let tr = if i == 0
            || (b.flags[i] & flag::ROLL_BUFFER != 0 && b.flags[i - 1] & flag::ROLL_BUFFER == 0)
        {
            range
        } else {
            let pc = b.close[i - 1];
            range.max((b.high[i] - pc).abs()).max((b.low[i] - pc).abs())
        };
        if i < n {
            sum += tr;
            if i + 1 == n {
                atr = sum / n as f64;
                b.atr[i] = atr;
            }
        } else {
            atr = ((n - 1) as f64 * atr + tr) / n as f64;
            b.atr[i] = atr;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wilder_atr_matches_a_hand_computation() {
        let mut b = CoarseBars {
            width: 5,
            open_us: vec![0; 4],
            high: vec![10.0, 11.0, 12.0, 15.0],
            low: vec![8.0, 9.0, 10.0, 11.0],
            close: vec![9.0, 10.0, 11.0, 14.0],
            flags: vec![0; 4],
            atr: vec![f64::NAN; 4],
            ..CoarseBars::default()
        };
        wilder_atr(&mut b, 3);
        assert!(b.atr[0].is_nan() && b.atr[1].is_nan());
        assert!((b.atr[2] - 2.0).abs() < 1e-12);
        assert!((b.atr[3] - 8.0 / 3.0).abs() < 1e-12);
    }
}
