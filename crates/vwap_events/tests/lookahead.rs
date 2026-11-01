//! Lookahead and invariant property tests for VWAP, ATR and touch detection.
#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)]

use proptest::prelude::*;
use vwap_core::DataConfig;
use vwap_events::{CoarseBars, build, detect, event_params};

const MIN: i64 = 60_000_000;

fn data_cfg() -> DataConfig {
    vwap_testkit::data_config()
}

fn events_cfg(k: f64, exclude: u32) -> vwap_core::EventsConfig {
    vwap_testkit::events_config(k, exclude)
}

fn session_bars(steps: &[i32], weights: &[u8]) -> vwap_data::CleanBars {
    vwap_testkit::session_bars(steps, weights)
}

fn coarse_prefix_equal(a: &CoarseBars, b: &CoarseBars, m: usize) -> bool {
    (0..m).all(|i| {
        a.open_us[i] == b.open_us[i]
            && a.open[i].to_bits() == b.open[i].to_bits()
            && a.high[i].to_bits() == b.high[i].to_bits()
            && a.low[i].to_bits() == b.low[i].to_bits()
            && a.close[i].to_bits() == b.close[i].to_bits()
            && a.vwap[i].to_bits() == b.vwap[i].to_bits()
            && a.vwap_sd[i].to_bits() == b.vwap_sd[i].to_bits()
            && a.atr[i].to_bits() == b.atr[i].to_bits()
            && a.n_minutes[i] == b.n_minutes[i]
    })
}

proptest! {
    /// Replacing every 1 minute bar after a bar boundary must not change any value known before it.
    #[test]
    fn vwap_atr_and_events_do_not_look_ahead(
        steps_a in proptest::collection::vec(-30i32..30, 420),
        steps_b in proptest::collection::vec(-30i32..30, 420),
        weights_a in proptest::collection::vec(0u8..50, 420),
        weights_b in proptest::collection::vec(0u8..50, 420),
        cut_quarter_hours in 8usize..26,
        k in 0.3f64..2.0,
    ) {
        let cut = cut_quarter_hours * 15; // minutes from the anchor, a multiple of 15
        let mut steps = steps_a.clone();
        steps[cut..].copy_from_slice(&steps_b[cut..]);
        let mut weights = weights_a.clone();
        weights[cut..].copy_from_slice(&weights_b[cut..]);
        let a = session_bars(&steps_a, &weights_a);
        let b = session_bars(&steps, &weights);
        let cfg = data_cfg();
        let ev = events_cfg(k, 30);
        let params = event_params(&cfg, &ev);
        for w in [5u32, 15] {
            let ca = build(&a, &cfg, w, ev.atr_length);
            let cb = build(&b, &cfg, w, ev.atr_length);
            let cut_us = a.ts_us[0] + cut as i64 * MIN;
            let m = (0..ca.len()).filter(|&i| ca.open_us[i] + i64::from(w) * MIN <= cut_us).count();
            prop_assert!(coarse_prefix_equal(&ca, &cb, m));
            let (ea, _) = detect(&ca, &params);
            let (eb, _) = detect(&cb, &params);
            let pa: Vec<_> = ea.iter().filter(|e| e.close_us <= cut_us).collect();
            let pb: Vec<_> = eb.iter().filter(|e| e.close_us <= cut_us).collect();
            prop_assert_eq!(pa, pb);
        }
    }

    /// Structural invariants of every emitted event.
    #[test]
    fn events_satisfy_the_documented_invariants(
        steps in proptest::collection::vec(-40i32..40, 600),
        weights in proptest::collection::vec(0u8..50, 600),
        k in 0.2f64..2.0,
        exclude in 0u32..90,
    ) {
        let cfg = data_cfg();
        let ev = events_cfg(k, exclude);
        let params = event_params(&cfg, &ev);
        let bars = session_bars(&steps, &weights);
        for w in [5u32, 15] {
            let c = build(&bars, &cfg, w, ev.atr_length);
            let (events, _) = detect(&c, &params);
            for e in &events {
                // the touch inequality, on the VWAP used for the test
                prop_assert!(e.low <= e.vwap && e.vwap <= e.high);
                // the side comes from the previous bar
                let prev = e.bar - 1;
                let expected = if c.close[prev] > c.vwap[prev] { 1 } else { -1 };
                prop_assert_eq!(e.side, expected);
                prop_assert!(e.minutes_since_anchor >= exclude);
                prop_assert!(e.minutes_to_flat >= 0);
                prop_assert!(e.atr > 0.0);
            }
            // consecutive events in a session are separated by a re arm excursion
            for pair in events.windows(2) {
                if pair[0].session == pair[1].session {
                    let rearmed = (pair[0].bar + 1..pair[1].bar)
                        .any(|j| (c.close[j] - c.vwap[j]).abs() / c.atr[j] >= k);
                    prop_assert!(rearmed, "events at bars {} and {} without a re arm", pair[0].bar, pair[1].bar);
                }
            }
        }
    }

    /// Coarse bars aggregate the 1 minute bars exactly.
    #[test]
    fn coarse_bars_aggregate_one_minute_bars(
        steps in proptest::collection::vec(-40i32..40, 300),
        weights in proptest::collection::vec(0u8..50, 300),
    ) {
        let cfg = data_cfg();
        let bars = session_bars(&steps, &weights);
        let c = build(&bars, &cfg, 5, 14);
        for i in 0..c.len() {
            prop_assert!(c.low[i] <= c.open[i] && c.low[i] <= c.close[i]);
            prop_assert!(c.high[i] >= c.open[i] && c.high[i] >= c.close[i]);
        }
        let total: f64 = c.weight.iter().sum();
        let expected: f64 = bars.weight.iter().sum();
        prop_assert!((total - expected).abs() < 1e-9);
        prop_assert_eq!(c.n_minutes.iter().map(|&n| usize::from(n)).sum::<usize>(), bars.len());
    }
}

#[test]
fn vwap_equals_the_hand_computed_value_and_welford_matches_two_pass() {
    let steps = vec![5, -3, 8, -2, 4, 1, -6, 7, 2, -1];
    let weights = vec![3u8, 1, 4, 1, 5, 9, 2, 6, 5, 3];
    let bars = session_bars(&steps, &weights);
    let cfg = data_cfg();
    let c = build(&bars, &cfg, 5, 14);
    // direct sums over the first five minutes
    let (mut spv, mut sv) = (0.0, 0.0);
    for k in 0..5 {
        let p = (bars.high[k] + bars.low[k] + bars.close[k]) / 3.0;
        spv += p * bars.weight[k];
        sv += bars.weight[k];
    }
    assert!((c.vwap[0] - spv / sv).abs() < 1e-9);
    let mean = spv / sv;
    let var: f64 = (0..5)
        .map(|k| {
            let p = (bars.high[k] + bars.low[k] + bars.close[k]) / 3.0;
            bars.weight[k] * (p - mean).powi(2)
        })
        .sum::<f64>()
        / sv;
    assert!((c.vwap_sd[0] - var.sqrt()).abs() < 1e-9);
}
