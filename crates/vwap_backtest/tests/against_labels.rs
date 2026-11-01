//! With no costs and an entry at the touch close, a trade's gross result equals the label
//! outcome of the same event (`docs/BACKTEST.md`, section 8).
#![allow(clippy::indexing_slicing, clippy::unwrap_used)]

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use vwap_backtest::{Signal, simulate};
use vwap_core::TICK_SIZE;
use vwap_events::fixture::session_fixture;
use vwap_labels::{Hit, scan_barriers};

#[test]
fn gross_ticks_equal_the_label_outcome_when_the_entry_is_the_touch_close() {
    let mut rng = ChaCha8Rng::seed_from_u64(17);
    let (sigma, horizon) = (2.0, 6usize);
    let mut compared = 0;
    for _ in 0..40 {
        // a contiguous random walk: every bar opens at the previous close
        let mut rows = Vec::new();
        let mut px = 100.0;
        for _ in 0..60 {
            let o = px;
            let step: f64 = (rng.random::<f64>() - 0.5) * 2.0;
            let c = ((o + step) * 4.0).round() / 4.0;
            let h = o.max(c) + rng.random::<f64>() * 0.5;
            let l = o.min(c) - rng.random::<f64>() * 0.5;
            rows.push((o, h, l, c));
            px = c;
        }
        let (bars, one) = session_fixture(5, &rows);
        for t in 0..(rows.len() - horizon - 1) {
            for side in [1i8, -1] {
                let s = f64::from(side);
                let p0 = bars.close[t];
                let (b_rej, b_brk) = (p0 + s * sigma, p0 - s * sigma);
                let scan = scan_barriers(&bars, &one, t, horizon, side, p0, b_rej, b_brk);
                let label = match scan.hit {
                    Hit::Rejection => sigma / TICK_SIZE,
                    Hit::Breakthrough => -sigma / TICK_SIZE,
                    Hit::None => s * (bars.close[t + horizon] - p0) / TICK_SIZE,
                    Hit::Ambiguous => continue,
                };
                let sig = Signal {
                    event_id: 0,
                    session: 1,
                    fold: 0,
                    bar: t,
                    side,
                    close: p0,
                    b_rej,
                    b_brk,
                    horizon,
                    p_rej: 1.0,
                    tau: 0.0,
                };
                let trade = simulate(&sig, &bars, &one).unwrap();
                assert!(
                    (trade.gross_ticks - label).abs() < 1e-9,
                    "bar {t} side {side}: {} vs {label}",
                    trade.gross_ticks
                );
                compared += 1;
            }
        }
    }
    assert!(compared > 1000);
}
