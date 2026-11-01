//! LightGBM multiclass with the deterministic settings of `CLAUDE.md`: one thread, row wise
//! histograms, and every seed taken from the configuration seed.

use lightgbm3::{Booster, Dataset};
use serde_json::json;
use vwap_core::{LgbmPoint, StrategyConfig};

use crate::classifier::Classifier;
use crate::error::ModelError;
use crate::matrix::{Matrix, Probs};

fn fit_err(e: impl std::fmt::Display) -> ModelError {
    ModelError::Fit(e.to_string())
}

fn params(point: &LgbmPoint, rounds: i64, seed: u64) -> serde_json::Value {
    json!({
        "objective": "multiclass",
        "num_class": 3,
        "num_iterations": rounds,
        "learning_rate": point.learning_rate,
        "num_leaves": point.num_leaves,
        "min_data_in_leaf": point.min_data_in_leaf,
        "feature_fraction": point.feature_fraction,
        "lambda_l2": point.lambda_l2,
        "verbose": -1,
        "num_threads": 1,
        "deterministic": true,
        "force_row_wise": true,
        "seed": seed,
        "bagging_seed": seed,
        "feature_fraction_seed": seed,
        "data_random_seed": seed,
    })
}

fn train(
    point: &LgbmPoint,
    rounds: i64,
    seed: u64,
    x: &Matrix,
    y: &[usize],
) -> Result<Booster, ModelError> {
    if x.n != y.len() {
        return Err(ModelError::Shape(format!(
            "{} rows but {} labels",
            x.n,
            y.len()
        )));
    }
    if x.n == 0 {
        return Err(ModelError::Fit("no training rows".into()));
    }
    let labels: Vec<f32> = y.iter().map(|&k| k as f32).collect();
    let ds = Dataset::from_slice(&x.data, &labels, x.p as i32, true).map_err(fit_err)?;
    Booster::train(ds, &params(point, rounds, seed)).map_err(fit_err)
}

fn probs_from(flat: &[f64]) -> Probs {
    flat.chunks(3)
        .map(|c| {
            let mut r = [0.0; 3];
            for (o, v) in r.iter_mut().zip(c) {
                *o = *v;
            }
            r
        })
        .collect()
}

/// A fitted LightGBM multiclass model.
pub struct Lgbm {
    point: LgbmPoint,
    rounds: i64,
    seed: u64,
    booster: Option<Booster>,
}

impl Lgbm {
    pub fn new(point: &LgbmPoint, rounds: i64, seed: u64) -> Self {
        Self {
            point: point.clone(),
            rounds,
            seed,
            booster: None,
        }
    }

    /// The text model, the input of the TreeSHAP in `shap.rs`.
    pub fn model_string(&self) -> Result<String, ModelError> {
        self.booster
            .as_ref()
            .ok_or(ModelError::NotFit)?
            .save_string()
            .map_err(fit_err)
    }

    /// Raw scores, `n` rows by 3 classes.
    pub fn raw_scores(&self, x: &Matrix) -> Result<Vec<[f64; 3]>, ModelError> {
        let b = self.booster.as_ref().ok_or(ModelError::NotFit)?;
        let flat = b.raw_scores(&x.data, x.p as i32, true).map_err(fit_err)?;
        Ok(probs_from(&flat))
    }
}

impl Classifier for Lgbm {
    fn fit(&mut self, x: &Matrix, y: &[usize]) -> Result<(), ModelError> {
        self.booster = Some(train(&self.point, self.rounds, self.seed, x, y)?);
        Ok(())
    }

    fn predict_proba(&self, x: &Matrix) -> Result<Probs, ModelError> {
        let b = self.booster.as_ref().ok_or(ModelError::NotFit)?;
        Ok(probs_from(
            &b.predict(&x.data, x.p as i32, true).map_err(fit_err)?,
        ))
    }
}

/// Trains once for `max_rounds` and returns the probabilities of `valid` after every
/// `step` rounds (and after the last round). One booster serves all the checkpoints.
pub fn fit_checkpoints(
    point: &LgbmPoint,
    cfg: &StrategyConfig,
    seed: u64,
    x: &Matrix,
    y: &[usize],
    valid: &Matrix,
) -> Result<Vec<(i64, Probs)>, ModelError> {
    let max = cfg.lgbm_max_rounds;
    let mut b = train(point, max, seed, x, y)?;
    let trained = i64::from(b.num_iterations());
    let mut out = Vec::new();
    let mut r = cfg.lgbm_round_step;
    loop {
        let r_eff = r.min(max).min(trained);
        b.set_max_iterations(r_eff as i32).map_err(fit_err)?;
        let flat = b
            .predict(&valid.data, valid.p as i32, true)
            .map_err(fit_err)?;
        out.push((r_eff, probs_from(&flat)));
        if r >= max || r_eff >= trained {
            break;
        }
        r += cfg.lgbm_round_step;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::log_loss;
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    pub fn cfg() -> StrategyConfig {
        vwap_core::load_strategy_config(std::path::Path::new("../../config")).unwrap()
    }

    pub fn planted(n: usize, seed: u64, signal: bool) -> (Matrix, Vec<usize>) {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let mut rows = Vec::new();
        let mut y = Vec::new();
        for _ in 0..n {
            let a: f64 = rng.random::<f64>() * 2.0 - 1.0;
            let b: f64 = rng.random::<f64>() * 2.0 - 1.0;
            let c = if signal {
                if a > 0.3 {
                    0
                } else if b > 0.3 {
                    1
                } else {
                    2
                }
            } else {
                rng.random_range(0..3)
            };
            rows.push(vec![
                a,
                b,
                rng.random(),
                if rng.random::<f64>() < 0.1 {
                    f64::NAN
                } else {
                    a * b
                },
            ]);
            y.push(c);
        }
        (Matrix::from_rows(&rows), y)
    }

    #[test]
    fn two_fits_with_one_seed_are_bit_identical_and_probabilities_sum_to_one() {
        let c = cfg();
        let (x, y) = planted(500, 1, true);
        let mut a = Lgbm::new(&c.lgbm_grid[0], 30, c.seed);
        let mut b = Lgbm::new(&c.lgbm_grid[0], 30, c.seed);
        a.fit(&x, &y).unwrap();
        b.fit(&x, &y).unwrap();
        let (pa, pb) = (a.predict_proba(&x).unwrap(), b.predict_proba(&x).unwrap());
        for (ra, rb) in pa.iter().zip(&pb) {
            assert!((ra.iter().sum::<f64>() - 1.0).abs() < 1e-9);
            for (u, v) in ra.iter().zip(rb) {
                assert_eq!(u.to_bits(), v.to_bits());
            }
        }
    }

    #[test]
    fn checkpoints_cover_the_rounds_and_match_a_fresh_fit_at_that_round() {
        let c = cfg();
        let (xt, yt) = planted(600, 2, true);
        let (xv, yv) = planted(300, 3, true);
        let cps = fit_checkpoints(&c.lgbm_grid[0], &c, c.seed, &xt, &yt, &xv).unwrap();
        assert_eq!(cps.len(), (c.lgbm_max_rounds / c.lgbm_round_step) as usize);
        assert_eq!(cps.first().unwrap().0, c.lgbm_round_step);
        assert_eq!(cps.last().unwrap().0, c.lgbm_max_rounds);
        // the loss falls on a real signal
        assert!(log_loss(&cps[5].1, &yv) < log_loss(&cps[0].1, &yv));
        // a model trained for 20 rounds equals the 20 round checkpoint
        let mut m = Lgbm::new(&c.lgbm_grid[0], 20, c.seed);
        m.fit(&xt, &yt).unwrap();
        let direct = m.predict_proba(&xv).unwrap();
        let cp = &cps[1];
        assert_eq!(cp.0, 20);
        for (r1, r2) in direct.iter().zip(&cp.1) {
            for (u, v) in r1.iter().zip(r2) {
                assert!((u - v).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn it_beats_the_majority_on_a_planted_signal() {
        let c = cfg();
        let (xt, yt) = planted(1200, 4, true);
        let (xv, yv) = planted(800, 5, true);
        let mut m = Lgbm::new(&c.lgbm_grid[2], 100, c.seed);
        m.fit(&xt, &yt).unwrap();
        let l = log_loss(&m.predict_proba(&xv).unwrap(), &yv);
        let base = log_loss(&vec![[0.35, 0.3, 0.35]; 800], &yv);
        assert!(l < base - 0.2, "{l} vs {base}");
    }
}
