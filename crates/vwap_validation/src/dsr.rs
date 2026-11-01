//! The probabilistic and Deflated Sharpe Ratio (Bailey and Lopez de Prado). All Sharpe ratios
//! are per period (daily), never annualised, inside these formulas.

use crate::math::{norm_cdf, norm_ppf};

/// The Euler Mascheroni constant.
const EULER_GAMMA: f64 = 0.577_215_664_901_532_9;

/// Expected maximum Sharpe ratio of `n_trials` independent trials whose true Sharpe ratio is
/// zero, given the variance of the trial Sharpe ratios. One trial has no selection effect, so it
/// returns 0.
pub fn expected_max_sharpe(var_sharpe: f64, n_trials: usize) -> f64 {
    if n_trials <= 1 || var_sharpe.is_nan() || var_sharpe <= 0.0 {
        return 0.0;
    }
    let n = n_trials as f64;
    var_sharpe.sqrt()
        * ((1.0 - EULER_GAMMA) * norm_ppf(1.0 - 1.0 / n)
            + EULER_GAMMA * norm_ppf(1.0 - 1.0 / (n * std::f64::consts::E)))
}

/// Probability that the true Sharpe ratio exceeds `benchmark`, given `sr` measured on `t_obs`
/// observations with skewness `skew` and (non excess) kurtosis `kurt`.
pub fn probabilistic_sharpe(sr: f64, benchmark: f64, t_obs: usize, skew: f64, kurt: f64) -> f64 {
    if t_obs < 2 {
        return f64::NAN;
    }
    let var_term = 1.0 - skew * sr + (kurt - 1.0) / 4.0 * sr * sr;
    if var_term <= 0.0 {
        return f64::NAN;
    }
    norm_cdf((sr - benchmark) * ((t_obs - 1) as f64).sqrt() / var_term.sqrt())
}

/// The Deflated Sharpe Ratio: the probabilistic Sharpe ratio against the expected maximum
/// Sharpe ratio of `n_trials` zero skill trials.
pub fn dsr(sr: f64, var_sharpe: f64, n_trials: usize, t_obs: usize, skew: f64, kurt: f64) -> f64 {
    probabilistic_sharpe(
        sr,
        expected_max_sharpe(var_sharpe, n_trials),
        t_obs,
        skew,
        kurt,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Reference values computed independently with Python statistics.NormalDist (not with this
    // code): bracket(N) = (1 - gamma) Phi^-1(1 - 1/N) + gamma Phi^-1(1 - 1/(N e)).
    const BRACKET: [(usize, f64); 4] = [
        (2, 0.519_755_344_3),
        (10, 1.574_598_301_3),
        (100, 2.530_602_893_2),
        (1000, 3.255_121_513_7),
    ];

    #[test]
    fn expected_max_sharpe_matches_independent_reference_values() {
        for (n, b) in BRACKET {
            // with unit variance the expected maximum equals the bracket
            assert!((expected_max_sharpe(1.0, n) - b).abs() < 1e-7, "N = {n}");
        }
        // it scales with the standard deviation of the trial Sharpe ratios
        assert!((expected_max_sharpe(0.04, 10) - 0.2 * 1.574_598_301_3).abs() < 1e-7);
    }

    #[test]
    fn one_trial_gives_the_probabilistic_sharpe_ratio() {
        // normal returns (skew 0, kurtosis 3), sr 0.2 per period, 101 observations
        let psr = probabilistic_sharpe(0.2, 0.0, 101, 0.0, 3.0);
        assert!((psr - 0.976_164_809_672).abs() < 1e-9);
        assert_eq!(dsr(0.2, 0.5, 1, 101, 0.0, 3.0), psr);
    }

    #[test]
    fn dsr_matches_a_worked_example() {
        // sr 0.5, trial Sharpe variance 0.04, 10 trials, 250 observations, skew -0.5, kurtosis 5
        let d = dsr(0.5, 0.04, 10, 250, -0.5, 5.0);
        assert!((d - 0.991_450_995_708).abs() < 1e-9, "{d}");
    }

    #[test]
    fn dsr_falls_as_the_number_of_trials_grows_and_rises_with_the_sample() {
        let mut last = f64::INFINITY;
        for n in [1, 2, 5, 20, 100, 1000] {
            let d = dsr(0.15, 0.01, n, 500, 0.0, 3.0);
            assert!(d <= last + 1e-12, "N = {n}");
            last = d;
        }
        assert!(dsr(0.3, 0.01, 10, 2000, 0.0, 3.0) > dsr(0.3, 0.01, 10, 200, 0.0, 3.0));
    }

    #[test]
    fn degenerate_inputs_do_not_panic() {
        assert!(probabilistic_sharpe(0.1, 0.0, 1, 0.0, 3.0).is_nan());
        assert!(probabilistic_sharpe(5.0, 0.0, 100, 3.0, 3.0).is_nan()); // non positive variance term
        assert_eq!(expected_max_sharpe(0.0, 10), 0.0);
    }
}
