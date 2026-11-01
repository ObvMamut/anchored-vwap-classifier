//! Lookahead, symmetry and invariant properties of the triple barrier labels.
#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)]

use proptest::prelude::*;
use vwap_core::{Class, DataConfig};
use vwap_events::{build, detect, event_params};
use vwap_labels::{LabelParams, label_events};

fn data_cfg() -> DataConfig {
    vwap_testkit::data_config()
}

fn events_cfg() -> vwap_core::EventsConfig {
    vwap_testkit::events_config(0.5, 30)
}

fn session_bars(steps: &[i32]) -> vwap_data::CleanBars {
    vwap_testkit::session_bars(steps, &vec![9u8; steps.len()])
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
