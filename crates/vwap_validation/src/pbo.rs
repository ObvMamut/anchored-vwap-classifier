//! Probability of Backtest Overfitting by combinatorially symmetric cross validation
//! (Bailey, Borwein, Lopez de Prado and Zhu).
#![allow(clippy::indexing_slicing)]

/// Result of the CSCV procedure.
#[derive(Debug, Clone)]
pub struct PboResult {
    /// Share of combinations where the in sample winner is at or below the out of sample median.
    pub pbo: f64,
    pub combinations: usize,
    pub trials: usize,
    /// $\lambda_c = \ln(\omega_c / (1 - \omega_c))$ for each combination.
    pub logits: Vec<f64>,
}

/// Per column sums of one block of rows: count, sum and sum of squares.
#[derive(Clone, Copy, Default)]
struct Sums {
    n: f64,
    s: f64,
    q: f64,
}

fn sharpe_of(s: Sums) -> f64 {
    if s.n < 2.0 {
        return 0.0;
    }
    let mean = s.s / s.n;
    let var = (s.q - s.n * mean * mean) / (s.n - 1.0);
    if var <= 1e-18 { 0.0 } else { mean / var.sqrt() }
}

/// `matrix` has one row per period and one column per trial (net returns on a common calendar).
/// Rows are cut into `blocks` equal contiguous blocks (a remainder at the end is ignored).
/// Returns `None` when `blocks` is odd or below 2, there are fewer than 4 trials, or there are
/// too few rows.
pub fn pbo(matrix: &[Vec<f64>], blocks: usize) -> Option<PboResult> {
    let n_trials = matrix.first().map_or(0, Vec::len);
    if blocks < 2 || !blocks.is_multiple_of(2) || n_trials < 4 || matrix.len() < blocks * 2 {
        return None;
    }
    if matrix.iter().any(|r| r.len() != n_trials) {
        return None;
    }
    let per = matrix.len() / blocks;
    // block sums per trial
    let mut sums = vec![vec![Sums::default(); n_trials]; blocks];
    for (b, block_sums) in sums.iter_mut().enumerate() {
        for row in &matrix[b * per..(b + 1) * per] {
            for (c, x) in row.iter().enumerate() {
                block_sums[c].n += 1.0;
                block_sums[c].s += x;
                block_sums[c].q += x * x;
            }
        }
    }
    let half = blocks / 2;
    let mut logits = Vec::new();
    for mask in 0u64..(1u64 << blocks) {
        if mask.count_ones() as usize != half {
            continue;
        }
        let (mut is_sr, mut oos_sr) = (vec![0.0; n_trials], vec![0.0; n_trials]);
        for c in 0..n_trials {
            let (mut a, mut b) = (Sums::default(), Sums::default());
            for (blk, s) in sums.iter().enumerate() {
                let t = if mask >> blk & 1 == 1 { &mut a } else { &mut b };
                t.n += s[c].n;
                t.s += s[c].s;
                t.q += s[c].q;
            }
            is_sr[c] = sharpe_of(a);
            oos_sr[c] = sharpe_of(b);
        }
        // the in sample winner, ties to the lowest index
        let mut best = 0;
        for c in 1..n_trials {
            if is_sr[c] > is_sr[best] {
                best = c;
            }
        }
        // out of sample rank with average ranks for ties, 1 is the worst
        let below = oos_sr.iter().filter(|&&v| v < oos_sr[best]).count() as f64;
        let equal = oos_sr.iter().filter(|&&v| v == oos_sr[best]).count() as f64;
        let rank = below + (equal + 1.0) / 2.0;
        let omega = rank / (n_trials as f64 + 1.0);
        logits.push((omega / (1.0 - omega)).ln());
    }
    let combos = logits.len();
    let pbo = logits.iter().filter(|&&l| l <= 0.0).count() as f64 / combos as f64;
    Some(PboResult {
        pbo,
        combinations: combos,
        trials: n_trials,
        logits,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    fn noise(rows: usize, cols: usize, seed: u64) -> Vec<Vec<f64>> {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        (0..rows)
            .map(|_| (0..cols).map(|_| rng.random::<f64>() - 0.5).collect())
            .collect()
    }

    #[test]
    fn the_number_of_combinations_is_the_binomial_coefficient() {
        let r = pbo(&noise(160, 5, 1), 16).unwrap();
        assert_eq!(r.combinations, 12_870); // C(16, 8)
        let r = pbo(&noise(40, 5, 1), 8).unwrap();
        assert_eq!(r.combinations, 70); // C(8, 4)
    }

    #[test]
    fn pure_noise_gives_a_pbo_near_one_half() {
        let r = pbo(&noise(1600, 30, 7), 16).unwrap();
        assert!((r.pbo - 0.5).abs() < 0.12, "PBO {}", r.pbo);
    }

    #[test]
    fn one_dominant_trial_gives_zero_overfitting() {
        let mut m = noise(1600, 10, 3);
        for row in &mut m {
            row[4] += 0.6; // a column that wins in every block
        }
        let r = pbo(&m, 16).unwrap();
        assert_eq!(r.pbo, 0.0);
        assert!(r.logits.iter().all(|&l| l > 0.0));
    }

    #[test]
    fn undefined_cases_return_none() {
        assert!(pbo(&noise(200, 3, 1), 16).is_none()); // fewer than 4 trials
        assert!(pbo(&noise(200, 6, 1), 7).is_none()); // odd number of blocks
        assert!(pbo(&noise(10, 6, 1), 16).is_none()); // too few rows
        assert!(pbo(&[], 16).is_none());
    }
}
