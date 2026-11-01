//! The `Classifier` trait and the two simple models: the majority class and the multinomial
//! logistic regression. See `docs/MODELING.md`, section 1.

use linfa::prelude::*;
use linfa_logistic::MultiLogisticRegression;
use ndarray::{Array1, Array2};
use vwap_core::StrategyConfig;

use crate::error::ModelError;
use crate::matrix::{Matrix, Probs};
use crate::prep::Preprocessor;

pub trait Classifier {
    /// `y` holds class indices 0, 1, 2.
    fn fit(&mut self, x: &Matrix, y: &[usize]) -> Result<(), ModelError>;
    fn predict_proba(&self, x: &Matrix) -> Result<Probs, ModelError>;
}

fn check(x: &Matrix, y: &[usize]) -> Result<(), ModelError> {
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
    Ok(())
}

/// Predicts the training class frequencies for every row.
#[derive(Debug, Clone, Default)]
pub struct Majority {
    freq: Option<[f64; 3]>,
}

impl Majority {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn frequencies(&self) -> Option<[f64; 3]> {
        self.freq
    }
}

impl Classifier for Majority {
    fn fit(&mut self, x: &Matrix, y: &[usize]) -> Result<(), ModelError> {
        check(x, y)?;
        let mut c = [0.0; 3];
        for &k in y {
            if let Some(v) = c.get_mut(k) {
                *v += 1.0;
            }
        }
        let n: f64 = c.iter().sum();
        if n == 0.0 {
            return Err(ModelError::Fit("no labelled rows".into()));
        }
        self.freq = Some(c.map(|v| v / n));
        Ok(())
    }

    fn predict_proba(&self, x: &Matrix) -> Result<Probs, ModelError> {
        let f = self.freq.ok_or(ModelError::NotFit)?;
        Ok(vec![f; x.n])
    }
}

/// Multinomial logistic regression with an L2 penalty on prepared inputs.
pub struct Logistic {
    alpha: f64,
    max_iterations: u64,
    cfg: StrategyConfig,
    state: Option<LogisticState>,
}

struct LogisticState {
    prep: Preprocessor,
    model: linfa_logistic::MultiFittedLogisticRegression<f64, usize>,
    /// Which of the three classes the fitted model knows, in its column order.
    present: Vec<usize>,
}

impl Logistic {
    pub fn new(alpha: f64, cfg: &StrategyConfig) -> Self {
        Self {
            alpha,
            max_iterations: cfg.logistic_max_iterations,
            cfg: cfg.clone(),
            state: None,
        }
    }
}

fn to_array(x: &Matrix) -> Result<Array2<f64>, ModelError> {
    Array2::from_shape_vec((x.n, x.p), x.data.clone()).map_err(|e| ModelError::Shape(e.to_string()))
}

impl Classifier for Logistic {
    fn fit(&mut self, x: &Matrix, y: &[usize]) -> Result<(), ModelError> {
        check(x, y)?;
        let prep = Preprocessor::fit(x, &self.cfg);
        let xs = to_array(&prep.transform(x))?;
        let mut present: Vec<usize> = (0..3).filter(|k| y.contains(k)).collect();
        present.sort_unstable();
        if present.len() < 2 {
            return Err(ModelError::Fit(
                "the training window holds fewer than two classes".into(),
            ));
        }
        let ys: Array1<usize> = y
            .iter()
            .map(|k| present.iter().position(|p| p == k).unwrap_or(0))
            .collect();
        let ds = linfa::Dataset::new(xs, ys);
        let model = MultiLogisticRegression::default()
            .alpha(self.alpha)
            .max_iterations(self.max_iterations)
            .fit(&ds)
            .map_err(|e| ModelError::Fit(e.to_string()))?;
        self.state = Some(LogisticState {
            prep,
            model,
            present,
        });
        Ok(())
    }

    fn predict_proba(&self, x: &Matrix) -> Result<Probs, ModelError> {
        let st = self.state.as_ref().ok_or(ModelError::NotFit)?;
        let xs = to_array(&st.prep.transform(x))?;
        let p = st.model.predict_probabilities(&xs);
        Ok(p.rows()
            .into_iter()
            .map(|r| {
                let mut out = [0.0; 3];
                for (col, &k) in st.present.iter().enumerate() {
                    if let (Some(o), Some(v)) = (out.get_mut(k), r.get(col)) {
                        *o = *v;
                    }
                }
                out
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::log_loss;
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    fn cfg() -> StrategyConfig {
        vwap_core::load_strategy_config(std::path::Path::new("../../config")).unwrap()
    }

    fn data(n: usize, seed: u64, signal: bool) -> (Matrix, Vec<usize>) {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let mut rows = Vec::new();
        let mut y = Vec::new();
        for _ in 0..n {
            let a: f64 = rng.random::<f64>() * 2.0 - 1.0;
            let b: f64 = rng.random::<f64>() * 2.0 - 1.0;
            let class = if signal {
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
                if rng.random::<f64>() < 0.1 {
                    f64::NAN
                } else {
                    a * b
                },
            ]);
            y.push(class);
        }
        (Matrix::from_rows(&rows), y)
    }

    #[test]
    fn majority_predicts_the_training_frequencies() {
        let x = Matrix::from_rows(&vec![vec![0.0]; 4]);
        let mut m = Majority::new();
        m.fit(&x, &[0, 0, 1, 2]).unwrap();
        let p = m.predict_proba(&x).unwrap();
        assert_eq!(p[0], [0.5, 0.25, 0.25]);
        assert!((p[3].iter().sum::<f64>() - 1.0).abs() < 1e-15);
        assert!(Majority::new().predict_proba(&x).is_err());
    }

    #[test]
    fn logistic_beats_the_majority_on_a_planted_signal_and_not_on_noise() {
        for (signal, must_win) in [(true, true), (false, false)] {
            let (xt, yt) = data(1500, 1, signal);
            let (xv, yv) = data(1500, 2, signal);
            let mut maj = Majority::new();
            maj.fit(&xt, &yt).unwrap();
            let mut lr = Logistic::new(1.0, &cfg());
            lr.fit(&xt, &yt).unwrap();
            let l_maj = log_loss(&maj.predict_proba(&xv).unwrap(), &yv);
            let l_lr = log_loss(&lr.predict_proba(&xv).unwrap(), &yv);
            if must_win {
                assert!(l_lr < l_maj - 0.1, "{l_lr} vs {l_maj}");
            } else {
                assert!(l_lr > l_maj - 0.02, "{l_lr} vs {l_maj}");
            }
        }
    }

    #[test]
    fn logistic_probabilities_are_valid_and_two_fits_are_identical() {
        let (x, y) = data(600, 7, true);
        let mut a = Logistic::new(1.0, &cfg());
        let mut b = Logistic::new(1.0, &cfg());
        a.fit(&x, &y).unwrap();
        b.fit(&x, &y).unwrap();
        let pa = a.predict_proba(&x).unwrap();
        let pb = b.predict_proba(&x).unwrap();
        for (ra, rb) in pa.iter().zip(&pb) {
            assert!((ra.iter().sum::<f64>() - 1.0).abs() < 1e-9);
            assert!(ra.iter().all(|&v| (0.0..=1.0).contains(&v)));
            for (u, v) in ra.iter().zip(rb) {
                assert_eq!(u.to_bits(), v.to_bits());
            }
        }
    }

    #[test]
    fn a_training_window_missing_a_class_still_predicts_three_columns() {
        let (x, y) = data(400, 3, true);
        let keep: Vec<usize> = (0..y.len()).filter(|&i| y[i] != 1).collect();
        let xs = x.select(&keep);
        let ys: Vec<usize> = keep.iter().map(|&i| y[i]).collect();
        let mut lr = Logistic::new(1.0, &cfg());
        lr.fit(&xs, &ys).unwrap();
        let p = lr.predict_proba(&x).unwrap();
        assert!(
            p.iter()
                .all(|r| r[1] == 0.0 && (r.iter().sum::<f64>() - 1.0).abs() < 1e-9)
        );
        let mut one = Logistic::new(1.0, &cfg());
        assert!(one.fit(&x.select(&[0, 1]), &[0, 0]).is_err());
    }

    #[test]
    fn shape_errors_are_reported() {
        let mut m = Majority::new();
        assert!(m.fit(&Matrix::from_rows(&[vec![0.0]]), &[0, 1]).is_err());
    }
}
