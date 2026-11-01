//! Synthetic bars for the tests of later stages: one session of coarse bars with the 1 minute
//! bars underneath. Not used by any pipeline stage.

use vwap_data::CleanBars;

use crate::CoarseBars;

/// One session of `width` minute bars built from `(open, high, low, close)` rows, flat VWAP 100
/// and ATR 2, session 1. Every coarse bar is `width` identical 1 minute bars (same open, high,
/// low, close) unless the caller overrides them in the returned `CleanBars`.
pub fn session_fixture(width: u32, rows: &[(f64, f64, f64, f64)]) -> (CoarseBars, CleanBars) {
    let n = rows.len();
    let w_us = i64::from(width) * 60_000_000;
    let bars = CoarseBars {
        width,
        open_us: (0..n as i64).map(|i| i * w_us).collect(),
        open: rows.iter().map(|r| r.0).collect(),
        high: rows.iter().map(|r| r.1).collect(),
        low: rows.iter().map(|r| r.2).collect(),
        close: rows.iter().map(|r| r.3).collect(),
        weight: vec![1.0; n],
        pv: vec![1.0; n],
        vwap: vec![100.0; n],
        vwap_sd: vec![1.0; n],
        session: vec![1; n],
        trading_date: vec![2; n],
        minute: (0..n)
            .map(|i| ((1080 + width as usize * i) % 1440) as u16)
            .collect(),
        n_minutes: vec![width as u8; n],
        flags: vec![0; n],
        atr: vec![2.0; n],
    };
    let mut one = CleanBars::default();
    for (i, r) in rows.iter().enumerate() {
        for m in 0..i64::from(width) {
            one.ts_us
                .push((i as i64 * i64::from(width) + m) * 60_000_000);
            one.open.push(r.0);
            one.high.push(r.1);
            one.low.push(r.2);
            one.close.push(r.3);
        }
    }
    (bars, one)
}
