//! Scoring rules and calibration diagnostics. See `docs/MODELING.md`, sections 3 and 4.
//! Labels are class indices 0, 1, 2. Every function takes probabilities and labels of equal
//! length; a length mismatch scores the common prefix.

use crate::matrix::Probs;

/// Probabilities are clipped here before a logarithm.
const LOG_FLOOR: f64 = 1e-12;

/// Multiclass log loss $-\frac1N\sum_n\ln \hat p_{n,y_n}$.
pub fn log_loss(p: &Probs, y: &[usize]) -> f64 {
    let n = p.len().min(y.len());
    if n == 0 {
        return f64::NAN;
    }
    let s: f64 = p
        .iter()
        .zip(y)
        .map(|(r, &c)| -r.get(c).copied().unwrap_or(0.0).max(LOG_FLOOR).ln())
        .sum();
    s / n as f64
}

/// Multiclass Brier score $\frac1N\sum_n\sum_k(\hat p_{nk}-y_{nk})^2$.
pub fn brier(p: &Probs, y: &[usize]) -> f64 {
    let n = p.len().min(y.len());
    if n == 0 {
        return f64::NAN;
    }
    let s: f64 = p
        .iter()
        .zip(y)
        .map(|(r, &c)| {
            r.iter()
                .enumerate()
                .map(|(k, &q)| {
                    let t = if k == c { 1.0 } else { 0.0 };
                    (q - t) * (q - t)
                })
                .sum::<f64>()
        })
        .sum();
    s / n as f64
}

/// Brier skill score $1-\mathrm{BS}/\mathrm{BS}_{\text{base}}$.
pub fn bss(bs: f64, bs_base: f64) -> f64 {
    1.0 - bs / bs_base
}

/// One bin of a reliability diagram.
#[derive(Debug, Clone, PartialEq)]
pub struct Bin {
    pub n: usize,
    pub mean_pred: f64,
    pub freq: f64,
}

/// Quantile bins of the predicted probability of `class`: rows sorted by score and cut into
/// `bins` groups of nearly equal size.
pub fn reliability(p: &Probs, y: &[usize], class: usize, bins: usize) -> Vec<Bin> {
    let n = p.len().min(y.len());
    if n == 0 || bins == 0 {
        return Vec::new();
    }
    let mut rows: Vec<(f64, f64)> = p
        .iter()
        .zip(y)
        .map(|(r, &c)| {
            (
                r.get(class).copied().unwrap_or(0.0),
                if c == class { 1.0 } else { 0.0 },
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    let nb = bins.min(n);
    (0..nb)
        .filter_map(|b| {
            let lo = b * n / nb;
            let hi = (b + 1) * n / nb;
            let part = rows.get(lo..hi)?;
            let m = part.len() as f64;
            (m > 0.0).then(|| Bin {
                n: part.len(),
                mean_pred: part.iter().map(|r| r.0).sum::<f64>() / m,
                freq: part.iter().map(|r| r.1).sum::<f64>() / m,
            })
        })
        .collect()
}

/// Expected calibration error of one class, $\sum_b\frac{|I_b|}{N}|\bar y_b-\bar p_b|$.
pub fn ece(p: &Probs, y: &[usize], class: usize, bins: usize) -> f64 {
    let n = p.len().min(y.len());
    if n == 0 {
        return f64::NAN;
    }
    reliability(p, y, class, bins)
        .iter()
        .map(|b| b.n as f64 / n as f64 * (b.freq - b.mean_pred).abs())
        .sum()
}

/// Area under the ROC curve of `score` for the positives, with ties counted one half
/// (Mann Whitney). NaN when either side is empty.
pub fn auc(score: &[f64], positive: &[bool]) -> f64 {
    let n = score.len().min(positive.len());
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&a, &b| {
        score
            .get(a)
            .copied()
            .unwrap_or(f64::NAN)
            .total_cmp(&score.get(b).copied().unwrap_or(f64::NAN))
    });
    let mut rank_sum = 0.0;
    let mut n_pos = 0usize;
    let mut i = 0;
    while i < n {
        let si = score.get(idx.get(i).copied().unwrap_or(0)).copied();
        let mut j = i;
        while j < n && score.get(idx.get(j).copied().unwrap_or(0)).copied() == si {
            j += 1;
        }
        let avg_rank = (i + 1 + j) as f64 / 2.0;
        for &k in idx.get(i..j).unwrap_or(&[]) {
            if positive.get(k).copied().unwrap_or(false) {
                rank_sum += avg_rank;
                n_pos += 1;
            }
        }
        i = j;
    }
    let n_neg = n - n_pos;
    if n_pos == 0 || n_neg == 0 {
        return f64::NAN;
    }
    (rank_sum - n_pos as f64 * (n_pos as f64 + 1.0) / 2.0) / (n_pos as f64 * n_neg as f64)
}

/// Precision of the rule "predicted rejection probability above `tau`" for class 0 and the
/// number of selected rows. NaN precision when nothing is selected.
pub fn precision_at(p: &Probs, y: &[usize], tau: f64) -> (f64, usize) {
    let mut sel = 0usize;
    let mut hit = 0usize;
    for (r, &c) in p.iter().zip(y) {
        if r.first().copied().unwrap_or(0.0) > tau {
            sel += 1;
            hit += usize::from(c == 0);
        }
    }
    if sel == 0 {
        (f64::NAN, 0)
    } else {
        (hit as f64 / sel as f64, sel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_loss_and_brier_match_hand_values() {
        let p = vec![[0.7, 0.2, 0.1], [0.1, 0.8, 0.1]];
        let y = [0, 1];
        let ll = -(0.7f64.ln() + 0.8f64.ln()) / 2.0;
        assert!((log_loss(&p, &y) - ll).abs() < 1e-12);
        // row 1: 0.09 + 0.04 + 0.01, row 2: 0.01 + 0.04 + 0.01
        let bs = (0.14 + 0.06) / 2.0;
        assert!((brier(&p, &y) - bs).abs() < 1e-12);
    }

    #[test]
    fn a_zero_probability_on_the_true_class_is_clipped_not_infinite() {
        let p = vec![[0.0, 1.0, 0.0]];
        assert!(log_loss(&p, &[0]).is_finite());
        assert!((log_loss(&p, &[0]) + (1e-12f64).ln()).abs() < 1e-9);
    }

    #[test]
    fn the_base_rate_forecast_has_zero_skill_and_a_better_forecast_positive_skill() {
        let y = [0, 0, 1, 2, 0, 1];
        let base = vec![[0.5, 1.0 / 3.0, 1.0 / 6.0]; 6];
        let b0 = brier(&base, &y);
        assert!(bss(b0, b0).abs() < 1e-15);
        let sharp: Probs = y
            .iter()
            .map(|&c| {
                let mut r = [0.05; 3];
                r[c] = 0.9;
                r
            })
            .collect();
        assert!(bss(brier(&sharp, &y), b0) > 0.5);
    }

    #[test]
    fn ece_is_zero_for_a_calibrated_forecast_and_the_gap_for_a_constant_one() {
        // 10 rows predict 0.3 and 3 of them are class 0: calibrated
        let mut p: Probs = vec![[0.3, 0.4, 0.3]; 10];
        let y = [0, 0, 0, 1, 1, 1, 1, 2, 2, 2];
        assert!(ece(&p, &y, 0, 1).abs() < 1e-12);
        // claim 0.8 for class 0 instead: gap 0.5
        for r in &mut p {
            r[0] = 0.8;
        }
        assert!((ece(&p, &y, 0, 1) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn reliability_bins_are_ordered_and_cover_every_row() {
        let p: Probs = (0..20).map(|i| [f64::from(i) / 20.0, 0.0, 0.0]).collect();
        let y: Vec<usize> = (0..20).map(|i| usize::from(i >= 10)).collect();
        let bins = reliability(&p, &y, 0, 4);
        assert_eq!(bins.len(), 4);
        assert_eq!(bins.iter().map(|b| b.n).sum::<usize>(), 20);
        assert!(bins.windows(2).all(|w| w[0].mean_pred <= w[1].mean_pred));
    }

    #[test]
    fn auc_matches_the_pairwise_definition_with_ties() {
        // positives 0.8, 0.4; negatives 0.4, 0.2: pairs (0.8>0.4, 0.8>0.2, 0.4=0.4, 0.4>0.2)
        let s = [0.8, 0.4, 0.4, 0.2];
        let pos = [true, true, false, false];
        assert!((auc(&s, &pos) - 3.5 / 4.0).abs() < 1e-12);
        assert!(auc(&s, &[true; 4]).is_nan());
    }

    #[test]
    fn precision_counts_only_selected_rows() {
        let p = vec![[0.9, 0.1, 0.0], [0.6, 0.2, 0.2], [0.2, 0.4, 0.4]];
        let y = [0, 1, 0];
        assert_eq!(precision_at(&p, &y, 0.5), (0.5, 2));
        assert_eq!(precision_at(&p, &y, 0.95).1, 0);
        assert!(precision_at(&p, &y, 0.95).0.is_nan());
    }
}
