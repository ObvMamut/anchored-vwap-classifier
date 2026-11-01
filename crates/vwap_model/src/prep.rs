//! Input preparation for the logistic model, fit on the training window only: median
//! imputation with missing indicators, winsorising at the training quantiles, standardising.
//! See `docs/FEATURES.md`, section 7.

use vwap_core::StrategyConfig;
use vwap_validation::percentile;

use crate::matrix::Matrix;

#[derive(Debug, Clone, PartialEq)]
pub struct Preprocessor {
    median: Vec<f64>,
    lo: Vec<f64>,
    hi: Vec<f64>,
    mean: Vec<f64>,
    sd: Vec<f64>,
    /// Columns with at least one missing value in the training window; each adds an indicator.
    indicators: Vec<usize>,
}

impl Preprocessor {
    pub fn fit(x: &Matrix, cfg: &StrategyConfig) -> Self {
        let (mut median, mut lo, mut hi) = (Vec::new(), Vec::new(), Vec::new());
        let (mut mean, mut sd, mut indicators) = (Vec::new(), Vec::new(), Vec::new());
        for j in 0..x.p {
            let col = x.column(j);
            let mut finite: Vec<f64> = col.iter().copied().filter(|v| v.is_finite()).collect();
            if finite.len() < col.len() {
                indicators.push(j);
            }
            finite.sort_by(f64::total_cmp);
            let med = if finite.is_empty() {
                0.0
            } else {
                percentile(&finite, 0.5)
            };
            let (l, h) = if finite.is_empty() {
                (0.0, 0.0)
            } else {
                (
                    percentile(&finite, cfg.winsor_lower),
                    percentile(&finite, cfg.winsor_upper),
                )
            };
            // statistics of the imputed, winsorised column
            let prepared: Vec<f64> = col
                .iter()
                .map(|&v| (if v.is_finite() { v } else { med }).clamp(l, h))
                .collect();
            let m = prepared.iter().sum::<f64>() / prepared.len().max(1) as f64;
            let var = prepared.iter().map(|v| (v - m) * (v - m)).sum::<f64>()
                / prepared.len().max(1) as f64;
            median.push(med);
            lo.push(l);
            hi.push(h);
            mean.push(m);
            sd.push(if var.sqrt() > 1e-12 { var.sqrt() } else { 1.0 });
        }
        Self {
            median,
            lo,
            hi,
            mean,
            sd,
            indicators,
        }
    }

    /// Width of the transformed matrix.
    pub fn output_width(&self) -> usize {
        self.median.len() + self.indicators.len()
    }

    pub fn transform(&self, x: &Matrix) -> Matrix {
        let p = self.median.len();
        let mut data = Vec::with_capacity(x.n * self.output_width());
        for i in 0..x.n {
            for j in 0..p {
                let v = x.get(i, j);
                let (med, l, h) = (
                    self.median.get(j).copied().unwrap_or(0.0),
                    self.lo.get(j).copied().unwrap_or(0.0),
                    self.hi.get(j).copied().unwrap_or(0.0),
                );
                let v = (if v.is_finite() { v } else { med }).clamp(l, h);
                data.push(
                    (v - self.mean.get(j).copied().unwrap_or(0.0))
                        / self.sd.get(j).copied().unwrap_or(1.0),
                );
            }
            for &j in &self.indicators {
                data.push(if x.get(i, j).is_finite() { 0.0 } else { 1.0 });
            }
        }
        Matrix {
            n: x.n,
            p: self.output_width(),
            data,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> StrategyConfig {
        vwap_core::load_strategy_config(std::path::Path::new("../../config")).unwrap()
    }

    #[test]
    fn missing_values_are_imputed_with_the_training_median_and_flagged() {
        let x = Matrix::from_rows(&[
            vec![1.0, 10.0],
            vec![2.0, f64::NAN],
            vec![3.0, 30.0],
            vec![4.0, 40.0],
        ]);
        let pp = Preprocessor::fit(&x, &cfg());
        assert_eq!(pp.output_width(), 3);
        let t = pp.transform(&x);
        assert_eq!(t.get(1, 2), 1.0);
        assert_eq!(t.get(0, 2), 0.0);
        // the imputed cell equals the median 30, so it standardises like a 30
        assert!((t.get(1, 1) - t.get(2, 1)).abs() < 1e-12);
    }

    #[test]
    fn training_columns_have_zero_mean_and_unit_spread() {
        let rows: Vec<Vec<f64>> = (0..200)
            .map(|i| vec![f64::from(i), f64::from(i % 7) * 3.0])
            .collect();
        let x = Matrix::from_rows(&rows);
        let pp = Preprocessor::fit(&x, &cfg());
        let t = pp.transform(&x);
        for j in 0..2 {
            let c = t.column(j);
            let m = c.iter().sum::<f64>() / 200.0;
            let v = c.iter().map(|a| (a - m) * (a - m)).sum::<f64>() / 200.0;
            assert!(m.abs() < 1e-9);
            assert!((v - 1.0).abs() < 1e-9);
        }
    }

    #[test]
    fn outliers_are_winsorised_at_the_training_quantiles() {
        let mut rows: Vec<Vec<f64>> = (0..100).map(|i| vec![f64::from(i)]).collect();
        let pp = Preprocessor::fit(&Matrix::from_rows(&rows), &cfg());
        rows.push(vec![1.0e9]);
        let t = pp.transform(&Matrix::from_rows(&rows));
        assert!(t.get(100, 0) < 3.0);
        assert!(
            (t.get(100, 0) - t.column(0)[..100].iter().cloned().fold(f64::MIN, f64::max)).abs()
                < 1e-9
        );
    }

    #[test]
    fn a_constant_column_does_not_divide_by_zero() {
        let x = Matrix::from_rows(&[vec![5.0], vec![5.0], vec![5.0]]);
        let t = Preprocessor::fit(&x, &cfg()).transform(&x);
        assert!(t.data.iter().all(|v| v.is_finite()));
    }
}
