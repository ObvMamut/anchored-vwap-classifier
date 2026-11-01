//! The threshold $\tau$ on the calibrated rejection probability. See `docs/MODELING.md`,
//! section 5. Trade only when $\tilde P(\text{rejection}) > \tau$; the threshold maximises a
//! lower quantile of a session clustered bootstrap of the mean net ticks, subject to a minimum
//! number of trades. If no threshold qualifies the fold abstains.

use vwap_core::StrategyConfig;
use vwap_validation::{cluster_draws, percentile};

#[derive(Debug, Clone, PartialEq)]
pub struct TauRow {
    pub tau: f64,
    pub n_trades: usize,
    pub mean_net: f64,
    /// The bootstrap quantile of the mean; NaN below the trade minimum.
    pub lcb: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TauChoice {
    /// `None` means the fold abstains ($\tau = 1$).
    pub tau: Option<f64>,
    pub table: Vec<TauRow>,
}

/// `p_rej[i]` is the calibrated rejection probability of event `i`, `net[i]` the net ticks of
/// trading it, `sessions[i]` its session. The three slices have equal length.
pub fn tune_tau(
    p_rej: &[f64],
    net: &[f64],
    sessions: &[i32],
    cfg: &StrategyConfig,
    seed: u64,
) -> TauChoice {
    let mut table = Vec::new();
    let mut best: Option<(f64, f64)> = None;
    for tau in cfg.tau_grid() {
        let sel: Vec<usize> = (0..p_rej.len())
            .filter(|&i| p_rej.get(i).copied().unwrap_or(0.0) > tau)
            .collect();
        let n = sel.len();
        let mean_net = if n == 0 {
            f64::NAN
        } else {
            sel.iter()
                .map(|&i| net.get(i).copied().unwrap_or(0.0))
                .sum::<f64>()
                / n as f64
        };
        let mut lcb = f64::NAN;
        if n >= cfg.min_trades_tau {
            let draws = cluster_draws(sessions, cfg.tau_bootstrap_draws, seed, |rows| {
                let (mut s, mut c) = (0.0, 0usize);
                for &i in rows {
                    if p_rej.get(i).copied().unwrap_or(0.0) > tau {
                        s += net.get(i).copied().unwrap_or(0.0);
                        c += 1;
                    }
                }
                if c == 0 { f64::NAN } else { s / c as f64 }
            });
            lcb = percentile(&draws, cfg.tau_lcb_quantile);
            if lcb.is_finite() && best.is_none_or(|(_, b)| lcb > b) {
                best = Some((tau, lcb));
            }
        }
        table.push(TauRow {
            tau,
            n_trades: n,
            mean_net,
            lcb,
        });
    }
    let tau = best
        .filter(|&(_, lcb)| !cfg.tau_require_positive_lcb || lcb > 0.0)
        .map(|b| b.0);
    TauChoice { tau, table }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    fn cfg() -> StrategyConfig {
        vwap_core::load_strategy_config(std::path::Path::new("../../config")).unwrap()
    }

    /// Net ticks that rise with the probability: low scores lose, high scores win.
    fn informative(n: usize, seed: u64) -> (Vec<f64>, Vec<f64>, Vec<i32>) {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let p: Vec<f64> = (0..n).map(|_| rng.random::<f64>()).collect();
        let net: Vec<f64> = p
            .iter()
            .map(|&q| (q - 0.6) * 20.0 + (rng.random::<f64>() - 0.5) * 10.0)
            .collect();
        let sessions: Vec<i32> = (0..n).map(|i| (i / 3) as i32).collect();
        (p, net, sessions)
    }

    #[test]
    fn an_informative_score_gets_a_threshold_near_the_break_even() {
        let (p, net, s) = informative(4000, 1);
        let c = tune_tau(&p, &net, &s, &cfg(), 7);
        let tau = c.tau.expect("a threshold");
        assert!((0.5..=0.8).contains(&tau), "tau = {tau}");
        let row = c.table.iter().find(|r| r.tau == tau).unwrap();
        assert!(row.mean_net > 0.0 && row.n_trades >= cfg().min_trades_tau);
    }

    #[test]
    fn too_few_trades_make_the_fold_abstain() {
        let (p, net, s) = informative(60, 2);
        let c = tune_tau(&p, &net, &s, &cfg(), 7);
        assert!(c.tau.is_none());
        assert!(c.table.iter().all(|r| r.lcb.is_nan()));
        assert_eq!(c.table.len(), cfg().tau_grid().len());
    }

    #[test]
    fn a_fold_abstains_when_no_threshold_has_a_positive_lower_bound() {
        let (p, mut net, s) = informative(3000, 4);
        for x in &mut net {
            *x = -x.abs() - 1.0;
        }
        let strict = cfg();
        let c = tune_tau(&p, &net, &s, &strict, 7);
        assert!(c.tau.is_none());
        // the table is still reported
        assert!(c.table.iter().any(|r| r.lcb.is_finite()));
        let mut literal = cfg();
        literal.tau_require_positive_lcb = false;
        assert!(tune_tau(&p, &net, &s, &literal, 7).tau.is_some());
    }

    #[test]
    fn the_choice_is_reproducible_for_one_seed() {
        let (p, net, s) = informative(1500, 3);
        let a = tune_tau(&p, &net, &s, &cfg(), 11);
        let b = tune_tau(&p, &net, &s, &cfg(), 11);
        assert_eq!(a.tau, b.tau);
        for (x, y) in a.table.iter().zip(&b.table) {
            assert_eq!(x.lcb.to_bits(), y.lcb.to_bits());
        }
    }
}
