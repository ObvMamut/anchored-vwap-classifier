//! Triple barrier labeling.
//!
//! The barriers are fixed from the close of the touch bar and the ATR at the touch,
//! then scanned over the next `horizon` coarse bars. Same bar double hits are
//! resolved by replaying the 1 minute bars inside that bar.
#![allow(clippy::indexing_slicing)]

use serde::Serialize;
use vwap_core::{Class, TICK_SIZE};
use vwap_data::CleanBars;
use vwap_data::clean::flag;
use vwap_events::{CoarseBars, Event};

const MIN_US: i64 = 60_000_000;
const BLOCKING_FLAGS: u32 = flag::GAP_BEFORE | flag::ROLL_BUFFER | flag::THIN_SESSION;

#[derive(Debug, Clone, Copy)]
pub struct LabelParams {
    pub m1: f64,
    pub m2: f64,
    pub horizon: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Labeled {
    pub event: Event,
    /// `None` only when the event is ambiguous.
    pub class: Option<Class>,
    pub ambiguous: bool,
    pub sigma: f64,
    pub b_rej: f64,
    pub b_brk: f64,
    /// Number of bars after the touch bar at which the label resolved.
    pub bars_to_barrier: u32,
    pub time_to_barrier_min: u32,
    /// Close time of the bar at which the label resolved.
    pub t_end_us: i64,
    /// Maximum adverse excursion in units of sigma.
    pub mae: f64,
    /// Return at the horizon in ticks, signed in the rejection direction.
    pub r_t_ticks: f64,
    /// Outcome in ticks under the barrier rule, before costs.
    pub outcome_ticks: f64,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct LabelDrops {
    pub input_events: usize,
    pub labeled: usize,
    pub horizon_does_not_fit: usize,
    pub missing_or_flagged_bars: usize,
    pub ambiguous: usize,
}

/// Index range of the 1 minute bars in `[start_us, end_us)`.
fn minute_range(one: &CleanBars, start_us: i64, end_us: i64) -> std::ops::Range<usize> {
    let a = one.ts_us.partition_point(|&t| t < start_us);
    let b = one.ts_us.partition_point(|&t| t < end_us);
    a..b
}

fn hits(side: i8, high: f64, low: f64, b_rej: f64, b_brk: f64) -> (bool, bool) {
    if side > 0 {
        (high >= b_rej, low <= b_brk)
    } else {
        (low <= b_rej, high >= b_brk)
    }
}

/// Which barrier a scan hit first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Rejection,
    Breakthrough,
    /// Both barriers inside one 1 minute bar, or no 1 minute data to decide: unresolved.
    Ambiguous,
    /// Neither barrier within the horizon.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scan {
    pub hit: Hit,
    /// Bars after the touch bar at which the scan stopped (the horizon when nothing was hit).
    pub resolved_at: usize,
    /// Maximum adverse excursion from `p0` up to the stop, in price units.
    pub adverse: f64,
}

/// Scans the coarse bars `t + 1 ..= t + horizon` for the first barrier. A bar that touches both
/// barriers is replayed on the 1 minute bars and the first minute that touches a barrier
/// decides; a minute that touches both leaves the result ambiguous. `side` is +1 when price came
/// from above. The label stage and the backtest both use this one implementation.
#[allow(clippy::too_many_arguments)]
pub fn scan_barriers(
    bars: &CoarseBars,
    one: &CleanBars,
    t: usize,
    horizon: usize,
    side: i8,
    p0: f64,
    b_rej: f64,
    b_brk: f64,
) -> Scan {
    let s = f64::from(side);
    let w_us = i64::from(bars.width) * MIN_US;
    let mut hit = Hit::None;
    let mut resolved_at = horizon;
    let mut adverse = 0.0f64;
    for j in 1..=horizon {
        let u = t + j;
        let x = if side > 0 { bars.low[u] } else { bars.high[u] };
        adverse = adverse.max(s * (p0 - x));
        let (hr, hb) = hits(side, bars.high[u], bars.low[u], b_rej, b_brk);
        if !hr && !hb {
            continue;
        }
        resolved_at = j;
        if hr && !hb {
            hit = Hit::Rejection;
        } else if hb && !hr {
            hit = Hit::Breakthrough;
        } else {
            let r = minute_range(one, bars.open_us[u], bars.open_us[u] + w_us);
            hit = Hit::Ambiguous;
            for k in r {
                let (mr, mb) = hits(side, one.high[k], one.low[k], b_rej, b_brk);
                if mr && mb {
                    break;
                }
                if mr || mb {
                    hit = if mr {
                        Hit::Rejection
                    } else {
                        Hit::Breakthrough
                    };
                    break;
                }
            }
        }
        break;
    }
    Scan {
        hit,
        resolved_at,
        adverse,
    }
}

pub fn label_events(
    events: &[Event],
    bars: &CoarseBars,
    one: &CleanBars,
    p: &LabelParams,
) -> (Vec<Labeled>, LabelDrops) {
    let mut out = Vec::with_capacity(events.len());
    let mut drops = LabelDrops {
        input_events: events.len(),
        ..LabelDrops::default()
    };
    let w_us = i64::from(bars.width) * MIN_US;
    for e in events {
        let t = e.bar;
        let t_horizon = p.horizon as i64 * i64::from(bars.width);
        if i64::from(e.minutes_to_flat) < t_horizon {
            drops.horizon_does_not_fit += 1;
            continue;
        }
        if t + p.horizon >= bars.len() {
            drops.missing_or_flagged_bars += 1;
            continue;
        }
        let contiguous = (1..=p.horizon).all(|j| {
            let u = t + j;
            bars.session[u] == e.session
                && bars.open_us[u] == bars.open_us[t] + j as i64 * w_us
                && !bars.partial(u)
                && bars.flags[u] & BLOCKING_FLAGS == 0
        });
        if !contiguous {
            drops.missing_or_flagged_bars += 1;
            continue;
        }
        let s = f64::from(e.side);
        let p0 = e.close;
        let sigma = e.atr;
        let b_rej = p0 + s * p.m1 * sigma;
        let b_brk = p0 - s * p.m2 * sigma;
        let scan = scan_barriers(bars, one, t, p.horizon, e.side, p0, b_rej, b_brk);
        let ambiguous = scan.hit == Hit::Ambiguous;
        let resolved_at = scan.resolved_at;
        let adverse = scan.adverse;
        let class = match scan.hit {
            Hit::Rejection => Some(Class::Rejection),
            Hit::Breakthrough => Some(Class::Breakthrough),
            Hit::Ambiguous => None,
            Hit::None => Some(Class::Chop),
        };
        if ambiguous {
            drops.ambiguous += 1;
        }
        let end = t + resolved_at;
        let r_t = s * (bars.close[t + p.horizon] - p0) / TICK_SIZE;
        let outcome = match class {
            Some(Class::Rejection) => p.m1 * sigma / TICK_SIZE,
            Some(Class::Breakthrough) => -p.m2 * sigma / TICK_SIZE,
            Some(Class::Chop) => r_t,
            None => f64::NAN,
        };
        out.push(Labeled {
            event: e.clone(),
            class,
            ambiguous,
            sigma,
            b_rej,
            b_brk,
            bars_to_barrier: resolved_at as u32,
            time_to_barrier_min: resolved_at as u32 * bars.width,
            t_end_us: bars.open_us[end] + w_us,
            mae: adverse / sigma,
            r_t_ticks: r_t,
            outcome_ticks: outcome,
        });
        drops.labeled += 1;
    }
    (out, drops)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session of 5 minute bars, one 1 minute bar per minute, flat at 100.
    fn fixture(rows: &[(f64, f64, f64, f64)]) -> (CoarseBars, CleanBars) {
        let n = rows.len();
        let bars = CoarseBars {
            width: 5,
            open_us: (0..n as i64).map(|i| i * 300 * 1_000_000).collect(),
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
            minute: (0..n as u16).map(|i| 1080 + 5 * i).collect(),
            n_minutes: vec![5; n],
            flags: vec![0; n],
            atr: vec![2.0; n],
        };
        // Each coarse bar is five identical minutes unless a test overrides them.
        let mut one = CleanBars::default();
        for (i, r) in rows.iter().enumerate() {
            for m in 0..5 {
                one.ts_us.push((i as i64 * 5 + m) * 60_000_000);
                one.open.push(r.0);
                one.high.push(r.1);
                one.low.push(r.2);
                one.close.push(r.3);
            }
        }
        (bars, one)
    }

    fn event(side: i8, bar: usize) -> Event {
        Event {
            id: 0,
            width: 5,
            bar,
            session: 1,
            trading_date: 2,
            open_us: bar as i64 * 300_000_000,
            close_us: (bar as i64 + 1) * 300_000_000,
            side,
            vwap: 100.0,
            vwap_bar: 100.0,
            vwap_prev: 100.0,
            vwap_sd: 1.0,
            open: 100.0,
            high: 101.0,
            low: 99.0,
            close: 100.0,
            atr: 2.0,
            minutes_since_anchor: 60,
            minutes_to_flat: 600,
            ordinal: 1,
            raw_touches_before: 0,
            flags: 0,
        }
    }

    const P: LabelParams = LabelParams {
        m1: 1.0,
        m2: 1.0,
        horizon: 4,
    };

    #[test]
    fn rejection_breakthrough_and_chop() {
        // Event at bar 0, close 100, sigma 2. From above: rejection is up to 102, breakthrough down to 98.
        let (b, o) = fixture(&[
            (100.0, 101.0, 99.0, 100.0),
            (100.0, 100.5, 99.5, 100.0),
            (100.0, 102.5, 99.9, 102.0), // reaches 102
            (102.0, 102.0, 102.0, 102.0),
            (102.0, 102.0, 102.0, 102.0),
        ]);
        let (l, _) = label_events(&[event(1, 0)], &b, &o, &P);
        assert_eq!(l[0].class, Some(Class::Rejection));
        assert_eq!(l[0].bars_to_barrier, 2);
        assert_eq!(l[0].time_to_barrier_min, 10);
        assert!((l[0].outcome_ticks - 8.0).abs() < 1e-9);

        // Same path seen from below flips the roles: 102 is now the breakthrough.
        let (l, _) = label_events(&[event(-1, 0)], &b, &o, &P);
        assert_eq!(l[0].class, Some(Class::Breakthrough));
        assert!((l[0].outcome_ticks + 8.0).abs() < 1e-9);

        // Nothing reached within the horizon is CHOP, with the horizon return recorded.
        let (b2, o2) = fixture(&[
            (100.0, 101.0, 99.0, 100.0),
            (100.0, 100.5, 99.5, 100.4),
            (100.4, 100.9, 100.1, 100.6),
            (100.6, 100.8, 100.2, 100.2),
            (100.2, 100.9, 100.0, 100.5),
        ]);
        let (l, _) = label_events(&[event(1, 0)], &b2, &o2, &P);
        assert_eq!(l[0].class, Some(Class::Chop));
        assert_eq!(l[0].bars_to_barrier, 4);
        assert!((l[0].r_t_ticks - 2.0).abs() < 1e-9);
        assert!((l[0].mae - 0.25).abs() < 1e-9); // lowest low 99.5 against a 100 reference
    }

    #[test]
    fn same_bar_double_hit_is_resolved_with_one_minute_bars() {
        let (b, mut o) = fixture(&[
            (100.0, 101.0, 99.0, 100.0),
            (100.0, 102.5, 97.5, 100.0), // reaches both 102 and 98 inside one bar
            (100.0, 100.0, 100.0, 100.0),
            (100.0, 100.0, 100.0, 100.0),
            (100.0, 100.0, 100.0, 100.0),
        ]);
        // Bar 1 covers 1 minute bars 5..10. Make the third minute reach 102 first, the fourth 98.
        for k in 5..10 {
            o.high[k] = 100.0;
            o.low[k] = 100.0;
        }
        o.high[7] = 102.5;
        o.low[8] = 97.5;
        let (l, _) = label_events(&[event(1, 0)], &b, &o, &P);
        assert_eq!(l[0].class, Some(Class::Rejection));
        assert!(!l[0].ambiguous);
        // Both barriers inside a single minute: ambiguous.
        o.high[7] = 100.0;
        o.high[8] = 102.5;
        let (l, d) = label_events(&[event(1, 0)], &b, &o, &P);
        assert!(l[0].ambiguous);
        assert_eq!(l[0].class, None);
        assert_eq!(d.ambiguous, 1);
    }

    #[test]
    fn events_whose_horizon_cannot_fit_are_dropped() {
        let (b, o) = fixture(&[(100.0, 101.0, 99.0, 100.0); 8]);
        let mut e = event(1, 0);
        e.minutes_to_flat = 10; // fewer than 4 bars of 5 minutes
        let (l, d) = label_events(&[e], &b, &o, &P);
        assert!(l.is_empty());
        assert_eq!(d.horizon_does_not_fit, 1);
        // an event too close to the end of the data
        let (l, d) = label_events(&[event(1, 6)], &b, &o, &P);
        assert!(l.is_empty());
        assert_eq!(d.missing_or_flagged_bars, 1);
    }
}
