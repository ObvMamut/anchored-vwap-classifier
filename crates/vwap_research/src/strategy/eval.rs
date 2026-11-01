//! Walk forward evaluation of one strategy on one timeframe: purged folds, the nested scheme
//! of `vwap_model` inside each, the signals above the tuned threshold, and the cost aware
//! backtest of the pooled out of fold signals. Pure: it reads the sample pool it is given and
//! writes nothing. The caller decides which view the pool comes from.

// The columns indexed below (`y`, `sessions`, the per-event score vectors) are built together
// from the same event list, so their lengths agree by construction.
#![allow(clippy::indexing_slicing)]

use serde::Serialize;
use vwap_backtest::{
    Metrics, Signal, SweepCell, break_even_slippage, daily_pnl, expectancy_interval, metrics, run,
    sessions_in, sweep,
};
use vwap_core::{BacktestConfig, StrategyConfig, ValidationConfig, WalkMode, round_trip_ticks};
use vwap_data::CleanBars;
use vwap_dataset::Sample;
use vwap_events::CoarseBars;
use vwap_features::feature_names;
use vwap_model::{
    Family, Matrix, Params, Table, auc, brier, bss, child_seed, ece, log_loss, precision_at,
    reliability as reliability_bins, run_fold, stability,
};
use vwap_validation::{EventSpan, Mode, WalkForwardConfig, cluster_draws, interval, walk_forward};

use crate::error::ResearchError;

/// The configuration sections a strategy run reads.
pub struct Configs<'a> {
    pub strategy: &'a StrategyConfig,
    pub validation: &'a ValidationConfig,
    pub backtest: &'a BacktestConfig,
    /// Train, select and score on resolved touches (rejection against breakthrough) only.
    pub direction: bool,
}

/// Where the test windows lie and how long the label horizon is.
#[derive(Debug, Clone, Copy)]
pub struct Geometry {
    /// `[start, end)` of the range the walk forward covers; the first test window starts
    /// `train_months` after `start`.
    pub range: (i64, i64),
    pub horizon_bars: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct FoldReport {
    pub fold: usize,
    pub test_start_us: i64,
    pub test_end_us: i64,
    pub n_train: usize,
    pub n_purged: usize,
    pub n_test: usize,
    /// Why the fold produced no predictions, if it did not.
    pub abstained: Option<String>,
    pub selected: Option<String>,
    pub rounds: Option<i64>,
    pub candidates: usize,
    pub tau: Option<f64>,
    pub log_loss: Option<f64>,
    pub brier: Option<f64>,
    pub brier_base: Option<f64>,
    pub bss: Option<f64>,
    pub base_rejection_rate: Option<f64>,
    pub precision_at_tau: Option<f64>,
    pub n_signals: usize,
    pub n_trades: usize,
    /// Mean net ticks of the fold's trades at the configured costs.
    pub expectancy_net_ticks: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Pooled {
    pub n_test: usize,
    pub log_loss: f64,
    pub brier: f64,
    pub brier_base: f64,
    pub bss: f64,
    pub bss_interval: (f64, f64),
    /// Expected calibration error per class on the pooled out of fold probabilities.
    pub ece: [f64; 3],
    pub auc_rejection: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportanceRow {
    pub feature: String,
    /// Mean absolute contribution per class, averaged over the folds that have one.
    pub per_class: [f64; 3],
}

#[derive(Debug, Clone, Serialize)]
pub struct ReliabilityBin {
    pub n: usize,
    pub mean_pred: f64,
    pub freq: f64,
}

/// One event of the beeswarm sample: feature values and the contribution of each feature to the
/// rejection score, rounded to 6 decimals.
#[derive(Debug, Clone, Serialize)]
pub struct ShapRow {
    pub values: Vec<f64>,
    pub phi: Vec<f64>,
}

/// In direction mode the three probabilities are renormalised over the two resolved classes; the
/// chop probability is dropped.
fn renorm(r: [f64; 3], direction: bool) -> [f64; 3] {
    if !direction {
        return r;
    }
    let s = r[0] + r[1];
    if s > 0.0 {
        [r[0] / s, r[1] / s, 0.0]
    } else {
        [0.5, 0.5, 0.0]
    }
}

/// What the trades of a run won and what they needed to win, as in `benchmark.rs`.
#[derive(Debug, Clone, Serialize)]
pub struct TradeStats {
    pub trades: usize,
    /// Share of trades with a gross result above zero, and net of the configured costs.
    pub win_rate: f64,
    pub win_rate_net: f64,
    pub mean_target_ticks: f64,
    pub mean_stop_ticks: f64,
    /// $(S + \kappa) / (T + S)$ for the mean target $T$ and stop $S$ of these trades.
    pub breakeven_win_rate: f64,
    /// `win_rate - breakeven_win_rate`; positive means an edge before the bootstrap interval.
    pub edge: f64,
}

fn round6(x: f64) -> f64 {
    if x.is_finite() {
        (x * 1e6).round() / 1e6
    } else {
        x
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StrategyResult {
    pub card_id: String,
    pub width: u32,
    pub model: String,
    pub n_events: usize,
    pub folds: Vec<FoldReport>,
    pub pooled: Option<Pooled>,
    pub backtest: Metrics,
    pub trade_stats: TradeStats,
    pub expectancy_interval: (f64, f64),
    pub sweep: Vec<SweepCell>,
    pub break_even_slippage: f64,
    /// Net expectancy in ticks at 2 ticks of slippage per side and the configured commission.
    pub net_at_2_ticks: f64,
    pub skipped_busy: usize,
    /// Net USD per evaluated day at the configured costs; feeds the DSR and the PBO.
    pub daily_pnl: Vec<f64>,
    pub importance: Vec<ImportanceRow>,
    /// Per fold with a contribution: importance of each selected feature summed over classes.
    pub importance_by_fold: Vec<Vec<f64>>,
    pub importance_stability: Option<(f64, f64)>,
    /// Pooled reliability bins per class (rejection, breakthrough, chop).
    pub reliability: Vec<Vec<ReliabilityBin>>,
    /// Index into `daily_pnl` where each fold's test window begins.
    pub fold_first_day: Vec<usize>,
    /// Names of the selected features, in the column order of `shap_sample`.
    pub feature_names: Vec<String>,
    pub shap_sample: Vec<ShapRow>,
    /// Candidates scored by the inner folds over all folds, counted as trials of the family.
    pub candidates_scored: usize,
}

fn mode_of(m: WalkMode) -> Mode {
    match m {
        WalkMode::Rolling => Mode::Rolling,
        WalkMode::Expanding => Mode::Expanding,
    }
}

/// Evaluates `family` on the events of `width` in `pool`.
#[allow(clippy::too_many_arguments)]
pub fn evaluate(
    card_id: &str,
    family: Family,
    width: u32,
    feature_idx: &[usize],
    pool: &[Sample],
    bars: &CoarseBars,
    one: &CleanBars,
    geom: &Geometry,
    cfg: &Configs,
) -> Result<StrategyResult, ResearchError> {
    let err = |m: String| ResearchError::Strategy(m);
    let mut usable: Vec<&Sample> = pool
        .iter()
        .filter(|s| {
            s.width == width && s.class.is_some() && !s.ambiguous && s.outcome_ticks.is_finite()
        })
        .collect();
    usable.sort_by_key(|s| (s.t_close_us, s.event_id));
    let kappa = cfg.backtest.round_trip_ticks();
    let rows: Vec<Vec<f64>> = usable
        .iter()
        .map(|s| {
            feature_idx
                .iter()
                .map(|&j| s.features.get(j).copied().unwrap_or(f64::NAN))
                .collect()
        })
        .collect();
    let x = Matrix::from_rows(&rows);
    let y: Vec<usize> = usable
        .iter()
        .map(|s| s.class.map_or(0, |c| c.index()))
        .collect();
    let spans: Vec<EventSpan> = usable
        .iter()
        .map(|s| EventSpan {
            t_close_us: s.t_close_us,
            t_end_us: s.t_end_us,
        })
        .collect();
    let net: Vec<f64> = usable.iter().map(|s| s.outcome_ticks - kappa).collect();
    let sessions: Vec<i32> = usable.iter().map(|s| s.session).collect();

    let embargo_us = (cfg.validation.embargo_horizons
        * geom.horizon_bars as f64
        * f64::from(width)
        * 60_000_000.0) as i64;
    let splits = walk_forward(
        &spans,
        geom.range,
        &WalkForwardConfig {
            train_months: cfg.validation.train_months,
            test_months: cfg.validation.test_months,
            step_months: cfg.validation.step_months,
            mode: mode_of(cfg.validation.mode),
            embargo_us,
        },
    );
    if splits.is_empty() {
        return Err(err("the range holds no complete test window".into()));
    }

    let table = Table {
        x: &x,
        y: &y,
        spans: &spans,
        net: &net,
        sessions: &sessions,
    };
    let names = feature_names();
    let mut folds = Vec::new();
    let mut signals: Vec<Signal> = Vec::new();
    // pooled out of fold records
    let (mut p_probs, mut p_y, mut p_sess, mut p_base) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut fold_importance: Vec<Vec<[f64; 3]>> = Vec::new();
    let mut shap_sample: Vec<ShapRow> = Vec::new();
    let mut candidates_scored = 0usize;
    for sp in &splits {
        let mut rep = FoldReport {
            fold: sp.fold,
            test_start_us: sp.test_start_us,
            test_end_us: sp.test_end_us,
            n_train: sp.train.len(),
            n_purged: sp.purged.len(),
            n_test: sp.test.len(),
            abstained: None,
            selected: None,
            rounds: None,
            candidates: 0,
            tau: None,
            log_loss: None,
            brier: None,
            brier_base: None,
            bss: None,
            base_rejection_rate: None,
            precision_at_tau: None,
            n_signals: 0,
            n_trades: 0,
            expectancy_net_ticks: None,
        };
        let params = Params {
            cfg: cfg.strategy,
            validation: cfg.validation,
            embargo_us,
            seed: child_seed(cfg.strategy.seed, &format!("{card_id}/{width}/{}", sp.fold)),
            direction: cfg.direction,
        };
        if sp.test.is_empty() {
            rep.abstained = Some("no test events".into());
            folds.push(rep);
            continue;
        }
        match run_fold(family, &table, &sp.train, &sp.test, &params) {
            Err(e) => rep.abstained = Some(e.to_string()),
            Ok(ff) => {
                let y_test: Vec<usize> = sp.test.iter().map(|&i| y[i]).collect();
                // the rows the forecast is scored on: all of them, or the resolved ones
                let scored: Vec<usize> = (0..sp.test.len())
                    .filter(|&k| !cfg.direction || y_test[k] != 2)
                    .collect();
                let score_probs: Vec<[f64; 3]> = scored
                    .iter()
                    .map(|&k| renorm(ff.calibrated[k], cfg.direction))
                    .collect();
                let y_score: Vec<usize> = scored.iter().map(|&k| y_test[k]).collect();
                let base_row = renorm(ff.base_freq, cfg.direction);
                let base = vec![base_row; y_score.len()];
                let (bs, bb) = (brier(&score_probs, &y_score), brier(&base, &y_score));
                rep.selected = Some(ff.selected.clone());
                rep.rounds = ff.rounds;
                rep.candidates = ff.candidates;
                candidates_scored += ff.candidates;
                rep.tau = ff.tau.tau;
                rep.log_loss = Some(log_loss(&score_probs, &y_score));
                rep.brier = Some(bs);
                rep.brier_base = Some(bb);
                rep.bss = Some(bss(bs, bb));
                rep.base_rejection_rate =
                    Some(y_test.iter().filter(|&&c| c == 0).count() as f64 / y_test.len() as f64);
                if let Some(tau) = ff.tau.tau {
                    let (prec, n_sel) = precision_at(&ff.calibrated, &y_test, tau);
                    rep.precision_at_tau = prec.is_finite().then_some(prec);
                    rep.n_signals = n_sel;
                    for (pos, &ev) in sp.test.iter().enumerate() {
                        let p_rej = ff.calibrated.get(pos).map_or(0.0, |r| r[0]);
                        if p_rej > tau
                            && let Some(s) = usable.get(ev)
                        {
                            signals.push(Signal {
                                event_id: s.event_id,
                                session: s.session,
                                fold: sp.fold,
                                bar: s.bar,
                                side: s.side,
                                close: s.close,
                                b_rej: s.b_rej,
                                b_brk: s.b_brk,
                                horizon: geom.horizon_bars,
                                p_rej,
                                tau,
                            });
                        }
                    }
                }
                for &pos in &scored {
                    let ev = sp.test[pos];
                    p_probs.push(renorm(
                        ff.calibrated.get(pos).copied().unwrap_or([0.0; 3]),
                        cfg.direction,
                    ));
                    p_y.push(y[ev]);
                    p_sess.push(sessions[ev]);
                    p_base.push(base_row);
                }
                if let Some(imp) = ff.importance {
                    fold_importance.push(imp);
                }
                shap_sample.extend(ff.contrib_sample.into_iter().map(|r| ShapRow {
                    values: r.values.into_iter().map(round6).collect(),
                    phi: r.phi.into_iter().map(round6).collect(),
                }));
            }
        }
        folds.push(rep);
    }

    // pooled forecast quality
    let pooled = if p_y.is_empty() {
        None
    } else {
        let bs_i: Vec<f64> = p_probs
            .iter()
            .zip(&p_y)
            .map(|(r, &c)| {
                r.iter()
                    .enumerate()
                    .map(|(k, &q)| (q - f64::from(u8::from(k == c))).powi(2))
                    .sum()
            })
            .collect();
        let bb_i: Vec<f64> = p_base
            .iter()
            .zip(&p_y)
            .map(|(r, &c)| {
                r.iter()
                    .enumerate()
                    .map(|(k, &q)| (q - f64::from(u8::from(k == c))).powi(2))
                    .sum()
            })
            .collect();
        let ratio = |rows: &[usize]| {
            let (a, b): (f64, f64) = rows
                .iter()
                .fold((0.0, 0.0), |(a, b), &i| (a + bs_i[i], b + bb_i[i]));
            1.0 - a / b
        };
        let draws = cluster_draws(
            &p_sess,
            cfg.validation.bootstrap_draws,
            child_seed(cfg.strategy.seed, &format!("{card_id}/{width}/bss")),
            ratio,
        );
        let bins = cfg.strategy.ece_bins;
        let (pb, pbb) = (
            bs_i.iter().sum::<f64>() / bs_i.len() as f64,
            bb_i.iter().sum::<f64>() / bb_i.len() as f64,
        );
        let score: Vec<f64> = p_probs.iter().map(|r| r[0]).collect();
        let positive: Vec<bool> = p_y.iter().map(|&c| c == 0).collect();
        Some(Pooled {
            n_test: p_y.len(),
            log_loss: log_loss(&p_probs, &p_y),
            brier: pb,
            brier_base: pbb,
            bss: bss(pb, pbb),
            bss_interval: interval(&draws, cfg.validation.interval_level),
            ece: [
                ece(&p_probs, &p_y, 0, bins),
                ece(&p_probs, &p_y, 1, bins),
                ece(&p_probs, &p_y, 2, bins),
            ],
            auc_rejection: auc(&score, &positive),
        })
    };

    // the backtest of the pooled out of fold signals
    let outcome = run(&signals, bars, one, cfg.backtest.position_policy);
    let mut days: Vec<i32> = Vec::new();
    for sp in &splits {
        days.extend(sessions_in(bars, sp.test_start_us, sp.test_end_us));
    }
    days.sort_unstable();
    days.dedup();
    let fold_first_day: Vec<usize> = splits
        .iter()
        .map(|sp| {
            let first = sessions_in(bars, sp.test_start_us, sp.test_end_us)
                .first()
                .copied()
                .unwrap_or(i32::MAX);
            days.partition_point(|&d| d < first)
        })
        .collect();
    let reliability: Vec<Vec<ReliabilityBin>> = (0..3)
        .map(|k| {
            reliability_bins(&p_probs, &p_y, k, cfg.strategy.ece_bins)
                .into_iter()
                .map(|b| ReliabilityBin {
                    n: b.n,
                    mean_pred: b.mean_pred,
                    freq: b.freq,
                })
                .collect()
        })
        .collect();
    let (slip, comm) = (cfg.backtest.slippage_ticks, cfg.backtest.commission_usd);
    for rep in &mut folds {
        let net_f: Vec<f64> = outcome
            .trades
            .iter()
            .filter(|t| t.fold == rep.fold)
            .map(|t| t.net_ticks(slip, comm))
            .collect();
        rep.n_trades = net_f.len();
        rep.expectancy_net_ticks =
            (!net_f.is_empty()).then(|| net_f.iter().sum::<f64>() / net_f.len() as f64);
    }
    let bt = metrics(&outcome.trades, &days, slip, comm, cfg.backtest);
    let trade_stats = {
        let by_event: std::collections::BTreeMap<u32, (f64, f64)> = signals
            .iter()
            .map(|g| {
                (
                    g.event_id,
                    (
                        (g.b_rej - g.close).abs() / vwap_core::TICK_SIZE,
                        (g.b_brk - g.close).abs() / vwap_core::TICK_SIZE,
                    ),
                )
            })
            .collect();
        let n = outcome.trades.len();
        let frac = |f: &dyn Fn(&vwap_backtest::Trade) -> bool| {
            if n == 0 {
                f64::NAN
            } else {
                outcome.trades.iter().filter(|t| f(t)).count() as f64 / n as f64
            }
        };
        let avg = |pick: &dyn Fn(&(f64, f64)) -> f64| {
            if n == 0 {
                f64::NAN
            } else {
                outcome
                    .trades
                    .iter()
                    .filter_map(|t| by_event.get(&t.event_id))
                    .map(pick)
                    .sum::<f64>()
                    / n as f64
            }
        };
        let (target, stop) = (avg(&|x| x.0), avg(&|x| x.1));
        let win_rate = frac(&|t| t.gross_ticks > 0.0);
        let be = crate::strategy::benchmark::breakeven_win_rate(
            cfg.backtest.round_trip_ticks(),
            target,
            stop,
        );
        TradeStats {
            trades: n,
            win_rate,
            win_rate_net: frac(&|t| t.net_ticks(slip, comm) > 0.0),
            mean_target_ticks: target,
            mean_stop_ticks: stop,
            breakeven_win_rate: be,
            edge: win_rate - be,
        }
    };
    let ci = expectancy_interval(
        &outcome.trades,
        slip,
        comm,
        cfg.validation.bootstrap_draws,
        cfg.validation.interval_level,
        child_seed(cfg.strategy.seed, &format!("{card_id}/{width}/expectancy")),
    );
    let gross_mean = if outcome.trades.is_empty() {
        f64::NAN
    } else {
        outcome.trades.iter().map(|t| t.gross_ticks).sum::<f64>() / outcome.trades.len() as f64
    };

    // importance averaged over folds, and its stability
    let kept: Vec<&str> = feature_idx
        .iter()
        .filter_map(|&j| names.get(j).copied())
        .collect();
    let mut importance = Vec::new();
    let mut stab = None;
    let importance_by_fold: Vec<Vec<f64>> = fold_importance
        .iter()
        .map(|f| f.iter().map(|c| c.iter().sum()).collect())
        .collect();
    if !fold_importance.is_empty() {
        let nf = fold_importance.len() as f64;
        for (j, name) in kept.iter().enumerate() {
            let mut acc = [0.0; 3];
            for f in &fold_importance {
                if let Some(c) = f.get(j) {
                    for k in 0..3 {
                        acc[k] += c[k] / nf;
                    }
                }
            }
            importance.push(ImportanceRow {
                feature: (*name).to_string(),
                per_class: acc,
            });
        }
        if fold_importance.len() >= 2 {
            let per_fold: Vec<Vec<f64>> = fold_importance
                .iter()
                .map(|f| f.iter().map(|c| c.iter().sum()).collect())
                .collect();
            let s = stability(&per_fold, 5.min(kept.len()));
            stab = Some((s.mean_rank_correlation, s.mean_top_overlap));
        }
    }

    Ok(StrategyResult {
        card_id: card_id.to_string(),
        width,
        model: format!("{family:?}").to_lowercase(),
        n_events: usable.len(),
        folds,
        pooled,
        backtest: bt,
        trade_stats,
        expectancy_interval: ci,
        sweep: sweep(&outcome.trades, cfg.backtest),
        break_even_slippage: break_even_slippage(&outcome.trades, comm),
        net_at_2_ticks: gross_mean - round_trip_ticks(2.0, comm),
        skipped_busy: outcome.skipped_busy,
        daily_pnl: daily_pnl(&outcome.trades, &days, slip, comm),
        importance,
        importance_by_fold,
        importance_stability: stab,
        reliability,
        fold_first_day,
        feature_names: kept.iter().map(|k| (*k).to_string()).collect(),
        shap_sample,
        candidates_scored,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::testutil::{HORIZON, market};
    use vwap_core::ResearchConfig;

    struct Setup {
        strategy: StrategyConfig,
        validation: ValidationConfig,
        backtest: BacktestConfig,
        geom: Geometry,
    }

    fn setup() -> Setup {
        let dir = std::path::Path::new("../../config");
        let mut strategy = vwap_core::load_strategy_config(dir).unwrap();
        strategy.lgbm_grid.truncate(2);
        strategy.lgbm_max_rounds = 60;
        strategy.tau_bootstrap_draws = 100;
        let mut validation = vwap_core::load_validation_config(dir).unwrap();
        validation.bootstrap_draws = 200;
        let r = vwap_core::load_research_config(dir).unwrap();
        Setup {
            strategy,
            validation,
            backtest: vwap_core::load_backtest_config(dir).unwrap(),
            geom: Geometry {
                range: (
                    ResearchConfig::date_us(&r.discovery_start),
                    ResearchConfig::date_us(&r.confirm_start),
                ),
                horizon_bars: HORIZON,
            },
        }
    }

    fn run_family(fam: Family, signal: bool, seed: u64) -> StrategyResult {
        let s = setup();
        let m = market(3600, signal, seed, s.geom.range.0, s.geom.range.1);
        let idx: Vec<usize> = (0..feature_names().len()).collect();
        evaluate(
            "S900",
            fam,
            15,
            &idx,
            &m.samples,
            &m.bars,
            &m.one,
            &s.geom,
            &Configs {
                strategy: &s.strategy,
                validation: &s.validation,
                backtest: &s.backtest,
                direction: false,
            },
        )
        .unwrap()
    }

    fn run_direction(signal: bool, seed: u64) -> StrategyResult {
        let s = setup();
        let m = market(3600, signal, seed, s.geom.range.0, s.geom.range.1);
        let idx: Vec<usize> = (0..feature_names().len()).collect();
        evaluate(
            "S901",
            Family::Logistic,
            15,
            &idx,
            &m.samples,
            &m.bars,
            &m.one,
            &s.geom,
            &Configs {
                strategy: &s.strategy,
                validation: &s.validation,
                backtest: &s.backtest,
                direction: true,
            },
        )
        .unwrap()
    }

    #[test]
    fn the_direction_target_scores_the_resolved_touches_and_finds_the_same_signal() {
        let r = run_direction(true, 8);
        let p = r.pooled.as_ref().unwrap();
        // scored on resolved touches only: about 90 percent of the events of the market
        assert!(p.n_test < 3600 && p.n_test > 800);
        assert!(p.bss > 0.1 && p.bss_interval.0 > 0.0, "{p:?}");
        let t = &r.trade_stats;
        assert!(t.trades > 100);
        assert!(t.win_rate > t.breakeven_win_rate, "{t:?}");
        assert!((t.edge - (t.win_rate - t.breakeven_win_rate)).abs() < 1e-12);
        // noise has no edge: the win rate does not clear the break even rate
        let n = run_direction(false, 9);
        assert!(n.pooled.as_ref().unwrap().bss_interval.0 <= 0.0);
    }

    #[test]
    fn trade_stats_state_the_break_even_rate_of_the_barriers_that_were_traded() {
        let r = run_family(Family::Logistic, true, 1);
        let t = &r.trade_stats;
        // the synthetic market has symmetric barriers of 8 ticks
        assert!((t.mean_target_ticks - 8.0).abs() < 1e-9 && (t.mean_stop_ticks - 8.0).abs() < 1e-9);
        let be = 0.5 + 2.8 / 16.0;
        assert!((t.breakeven_win_rate - be).abs() < 1e-9, "{t:?}");
        assert_eq!(t.trades, r.backtest.n_trades);
    }

    #[test]
    fn a_planted_signal_gives_skill_and_a_positive_net_expectancy() {
        let r = run_family(Family::Logistic, true, 1);
        assert_eq!(r.folds.len(), 5);
        assert!(
            r.folds.iter().all(|f| f.abstained.is_none()),
            "{:?}",
            r.folds
        );
        let p = r.pooled.as_ref().unwrap();
        assert!(p.bss > 0.05 && p.bss_interval.0 > 0.0, "{p:?}");
        assert!(p.auc_rejection > 0.65);
        assert!(r.backtest.n_trades > 100);
        assert!(r.backtest.expectancy_ticks > 0.0, "{:?}", r.backtest);
        assert!(r.expectancy_interval.0 > 0.0, "{:?}", r.expectancy_interval);
        // the same trades earn less once slippage rises
        assert!(r.net_at_2_ticks < r.backtest.expectancy_ticks);
        assert!(r.break_even_slippage > 1.0);
        assert_eq!(r.daily_pnl.len(), r.backtest.days);
        let top = r
            .importance
            .iter()
            .max_by(|a, b| a.per_class[0].total_cmp(&b.per_class[0]))
            .unwrap();
        assert_eq!(top.feature, feature_names()[0]);

        // what the report needs is stored
        assert_eq!(r.reliability.len(), 3);
        assert!(
            r.reliability
                .iter()
                .all(|c| c.len() == setup().strategy.ece_bins)
        );
        assert_eq!(
            r.reliability[0].iter().map(|b| b.n).sum::<usize>(),
            p.n_test
        );
        assert_eq!(r.fold_first_day.len(), r.folds.len());
        assert!(r.fold_first_day.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(r.importance_by_fold.len(), r.folds.len());
        assert!(!r.shap_sample.is_empty());
        assert!(r.shap_sample.len() <= r.folds.len() * setup().strategy.contrib_rows_per_fold);
        assert!(
            r.shap_sample
                .iter()
                .all(|row| row.phi.len() == r.feature_names.len())
        );
    }

    #[test]
    fn noise_gives_no_skill_and_no_money() {
        let r = run_family(Family::Logistic, false, 2);
        let p = r.pooled.as_ref().unwrap();
        assert!(p.bss_interval.0 <= 0.0, "{p:?}");
        assert!(r.expectancy_interval.0 <= 0.0 || r.backtest.n_trades == 0);
    }

    #[test]
    fn the_majority_model_has_no_skill_even_on_a_signal() {
        let r = run_family(Family::Majority, true, 3);
        let p = r.pooled.as_ref().unwrap();
        assert!(p.bss.abs() < 0.02, "{p:?}");
    }

    #[test]
    fn two_runs_are_identical() {
        let a = serde_json::to_string(&run_family(Family::Lgbm, true, 4)).unwrap();
        let b = serde_json::to_string(&run_family(Family::Lgbm, true, 4)).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn a_range_without_a_complete_test_window_is_an_error() {
        let mut s = setup();
        let m = market(300, true, 5, s.geom.range.0, s.geom.range.1);
        s.geom.range.1 = s.geom.range.0 + 86_400_000_000 * 30;
        let idx: Vec<usize> = (0..feature_names().len()).collect();
        let r = evaluate(
            "S900",
            Family::Majority,
            15,
            &idx,
            &m.samples,
            &m.bars,
            &m.one,
            &s.geom,
            &Configs {
                strategy: &s.strategy,
                validation: &s.validation,
                backtest: &s.backtest,
                direction: false,
            },
        );
        assert!(r.is_err());
    }
}
