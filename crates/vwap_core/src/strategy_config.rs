//! Configuration of the walk forward models: the validation geometry and gate limits
//! (`validation.toml`) and the model, calibration and threshold settings (`strategy.toml`).
//! Neither file defines any split date: the segment boundaries live in `research.toml` only.

use std::path::Path;

use serde::Deserialize;

use crate::config::read_toml;
use crate::error::CoreError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WalkMode {
    Rolling,
    Expanding,
}

/// `validation.toml`: how the outer folds are cut and what the gates demand.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidationConfig {
    pub train_months: u32,
    pub test_months: u32,
    pub step_months: u32,
    pub mode: WalkMode,
    /// Embargo as a multiple of the label horizon (horizon bars times bar width). At least 1.
    pub embargo_horizons: f64,
    /// Inner blocks of the nested cross fitted scheme (the window is cut into this plus one).
    pub inner_blocks: usize,
    /// Blocks of the combinatorially symmetric cross validation (even).
    pub pbo_blocks: usize,
    /// Fewer trials than this make the PBO undefined.
    pub pbo_min_trials: usize,
    /// Gate: minimum Deflated Sharpe Ratio.
    pub dsr_min: f64,
    /// Gate: maximum Probability of Backtest Overfitting.
    pub pbo_max: f64,
    /// Gate: maximum expected calibration error of any class.
    pub max_ece: f64,
    /// Gate: share of folds with positive net expectancy.
    pub min_fold_fraction: f64,
    /// Session clustered bootstrap draws of the gate intervals.
    pub bootstrap_draws: usize,
    /// Level of the two sided bootstrap interval of the gates.
    pub interval_level: f64,
}

impl ValidationConfig {
    pub fn validate(&self) -> Result<(), CoreError> {
        let bad = self.train_months == 0
            || self.test_months == 0
            || self.step_months == 0
            || self.embargo_horizons < 1.0
            || self.inner_blocks < 2
            || self.pbo_blocks < 4
            || !self.pbo_blocks.is_multiple_of(2)
            || self.pbo_min_trials < 4
            || !(0.0..=1.0).contains(&self.dsr_min)
            || !(0.0..=1.0).contains(&self.pbo_max)
            || !(0.0..=1.0).contains(&self.max_ece)
            || !(0.0..=1.0).contains(&self.min_fold_fraction)
            || self.bootstrap_draws < 100
            || !(0.5..1.0).contains(&self.interval_level);
        if bad {
            return Err(CoreError::ConfigInvalid(
                "validation.toml: positive window lengths, an embargo of at least one horizon, at least 2 inner blocks, an even number of at least 4 PBO blocks, at least 4 PBO trials and gate limits inside [0, 1] are required".into(),
            ));
        }
        Ok(())
    }
}

/// One point of the pre registered LightGBM grid.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LgbmPoint {
    pub num_leaves: i64,
    pub learning_rate: f64,
    pub min_data_in_leaf: i64,
    pub lambda_l2: f64,
    pub feature_fraction: f64,
}

/// `strategy.toml`: models, calibration and the threshold rule.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategyConfig {
    /// Master seed. Child seeds are derived from it by hashing a stage name.
    pub seed: u64,
    /// Fewer calibration events than this use Platt scaling instead of isotonic regression.
    pub isotonic_min_events: usize,
    /// Calibrated probabilities are clipped to `[clip_eps, 1 - clip_eps]`.
    pub clip_eps: f64,
    /// Quantile bins of the reliability diagram and the ECE.
    pub ece_bins: usize,
    /// Quantiles of the winsorising of the logistic inputs, fit on the training window.
    pub winsor_lower: f64,
    pub winsor_upper: f64,
    /// L2 penalty grid of the logistic regression.
    pub logistic_l2_grid: Vec<f64>,
    pub logistic_max_iterations: u64,
    /// Upper bound of boosting rounds searched by the inner folds.
    pub lgbm_max_rounds: i64,
    /// Spacing of the checkpoint rounds at which the inner folds are scored.
    pub lgbm_round_step: i64,
    /// Fewer training events than this make a fold abstain.
    pub min_train_events: usize,
    /// Lower and upper end and step of the threshold grid on the rejection probability.
    pub tau_min: f64,
    pub tau_max: f64,
    pub tau_step: f64,
    /// Fewer selected events than this disqualify a threshold.
    pub min_trades_tau: usize,
    /// Quantile of the bootstrap of the mean net ticks that the threshold maximises.
    pub tau_lcb_quantile: f64,
    /// Draws of that bootstrap.
    pub tau_bootstrap_draws: usize,
    /// A fold abstains unless the best threshold has a bootstrap lower bound above zero on the
    /// out of fold data. Without it a useless model still trades at its least bad threshold.
    pub tau_require_positive_lcb: bool,
    pub lgbm_grid: Vec<LgbmPoint>,
}

impl StrategyConfig {
    pub fn validate(&self) -> Result<(), CoreError> {
        let bad = self.clip_eps <= 0.0
            || self.clip_eps >= 0.5
            || self.ece_bins < 2
            || !(0.0..0.5).contains(&self.winsor_lower)
            || !(0.5..=1.0).contains(&self.winsor_upper)
            || self.logistic_l2_grid.is_empty()
            || self.logistic_l2_grid.iter().any(|&a| a < 0.0)
            || self.lgbm_max_rounds < 1
            || self.lgbm_round_step < 1
            || self.lgbm_round_step > self.lgbm_max_rounds
            || self.lgbm_grid.is_empty()
            || self.lgbm_grid.iter().any(|g| g.num_leaves < 2)
            || !(self.tau_min > 0.0 && self.tau_min < self.tau_max && self.tau_max < 1.0)
            || self.tau_step <= 0.0
            || !(0.0..0.5).contains(&self.tau_lcb_quantile)
            || self.tau_bootstrap_draws < 100
            || self.min_trades_tau == 0;
        if bad {
            return Err(CoreError::ConfigInvalid(
                "strategy.toml: clipping, bins, quantiles, grids and the threshold range must be positive and ordered".into(),
            ));
        }
        Ok(())
    }

    /// The threshold grid, from `tau_min` to `tau_max` inclusive.
    pub fn tau_grid(&self) -> Vec<f64> {
        let n = ((self.tau_max - self.tau_min) / self.tau_step + 1e-9).floor() as usize;
        (0..=n)
            .map(|k| self.tau_min + self.tau_step * k as f64)
            .collect()
    }
}

pub fn load_validation_config(dir: &Path) -> Result<ValidationConfig, CoreError> {
    let cfg: ValidationConfig = read_toml(dir, "validation.toml")?;
    cfg.validate()?;
    Ok(cfg)
}

pub fn load_strategy_config(dir: &Path) -> Result<StrategyConfig, CoreError> {
    let cfg: StrategyConfig = read_toml(dir, "strategy.toml")?;
    cfg.validate()?;
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> &'static Path {
        Path::new("../../config")
    }

    #[test]
    fn the_shipped_files_load() {
        let v = load_validation_config(dir()).unwrap();
        assert_eq!(v.train_months, 24);
        let s = load_strategy_config(dir()).unwrap();
        assert_eq!(s.lgbm_grid.len(), 8);
        let g = s.tau_grid();
        assert!((g[0] - 0.35).abs() < 1e-12);
        assert!((g[g.len() - 1] - 0.80).abs() < 1e-9);
        assert_eq!(g.len(), 46);
    }

    #[test]
    fn the_backtest_file_loads_and_the_round_trip_cost_is_2_8_ticks() {
        let b = load_backtest_config(dir()).unwrap();
        assert!((b.round_trip_ticks() - 2.8).abs() < 1e-12);
        let mut low = b.clone();
        low.slippage_ticks = 0.5;
        assert!(low.validate().is_err());
    }

    #[test]
    fn an_embargo_below_one_horizon_is_rejected() {
        let mut v = load_validation_config(dir()).unwrap();
        v.embargo_horizons = 0.5;
        assert!(v.validate().is_err());
        let mut v = load_validation_config(dir()).unwrap();
        v.pbo_blocks = 15;
        assert!(v.validate().is_err());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PositionPolicy {
    OneAtATime,
    OnePerSession,
    Overlap,
}

/// `backtest.toml`: fills, costs and the position policy (`docs/BACKTEST.md`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BacktestConfig {
    /// Adverse slippage per side, in ticks. Headline runs need at least 1.
    pub slippage_ticks: f64,
    /// Commission and fees per side, in USD.
    pub commission_usd: f64,
    pub position_policy: PositionPolicy,
    /// Slippage and commission levels of the cost sweep.
    pub sweep_slippage: Vec<f64>,
    pub sweep_commission: Vec<f64>,
    /// Trading days per year, used only to annualise the Sharpe ratio.
    pub trading_days_per_year: f64,
}

impl BacktestConfig {
    pub fn validate(&self) -> Result<(), CoreError> {
        let bad = self.slippage_ticks < 1.0
            || self.commission_usd < 0.0
            || self.sweep_slippage.is_empty()
            || self.sweep_commission.is_empty()
            || self.sweep_slippage.iter().any(|&s| s < 0.0)
            || self.sweep_commission.iter().any(|&c| c < 0.0)
            || self.trading_days_per_year <= 0.0;
        if bad {
            return Err(CoreError::ConfigInvalid(
                "backtest.toml: slippage must be at least 1 tick per side, costs and sweep levels non negative".into(),
            ));
        }
        Ok(())
    }

    /// Round trip cost in ticks, $\kappa = 2 n_{slip} + 2 c / v$ with $v$ the tick value.
    pub fn round_trip_ticks(&self) -> f64 {
        round_trip_ticks(self.slippage_ticks, self.commission_usd)
    }
}

/// $\kappa = 2 n_{slip} + 2 c_{usd} / v_{tick}$ in ticks.
pub fn round_trip_ticks(slippage_ticks: f64, commission_usd: f64) -> f64 {
    2.0 * slippage_ticks + 2.0 * commission_usd / crate::consts::TICK_VALUE_USD
}

pub fn load_backtest_config(dir: &Path) -> Result<BacktestConfig, CoreError> {
    let cfg: BacktestConfig = read_toml(dir, "backtest.toml")?;
    cfg.validate()?;
    Ok(cfg)
}
