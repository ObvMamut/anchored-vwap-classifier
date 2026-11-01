//! Cluster bootstrap: events of one session are resampled together, because touches inside a
//! session are not independent. ChaCha8 with the given seed, sequential, so the draws are
//! reproducible bit for bit.

use std::collections::BTreeMap;

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

/// Draws `draws` cluster resamples of the rows `0..clusters.len()` and returns the sorted values
/// of `stat` on each. `clusters[i]` is the cluster id of row `i`. `stat` receives the row indices
/// of one resample (clusters in drawn order, rows in their original order inside a cluster).
/// Draws where `stat` is not finite are dropped.
pub fn cluster_draws<F>(clusters: &[i32], draws: usize, seed: u64, stat: F) -> Vec<f64>
where
    F: Fn(&[usize]) -> f64,
{
    let mut groups: BTreeMap<i32, Vec<usize>> = BTreeMap::new();
    for (i, &c) in clusters.iter().enumerate() {
        groups.entry(c).or_default().push(i);
    }
    let groups: Vec<Vec<usize>> = groups.into_values().collect();
    let g = groups.len();
    if g == 0 {
        return Vec::new();
    }
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut out = Vec::with_capacity(draws);
    let mut rows: Vec<usize> = Vec::with_capacity(clusters.len());
    for _ in 0..draws {
        rows.clear();
        for _ in 0..g {
            let k = rng.random_range(0..g);
            if let Some(grp) = groups.get(k) {
                rows.extend_from_slice(grp);
            }
        }
        let v = stat(&rows);
        if v.is_finite() {
            out.push(v);
        }
    }
    out.sort_by(f64::total_cmp);
    out
}

/// Linear interpolation percentile of sorted values, `q` in `[0, 1]`. NaN for no values.
pub fn percentile(sorted: &[f64], q: f64) -> f64 {
    let n = sorted.len();
    if n == 0 {
        return f64::NAN;
    }
    let pos = q.clamp(0.0, 1.0) * (n - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    let a = sorted.get(lo).copied().unwrap_or(f64::NAN);
    let b = sorted.get(hi).copied().unwrap_or(f64::NAN);
    a + (b - a) * (pos - lo as f64)
}

/// Two sided percentile interval of sorted draws at `level` (for example 0.95).
pub fn interval(sorted: &[f64], level: f64) -> (f64, f64) {
    let tail = (1.0 - level) / 2.0;
    (percentile(sorted, tail), percentile(sorted, 1.0 - tail))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_matches_hand_values() {
        let v = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(percentile(&v, 0.0), 1.0);
        assert_eq!(percentile(&v, 0.5), 3.0);
        assert_eq!(percentile(&v, 1.0), 5.0);
        assert!((percentile(&v, 0.125) - 1.5).abs() < 1e-12);
        assert!(percentile(&[], 0.5).is_nan());
    }

    #[test]
    fn the_mean_of_a_cluster_bootstrap_is_centred_and_reproducible() {
        let clusters: Vec<i32> = (0..200).map(|i| i / 4).collect();
        let x: Vec<f64> = (0..200).map(|i| f64::from(i % 7)).collect();
        let mean = |rows: &[usize]| rows.iter().map(|&i| x[i]).sum::<f64>() / rows.len() as f64;
        let a = cluster_draws(&clusters, 400, 5, mean);
        let b = cluster_draws(&clusters, 400, 5, mean);
        assert_eq!(a, b);
        let full = x.iter().sum::<f64>() / 200.0;
        let centre = a.iter().sum::<f64>() / a.len() as f64;
        assert!((centre - full).abs() < 0.1);
        let (lo, hi) = interval(&a, 0.95);
        assert!(lo < full && full < hi);
    }

    #[test]
    fn rows_of_a_session_are_drawn_together() {
        // two clusters of different size: every resample holds whole clusters
        let clusters = [1, 1, 1, 2];
        let sizes = cluster_draws(&clusters, 50, 1, |rows| rows.len() as f64);
        assert!(sizes.iter().all(|&s| [2.0, 4.0, 6.0].contains(&s)));
    }
}
