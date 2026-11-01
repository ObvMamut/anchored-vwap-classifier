//! The "trade every touch" benchmark: what a trader who takes every VWAP touch toward rejection,
//! with the label barriers as target and stop and the configured costs, would have seen on the
//! test windows of the walk forward. It is descriptive: no model, no threshold, no ledger entry
//! and no alpha. It reads the discovery view only and uses the same folds and the same backtest
//! as the strategies, so the numbers are comparable with theirs.
//!
//! For a target of $T$ ticks, a stop of $S$ ticks and a round trip cost of $\kappa$ ticks, a trade
//! that always resolves needs a win rate of
//! $$ p^{*} = \frac{S + \kappa}{T + S} $$
//! to break even (for symmetric barriers $\tfrac12 + \kappa / 2T$). Chop trades, which end at the
//! horizon, make it an approximation. A higher win rate bought with a smaller target raises
//! $p^{*}$ by the same amount, so the edge to look at is the win rate minus $p^{*}$.

use serde::Serialize;
use vwap_backtest::{Signal, Trade, expectancy_interval, run};
use vwap_core::{Class, PositionPolicy, TICK_SIZE};
use vwap_dataset::Sample;
use vwap_validation::{EventSpan, Mode, WalkForwardConfig, walk_forward};

use crate::error::ResearchError;
use crate::split::{Bounds, DiscoveryView};
use crate::strategy::exec::Env;

/// Break even win rate for a target and a stop of `target_ticks` and `stop_ticks` and a round
/// trip cost of `kappa_ticks`.
pub fn breakeven_win_rate(kappa_ticks: f64, target_ticks: f64, stop_ticks: f64) -> f64 {
    (stop_ticks + kappa_ticks) / (target_ticks + stop_ticks)
}

#[derive(Debug, Clone, Serialize)]
pub struct Row {
    /// Fold number, or `None` for the pooled row.
    pub fold: Option<usize>,
    pub touches: usize,
    pub trades: usize,
    /// Rejection share among the test touches that hit a barrier.
    pub rejection_share_resolved: f64,
    /// Share of trades with a gross result above zero.
    pub win_rate: f64,
    /// Share of trades with a net result above zero.
    pub win_rate_net: f64,
    pub avg_gross_ticks: f64,
    pub avg_net_ticks: f64,
    pub profit_factor_net: f64,
    /// Mean target and stop distance in ticks.
    pub mean_target_ticks: f64,
    pub mean_stop_ticks: f64,
    pub breakeven_win_rate: f64,
    /// `win_rate - breakeven_win_rate`.
    pub edge: f64,
    pub target_share: f64,
    pub stop_share: f64,
    pub time_share: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PolicyResult {
    pub policy: String,
    pub folds: Vec<Row>,
    pub pooled: Row,
    /// Session clustered bootstrap interval of the pooled net ticks per trade.
    pub net_interval: (f64, f64),
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkResult {
    pub width: u32,
    pub round_trip_ticks: f64,
    pub results: Vec<PolicyResult>,
}

fn mean(x: impl Iterator<Item = f64>) -> f64 {
    let (s, n) = x.fold((0.0, 0usize), |(s, n), v| (s + v, n + 1));
    if n == 0 { f64::NAN } else { s / n as f64 }
}

fn row(
    fold: Option<usize>,
    trades: &[&Trade],
    touches: &[&Sample],
    slip: f64,
    comm: f64,
    kappa: f64,
) -> Row {
    let rej = touches
        .iter()
        .filter(|s| s.class == Some(Class::Rejection))
        .count();
    let brk = touches
        .iter()
        .filter(|s| s.class == Some(Class::Breakthrough))
        .count();
    let n = trades.len();
    let share = |f: &dyn Fn(&Trade) -> bool| {
        if n == 0 {
            f64::NAN
        } else {
            trades.iter().filter(|t| f(t)).count() as f64 / n as f64
        }
    };
    let net: Vec<f64> = trades.iter().map(|t| t.net_ticks(slip, comm)).collect();
    let gain: f64 = net.iter().filter(|&&x| x > 0.0).sum();
    let loss: f64 = net.iter().filter(|&&x| x < 0.0).map(|x| -x).sum();
    Row {
        fold,
        touches: touches.len(),
        trades: n,
        rejection_share_resolved: if rej + brk == 0 {
            f64::NAN
        } else {
            rej as f64 / (rej + brk) as f64
        },
        win_rate: share(&|t| t.gross_ticks > 0.0),
        win_rate_net: share(&|t| t.net_ticks(slip, comm) > 0.0),
        avg_gross_ticks: mean(trades.iter().map(|t| t.gross_ticks)),
        avg_net_ticks: mean(net.iter().copied()),
        profit_factor_net: if loss > 0.0 { gain / loss } else { f64::NAN },
        mean_target_ticks: f64::NAN,
        mean_stop_ticks: f64::NAN,
        breakeven_win_rate: f64::NAN,
        edge: f64::NAN,
        target_share: share(&|t| t.reason == vwap_backtest::ExitReason::Target),
        stop_share: share(&|t| t.reason == vwap_backtest::ExitReason::Stop),
        time_share: share(&|t| t.reason == vwap_backtest::ExitReason::Time),
    }
    .with_sigma(trades, touches, kappa)
}

impl Row {
    /// Fills the barrier size and the break even win rate from the touches the trades came from.
    fn with_sigma(mut self, trades: &[&Trade], touches: &[&Sample], kappa: f64) -> Self {
        let ids: std::collections::BTreeSet<u32> = trades.iter().map(|t| t.event_id).collect();
        let of = |f: &dyn Fn(&Sample) -> f64| {
            mean(
                touches
                    .iter()
                    .filter(|s| ids.contains(&s.event_id))
                    .map(|s| f(s)),
            )
        };
        let target = of(&|s| (s.b_rej - s.close).abs() / TICK_SIZE);
        let stop = of(&|s| (s.b_brk - s.close).abs() / TICK_SIZE);
        self.mean_target_ticks = target;
        self.mean_stop_ticks = stop;
        self.breakeven_win_rate = breakeven_win_rate(kappa, target, stop);
        self.edge = self.win_rate - self.breakeven_win_rate;
        self
    }
}

/// The benchmark on every timeframe of `env.coarse` for the discovery view of `samples`.
pub fn run_benchmark(env: &Env, samples: &[Sample]) -> Result<Vec<BenchmarkResult>, ResearchError> {
    let view = DiscoveryView::select(samples, &Bounds::from_config(env.research));
    if view.samples().is_empty() {
        return Err(ResearchError::NotEnoughData(
            "the discovery view is empty".into(),
        ));
    }
    let range = (
        vwap_core::ResearchConfig::date_us(&env.research.discovery_start),
        vwap_core::ResearchConfig::date_us(&env.research.confirm_start),
    );
    let (slip, comm) = (env.backtest.slippage_ticks, env.backtest.commission_usd);
    let kappa = env.backtest.round_trip_ticks();
    let mut out = Vec::new();
    for (&width, bars) in env.coarse {
        let mut usable: Vec<&Sample> = view
            .samples()
            .iter()
            .filter(|s| {
                s.width == width && s.class.is_some() && !s.ambiguous && s.outcome_ticks.is_finite()
            })
            .collect();
        usable.sort_by_key(|s| (s.t_close_us, s.event_id));
        let spans: Vec<EventSpan> = usable
            .iter()
            .map(|s| EventSpan {
                t_close_us: s.t_close_us,
                t_end_us: s.t_end_us,
            })
            .collect();
        let splits = walk_forward(
            &spans,
            range,
            &WalkForwardConfig {
                train_months: env.validation.train_months,
                test_months: env.validation.test_months,
                step_months: env.validation.step_months,
                mode: match env.validation.mode {
                    vwap_core::WalkMode::Rolling => Mode::Rolling,
                    vwap_core::WalkMode::Expanding => Mode::Expanding,
                },
                embargo_us: 0,
            },
        );
        if splits.is_empty() {
            return Err(ResearchError::Strategy(
                "the range holds no complete test window".into(),
            ));
        }
        let horizon = env.labels.horizon(width);
        let mut signals = Vec::new();
        let mut by_fold: Vec<Vec<&Sample>> = Vec::new();
        for sp in &splits {
            let touches: Vec<&Sample> = sp
                .test
                .iter()
                .filter_map(|&i| usable.get(i).copied())
                .collect();
            for s in &touches {
                signals.push(Signal {
                    event_id: s.event_id,
                    session: s.session,
                    fold: sp.fold,
                    bar: s.bar,
                    side: s.side,
                    close: s.close,
                    b_rej: s.b_rej,
                    b_brk: s.b_brk,
                    horizon,
                    p_rej: 1.0,
                    tau: 0.0,
                });
            }
            by_fold.push(touches);
        }
        let mut results = Vec::new();
        for (name, policy) in [
            ("every touch", PositionPolicy::Overlap),
            ("one at a time", env.backtest.position_policy),
        ] {
            let outcome = run(&signals, bars, env.one, policy);
            let trades = outcome.trades;
            let mut folds = Vec::new();
            for (sp, touches) in splits.iter().zip(&by_fold) {
                let t: Vec<&Trade> = trades.iter().filter(|t| t.fold == sp.fold).collect();
                folds.push(row(Some(sp.fold), &t, touches, slip, comm, kappa));
            }
            let all_touches: Vec<&Sample> = by_fold.iter().flatten().copied().collect();
            let all: Vec<&Trade> = trades.iter().collect();
            let pooled = row(None, &all, &all_touches, slip, comm, kappa);
            let interval = expectancy_interval(
                &trades,
                slip,
                comm,
                env.validation.bootstrap_draws,
                env.validation.interval_level,
                env.strategy.seed,
            );
            results.push(PolicyResult {
                policy: name.to_string(),
                folds,
                pooled,
                net_interval: interval,
            });
        }
        out.push(BenchmarkResult {
            width,
            round_trip_ticks: kappa,
            results,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::testutil::market;
    use std::collections::BTreeMap;
    use std::path::Path;
    use vwap_core::ResearchConfig;

    const CONFIG: &str = "../../config";

    fn run_on(signal: bool, seed: u64) -> Vec<BenchmarkResult> {
        let cfg = Path::new(CONFIG);
        let research = vwap_core::load_research_config(cfg).unwrap();
        let strategy = vwap_core::load_strategy_config(cfg).unwrap();
        let mut validation = vwap_core::load_validation_config(cfg).unwrap();
        validation.bootstrap_draws = 200;
        let backtest = vwap_core::load_backtest_config(cfg).unwrap();
        let labels = vwap_core::load_labels_config(cfg).unwrap();
        let m = market(
            6000,
            signal,
            seed,
            ResearchConfig::date_us("2016-11-15"),
            ResearchConfig::date_us("2025-10-01"),
        );
        let mut coarse = BTreeMap::new();
        coarse.insert(15u32, m.bars.clone());
        let env = Env {
            research: &research,
            strategy: &strategy,
            validation: &validation,
            backtest: &backtest,
            labels: &labels,
            config_dir: cfg,
            coarse: &coarse,
            one: &m.one,
        };
        run_benchmark(&env, &m.samples).unwrap()
    }

    #[test]
    fn the_break_even_rate_matches_a_hand_value() {
        // barriers of 40 ticks and a round trip of 2.8 ticks: 0.5 + 2.8 / 80
        assert!((breakeven_win_rate(2.8, 40.0, 40.0) - 0.535).abs() < 1e-12);
        assert!((breakeven_win_rate(0.0, 40.0, 40.0) - 0.5).abs() < 1e-12);
        // a target of half the stop must win two trades in three just to break even
        assert!((breakeven_win_rate(0.0, 20.0, 40.0) - 2.0 / 3.0).abs() < 1e-12);
        assert!((breakeven_win_rate(2.0, 20.0, 40.0) - 42.0 / 60.0).abs() < 1e-12);
    }

    #[test]
    fn taking_every_touch_in_noise_wins_at_the_base_rate_and_loses_the_costs() {
        let r = run_on(false, 21);
        let b = &r[0];
        assert_eq!(b.width, 15);
        assert!((b.round_trip_ticks - 2.8).abs() < 1e-12);
        assert_eq!(b.results.len(), 2);
        assert_eq!(b.results[0].folds.len(), 5);
        let every = &b.results[0].pooled;
        // the market rejects with probability 0.45 among resolved touches
        assert!(
            (every.rejection_share_resolved - 0.45).abs() < 0.04,
            "{every:?}"
        );
        assert!(every.win_rate < every.breakeven_win_rate);
        assert!(every.avg_net_ticks < 0.0 && b.results[0].net_interval.1 < 0.0);
        // every touch is a trade, one at a time can only skip some
        assert_eq!(every.trades, every.touches);
        assert!(b.results[1].pooled.trades <= every.trades);
        // net is gross minus the 2.8 ticks of cost, trade by trade
        assert!((every.avg_gross_ticks - every.avg_net_ticks - 2.8).abs() < 1e-9);
        // exits add up
        let s = every.target_share + every.stop_share + every.time_share;
        assert!((s - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_signal_that_needs_a_feature_does_not_show_up_in_the_all_touch_win_rate() {
        // the planted signal only helps a model that reads the feature: averaged over all
        // touches the rejection share is one half
        let r = run_on(true, 22);
        let share = r[0].results[0].pooled.rejection_share_resolved;
        assert!((share - 0.5).abs() < 0.04, "{share}");
    }

    #[test]
    fn the_benchmark_is_deterministic() {
        let a = serde_json::to_string(&run_on(false, 23)).unwrap();
        let b = serde_json::to_string(&run_on(false, 23)).unwrap();
        assert_eq!(a, b);
    }
}
