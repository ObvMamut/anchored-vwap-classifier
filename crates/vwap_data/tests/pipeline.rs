//! Integration and property tests for the cleaning pipeline on synthetic bars.
#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)]

use proptest::prelude::*;
use vwap_core::DataConfig;
use vwap_data::{RawBars, clean::clean, flag, quality};

fn cfg(kind: &str) -> DataConfig {
    let text = format!(
        r#"
input_path = "x"
output_dir = "x"
artifact_dir = "x"
timestamp_kind = "{kind}"
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
"#
    );
    toml::from_str(&text).unwrap()
}

const T0: i64 = 1_700_000_000_000_000; // an arbitrary Tuesday in UTC microseconds
const MIN: i64 = 60_000_000;

fn raw_from(ts: &[i64], price: impl Fn(usize) -> f64) -> RawBars {
    let n = ts.len();
    let p: Vec<f64> = (0..n).map(&price).collect();
    RawBars {
        ts_us: ts.to_vec(),
        open: p.clone(),
        high: p.iter().map(|x| x + 0.5).collect(),
        low: p.iter().map(|x| x - 0.5).collect(),
        close: p.clone(),
        volume: vec![0.0; n],
        tick_volume: (0..n).map(|i| 10.0 + (i % 7) as f64).collect(),
        source_dtype: "test".into(),
    }
}

#[test]
fn output_is_ascending_and_prices_are_untouched() {
    // Descending input, like the real file.
    let ts: Vec<i64> = (0..300).rev().map(|i| T0 + i * MIN).collect();
    let raw = raw_from(&ts, |i| 100.0 + (i as f64) * 0.1);
    let (out, counts) = clean(&raw, &cfg("utc")).unwrap();
    assert_eq!(counts.rows_out, 300);
    assert!(out.ts_us.windows(2).all(|w| w[0] < w[1]));
    for (k, t) in out.ts_us.iter().enumerate() {
        let i = raw.ts_us.iter().position(|x| x == t).unwrap();
        assert_eq!(out.close[k].to_bits(), raw.close[i].to_bits());
        assert_eq!(out.open[k].to_bits(), raw.open[i].to_bits());
        assert_eq!(out.high[k].to_bits(), raw.high[i].to_bits());
        assert_eq!(out.low[k].to_bits(), raw.low[i].to_bits());
    }
}

#[test]
fn identical_duplicates_are_dropped_and_conflicts_are_errors() {
    let ts = vec![T0, T0 + MIN, T0 + MIN, T0 + 2 * MIN];
    let mut raw = raw_from(&ts, |_| 100.0);
    raw.tick_volume = vec![10.0; 4];
    let (out, counts) = clean(&raw, &cfg("utc")).unwrap();
    assert_eq!(out.len(), 3);
    assert_eq!(counts.identical_duplicates_dropped, 1);

    let mut conflicting = raw.clone();
    conflicting.close[2] = 100.3;
    conflicting.high[2] = 100.8;
    assert!(clean(&conflicting, &cfg("utc")).is_err());
}

#[test]
fn invalid_bars_are_dropped_and_counted() {
    let ts: Vec<i64> = (0..5).map(|i| T0 + i * MIN).collect();
    let mut raw = raw_from(&ts, |_| 100.0);
    raw.high[2] = 99.0; // high below low
    let (out, counts) = clean(&raw, &cfg("utc")).unwrap();
    assert_eq!(counts.invalid_ohlc_dropped, 1);
    assert_eq!(out.len(), 4);
}

#[test]
fn cleaning_is_idempotent() {
    let ts: Vec<i64> = (0..200).rev().map(|i| T0 + i * MIN).collect();
    let raw = raw_from(&ts, |i| 100.0 + (i % 13) as f64 * 0.1);
    let (a, _) = clean(&raw, &cfg("utc")).unwrap();
    let again = RawBars {
        ts_us: a.ts_us.clone(),
        open: a.open.clone(),
        high: a.high.clone(),
        low: a.low.clone(),
        close: a.close.clone(),
        volume: a.volume.clone(),
        tick_volume: a.tick_volume.clone(),
        source_dtype: "test".into(),
    };
    let (b, _) = clean(&again, &cfg("utc")).unwrap();
    assert_eq!(a.ts_us, b.ts_us);
    assert_eq!(a.close, b.close);
    assert_eq!(a.flags, b.flags);
    assert_eq!(a.globex_session, b.globex_session);
}

#[test]
fn server_labels_are_shifted_to_new_york_time() {
    // Label 2024-01-16 01:00 is New York 2024-01-15 18:00, the Globex open.
    let label = chrono::NaiveDate::from_ymd_opt(2024, 1, 16)
        .and_then(|d| d.and_hms_opt(1, 0, 0))
        .unwrap()
        .and_utc()
        .timestamp_micros();
    let raw = raw_from(&[label], |_| 100.0);
    let (out, _) = clean(&raw, &cfg("server_ny_synced")).unwrap();
    assert_eq!(out.ny_minute[0], 18 * 60);
    // The 18:00 bar opens the session whose anchor date is that same day.
    assert_eq!(out.ny_date[0], out.globex_session[0]);
}

#[test]
fn unexpected_gap_is_counted_and_flagged() {
    use chrono::NaiveDate;
    use vwap_data::time::utc_of_local;
    let d0 = NaiveDate::from_ymd_opt(2024, 1, 16).unwrap();
    let d1 = d0.succ_opt().unwrap();
    let mut ts: Vec<i64> = Vec::new();
    // Session 1: 18:00 to 23:59 on d0, then 00:00 to 16:59 on d1 with a 10 minute hole at 03:00.
    for m in 1080..1440 {
        ts.push(utc_of_local(d0, m).unwrap());
    }
    for m in 0..1020 {
        if !(180..190).contains(&m) {
            ts.push(utc_of_local(d1, m).unwrap());
        }
    }
    // Maintenance break, then the next session opens at 18:00.
    for m in 1080..1110 {
        ts.push(utc_of_local(d1, m).unwrap());
    }
    let raw = raw_from(&ts, |_| 100.0);
    let cfg = cfg("utc");
    let (mut bars, _) = clean(&raw, &cfg).unwrap();
    let rep = quality::analyze(&mut bars, &cfg).unwrap();
    assert_eq!(rep.gaps.unexpected_long, 1);
    assert_eq!(rep.gaps.scheduled_maintenance, 1);
    assert_eq!(rep.label_check.detected, "open");
    assert_eq!(
        bars.flags
            .iter()
            .filter(|f| **f & flag::GAP_BEFORE != 0)
            .count(),
        1
    );
    // Both Globex sessions are found and the second is thin.
    assert_eq!(rep.sessions.sessions, 2);
    assert_eq!(rep.sessions.thin_sessions, 1);
}

#[test]
fn contradicting_bar_label_is_an_error() {
    use chrono::NaiveDate;
    use vwap_data::time::utc_of_local;
    let d0 = NaiveDate::from_ymd_opt(2024, 1, 16).unwrap();
    let d1 = d0.succ_opt().unwrap();
    // Close labelled series: last bar before the break is stamped 17:00 and the first bar 18:01.
    let mut ts: Vec<i64> = (1081..1440).map(|m| utc_of_local(d0, m).unwrap()).collect();
    ts.extend((0..=1020).map(|m| utc_of_local(d1, m).unwrap()));
    ts.extend((1081..1100).map(|m| utc_of_local(d1, m).unwrap()));
    let raw = raw_from(&ts, |_| 100.0);
    let cfg = cfg("utc");
    let (mut bars, _) = clean(&raw, &cfg).unwrap();
    assert!(quality::analyze(&mut bars, &cfg).is_err());
}

proptest! {
    #[test]
    fn shuffled_input_with_duplicates_cleans_to_sorted_unique(
        n in 5usize..80,
        dup_every in 2usize..6,
        seed in 0u64..1000,
    ) {
        let mut ts: Vec<i64> = (0..n as i64).map(|i| T0 + i * MIN).collect();
        let dups: Vec<i64> = ts.iter().step_by(dup_every).copied().collect();
        ts.extend(dups);
        // deterministic shuffle by a multiplicative hash
        ts.sort_by_key(|t| (t.wrapping_mul(2654435761).wrapping_add(seed as i64)) % 9973);
        let price = |i: usize| 100.0 + (i % 5) as f64 * 0.1;
        // price must depend only on the timestamp so duplicates are identical
        let by_ts: Vec<f64> = ts.iter().map(|t| price(((t - T0) / MIN) as usize)).collect();
        let m = ts.len();
        let raw = RawBars {
            ts_us: ts.clone(),
            open: by_ts.clone(),
            high: by_ts.iter().map(|x| x + 0.5).collect(),
            low: by_ts.iter().map(|x| x - 0.5).collect(),
            close: by_ts.clone(),
            volume: vec![0.0; m],
            tick_volume: vec![5.0; m],
            source_dtype: "test".into(),
        };
        let (out, counts) = clean(&raw, &cfg("utc")).unwrap();
        prop_assert_eq!(out.len(), n);
        prop_assert!(out.ts_us.windows(2).all(|w| w[0] < w[1]));
        prop_assert_eq!(counts.identical_duplicates_dropped, m - n);
        for (k, t) in out.ts_us.iter().enumerate() {
            let expected = price(((t - T0) / MIN) as usize);
            prop_assert_eq!(out.close[k].to_bits(), expected.to_bits());
        }
    }
}
