//! No lookahead: replacing every 1 minute bar after a cut leaves the features of
//! all events that closed before the cut bit for bit unchanged.
#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)]

use chrono::NaiveDate;
use proptest::prelude::*;
use vwap_core::DataConfig;
use vwap_data::{RawBars, clean::clean, time::utc_of_local};
use vwap_events::{build, detect, event_params};
use vwap_features::{LevelTable, SessionIndex, build_features, feature_names};

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
min_bars_fraction = 0.5
early_close_time = "13:15"
flat_time_globex = "16:55"
flat_time_rth = "16:00"
expected_session_bars = 1380.0
"#,
    )
    .unwrap()
}

fn events_cfg() -> vwap_core::EventsConfig {
    toml::from_str(
        r#"
timeframes = [5, 15]
atr_length = 14
rearm_k = 0.3
exclude_minutes = 30
vwap_reference = "bar_close"
output_dir = "x"
spotcheck_days = 1
spotcheck_seed = 1
spotcheck_dir = "x"
font_paths = ["x"]
spotcheck_min_session_minutes = 1000
"#,
    )
    .unwrap()
}

const DAYS: usize = 5;
const PER_DAY: usize = 1380; // 18:00 to 17:00

/// Five Globex sessions on consecutive weekdays, a random walk with random tick volume.
fn series(steps: &[i32], weights: &[u8]) -> vwap_data::CleanBars {
    let mut ts = Vec::new();
    let mut anchor = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(); // a Monday
    for _ in 0..DAYS {
        for m in 1080..1440u32 {
            ts.push(utc_of_local(anchor, m).unwrap());
        }
        let next = anchor.succ_opt().unwrap();
        for m in 0..1020u32 {
            ts.push(utc_of_local(next, m).unwrap());
        }
        anchor = next;
    }
    let n = ts.len();
    assert_eq!(n, DAYS * PER_DAY);
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

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn features_do_not_look_ahead(
        steps_a in proptest::collection::vec(-40i32..40, DAYS * PER_DAY),
        steps_b in proptest::collection::vec(-40i32..40, DAYS * PER_DAY),
        weights_a in proptest::collection::vec(0u8..60, DAYS * PER_DAY),
        weights_b in proptest::collection::vec(0u8..60, DAYS * PER_DAY),
        cut_quarters in 150usize..440,
        width in prop_oneof![Just(5u32), Just(15u32)],
    ) {
        let cut = cut_quarters * 15;
        let mut steps = steps_a.clone();
        steps[cut..].copy_from_slice(&steps_b[cut..]);
        let mut weights = weights_a.clone();
        weights[cut..].copy_from_slice(&weights_b[cut..]);
        let a = series(&steps_a, &weights_a);
        let b = series(&steps, &weights);
        let cfg = data_cfg();
        let ev = events_cfg();
        let params = event_params(&cfg, &ev);
        let cut_us = a.ts_us[cut];
        let table = |bars: &vwap_data::CleanBars| {
            let c = build(bars, &cfg, width, ev.atr_length);
            let (events, _) = detect(&c, &params);
            let keep: Vec<_> = events.into_iter().filter(|e| e.close_us <= cut_us).collect();
            let lv = LevelTable::build(bars, &vwap_core::FeaturesConfig::default());
            let si = SessionIndex::build(&c);
            let f = build_features(&keep, &c, &lv, &si, &vwap_core::FeaturesConfig::default());
            (keep, f)
        };
        let (ea, fa) = table(&a);
        let (eb, fb) = table(&b);
        prop_assert_eq!(ea.len(), eb.len());
        for (name, (ca, cb)) in feature_names().iter().zip(fa.cols.iter().zip(&fb.cols)) {
            for (i, (x, y)) in ca.iter().zip(cb).enumerate() {
                prop_assert!(x.to_bits() == y.to_bits(), "feature {name} differs at event {i}: {x} vs {y}");
            }
        }
    }
}

#[test]
fn features_are_defined_for_a_typical_event() {
    let steps: Vec<i32> = (0..DAYS * PER_DAY)
        .map(|i| ((i * 7919) % 41) as i32 - 20)
        .collect();
    let weights: Vec<u8> = (0..DAYS * PER_DAY).map(|i| (i % 50) as u8).collect();
    let bars = series(&steps, &weights);
    let cfg = data_cfg();
    let ev = events_cfg();
    let c = build(&bars, &cfg, 15, ev.atr_length);
    let (events, _) = detect(&c, &event_params(&cfg, &ev));
    assert!(!events.is_empty());
    let f = build_features(
        &events,
        &c,
        &LevelTable::build(&bars, &vwap_core::FeaturesConfig::default()),
        &SessionIndex::build(&c),
        &vwap_core::FeaturesConfig::default(),
    );
    // late events on later days have prior day levels and relative volume
    let late = events
        .iter()
        .position(|e| e.session > events[0].session + 2)
        .unwrap();
    assert!(f.col("dist_pdh").unwrap()[late].is_finite());
    // fewer than 10 earlier sessions exist in this fixture, so relative volume is undefined by design
    assert!(f.col("rel_volume").unwrap()[late].is_nan());
    assert!(
        f.col("dist_travelled")
            .unwrap()
            .iter()
            .all(|x| x.is_nan() || *x > 0.0)
    );
    assert!(f.col("bars_on_side").unwrap().iter().all(|x| *x >= 1.0));
}

/// A fingerprint of every feature value on a fixed synthetic series. It pins the numeric
/// behaviour of the feature code, so moving constants into configuration cannot change a result
/// without this test failing.
#[test]
fn golden_feature_fingerprint() {
    let steps: Vec<i32> = (0..DAYS * PER_DAY)
        .map(|i| ((i * 7919) % 41) as i32 - 20)
        .collect();
    let weights: Vec<u8> = (0..DAYS * PER_DAY).map(|i| (i % 50) as u8).collect();
    let bars = series(&steps, &weights);
    let cfg = data_cfg();
    let ev = events_cfg();
    let params = event_params(&cfg, &ev);
    let mut bytes: Vec<u8> = Vec::new();
    for width in [5u32, 15] {
        let c = build(&bars, &cfg, width, ev.atr_length);
        let (events, _) = detect(&c, &params);
        let f = build_features(
            &events,
            &c,
            &LevelTable::build(&bars, &vwap_core::FeaturesConfig::default()),
            &SessionIndex::build(&c),
            &vwap_core::FeaturesConfig::default(),
        );
        bytes.extend((events.len() as u64).to_le_bytes());
        for col in &f.cols {
            for x in col {
                bytes.extend(x.to_bits().to_le_bytes());
            }
        }
    }
    let hash = vwap_core::manifest::sha256_bytes(&bytes);
    assert_eq!(
        hash, "6964c46b48b653d076962b26156e6b2a017e32f2ca887da68d474cc3803ce023",
        "feature fingerprint"
    );
}
