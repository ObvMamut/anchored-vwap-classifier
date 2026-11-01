//! Feature importance summaries and their stability across folds. See `docs/MODELING.md`,
//! section 7. Importances are for reporting only and never feed back into a study.

/// Average ranks (ties share the mean rank), starting at 1.
fn ranks(v: &[f64]) -> Vec<f64> {
    let mut idx: Vec<usize> = (0..v.len()).collect();
    idx.sort_by(|&a, &b| {
        v.get(a)
            .copied()
            .unwrap_or(f64::NAN)
            .total_cmp(&v.get(b).copied().unwrap_or(f64::NAN))
    });
    let mut out = vec![0.0; v.len()];
    let mut i = 0;
    while i < idx.len() {
        let key = v.get(idx.get(i).copied().unwrap_or(0)).copied();
        let mut j = i;
        while j < idx.len() && v.get(idx.get(j).copied().unwrap_or(0)).copied() == key {
            j += 1;
        }
        let r = (i + 1 + j) as f64 / 2.0;
        for &k in idx.get(i..j).unwrap_or(&[]) {
            if let Some(o) = out.get_mut(k) {
                *o = r;
            }
        }
        i = j;
    }
    out
}

/// Spearman rank correlation. NaN when either vector is constant or the lengths differ.
pub fn spearman(a: &[f64], b: &[f64]) -> f64 {
    if a.len() != b.len() || a.len() < 2 {
        return f64::NAN;
    }
    let (ra, rb) = (ranks(a), ranks(b));
    let n = a.len() as f64;
    let (ma, mb) = (ra.iter().sum::<f64>() / n, rb.iter().sum::<f64>() / n);
    let (mut sab, mut saa, mut sbb) = (0.0, 0.0, 0.0);
    for (x, y) in ra.iter().zip(&rb) {
        sab += (x - ma) * (y - mb);
        saa += (x - ma) * (x - ma);
        sbb += (y - mb) * (y - mb);
    }
    if saa == 0.0 || sbb == 0.0 {
        f64::NAN
    } else {
        sab / (saa * sbb).sqrt()
    }
}

/// Indices of the `m` largest values, ties broken by the lower index.
pub fn top_m(v: &[f64], m: usize) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..v.len()).collect();
    idx.sort_by(|&a, &b| {
        v.get(b)
            .copied()
            .unwrap_or(f64::NAN)
            .total_cmp(&v.get(a).copied().unwrap_or(f64::NAN))
            .then(a.cmp(&b))
    });
    idx.truncate(m);
    idx
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stability {
    /// Mean Spearman correlation over fold pairs.
    pub mean_rank_correlation: f64,
    /// Mean overlap $|\mathrm{top}_m(a)\cap\mathrm{top}_m(b)|/m$ over fold pairs.
    pub mean_top_overlap: f64,
    /// For each feature, the share of folds in which it is in the top `m`.
    pub top_share: Vec<f64>,
}

/// `per_fold[k][f]` is the importance of feature `f` in fold `k`.
pub fn stability(per_fold: &[Vec<f64>], m: usize) -> Stability {
    let n_feat = per_fold.first().map_or(0, Vec::len);
    let (mut rho, mut ov, mut pairs) = (0.0, 0.0, 0usize);
    let mut rho_n = 0usize;
    for (i, a) in per_fold.iter().enumerate() {
        for b in per_fold.iter().skip(i + 1) {
            let r = spearman(a, b);
            if r.is_finite() {
                rho += r;
                rho_n += 1;
            }
            let (ta, tb) = (top_m(a, m), top_m(b, m));
            let common = ta.iter().filter(|f| tb.contains(f)).count();
            ov += common as f64 / m.max(1) as f64;
            pairs += 1;
        }
    }
    let mut share = vec![0.0; n_feat];
    for f in per_fold {
        for k in top_m(f, m) {
            if let Some(s) = share.get_mut(k) {
                *s += 1.0;
            }
        }
    }
    let folds = per_fold.len().max(1) as f64;
    Stability {
        mean_rank_correlation: if rho_n == 0 {
            f64::NAN
        } else {
            rho / rho_n as f64
        },
        mean_top_overlap: if pairs == 0 {
            f64::NAN
        } else {
            ov / pairs as f64
        },
        top_share: share.into_iter().map(|s| s / folds).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spearman_matches_hand_values() {
        assert!((spearman(&[1.0, 2.0, 3.0, 4.0], &[10.0, 20.0, 30.0, 40.0]) - 1.0).abs() < 1e-12);
        assert!((spearman(&[1.0, 2.0, 3.0, 4.0], &[4.0, 3.0, 2.0, 1.0]) + 1.0).abs() < 1e-12);
        // ranks (1,2,3,4) and (2,1,4,3): d^2 = 4, rho = 1 - 6*4/(4*15) = 0.6
        assert!((spearman(&[1.0, 2.0, 3.0, 4.0], &[2.0, 1.0, 4.0, 3.0]) - 0.6).abs() < 1e-12);
        assert!(spearman(&[1.0, 1.0, 1.0], &[1.0, 2.0, 3.0]).is_nan());
    }

    #[test]
    fn ties_share_the_mean_rank() {
        assert_eq!(ranks(&[5.0, 1.0, 5.0, 3.0]), vec![3.5, 1.0, 3.5, 2.0]);
    }

    #[test]
    fn identical_folds_are_perfectly_stable_and_reversed_ones_are_not() {
        let a = vec![5.0, 4.0, 3.0, 2.0, 1.0];
        let s = stability(&[a.clone(), a.clone(), a.clone()], 2);
        assert!((s.mean_rank_correlation - 1.0).abs() < 1e-12);
        assert!((s.mean_top_overlap - 1.0).abs() < 1e-12);
        assert_eq!(s.top_share, vec![1.0, 1.0, 0.0, 0.0, 0.0]);
        let r = stability(&[a.clone(), a.iter().rev().cloned().collect()], 2);
        assert!((r.mean_rank_correlation + 1.0).abs() < 1e-12);
        assert_eq!(r.mean_top_overlap, 0.0);
    }

    #[test]
    fn top_m_breaks_ties_by_index() {
        assert_eq!(top_m(&[1.0, 3.0, 3.0, 2.0], 2), vec![1, 2]);
    }
}
