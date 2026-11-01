//! Lookahead, symmetry and invariant properties of the triple barrier labels.
#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)]

use chrono::NaiveDate;
use proptest::prelude::*;
use vwap_core::{Class, DataConfig};
use vwap_data::{RawBars, clean::clean, time::utc_of_local};
use vwap_events::{build, detect, event_params};
use vwap_labels::{LabelParams, label_events};

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
"#,
    )
    .unwrap()
}

fn events_cfg() -> vwap_core::EventsConfig {
    toml::from_str(
        r#"
timeframes = [5, 15]
atr_length = 14
rearm_k = 0.5
exclude_minutes = 30
vwap_reference = "bar_close"
output_dir = "x"
spotcheck_days = 1
spotcheck_seed = 1
spotcheck_dir = "x"
font_path = "x"
"#,
    )
    .unwrap()
}

fn session_bars(steps: &[i32]) -> vwap_data::CleanBars {
    let d0 = NaiveDate::from_ymd_opt(2024, 1, 16).unwrap();
    let (mut m, mut day) = (1080u32, d0);
    let mut ts = Vec::new();
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
        tick_volume: vec![10.0; n],
        source_dtype: "test".into(),
    };
    clean(&raw, &data_cfg()).unwrap().0
}

proptest! {
    /// Replacing every 1 minute bar after the end of a label leaves that label unchanged.
    #[test]
    fn labels_do_not_look_past_their_resolution(
        steps_a in proptest::collection::vec(-40i32..40, 700),
        steps_b in proptest::collection::vec(-40i32..40, 700),
        pick in 0usize..1000,
        width in prop_oneof![Just(5u32), Just(15u32)],
    ) {
        let cfg = data_cfg();
        let ev = events_cfg();
        let params = event_params(&cfg, &ev);
        let lp = LabelParams { m1: 1.0, m2: 1.0, horizon: 6 };
        let a = session_bars(&steps_a);
        let ca = build(&a, &cfg, width, ev.atr_length);
        let (ea, _) = detect(&ca, &params);
        let (la, _) = label_events(&ea, &ca, &a, &lp);
        prop_assume!(!la.is_empty());
        let target = &la[pick % la.len()];
        let cut = target.t_end_us;
        // Splice: keep the first part, replace everything from the cut with other random data.
        let first_bad = a.ts_us.partition_point(|&t| t < cut);
        let mut steps = steps_a.clone();
        steps[first_bad..].copy_from_slice(&steps_b[first_bad..]);
        let b = session_bars(&steps);
        let cb = build(&b, &cfg, width, ev.atr_length);
        let (eb, _) = detect(&cb, &params);
        let (lb, _) = label_events(&eb, &cb, &b, &lp);
        let twin = lb.iter().find(|l| l.event.bar == target.event.bar);
        let twin = twin.expect("the same event must exist");
        prop_assert_eq!(twin.class, target.class);
        prop_assert_eq!(twin.ambiguous, target.ambiguous);
        prop_assert_eq!(twin.bars_to_barrier, target.bars_to_barrier);
        prop_assert_eq!(twin.t_end_us, target.t_end_us);
        prop_assert_eq!(twin.mae.to_bits(), target.mae.to_bits());
        if !target.outcome_ticks.is_nan() && target.class != Some(Class::Chop) {
            prop_assert_eq!(twin.outcome_ticks.to_bits(), target.outcome_ticks.to_bits());
        }
    }

    /// Basic invariants on every label.
    #[test]
    fn label_invariants(
        steps in proptest::collection::vec(-60i32..60, 700),
        width in prop_oneof![Just(5u32), Just(15u32)],
        horizon in 2usize..10,
    ) {
        let cfg = data_cfg();
        let ev = events_cfg();
        let params = event_params(&cfg, &ev);
        let bars = session_bars(&steps);
        let c = build(&bars, &cfg, width, ev.atr_length);
        let (events, _) = detect(&c, &params);
        let lp = LabelParams { m1: 0.8, m2: 1.2, horizon };
        let (labels, _) = label_events(&events, &c, &bars, &lp);
        for l in &labels {
            prop_assert!(l.mae >= 0.0);
            prop_assert!(l.time_to_barrier_min as usize <= horizon * width as usize);
            prop_assert!(l.t_end_us > l.event.close_us);
            if l.class == Some(Class::Chop) {
                // neither barrier was reached in any scanned bar
                for j in 1..=horizon {
                    let u = l.event.bar + j;
                    let (hr, hb) = if l.event.side > 0 {
                        (c.high[u] >= l.b_rej, c.low[u] <= l.b_brk)
                    } else {
                        (c.low[u] <= l.b_rej, c.high[u] >= l.b_brk)
                    };
                    prop_assert!(!hr && !hb);
                }
            }
        }
    }
}
