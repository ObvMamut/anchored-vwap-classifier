//! A synthetic market for the strategy tests: events spread over the discovery range, each with
//! a short price path that either rejects, breaks through or chops, and samples whose labels
//! come from the same barrier scan the backtest uses.

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use vwap_data::CleanBars;
use vwap_dataset::Sample;
use vwap_events::{CoarseBars, fixture::session_fixture};
use vwap_features::feature_names;
use vwap_labels::{Hit, scan_barriers};

pub const HORIZON: usize = 8;
const BLOCK: usize = 12;
const WIDTH_US: i64 = 15 * 60_000_000;
const DAY_US: i64 = 86_400_000_000;

pub struct Market {
    pub bars: CoarseBars,
    pub one: CleanBars,
    pub samples: Vec<Sample>,
}

/// `signal`: the feature at column 0 moves the chance of a rejection from 0.02 to 0.98;
/// otherwise the chance is 0.45 whatever the features say.
pub fn market(n_events: usize, signal: bool, seed: u64, start_us: i64, end_us: i64) -> Market {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let n_feat = feature_names().len();
    let mut rows: Vec<(f64, f64, f64, f64)> = Vec::new();
    let mut meta = Vec::new();
    for e in 0..n_events {
        let u: f64 = rng.random();
        let side: i8 = if rng.random::<bool>() { 1 } else { -1 };
        let chop = rng.random::<f64>() < 0.10;
        let p_rej = if signal { 0.02 + 0.96 * u } else { 0.45 };
        let rej = rng.random::<f64>() < p_rej;
        let d = f64::from(side) * if rej { 1.0 } else { -1.0 };
        rows.push((100.0, 100.3, 99.7, 100.0));
        for j in 1..=HORIZON {
            if chop {
                rows.push((100.0, 100.2, 99.8, 100.0));
            } else if j == 1 {
                let c = 100.0 + d * 1.2;
                rows.push((100.0, 100.0f64.max(c) + 0.1, 100.0f64.min(c) - 0.1, c));
            } else if j == 2 {
                let (o, c) = (100.0 + d * 1.2, 100.0 + d * 2.4);
                rows.push((o, o.max(c) + 0.2, o.min(c) - 0.2, c));
            } else {
                let c = 100.0 + d * 2.4;
                rows.push((c, c + 0.2, c - 0.2, c));
            }
        }
        for _ in 0..(BLOCK - 1 - HORIZON) {
            rows.push((100.0, 100.2, 99.8, 100.0));
        }
        let noise: Vec<f64> = (0..n_feat).map(|_| rng.random()).collect();
        meta.push((e, u, side, noise));
    }
    let (mut bars, mut one) = session_fixture(15, &rows);
    let span = (end_us - start_us) as f64;
    for (e, _, _, _) in &meta {
        let t0 = start_us + (span * (*e as f64) / n_events as f64) as i64;
        for j in 0..BLOCK {
            let i = e * BLOCK + j;
            bars.open_us[i] = t0 + j as i64 * WIDTH_US;
            bars.session[i] = (bars.open_us[i] / DAY_US) as i32;
            for m in 0..15 {
                one.ts_us[i * 15 + m] = bars.open_us[i] + m as i64 * 60_000_000;
            }
        }
    }
    let mut samples = Vec::new();
    for (e, u, side, noise) in meta {
        let t = e * BLOCK;
        let s = f64::from(side);
        let (p0, sigma) = (bars.close[t], 2.0);
        let (b_rej, b_brk) = (p0 + s * sigma, p0 - s * sigma);
        let scan = scan_barriers(&bars, &one, t, HORIZON, side, p0, b_rej, b_brk);
        let class = match scan.hit {
            Hit::Rejection => Some(vwap_core::Class::Rejection),
            Hit::Breakthrough => Some(vwap_core::Class::Breakthrough),
            Hit::None => Some(vwap_core::Class::Chop),
            Hit::Ambiguous => None,
        };
        let outcome = match scan.hit {
            Hit::Rejection => 8.0,
            Hit::Breakthrough => -8.0,
            Hit::None => s * (bars.close[t + HORIZON] - p0) / 0.25,
            Hit::Ambiguous => f64::NAN,
        };
        let mut features = noise;
        features[0] = u;
        samples.push(Sample {
            width: 15,
            event_id: e as u32,
            session: bars.session[t],
            year: 2018,
            t_close_us: bars.open_us[t] + WIDTH_US,
            t_end_us: bars.open_us[t + scan.resolved_at] + WIDTH_US,
            side,
            class,
            ambiguous: class.is_none(),
            outcome_ticks: outcome,
            sigma_over_price: 0.02,
            bar: t,
            close: p0,
            sigma,
            b_rej,
            b_brk,
            r_t_ticks: 0.0,
            features,
        });
    }
    Market { bars, one, samples }
}
