//! Trade metrics, the daily series for the Sharpe ratio, the cost sweep and the break even
//! slippage. See `docs/BACKTEST.md`, sections 4 and 5.

use std::collections::BTreeMap;

use serde::Serialize;
use vwap_core::{BacktestConfig, TICK_VALUE_USD};
use vwap_events::CoarseBars;
use vwap_validation::{cluster_draws, interval, mean, sample_sd};

use crate::sim::{ExitReason, Trade};

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct ExitMix {
    pub target: usize,
    pub stop: usize,
    pub time: usize,
    pub gap: usize,
    pub ambiguous: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Metrics {
    pub n_trades: usize,
    pub expectancy_ticks: f64,
    pub total_usd: f64,
    pub hit_rate: f64,
    /// Gross profit over gross loss; infinite when nothing was lost.
    pub profit_factor: f64,
    /// Daily Sharpe ratio of the net USD series over `days`, not annualised (feeds the DSR).
    pub sharpe_daily: f64,
    pub sharpe_annual: f64,
    pub max_drawdown_usd: f64,
    pub avg_bars_held: f64,
    pub avg_mae_ticks: f64,
    pub exits: ExitMix,
    pub days: usize,
}

/// Sessions of the bars that open in `[start_us, end_us)`, in order: the days of an evaluation
/// window, including those without a trade.
pub fn sessions_in(bars: &CoarseBars, start_us: i64, end_us: i64) -> Vec<i32> {
    let mut out: Vec<i32> = Vec::new();
    for (t, s) in bars.open_us.iter().zip(&bars.session) {
        if *t >= start_us && *t < end_us && out.last() != Some(s) && !out.contains(s) {
            out.push(*s);
        }
    }
    out.sort_unstable();
    out
}

/// Net USD per day over `days` (zero on days without a trade); a trade belongs to its session.
pub fn daily_pnl(trades: &[Trade], days: &[i32], slip: f64, commission_usd: f64) -> Vec<f64> {
    let mut by_day: BTreeMap<i32, f64> = days.iter().map(|&d| (d, 0.0)).collect();
    for t in trades {
        if let Some(v) = by_day.get_mut(&t.session) {
            *v += t.net_usd(slip, commission_usd);
        }
    }
    by_day.into_values().collect()
}

fn max_drawdown(trades: &[Trade], slip: f64, commission_usd: f64) -> f64 {
    let mut order: Vec<&Trade> = trades.iter().collect();
    order.sort_by_key(|t| (t.t_exit_us, t.event_id));
    let (mut equity, mut peak, mut worst) = (0.0f64, 0.0f64, 0.0f64);
    for t in order {
        equity += t.net_usd(slip, commission_usd);
        peak = peak.max(equity);
        worst = worst.max(peak - equity);
    }
    worst
}

/// Metrics of `trades` at the given costs over the evaluation `days`.
pub fn metrics(
    trades: &[Trade],
    days: &[i32],
    slip: f64,
    commission_usd: f64,
    cfg: &BacktestConfig,
) -> Metrics {
    let n = trades.len();
    let net: Vec<f64> = trades
        .iter()
        .map(|t| t.net_ticks(slip, commission_usd))
        .collect();
    let gain: f64 = net.iter().filter(|&&x| x > 0.0).sum();
    let loss: f64 = net.iter().filter(|&&x| x < 0.0).map(|x| -x).sum();
    let daily = daily_pnl(trades, days, slip, commission_usd);
    let sharpe_daily = {
        let sd = sample_sd(&daily);
        if daily.len() < 2 || sd.is_nan() || sd <= 0.0 {
            0.0
        } else {
            mean(&daily) / sd
        }
    };
    let mut exits = ExitMix::default();
    for t in trades {
        match t.reason {
            ExitReason::Target => exits.target += 1,
            ExitReason::Stop => exits.stop += 1,
            ExitReason::Time => exits.time += 1,
        }
        exits.gap += usize::from(t.gap);
        exits.ambiguous += usize::from(t.ambiguous);
    }
    let nf = n.max(1) as f64;
    Metrics {
        n_trades: n,
        expectancy_ticks: if n == 0 {
            f64::NAN
        } else {
            net.iter().sum::<f64>() / nf
        },
        total_usd: net.iter().sum::<f64>() * TICK_VALUE_USD,
        hit_rate: if n == 0 {
            f64::NAN
        } else {
            net.iter().filter(|&&x| x > 0.0).count() as f64 / nf
        },
        profit_factor: if loss > 0.0 {
            gain / loss
        } else if gain > 0.0 {
            f64::INFINITY
        } else {
            f64::NAN
        },
        sharpe_daily,
        sharpe_annual: sharpe_daily * cfg.trading_days_per_year.sqrt(),
        max_drawdown_usd: max_drawdown(trades, slip, commission_usd),
        avg_bars_held: trades.iter().map(|t| t.bars_held as f64).sum::<f64>() / nf,
        avg_mae_ticks: trades.iter().map(|t| t.mae_ticks).sum::<f64>() / nf,
        exits,
        days: days.len(),
    }
}

/// Session clustered bootstrap interval of the mean net ticks.
pub fn expectancy_interval(
    trades: &[Trade],
    slip: f64,
    commission_usd: f64,
    draws: usize,
    level: f64,
    seed: u64,
) -> (f64, f64) {
    let net: Vec<f64> = trades
        .iter()
        .map(|t| t.net_ticks(slip, commission_usd))
        .collect();
    let sessions: Vec<i32> = trades.iter().map(|t| t.session).collect();
    let d = cluster_draws(&sessions, draws, seed, |rows| {
        if rows.is_empty() {
            f64::NAN
        } else {
            rows.iter()
                .map(|&i| net.get(i).copied().unwrap_or(0.0))
                .sum::<f64>()
                / rows.len() as f64
        }
    });
    interval(&d, level)
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SweepCell {
    pub slippage: f64,
    pub commission_usd: f64,
    pub expectancy_ticks: f64,
}

/// Net expectancy over the grid of slippage and commission levels of the configuration. The
/// trades (and so the threshold) are the same in every cell.
pub fn sweep(trades: &[Trade], cfg: &BacktestConfig) -> Vec<SweepCell> {
    let n = trades.len().max(1) as f64;
    let gross: f64 = trades.iter().map(|t| t.gross_ticks).sum();
    let mut out = Vec::new();
    for &s in &cfg.sweep_slippage {
        for &c in &cfg.sweep_commission {
            out.push(SweepCell {
                slippage: s,
                commission_usd: c,
                expectancy_ticks: if trades.is_empty() {
                    f64::NAN
                } else {
                    gross / n - vwap_core::round_trip_ticks(s, c)
                },
            });
        }
    }
    out
}

/// The slippage per side, in ticks, at which pooled net expectancy is exactly zero:
/// $(\bar g - 2c/v)/2$. Negative when even free fills lose money after commission.
pub fn break_even_slippage(trades: &[Trade], commission_usd: f64) -> f64 {
    if trades.is_empty() {
        return f64::NAN;
    }
    let g = trades.iter().map(|t| t.gross_ticks).sum::<f64>() / trades.len() as f64;
    (g - 2.0 * commission_usd / TICK_VALUE_USD) / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::ExitReason;

    fn cfg() -> BacktestConfig {
        vwap_core::load_backtest_config(std::path::Path::new("../../config")).unwrap()
    }

    fn trade(session: i32, exit_us: i64, gross: f64, reason: ExitReason) -> Trade {
        Trade {
            event_id: exit_us as u32,
            session,
            fold: 0,
            dir: 1,
            t_signal_us: exit_us - 10,
            t_entry_us: exit_us - 5,
            t_exit_us: exit_us,
            entry: 100.0,
            exit: 100.0,
            reason,
            gap: false,
            ambiguous: false,
            gross_ticks: gross,
            bars_held: 2,
            mae_ticks: 1.0,
            p_rej: 0.7,
            tau: 0.5,
        }
    }

    /// Net ticks 6, -3, 9, -6 at zero cost.
    fn set() -> Vec<Trade> {
        vec![
            trade(1, 10, 6.0, ExitReason::Target),
            trade(1, 20, -3.0, ExitReason::Stop),
            trade(2, 30, 9.0, ExitReason::Target),
            trade(3, 40, -6.0, ExitReason::Time),
        ]
    }

    #[test]
    fn expectancy_hit_rate_profit_factor_and_drawdown_match_hand_values() {
        let m = metrics(&set(), &[1, 2, 3, 4], 0.0, 0.0, &cfg());
        assert_eq!(m.n_trades, 4);
        assert!((m.expectancy_ticks - 1.5).abs() < 1e-12);
        assert!((m.hit_rate - 0.5).abs() < 1e-12);
        assert!((m.profit_factor - 15.0 / 9.0).abs() < 1e-12);
        assert!((m.total_usd - 30.0).abs() < 1e-12);
        // equity in USD: 30, 15, 60, 30; peak 60 then 30: drawdown 30 (from 60) and 15 earlier
        assert!((m.max_drawdown_usd - 30.0).abs() < 1e-12);
        assert_eq!(
            m.exits,
            ExitMix {
                target: 2,
                stop: 1,
                time: 1,
                gap: 0,
                ambiguous: 0
            }
        );
    }

    #[test]
    fn the_daily_series_has_zeros_on_days_without_trades() {
        let d = daily_pnl(&set(), &[1, 2, 3, 4, 5], 0.0, 0.0);
        // session 1: (6 - 3) ticks = 15 USD, session 2: 45, session 3: -30
        assert_eq!(d, vec![15.0, 45.0, -30.0, 0.0, 0.0]);
        let m = metrics(&set(), &[1, 2, 3, 4, 5], 0.0, 0.0, &cfg());
        let mean_d = 30.0 / 5.0;
        let var = d.iter().map(|x| (x - mean_d) * (x - mean_d)).sum::<f64>() / 4.0;
        assert!((m.sharpe_daily - mean_d / var.sqrt()).abs() < 1e-12);
        assert!((m.sharpe_annual - m.sharpe_daily * 252f64.sqrt()).abs() < 1e-12);
    }

    #[test]
    fn costs_enter_every_trade_once() {
        let m0 = metrics(&set(), &[1, 2, 3], 0.0, 0.0, &cfg());
        let m1 = metrics(&set(), &[1, 2, 3], 1.0, 2.0, &cfg());
        assert!((m0.expectancy_ticks - m1.expectancy_ticks - 2.8).abs() < 1e-12);
    }

    #[test]
    fn the_sweep_and_the_break_even_slippage_agree() {
        let c = cfg();
        let cells = sweep(&set(), &c);
        assert_eq!(
            cells.len(),
            c.sweep_slippage.len() * c.sweep_commission.len()
        );
        let cell = cells
            .iter()
            .find(|x| x.slippage == 1.0 && x.commission_usd == 2.0)
            .unwrap();
        assert!((cell.expectancy_ticks - (1.5 - 2.8)).abs() < 1e-12);
        // gross mean 1.5 ticks, free commission: break even at 0.75 ticks per side
        assert!((break_even_slippage(&set(), 0.0) - 0.75).abs() < 1e-12);
        let be = break_even_slippage(&set(), 2.0);
        let at = sweep(
            &set(),
            &BacktestConfig {
                sweep_slippage: vec![be],
                sweep_commission: vec![2.0],
                ..c
            },
        );
        assert!(at[0].expectancy_ticks.abs() < 1e-12);
    }

    #[test]
    fn the_bootstrap_interval_brackets_the_mean_and_is_reproducible() {
        let trades: Vec<Trade> = (0..120)
            .map(|i| {
                trade(
                    i / 2,
                    i64::from(i) * 10,
                    if i % 3 == 0 { 8.0 } else { -2.0 },
                    ExitReason::Target,
                )
            })
            .collect();
        let a = expectancy_interval(&trades, 0.0, 0.0, 500, 0.95, 3);
        let b = expectancy_interval(&trades, 0.0, 0.0, 500, 0.95, 3);
        assert_eq!(a, b);
        let m = metrics(&trades, &[], 0.0, 0.0, &cfg()).expectancy_ticks;
        assert!(a.0 < m && m < a.1);
    }

    #[test]
    fn empty_input_gives_nan_not_a_crash() {
        let m = metrics(&[], &[1, 2], 1.0, 2.0, &cfg());
        assert_eq!(m.n_trades, 0);
        assert!(m.expectancy_ticks.is_nan() && m.profit_factor.is_nan());
        assert_eq!(m.sharpe_daily, 0.0);
        assert!(break_even_slippage(&[], 2.0).is_nan());
    }

    #[test]
    fn sessions_in_lists_the_days_of_a_window() {
        let (mut bars, _) = vwap_events::fixture::session_fixture(5, &[(1.0, 1.0, 1.0, 1.0); 6]);
        bars.session = vec![5, 5, 6, 6, 7, 7];
        let w = 5 * 60_000_000;
        assert_eq!(sessions_in(&bars, 0, 6 * w), vec![5, 6, 7]);
        assert_eq!(sessions_in(&bars, 2 * w, 4 * w), vec![6]);
    }
}
