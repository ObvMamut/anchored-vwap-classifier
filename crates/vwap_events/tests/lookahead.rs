//! Lookahead and invariant property tests for VWAP, ATR and touch detection.
#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)]

use chrono::NaiveDate;
use proptest::prelude::*;
use vwap_core::DataConfig;
use vwap_data::{RawBars, clean::clean, time::utc_of_local};
use vwap_events::{CoarseBars, build, detect, event_params};

const MIN: i64 = 60_000_000;

fn data_cfg() -> DataConfig {
    toml::from_str(
        r#"
input_path = "x"
output_dir = "x"
artifact_dir = "x"
timestamp_kind = "utc"
server_offset_hours = 7
bar_label = "open"
session_anchor = "globex"
weight_column = "tick_volume"
price_grid = 0.1
drop_invalid_bars = true
roll_offset_days = 8
roll_buffer_sessions = 1
roll_confirm_percentile = 0.99
roll_search_window_days = 21
max_gap_minutes_ok = 2
max_gap_minutes_event = 5
outlier_z = 12.0
outlier_window_bars = 100
max_range_median_multiple = 40.0
min_bars_fraction = 0.9
early_close_time = "13:15"
flat_time_globex = "16:55"
flat_time_rth = "16:00"
expected_session_bars = 1380.0
"#,
    )
    .unwrap()
}

fn events_cfg(k: f64, exclude: u32) -> vwap_core::EventsConfig {
    toml::from_str(&format!(
        r#"
timeframes = [5, 15]
atr_length = 14
rearm_k = {k}
exclude_minutes = {exclude}
vwap_reference = "bar_close"
output_dir = "x"
spotcheck_days = 1
spotcheck_seed = 1
spotcheck_dir = "x"
font_paths = ["x"]
spotcheck_min_session_minutes = 1000
"#
    ))
    .unwrap()
}

/// One Globex session of `minutes` bars from 18:00 New York, a random walk on the 0.1 grid.
fn session_bars(steps: &[i32], weights: &[u8]) -> vwap_data::CleanBars {
    let d0 = NaiveDate::from_ymd_opt(2024, 1, 16).unwrap();
    let mut ts = Vec::new();
    let mut m = 1080u32;
    let mut day = d0;
    for _ in 0..steps.len() {
        ts.push(utc_of_local(day, m).unwrap());
        m += 1;
        if m == 1440 {
            m = 0;
            day = day.succ_opt().unwrap();
        }
    }
    let mut price = 18000.0f64;
    let (mut o, mut h, mut l, mut c) = (vec![], vec![], vec![], vec![]);
    for s in steps {
        let open = price;
        price = (price + f64::from(*s) * 0.1).max(1000.0);
        let close = (price * 10.0).round() / 10.0;
        o.push(open);
        c.push(close);
        h.push(open.max(close) + 0.2);
        l.push(open.min(close) - 0.2);
    }
    let n = steps.len();
    let raw = RawBars {
        ts_us: ts,
        open: o,
        high: h,
        low: l,
        close: c,
        volume: vec![0.0; n],
        tick_volume: weights.iter().map(|&w| f64::from(w) + 1.0).collect(),
        source_dtype: "test".into(),
    };
    clean(&raw, &data_cfg()).unwrap().0
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
