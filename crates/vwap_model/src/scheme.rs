//! The nested, cross fitted scheme run inside one outer training window. Nothing in it touches
//! the outer test window. See `docs/MODELING.md`, section 2.
//!
//! 1. Inner expanding folds give out of fold probabilities $Q_k$ for blocks $1..K$.
//! 2. The hyperparameters (and for LightGBM the number of rounds) minimise the block size
//!    weighted out of fold log loss.
//! 3. The calibrator of block $k$ is fit on the out of fold predictions of the other blocks,
//!    after removing the events within the embargo of block $k$, giving honest $\tilde Q_k$.
//! 4. $\tau$ is tuned on $\tilde Q$ (`threshold.rs`).
//! 5. The selected model is refit on the whole window, its test predictions are calibrated by a
//!    calibrator fit on all out of fold predictions, and $\tau$ is applied by the caller.

use vwap_core::{LgbmPoint, StrategyConfig, ValidationConfig};
use vwap_validation::{EventSpan, InnerFold, blocked_expanding};

use crate::calibrate::MultiCalibrator;
use crate::classifier::{Classifier, Logistic, Majority};
use crate::error::ModelError;
use crate::lgbm::{Lgbm, fit_checkpoints};
use crate::matrix::{Matrix, Probs};
use crate::metrics::log_loss;
use crate::shap::TreeModel;
use crate::threshold::{TauChoice, tune_tau};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Majority,
    Logistic,
    Lgbm,
}

/// All events of one timeframe that may be used (labelled, not ambiguous), in any order.
pub struct Table<'a> {
    pub x: &'a Matrix,
    /// Class indices 0, 1, 2.
    pub y: &'a [usize],
    pub spans: &'a [EventSpan],
    /// Net ticks of trading the event toward rejection, costs included (a proxy for tuning).
    pub net: &'a [f64],
    pub sessions: &'a [i32],
}

pub struct Params<'a> {
    pub cfg: &'a StrategyConfig,
    pub validation: &'a ValidationConfig,
    pub embargo_us: i64,
    pub seed: u64,
}

#[derive(Debug, Clone)]
pub struct FoldFit {
    /// Probabilities on the test events before and after calibration.
    pub raw: Probs,
    pub calibrated: Probs,
    pub tau: TauChoice,
    /// What the inner folds selected, for the log.
    pub selected: String,
    pub rounds: Option<i64>,
    /// Inner out of fold log loss of the selected candidate.
    pub selection_log_loss: f64,
    /// Number of candidates scored by the inner folds (counted as trials of the family).
    pub candidates: usize,
    /// Class frequencies of the training window (the majority forecast for this fold).
    pub base_freq: [f64; 3],
    /// Mean absolute contribution per feature and class on the test events, when the model has one.
    pub importance: Option<Vec<[f64; 3]>>,
    pub n_train: usize,
}

/// Child seed from the master seed and a stage name, so adding a stage shifts no other.
pub fn child_seed(master: u64, name: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ master;
    for b in name.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

struct Cand {
    desc: String,
    point: Option<usize>,
    alpha: Option<f64>,
    rounds: Option<i64>,
    per_fold: Vec<Probs>,
}

fn rows_of(t: &Table, idx: &[usize]) -> (Matrix, Vec<usize>) {
    (
        t.x.select(idx),
        idx.iter()
            .map(|&i| t.y.get(i).copied().unwrap_or(0))
            .collect(),
    )
}

fn candidates(
    family: Family,
    t: &Table,
    folds: &[InnerFold],
    p: &Params,
) -> Result<Vec<Cand>, ModelError> {
    match family {
        Family::Majority => {
            let mut per_fold = Vec::new();
            for f in folds {
                let (x, y) = rows_of(t, &f.train);
                let mut m = Majority::new();
                m.fit(&x, &y)?;
                per_fold.push(m.predict_proba(&t.x.select(&f.test))?);
            }
            Ok(vec![Cand {
                desc: "majority".into(),
                point: None,
                alpha: None,
                rounds: None,
                per_fold,
            }])
        }
        Family::Logistic => {
            let mut out = Vec::new();
            for &alpha in &p.cfg.logistic_l2_grid {
                let mut per_fold = Vec::new();
                for f in folds {
                    let (x, y) = rows_of(t, &f.train);
                    let mut m = Logistic::new(alpha, p.cfg);
                    m.fit(&x, &y)?;
                    per_fold.push(m.predict_proba(&t.x.select(&f.test))?);
                }
                out.push(Cand {
                    desc: format!("logistic alpha {alpha}"),
                    point: None,
                    alpha: Some(alpha),
                    rounds: None,
                    per_fold,
                });
            }
            Ok(out)
        }
        Family::Lgbm => {
            let mut out: Vec<Cand> = Vec::new();
            for (gi, point) in p.cfg.lgbm_grid.iter().enumerate() {
                // checkpoints[fold] = list of (round, probs)
                let mut cps = Vec::new();
                for f in folds {
                    let (x, y) = rows_of(t, &f.train);
                    let valid = t.x.select(&f.test);
                    cps.push(fit_checkpoints(point, p.cfg, p.seed, &x, &y, &valid)?);
                }
                let n_cp = cps.iter().map(Vec::len).min().unwrap_or(0);
                for c in 0..n_cp {
                    let rounds = cps.first().and_then(|f| f.get(c)).map_or(0, |(r, _)| *r);
                    out.push(Cand {
                        desc: format!("lgbm grid {gi} rounds {rounds}"),
                        point: Some(gi),
                        alpha: None,
                        rounds: Some(rounds),
                        per_fold: cps
                            .iter()
                            .map(|f| f.get(c).map(|(_, pr)| pr.clone()).unwrap_or_default())
                            .collect(),
                    });
                }
            }
            Ok(out)
        }
    }
}

fn weighted_loss(c: &Cand, t: &Table, folds: &[InnerFold]) -> f64 {
    let (mut s, mut n) = (0.0, 0.0);
    for (pr, f) in c.per_fold.iter().zip(folds) {
        let y: Vec<usize> = f
            .test
            .iter()
            .map(|&i| t.y.get(i).copied().unwrap_or(0))
            .collect();
        s += log_loss(pr, &y) * f.test.len() as f64;
        n += f.test.len() as f64;
    }
    s / n
}

/// Time range of the events of one inner block: first close and last label end.
fn block_range(spans: &[EventSpan], f: &InnerFold) -> (i64, i64) {
    let mut lo = i64::MAX;
    let mut hi = i64::MIN;
    for &i in &f.test {
        if let Some(s) = spans.get(i) {
            lo = lo.min(s.t_close_us);
            hi = hi.max(s.t_end_us);
        }
    }
    (lo, hi)
}

/// Runs the scheme on the outer training events `train` and predicts the outer test events
/// `test`. Errors when the window is too small for the inner folds or a model cannot be fit.
pub fn run_fold(
    family: Family,
    t: &Table,
    train: &[usize],
    test: &[usize],
    p: &Params,
) -> Result<FoldFit, ModelError> {
    if train.len() < p.cfg.min_train_events {
        return Err(ModelError::Fit(format!(
            "only {} training events, at least {} are required",
            train.len(),
            p.cfg.min_train_events
        )));
    }
    let folds = blocked_expanding(t.spans, train, p.validation.inner_blocks, p.embargo_us);
    if folds.len() < 2 {
        return Err(ModelError::Fit("too few events for the inner folds".into()));
    }

    // 1 and 2: out of fold probabilities and the selection
    let cands = candidates(family, t, &folds, p)?;
    let n_cands = cands.len();
    let (best, best_loss) = cands
        .iter()
        .enumerate()
        .map(|(i, c)| (i, weighted_loss(c, t, &folds)))
        .filter(|(_, l)| l.is_finite())
        .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)))
        .ok_or_else(|| ModelError::Fit("no candidate has a finite inner loss".into()))?;
    let chosen = cands
        .get(best)
        .ok_or_else(|| ModelError::Fit("selection out of range".into()))?;

    // 3: cross fitted calibration of the out of fold probabilities
    let ranges: Vec<(i64, i64)> = folds.iter().map(|f| block_range(t.spans, f)).collect();
    let mut honest: Vec<(usize, [f64; 3])> = Vec::new();
    for (fi, f) in folds.iter().enumerate() {
        let (start, end) = ranges.get(fi).copied().unwrap_or((0, 0));
        let (mut ps, mut ys): (Probs, Vec<usize>) = (Vec::new(), Vec::new());
        for (gi, g) in folds.iter().enumerate() {
            if gi == fi {
                continue;
            }
            for (pos, &ev) in g.test.iter().enumerate() {
                let Some(s) = t.spans.get(ev) else { continue };
                if s.t_end_us + p.embargo_us >= start && s.t_close_us <= end + p.embargo_us {
                    continue;
                }
                if let Some(row) = chosen.per_fold.get(gi).and_then(|b| b.get(pos)) {
                    ps.push(*row);
                    ys.push(t.y.get(ev).copied().unwrap_or(0));
                }
            }
        }
        let cal = MultiCalibrator::fit(&ps, &ys, p.cfg);
        let q = chosen.per_fold.get(fi).cloned().unwrap_or_default();
        for (&ev, row) in f.test.iter().zip(cal.transform(&q)) {
            honest.push((ev, row));
        }
    }

    // 4: the threshold on the honest calibrated probabilities
    let p_rej: Vec<f64> = honest.iter().map(|(_, r)| r[0]).collect();
    let net: Vec<f64> = honest
        .iter()
        .map(|(e, _)| t.net.get(*e).copied().unwrap_or(0.0))
        .collect();
    let sessions: Vec<i32> = honest
        .iter()
        .map(|(e, _)| t.sessions.get(*e).copied().unwrap_or(0))
        .collect();
    let tau = tune_tau(
        &p_rej,
        &net,
        &sessions,
        p.cfg,
        crate::scheme::child_seed(p.seed, "tau"),
    );

    // 5: final fit, final calibrator, test predictions
    let (x_train, y_train) = rows_of(t, train);
    let x_test = t.x.select(test);
    let (raw, importance) = match family {
        Family::Majority => {
            let mut m = Majority::new();
            m.fit(&x_train, &y_train)?;
            (m.predict_proba(&x_test)?, None)
        }
        Family::Logistic => {
            let mut m = Logistic::new(chosen.alpha.unwrap_or(1.0), p.cfg);
            m.fit(&x_train, &y_train)?;
            (m.predict_proba(&x_test)?, Some(m.importance(&x_test)?))
        }
        Family::Lgbm => {
            let point: &LgbmPoint = chosen
                .point
                .and_then(|g| p.cfg.lgbm_grid.get(g))
                .ok_or_else(|| ModelError::Fit("selected grid point is missing".into()))?;
            let mut m = Lgbm::new(point, chosen.rounds.unwrap_or(1), p.seed);
            m.fit(&x_train, &y_train)?;
            let raw = m.predict_proba(&x_test)?;
            let tm = TreeModel::parse(&m.model_string()?)?;
            let w = tm.n_features() + 1;
            let mut imp = vec![[0.0; 3]; tm.n_features()];
            for i in 0..x_test.n {
                let phi = tm.shap(x_test.row(i));
                for (f, cell) in imp.iter_mut().enumerate() {
                    for (k, c) in cell.iter_mut().enumerate() {
                        *c += phi.get(k * w + f).copied().unwrap_or(0.0).abs()
                            / x_test.n.max(1) as f64;
                    }
                }
            }
            (raw, Some(imp))
        }
    };
    let (mut all_p, mut all_y): (Probs, Vec<usize>) = (Vec::new(), Vec::new());
    for (f, b) in folds.iter().zip(&chosen.per_fold) {
        for (&ev, row) in f.test.iter().zip(b) {
            all_p.push(*row);
            all_y.push(t.y.get(ev).copied().unwrap_or(0));
        }
    }
    let final_cal = MultiCalibrator::fit(&all_p, &all_y, p.cfg);
    let calibrated = final_cal.transform(&raw);

    let mut base = [0.0; 3];
    for &k in &y_train {
        if let Some(v) = base.get_mut(k) {
            *v += 1.0;
        }
    }
    let n_tr = y_train.len().max(1) as f64;
    Ok(FoldFit {
        raw,
        calibrated,
        tau,
        selected: chosen.desc.clone(),
        rounds: chosen.rounds,
        selection_log_loss: best_loss,
        candidates: n_cands,
        base_freq: base.map(|c| c / n_tr),
        importance,
        n_train: train.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    const HOUR: i64 = 3_600_000_000;

    struct Synth {
        x: Matrix,
        y: Vec<usize>,
        spans: Vec<EventSpan>,
        net: Vec<f64>,
        sessions: Vec<i32>,
    }

    fn synth(n: usize, seed: u64, signal: bool) -> Synth {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let (mut rows, mut y, mut spans, mut net, mut sessions) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for i in 0..n {
            let a: f64 = rng.random::<f64>() * 2.0 - 1.0;
            let b: f64 = rng.random::<f64>();
            let p_rej = if signal { 0.5 + 0.4 * a } else { 0.5 };
            let class = if rng.random::<f64>() < p_rej { 0 } else { 1 };
            rows.push(vec![
                a,
                b,
                if rng.random::<f64>() < 0.1 {
                    f64::NAN
                } else {
                    a * b
                },
            ]);
            y.push(class);
            let t = i as i64 * HOUR;
            spans.push(EventSpan {
                t_close_us: t,
                t_end_us: t + 2 * HOUR,
            });
            net.push(if class == 0 { 6.0 } else { -6.0 } - 3.0);
            sessions.push((i / 4) as i32);
        }
        Synth {
            x: Matrix::from_rows(&rows),
            y,
            spans,
            net,
            sessions,
        }
    }

    fn setup() -> (StrategyConfig, ValidationConfig) {
        let dir = std::path::Path::new("../../config");
        let mut c = vwap_core::load_strategy_config(dir).unwrap();
        // a smaller search keeps the test quick; the logic is the same
        c.lgbm_grid.truncate(2);
        c.lgbm_max_rounds = 60;
        c.tau_bootstrap_draws = 100;
        (c, vwap_core::load_validation_config(dir).unwrap())
    }

    fn run(family: Family, s: &Synth, cfg: &StrategyConfig, v: &ValidationConfig) -> FoldFit {
        let t = Table {
            x: &s.x,
            y: &s.y,
            spans: &s.spans,
            net: &s.net,
            sessions: &s.sessions,
        };
        let n = s.y.len();
        let train: Vec<usize> = (0..n * 3 / 4).collect();
        let test: Vec<usize> = (n * 3 / 4..n).collect();
        let p = Params {
            cfg,
            validation: v,
            embargo_us: 2 * HOUR,
            seed: child_seed(cfg.seed, "test"),
        };
        run_fold(family, &t, &train, &test, &p).unwrap()
    }

    #[test]
    fn child_seeds_differ_by_stage_and_are_stable() {
        assert_eq!(child_seed(7, "tau"), child_seed(7, "tau"));
        assert_ne!(child_seed(7, "tau"), child_seed(7, "fit"));
        assert_ne!(child_seed(7, "tau"), child_seed(8, "tau"));
    }

    #[test]
    fn every_family_returns_valid_calibrated_probabilities() {
        let (c, v) = setup();
        let s = synth(1600, 1, true);
        for fam in [Family::Majority, Family::Logistic, Family::Lgbm] {
            let f = run(fam, &s, &c, &v);
            assert_eq!(f.raw.len(), 400);
            for r in f.calibrated.iter().chain(&f.raw) {
                assert!((r.iter().sum::<f64>() - 1.0).abs() < 1e-9, "{fam:?}");
                assert!(r.iter().all(|x| (0.0..=1.0).contains(x)));
            }
            assert!((f.base_freq.iter().sum::<f64>() - 1.0).abs() < 1e-12);
            assert!(f.candidates >= 1 && f.selection_log_loss.is_finite());
        }
    }

    #[test]
    fn a_planted_signal_beats_the_majority_out_of_sample_and_noise_does_not() {
        let (c, v) = setup();
        for (signal, wins) in [(true, true), (false, false)] {
            let s = synth(2000, 2, signal);
            let y_test = &s.y[1500..];
            let maj = run(Family::Majority, &s, &c, &v);
            let lr = run(Family::Logistic, &s, &c, &v);
            let l_maj = log_loss(&maj.calibrated, y_test);
            let l_lr = log_loss(&lr.calibrated, y_test);
            if wins {
                assert!(l_lr < l_maj - 0.03, "{l_lr} vs {l_maj}");
            } else {
                assert!(l_lr > l_maj - 0.02, "{l_lr} vs {l_maj}");
            }
        }
    }

    #[test]
    fn a_threshold_is_found_on_a_signal_and_the_importance_points_at_the_signal() {
        let (c, v) = setup();
        let s = synth(3000, 3, true);
        let f = run(Family::Lgbm, &s, &c, &v);
        assert!(f.tau.tau.is_some(), "no threshold");
        let imp = f.importance.unwrap();
        // feature 0 carries the signal, feature 1 is independent noise
        assert!(imp[0][0] > imp[1][0]);
    }

    #[test]
    fn the_scheme_is_deterministic() {
        let (c, v) = setup();
        let s = synth(1200, 4, true);
        let a = run(Family::Lgbm, &s, &c, &v);
        let b = run(Family::Lgbm, &s, &c, &v);
        assert_eq!(a.selected, b.selected);
        assert_eq!(a.tau.tau, b.tau.tau);
        assert_eq!(a.tau.table.len(), b.tau.table.len());
        for (ra, rb) in a.tau.table.iter().zip(&b.tau.table) {
            assert_eq!(ra.n_trades, rb.n_trades);
            assert_eq!(ra.mean_net.to_bits(), rb.mean_net.to_bits());
            assert_eq!(ra.lcb.to_bits(), rb.lcb.to_bits());
        }
        for (ra, rb) in a.calibrated.iter().zip(&b.calibrated) {
            for (u, w) in ra.iter().zip(rb) {
                assert_eq!(u.to_bits(), w.to_bits());
            }
        }
    }

    #[test]
    fn a_window_that_is_too_small_is_an_error_not_a_guess() {
        let (c, v) = setup();
        let s = synth(100, 5, true);
        let t = Table {
            x: &s.x,
            y: &s.y,
            spans: &s.spans,
            net: &s.net,
            sessions: &s.sessions,
        };
        let p = Params {
            cfg: &c,
            validation: &v,
            embargo_us: 2 * HOUR,
            seed: 1,
        };
        let train: Vec<usize> = (0..80).collect();
        let test: Vec<usize> = (80..100).collect();
        assert!(run_fold(Family::Majority, &t, &train, &test, &p).is_err());
    }
}
