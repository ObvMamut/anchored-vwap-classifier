//! One trade per signal, with the exit rules of `docs/BACKTEST.md`, section 3.
// Indices are bar positions checked against `bars.len()` before use.
#![allow(clippy::indexing_slicing)]

use serde::Serialize;
use vwap_core::{PositionPolicy, TICK_SIZE, TICK_VALUE_USD};
use vwap_data::CleanBars;
use vwap_events::CoarseBars;
use vwap_labels::{Hit, scan_barriers};

const MIN_US: i64 = 60_000_000;

/// A trade signal at the close of touch bar `bar`. The barrier prices are the label barriers,
/// fixed from the touch close and the volatility scale at the touch.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Signal {
    pub event_id: u32,
    pub session: i32,
    pub fold: usize,
    pub bar: usize,
    /// +1 when price came from above: the rejection trade is long.
    pub side: i8,
    /// Close of the touch bar.
    pub close: f64,
    pub b_rej: f64,
    pub b_brk: f64,
    /// Bars of the time exit after the touch bar.
    pub horizon: usize,
    pub p_rej: f64,
    pub tau: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ExitReason {
    Target,
    Stop,
    Time,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Trade {
    pub event_id: u32,
    pub session: i32,
    pub fold: usize,
    pub dir: i8,
    pub t_signal_us: i64,
    pub t_entry_us: i64,
    pub t_exit_us: i64,
    /// Frictionless entry price, the open of the bar after the touch.
    pub entry: f64,
    /// Frictionless exit price.
    pub exit: f64,
    pub reason: ExitReason,
    /// The exit filled at the open of a bar that had gapped beyond the barrier.
    pub gap: bool,
    /// Both barriers inside one 1 minute bar: booked as a stop.
    pub ambiguous: bool,
    /// Gross result in ticks before any cost.
    pub gross_ticks: f64,
    pub bars_held: usize,
    /// Maximum adverse excursion from the entry in ticks.
    pub mae_ticks: f64,
    pub p_rej: f64,
    pub tau: f64,
}

impl Trade {
    /// Net ticks with `slip` ticks of adverse slippage on each fill and `commission_usd` per
    /// side: $\text{gross} - 2 n_{slip} - 2c/v$.
    pub fn net_ticks(&self, slip: f64, commission_usd: f64) -> f64 {
        self.gross_ticks - vwap_core::round_trip_ticks(slip, commission_usd)
    }

    pub fn net_usd(&self, slip: f64, commission_usd: f64) -> f64 {
        self.net_ticks(slip, commission_usd) * TICK_VALUE_USD
    }
}

/// Simulates one signal. `None` when the bars after the touch do not cover the horizon.
pub fn simulate(sig: &Signal, bars: &CoarseBars, one: &CleanBars) -> Option<Trade> {
    let t = sig.bar;
    if sig.horizon == 0 || t + sig.horizon >= bars.len() {
        return None;
    }
    let w_us = i64::from(bars.width) * MIN_US;
    let dir = f64::from(sig.side);
    let entry = bars.open[t + 1];
    let scan = scan_barriers(
        bars,
        one,
        t,
        sig.horizon,
        sig.side,
        sig.close,
        sig.b_rej,
        sig.b_brk,
    );
    let u = t + scan.resolved_at;
    let (reason, exit, gap) = match scan.hit {
        Hit::Rejection => {
            // a limit order fills at the barrier or better
            let better = if sig.side > 0 {
                sig.b_rej.max(bars.open[u])
            } else {
                sig.b_rej.min(bars.open[u])
            };
            (ExitReason::Target, better, better != sig.b_rej)
        }
        Hit::Breakthrough | Hit::Ambiguous => {
            // a stop fills at the barrier or worse
            let worse = if sig.side > 0 {
                sig.b_brk.min(bars.open[u])
            } else {
                sig.b_brk.max(bars.open[u])
            };
            (ExitReason::Stop, worse, worse != sig.b_brk)
        }
        Hit::None => (ExitReason::Time, bars.close[t + sig.horizon], false),
    };
    let mut worst = 0.0f64;
    for j in (t + 1)..=u {
        let adverse = if sig.side > 0 {
            entry - bars.low[j]
        } else {
            bars.high[j] - entry
        };
        worst = worst.max(adverse);
    }
    Some(Trade {
        event_id: sig.event_id,
        session: sig.session,
        fold: sig.fold,
        dir: sig.side,
        t_signal_us: bars.open_us[t] + w_us,
        t_entry_us: bars.open_us[t + 1],
        t_exit_us: bars.open_us[u] + w_us,
        entry,
        exit,
        reason,
        gap,
        ambiguous: scan.hit == Hit::Ambiguous,
        gross_ticks: dir * (exit - entry) / TICK_SIZE,
        bars_held: u - t,
        mae_ticks: worst / TICK_SIZE,
        p_rej: sig.p_rej,
        tau: sig.tau,
    })
}

#[derive(Debug, Clone, Default)]
pub struct RunOutcome {
    pub trades: Vec<Trade>,
    /// Signals ignored because a trade was open (`one_at_a_time`).
    pub skipped_busy: usize,
    /// Signals ignored because the session already had a trade (`one_per_session`).
    pub skipped_session: usize,
    /// Signals whose bars do not cover the horizon.
    pub unavailable: usize,
}

/// Runs the signals of one timeframe in time order under the position policy.
pub fn run(
    signals: &[Signal],
    bars: &CoarseBars,
    one: &CleanBars,
    policy: PositionPolicy,
) -> RunOutcome {
    let w_us = i64::from(bars.width) * MIN_US;
    let mut order: Vec<&Signal> = signals.iter().collect();
    order.sort_by_key(|s| {
        (
            bars.open_us.get(s.bar).copied().unwrap_or(i64::MAX),
            s.event_id,
        )
    });
    let mut out = RunOutcome::default();
    let mut free_at = i64::MIN;
    let mut last_session: Option<i32> = None;
    for s in order {
        let t_signal = bars.open_us.get(s.bar).copied().unwrap_or(0) + w_us;
        match policy {
            PositionPolicy::OneAtATime if t_signal < free_at => {
                out.skipped_busy += 1;
                continue;
            }
            PositionPolicy::OnePerSession if last_session == Some(s.session) => {
                out.skipped_session += 1;
                continue;
            }
            _ => {}
        }
        match simulate(s, bars, one) {
            Some(tr) => {
                free_at = free_at.max(tr.t_exit_us);
                last_session = Some(s.session);
                out.trades.push(tr);
            }
            None => out.unavailable += 1,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use vwap_events::fixture::session_fixture;

    fn sig(side: i8, bar: usize) -> Signal {
        let s = f64::from(side);
        Signal {
            event_id: bar as u32,
            session: 1,
            fold: 0,
            bar,
            side,
            close: 100.0,
            b_rej: 100.0 + s * 2.0,
            b_brk: 100.0 - s * 2.0,
            horizon: 4,
            p_rej: 0.7,
            tau: 0.5,
        }
    }

    /// The touch bar followed by the listed bars, as 5 minute bars.
    fn path(after: &[(f64, f64, f64, f64)]) -> (CoarseBars, CleanBars) {
        let mut rows = vec![(100.0, 100.5, 99.5, 100.0)];
        rows.extend_from_slice(after);
        session_fixture(5, &rows)
    }

    const FLAT: (f64, f64, f64, f64) = (100.0, 100.5, 99.5, 100.0);

    #[test]
    fn a_long_trade_that_reaches_the_target() {
        let (b, o) = path(&[FLAT, (100.0, 102.5, 100.0, 102.0), FLAT, FLAT]);
        let t = simulate(&sig(1, 0), &b, &o).unwrap();
        assert_eq!(t.reason, ExitReason::Target);
        assert_eq!((t.entry, t.exit), (100.0, 102.0));
        assert_eq!(t.gross_ticks, 8.0);
        assert!(!t.gap && !t.ambiguous);
        assert_eq!(t.bars_held, 2);
        // 8 ticks less 2 for slippage and 0.8 for two commissions of 2 USD
        assert!((t.net_ticks(1.0, 2.0) - 5.2).abs() < 1e-12);
        assert!((t.net_usd(1.0, 2.0) - 26.0).abs() < 1e-12);
    }

    #[test]
    fn a_short_trade_mirrors_the_long_one() {
        let (b, o) = path(&[FLAT, (100.0, 100.0, 97.5, 98.0), FLAT, FLAT]);
        let t = simulate(&sig(-1, 0), &b, &o).unwrap();
        assert_eq!(t.reason, ExitReason::Target);
        assert_eq!(t.exit, 98.0);
        assert_eq!(t.gross_ticks, 8.0);
    }

    #[test]
    fn a_stop_exit_loses_the_barrier_distance() {
        let (b, o) = path(&[FLAT, (100.0, 100.5, 97.8, 98.5), FLAT, FLAT]);
        let t = simulate(&sig(1, 0), &b, &o).unwrap();
        assert_eq!(t.reason, ExitReason::Stop);
        assert_eq!(t.exit, 98.0);
        assert_eq!(t.gross_ticks, -8.0);
        assert!(t.mae_ticks >= 8.0 - 1e-12);
    }

    #[test]
    fn the_time_exit_uses_the_close_of_the_last_bar() {
        let (b, o) = path(&[FLAT, FLAT, FLAT, (100.0, 100.8, 99.8, 100.5)]);
        let t = simulate(&sig(1, 0), &b, &o).unwrap();
        assert_eq!(t.reason, ExitReason::Time);
        assert_eq!(t.exit, 100.5);
        assert_eq!(t.gross_ticks, 2.0);
        assert_eq!(t.bars_held, 4);
    }

    #[test]
    fn a_gap_through_the_stop_fills_at_the_open() {
        // the bar after the touch opens at 97, beyond the stop at 98
        let (b, o) = path(&[(97.0, 97.5, 96.0, 96.5), FLAT, FLAT, FLAT]);
        let t = simulate(&sig(1, 0), &b, &o).unwrap();
        assert_eq!(t.reason, ExitReason::Stop);
        assert!(t.gap);
        // entered and stopped at the same open: no gross result, only costs
        assert_eq!((t.entry, t.exit, t.gross_ticks), (97.0, 97.0, 0.0));
        assert!((t.net_ticks(1.0, 2.0) + 2.8).abs() < 1e-12);
    }

    #[test]
    fn a_later_gap_through_the_stop_fills_at_the_worse_open() {
        let (b, o) = path(&[FLAT, (96.0, 96.5, 95.0, 95.5), FLAT, FLAT]);
        let t = simulate(&sig(1, 0), &b, &o).unwrap();
        assert_eq!((t.reason, t.exit, t.gap), (ExitReason::Stop, 96.0, true));
        assert_eq!(t.gross_ticks, -16.0);
    }

    #[test]
    fn a_gap_through_the_target_fills_at_the_open_or_better() {
        let (b, o) = path(&[FLAT, (103.0, 103.5, 102.5, 103.0), FLAT, FLAT]);
        let t = simulate(&sig(1, 0), &b, &o).unwrap();
        assert_eq!((t.reason, t.exit, t.gap), (ExitReason::Target, 103.0, true));
        assert_eq!(t.gross_ticks, 12.0);
    }

    #[test]
    fn a_double_hit_bar_is_decided_by_the_first_minute() {
        // coarse bar 2 reaches both barriers; minute 0 of it reaches only the target
        let (b, mut o) = path(&[FLAT, (100.0, 103.0, 97.0, 100.0), FLAT, FLAT]);
        let first = 2 * 5;
        for k in first..first + 5 {
            o.high[k] = 100.5;
            o.low[k] = 99.5;
        }
        o.high[first] = 102.5;
        o.low[first + 3] = 97.5;
        let t = simulate(&sig(1, 0), &b, &o).unwrap();
        assert_eq!(t.reason, ExitReason::Target);
        assert!(!t.ambiguous);
        // and the other way round: the stop minute comes first
        let (b, mut o) = path(&[FLAT, (100.0, 103.0, 97.0, 100.0), FLAT, FLAT]);
        for k in first..first + 5 {
            o.high[k] = 100.5;
            o.low[k] = 99.5;
        }
        o.low[first] = 97.5;
        o.high[first + 3] = 102.5;
        let t = simulate(&sig(1, 0), &b, &o).unwrap();
        assert_eq!(t.reason, ExitReason::Stop);
        assert!(!t.ambiguous);
    }

    #[test]
    fn a_single_minute_touching_both_barriers_is_booked_as_a_stop() {
        let (b, o) = path(&[FLAT, (100.0, 103.0, 97.0, 100.0), FLAT, FLAT]);
        // the five identical minutes each touch both barriers
        let t = simulate(&sig(1, 0), &b, &o).unwrap();
        assert!(t.ambiguous);
        assert_eq!(t.reason, ExitReason::Stop);
        assert_eq!(t.gross_ticks, -8.0);
    }

    #[test]
    fn a_zero_move_round_trip_loses_exactly_the_costs() {
        let (b, o) = path(&[FLAT, FLAT, FLAT, FLAT]);
        let t = simulate(&sig(1, 0), &b, &o).unwrap();
        assert_eq!(t.gross_ticks, 0.0);
        for c in [0.0, 2.0, 4.5] {
            assert!((t.net_ticks(1.0, c) + (2.0 + 2.0 * c / 5.0)).abs() < 1e-12);
        }
    }

    #[test]
    fn the_entry_uses_only_the_next_open_and_nothing_after_the_exit() {
        let (b, o) = path(&[FLAT, (100.0, 102.5, 100.0, 102.0), FLAT, FLAT]);
        let base = simulate(&sig(1, 0), &b, &o).unwrap();
        // altering the touch close without changing the barriers leaves the trade as it was
        let mut moved = sig(1, 0);
        moved.close = 100.4;
        let m = simulate(&moved, &b, &o).unwrap();
        assert_eq!(
            (m.entry, m.exit, m.gross_ticks),
            (base.entry, base.exit, base.gross_ticks)
        );
        // altering the bars after the exit changes nothing
        let (mut b2, o2) = (b.clone(), o.clone());
        for j in 3..b2.len() {
            b2.open[j] = 90.0;
            b2.high[j] = 120.0;
            b2.low[j] = 80.0;
            b2.close[j] = 85.0;
        }
        let a = simulate(&sig(1, 0), &b2, &o2).unwrap();
        assert_eq!(
            (a.entry, a.exit, a.gross_ticks, a.t_exit_us),
            (base.entry, base.exit, base.gross_ticks, base.t_exit_us)
        );
    }

    #[test]
    fn signals_without_enough_bars_are_unavailable() {
        let (b, o) = path(&[FLAT, FLAT]);
        assert!(simulate(&sig(1, 0), &b, &o).is_none());
        let r = run(&[sig(1, 0)], &b, &o, PositionPolicy::OneAtATime);
        assert_eq!((r.trades.len(), r.unavailable), (0, 1));
    }

    fn long_path() -> (CoarseBars, CleanBars) {
        let rows = vec![FLAT; 14];
        session_fixture(5, &rows)
    }

    #[test]
    fn one_at_a_time_skips_signals_during_a_trade_and_never_overlaps() {
        let (b, o) = long_path();
        // each flat trade runs to its time exit 4 bars after the touch
        let sigs = [sig(1, 0), sig(1, 2), sig(1, 4), sig(1, 8)];
        let r = run(&sigs, &b, &o, PositionPolicy::OneAtATime);
        assert_eq!(r.trades.len(), 3);
        assert_eq!(r.skipped_busy, 1);
        for w in r.trades.windows(2) {
            assert!(w[0].t_exit_us <= w[1].t_signal_us);
        }
        // a signal at exactly the exit bar's close is taken
        assert!(r.trades.iter().any(|t| t.event_id == 4));
    }

    #[test]
    fn overlap_takes_everything_and_one_per_session_takes_the_first() {
        let (b, o) = long_path();
        let sigs = [sig(1, 0), sig(1, 2), sig(1, 4)];
        assert_eq!(run(&sigs, &b, &o, PositionPolicy::Overlap).trades.len(), 3);
        let r = run(&sigs, &b, &o, PositionPolicy::OnePerSession);
        assert_eq!((r.trades.len(), r.skipped_session), (1, 2));
        assert_eq!(r.trades[0].event_id, 0);
    }
}
