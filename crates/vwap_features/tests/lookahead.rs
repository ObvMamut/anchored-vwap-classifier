//! No lookahead: replacing every 1 minute bar after a cut leaves the features of
//! all events that closed before the cut bit for bit unchanged.
#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)]

use proptest::prelude::*;
use vwap_core::DataConfig;
use vwap_events::{build, detect, event_params};
use vwap_features::{LevelTable, SessionIndex, build_features, feature_names};

fn data_cfg() -> DataConfig {
    vwap_testkit::data_config()
}

fn events_cfg() -> vwap_core::EventsConfig {
    vwap_testkit::events_config(0.3, 30)
}

const DAYS: usize = 5;
const PER_DAY: usize = vwap_testkit::BARS_PER_DAY;

/// Five Globex sessions on consecutive weekdays, a random walk with random tick volume.
fn series(steps: &[i32], weights: &[u8]) -> vwap_data::CleanBars {
    vwap_testkit::multi_day_bars(DAYS, steps, weights)
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
