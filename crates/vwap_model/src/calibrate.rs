//! Probability calibration: isotonic regression by pool adjacent violators, and Platt scaling.
//! Each class score is calibrated one versus rest, then the rows are renormalised.
//! See `docs/MODELING.md`, section 3.

use vwap_core::StrategyConfig;

use crate::matrix::Probs;

/// Weighted pool adjacent violators. `y` is in the order of increasing score. Returns the
/// non decreasing fit that minimises $\sum_i w_i(y_i-f_i)^2$.
pub fn pav(y: &[f64], w: &[f64]) -> Vec<f64> {
    // each block holds (weighted sum of y, total weight, length)
    let mut blocks: Vec<(f64, f64, usize)> = Vec::with_capacity(y.len());
    for (&yi, &wi) in y.iter().zip(w) {
        blocks.push((yi * wi, wi, 1));
        while blocks.len() >= 2 {
            let n = blocks.len();
            let (s2, w2, l2) = blocks.get(n - 1).copied().unwrap_or_default();
            let (s1, w1, l1) = blocks.get(n - 2).copied().unwrap_or_default();
            if s1 / w1 > s2 / w2 {
                blocks.truncate(n - 2);
                blocks.push((s1 + s2, w1 + w2, l1 + l2));
            } else {
                break;
            }
        }
    }
    let mut out = Vec::with_capacity(y.len());
    for (s, wt, len) in blocks {
        out.extend(std::iter::repeat_n(s / wt, len));
    }
    out
}

/// Isotonic calibrator: linear interpolation between the block means, clamped outside.
#[derive(Debug, Clone, PartialEq)]
pub struct Isotonic {
    xs: Vec<f64>,
    ys: Vec<f64>,
    eps: f64,
}

impl Isotonic {
    /// Fits scores against binary targets. Equal scores are merged first.
    pub fn fit(scores: &[f64], targets: &[f64], eps: f64) -> Self {
        let mut rows: Vec<(f64, f64)> = scores
            .iter()
            .copied()
            .zip(targets.iter().copied())
            .collect();
        rows.sort_by(|a, b| a.0.total_cmp(&b.0));
        // merge ties: (score, mean target, weight)
        let mut groups: Vec<(f64, f64, f64)> = Vec::new();
        for (x, t) in rows {
            match groups.last_mut() {
                Some(g) if g.0 == x => {
                    g.1 = (g.1 * g.2 + t) / (g.2 + 1.0);
                    g.2 += 1.0;
                }
                _ => groups.push((x, t, 1.0)),
            }
        }
        let y: Vec<f64> = groups.iter().map(|g| g.1).collect();
        let w: Vec<f64> = groups.iter().map(|g| g.2).collect();
        let fit = pav(&y, &w);
        // knots at the weighted mean score of each run of equal fitted values
        let mut xs: Vec<f64> = Vec::new();
        let mut ys: Vec<f64> = Vec::new();
        let mut i = 0;
        while i < fit.len() {
            let v = fit.get(i).copied().unwrap_or(f64::NAN);
            let mut j = i;
            let (mut sx, mut sw) = (0.0, 0.0);
            while j < fit.len() && fit.get(j).copied() == Some(v) {
                let (gx, _, gw) = groups.get(j).copied().unwrap_or_default();
                sx += gx * gw;
                sw += gw;
                j += 1;
            }
            xs.push(sx / sw);
            ys.push(v);
            i = j;
        }
        Self { xs, ys, eps }
    }

    pub fn predict(&self, s: f64) -> f64 {
        let n = self.xs.len();
        let v = if n == 0 {
            0.5
        } else {
            let k = self.xs.partition_point(|&x| x < s);
            if k == 0 {
                self.ys.first().copied().unwrap_or(0.5)
            } else if k >= n {
                self.ys.last().copied().unwrap_or(0.5)
            } else {
                match (self.xs.get(k - 1..=k), self.ys.get(k - 1..=k)) {
                    (Some([x0, x1]), Some([y0, y1])) => y0 + (y1 - y0) * (s - x0) / (x1 - x0),
                    _ => 0.5,
                }
            }
        };
        v.clamp(self.eps, 1.0 - self.eps)
    }
}

fn logit(p: f64, eps: f64) -> f64 {
    let p = p.clamp(eps, 1.0 - eps);
    (p / (1.0 - p)).ln()
}

/// Platt scaling $g(q)=1/(1+e^{aq+b})$ on the logit $q$ of the raw probability.
#[derive(Debug, Clone, PartialEq)]
pub struct Platt {
    pub a: f64,
    pub b: f64,
    eps: f64,
}

impl Platt {
    /// Newton iterations with the Platt target smoothing
    /// $t_+=\frac{N_++1}{N_++2}$, $t_-=\frac1{N_-+2}$ and backtracking on the loss.
    pub fn fit(scores: &[f64], targets: &[f64], eps: f64) -> Self {
        let q: Vec<f64> = scores.iter().map(|&s| logit(s, eps)).collect();
        let n_pos = targets.iter().filter(|&&t| t > 0.5).count() as f64;
        let n_neg = targets.len() as f64 - n_pos;
        let t_pos = (n_pos + 1.0) / (n_pos + 2.0);
        let t_neg = 1.0 / (n_neg + 2.0);
        let t: Vec<f64> = targets
            .iter()
            .map(|&x| if x > 0.5 { t_pos } else { t_neg })
            .collect();
        let loss = |a: f64, b: f64| -> f64 {
            q.iter()
                .zip(&t)
                .map(|(&qi, &ti)| {
                    let z = a * qi + b;
                    // -t ln p - (1-t) ln(1-p) with p = 1/(1+e^z), stable form
                    let lp = -softplus(z);
                    let l1p = -softplus(-z);
                    -(ti * lp + (1.0 - ti) * l1p)
                })
                .sum()
        };
        let (mut a, mut b) = (-1.0, 0.0);
        let mut cur = loss(a, b);
        for _ in 0..100 {
            let (mut ga, mut gb, mut haa, mut hab, mut hbb) = (0.0, 0.0, 0.0, 0.0, 0.0);
            for (&qi, &ti) in q.iter().zip(&t) {
                let p = 1.0 / (1.0 + (a * qi + b).exp());
                let d = ti - p;
                let h = p * (1.0 - p);
                ga += d * qi;
                gb += d;
                haa += h * qi * qi;
                hab += h * qi;
                hbb += h;
            }
            let ridge = 1e-9;
            let (haa, hbb) = (haa + ridge, hbb + ridge);
            let det = haa * hbb - hab * hab;
            if !det.is_finite() || det.abs() < 1e-18 {
                break;
            }
            // gradient of the loss is (ga, gb) with g = sum (t - p) (q, 1); Newton: theta <- theta - H^{-1} g
            let da = (hbb * ga - hab * gb) / det;
            let db = (-hab * ga + haa * gb) / det;
            let mut step = 1.0;
            let mut accepted = false;
            for _ in 0..30 {
                let (na, nb) = (a - step * da, b - step * db);
                let l = loss(na, nb);
                if l.is_finite() && l <= cur + 1e-12 {
                    let moved = (na - a).abs() + (nb - b).abs();
                    (a, b, cur) = (na, nb, l);
                    accepted = moved > 1e-10;
                    break;
                }
                step *= 0.5;
            }
            if !accepted {
                break;
            }
        }
        Self { a, b, eps }
    }

    pub fn predict(&self, s: f64) -> f64 {
        let z = self.a * logit(s, self.eps) + self.b;
        (1.0 / (1.0 + z.exp())).clamp(self.eps, 1.0 - self.eps)
    }
}

fn softplus(z: f64) -> f64 {
    if z > 0.0 {
        z + (-z).exp().ln_1p()
    } else {
        z.exp().ln_1p()
    }
}

/// A one versus rest calibrator for one class score.
#[derive(Debug, Clone, PartialEq)]
pub enum Calibrator {
    Isotonic(Isotonic),
    Platt(Platt),
    Identity,
}

impl Calibrator {
    pub fn predict(&self, s: f64) -> f64 {
        match self {
            Calibrator::Isotonic(m) => m.predict(s),
            Calibrator::Platt(m) => m.predict(s),
            Calibrator::Identity => s,
        }
    }
}

/// Calibrators for the three classes with row renormalisation.
#[derive(Debug, Clone, PartialEq)]
pub struct MultiCalibrator {
    pub per_class: [Calibrator; 3],
}

impl MultiCalibrator {
    /// Fits on out of fold probabilities. Isotonic when there are at least
    /// `cfg.isotonic_min_events` events, Platt below that, identity with no events.
    pub fn fit(p: &Probs, y: &[usize], cfg: &StrategyConfig) -> Self {
        let n = p.len().min(y.len());
        let make = |k: usize| -> Calibrator {
            if n == 0 {
                return Calibrator::Identity;
            }
            let scores: Vec<f64> = p
                .iter()
                .take(n)
                .map(|r| r.get(k).copied().unwrap_or(0.0))
                .collect();
            let targets: Vec<f64> = y
                .iter()
                .take(n)
                .map(|&c| if c == k { 1.0 } else { 0.0 })
                .collect();
            if n >= cfg.isotonic_min_events {
                Calibrator::Isotonic(Isotonic::fit(&scores, &targets, cfg.clip_eps))
            } else {
                Calibrator::Platt(Platt::fit(&scores, &targets, cfg.clip_eps))
            }
        };
        Self {
            per_class: [make(0), make(1), make(2)],
        }
    }

    /// Calibrated, renormalised probabilities.
    pub fn transform(&self, p: &Probs) -> Probs {
        p.iter()
            .map(|r| {
                let mut g = [0.0; 3];
                for (k, (gk, c)) in g.iter_mut().zip(&self.per_class).enumerate() {
                    *gk = c.predict(r.get(k).copied().unwrap_or(0.0));
                }
                let s: f64 = g.iter().sum();
                if s > 0.0 && s.is_finite() {
                    g.map(|x| x / s)
                } else {
                    [1.0 / 3.0; 3]
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    fn cfg() -> StrategyConfig {
        vwap_core::load_strategy_config(std::path::Path::new("../../config")).unwrap()
    }

    #[test]
    fn pav_matches_the_reference_examples() {
        let ones = [1.0; 6];
        assert_eq!(
            pav(&[1.0, 3.0, 2.0, 4.0, 3.0, 5.0], &ones),
            vec![1.0, 2.5, 2.5, 3.5, 3.5, 5.0]
        );
        assert_eq!(pav(&[5.0, 4.0, 3.0, 2.0, 1.0], &[1.0; 5]), vec![3.0; 5]);
        assert_eq!(pav(&[2.0, 1.0], &[1.0, 3.0]), vec![1.25, 1.25]);
    }

    #[test]
    fn pav_is_monotone_and_preserves_the_weighted_mean() {
        let mut rng = ChaCha8Rng::seed_from_u64(3);
        for _ in 0..50 {
            let n = rng.random_range(1..60);
            let y: Vec<f64> = (0..n).map(|_| rng.random::<f64>()).collect();
            let w: Vec<f64> = (0..n).map(|_| 0.5 + rng.random::<f64>()).collect();
            let f = pav(&y, &w);
            assert!(f.windows(2).all(|p| p[0] <= p[1] + 1e-12));
            let a: f64 = y.iter().zip(&w).map(|(a, b)| a * b).sum();
            let b: f64 = f.iter().zip(&w).map(|(a, b)| a * b).sum();
            assert!((a - b).abs() < 1e-9);
        }
    }

    #[test]
    fn isotonic_interpolates_clamps_and_clips() {
        let s = [0.1, 0.2, 0.3, 0.4];
        let t = [0.0, 0.0, 1.0, 1.0];
        let m = Isotonic::fit(&s, &t, 1e-6);
        assert!((m.predict(0.0) - 1e-6).abs() < 1e-12);
        assert!((m.predict(0.9) - (1.0 - 1e-6)).abs() < 1e-12);
        let mid = m.predict(0.25);
        assert!(mid > 0.1 && mid < 0.9);
        assert!(m.predict(0.15) <= m.predict(0.25));
    }

    #[test]
    fn isotonic_merges_tied_scores() {
        let m = Isotonic::fit(&[0.5, 0.5, 0.5, 0.5], &[1.0, 0.0, 0.0, 0.0], 1e-6);
        assert!((m.predict(0.5) - 0.25).abs() < 1e-12);
    }

    #[test]
    fn platt_recovers_planted_parameters() {
        let mut rng = ChaCha8Rng::seed_from_u64(11);
        let (ta, tb) = (-1.5, 0.3);
        let n = 30_000;
        let mut scores = Vec::new();
        let mut targets = Vec::new();
        for _ in 0..n {
            let q: f64 = rng.random::<f64>() * 6.0 - 3.0;
            let p_true = 1.0 / (1.0 + (ta * q + tb).exp());
            targets.push(if rng.random::<f64>() < p_true {
                1.0
            } else {
                0.0
            });
            scores.push(1.0 / (1.0 + (-q).exp()));
        }
        let m = Platt::fit(&scores, &targets, 1e-6);
        assert!((m.a - ta).abs() < 0.08, "a = {}", m.a);
        assert!((m.b - tb).abs() < 0.08, "b = {}", m.b);
    }

    #[test]
    fn calibrated_rows_sum_to_one_and_stay_in_the_unit_interval() {
        let mut rng = ChaCha8Rng::seed_from_u64(5);
        let c = cfg();
        for n in [50usize, 1500] {
            let p: Probs = (0..n)
                .map(|_| {
                    let a = rng.random::<f64>();
                    let b = rng.random::<f64>() * (1.0 - a);
                    [a, b, 1.0 - a - b]
                })
                .collect();
            let y: Vec<usize> = (0..n).map(|_| rng.random_range(0..3)).collect();
            let m = MultiCalibrator::fit(&p, &y, &c);
            let expect_iso = n >= c.isotonic_min_events;
            assert_eq!(
                matches!(m.per_class[0], Calibrator::Isotonic(_)),
                expect_iso
            );
            for r in m.transform(&p) {
                assert!((r.iter().sum::<f64>() - 1.0).abs() < 1e-12);
                assert!(r.iter().all(|&x| (0.0..=1.0).contains(&x)));
            }
        }
    }

    #[test]
    fn calibration_repairs_an_overconfident_forecast() {
        let mut rng = ChaCha8Rng::seed_from_u64(9);
        let n = 4000;
        let c = cfg();
        // true probability 0.5 for class 0 whatever the claim of 0.9
        let p: Probs = vec![[0.9, 0.05, 0.05]; n];
        let y: Vec<usize> = (0..n)
            .map(|_| if rng.random::<f64>() < 0.5 { 0 } else { 1 })
            .collect();
        let m = MultiCalibrator::fit(&p, &y, &c);
        let out = m.transform(&p[..1].to_vec());
        assert!((out[0][0] - 0.5).abs() < 0.06, "{:?}", out[0]);
    }
}
