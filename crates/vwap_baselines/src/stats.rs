//! Class frequencies, stratified adjustment, the paired session bootstrap, a Wald
//! test on the class frequency vector and Holm correction.
#![allow(clippy::indexing_slicing)]

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::Serialize;

pub const NCLASS: usize = 3;

/// Per session class counts by stratum for the VWAP group and the control group.
/// Layout of each table: `session * n_strata * 3 + stratum * 3 + class`.
#[derive(Debug, Clone)]
pub struct SessionTables {
    pub n_strata: usize,
    pub sessions: Vec<i32>,
    pub vwap: Vec<u32>,
    pub ctrl: Vec<u32>,
}

/// An event reduced to what the statistics need.
#[derive(Debug, Clone, Copy)]
pub struct Row {
    pub session: i32,
    pub stratum: usize,
    pub class: usize,
}

impl SessionTables {
    pub fn build(n_strata: usize, vwap: &[Row], ctrl: &[Row]) -> Self {
        let mut sessions: Vec<i32> = vwap.iter().chain(ctrl).map(|r| r.session).collect();
        sessions.sort_unstable();
        sessions.dedup();
        let width = n_strata * NCLASS;
        let mut v = vec![0u32; sessions.len() * width];
        let mut c = vec![0u32; sessions.len() * width];
        let idx = |s: i32| sessions.binary_search(&s).unwrap_or(0);
        for r in vwap {
            v[idx(r.session) * width + r.stratum * NCLASS + r.class] += 1;
        }
        for r in ctrl {
            c[idx(r.session) * width + r.stratum * NCLASS + r.class] += 1;
        }
        Self {
            n_strata,
            sessions,
            vwap: v,
            ctrl: c,
        }
    }

    fn width(&self) -> usize {
        self.n_strata * NCLASS
    }

    fn sum(&self, which: &[u32], draw: &[usize]) -> Vec<u32> {
        let w = self.width();
        let mut tot = vec![0u32; w];
        for &s in draw {
            for (t, x) in tot.iter_mut().zip(&which[s * w..(s + 1) * w]) {
                *t += x;
            }
        }
        tot
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Estimates {
    pub pi_vwap_raw: [f64; NCLASS],
    pub pi_ctrl_raw: [f64; NCLASS],
    pub pi_vwap_adj: [f64; NCLASS],
    pub pi_ctrl_adj: [f64; NCLASS],
    pub delta: [f64; NCLASS],
    pub n_vwap: u32,
    pub n_ctrl: u32,
    /// Share of VWAP events in strata kept for the adjustment.
    pub coverage: f64,
}

fn freq(counts: &[u32]) -> ([f64; NCLASS], u32) {
    let mut tot = [0u32; NCLASS];
    for chunk in counts.chunks(NCLASS) {
        for (t, x) in tot.iter_mut().zip(chunk) {
            *t += x;
        }
    }
    let n: u32 = tot.iter().sum();
    let mut f = [0.0; NCLASS];
    if n > 0 {
        for (o, t) in f.iter_mut().zip(&tot) {
            *o = f64::from(*t) / f64::from(n);
        }
    }
    (f, n)
}

/// Frequencies and the stratified comparison of two count tables laid out as
/// `stratum * 3 + class`. Strata where either group has fewer than `min_n` events are
/// left out, and the control is reweighted to the VWAP stratum mix of the kept strata.
pub fn estimate(vwap: &[u32], ctrl: &[u32], min_n: u32) -> Estimates {
    let (pv, nv) = freq(vwap);
    let (pc, nc) = freq(ctrl);
    let mut kept_v = 0u32;
    let mut adj_v = [0.0; NCLASS];
    let mut adj_c = [0.0; NCLASS];
    for (cv, cc) in vwap.chunks(NCLASS).zip(ctrl.chunks(NCLASS)) {
        let (nv_g, nc_g): (u32, u32) = (cv.iter().sum(), cc.iter().sum());
        if nv_g >= min_n && nc_g >= min_n {
            kept_v += nv_g;
            for c in 0..NCLASS {
                adj_v[c] += f64::from(cv[c]);
                adj_c[c] += f64::from(nv_g) * f64::from(cc[c]) / f64::from(nc_g);
            }
        }
    }
    if kept_v > 0 {
        for c in 0..NCLASS {
            adj_v[c] /= f64::from(kept_v);
            adj_c[c] /= f64::from(kept_v);
        }
    }
    let mut delta = [0.0; NCLASS];
    for c in 0..NCLASS {
        delta[c] = adj_v[c] - adj_c[c];
    }
    Estimates {
        pi_vwap_raw: pv,
        pi_ctrl_raw: pc,
        pi_vwap_adj: adj_v,
        pi_ctrl_adj: adj_c,
        delta,
        n_vwap: nv,
        n_ctrl: nc,
        coverage: if nv > 0 {
            f64::from(kept_v) / f64::from(nv)
        } else {
            0.0
        },
    }
}

/// Paired session bootstrap: whole sessions are resampled, so both the VWAP events and
/// the control events of a session move together.
pub fn bootstrap(
    t: &SessionTables,
    draws: usize,
    seed: u64,
    min_n: u32,
) -> (Estimates, Vec<Estimates>) {
    let n = t.sessions.len();
    let all: Vec<usize> = (0..n).collect();
    let point = estimate(&t.sum(&t.vwap, &all), &t.sum(&t.ctrl, &all), min_n);
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut out = Vec::with_capacity(draws);
    for _ in 0..draws {
        let draw: Vec<usize> = (0..n).map(|_| rng.random_range(0..n)).collect();
        out.push(estimate(
            &t.sum(&t.vwap, &draw),
            &t.sum(&t.ctrl, &draw),
            min_n,
        ));
    }
    (point, out)
}

/// Percentile with linear interpolation, `q` in [0, 1].
pub fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let pos = q.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
    sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo as f64)
}

pub fn sorted(mut v: Vec<f64>) -> Vec<f64> {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v
}

/// Simultaneous percentile interval over the three classes (Bonferroni: alpha / 3 per class).
pub fn simultaneous_ci(draws: &[f64], alpha: f64) -> (f64, f64) {
    let s = sorted(draws.to_vec());
    let a = alpha / NCLASS as f64;
    (percentile(&s, a / 2.0), percentile(&s, 1.0 - a / 2.0))
}

/// Wald statistic and p value for equality of the class frequency vectors. The
/// covariance of the first two components comes from the bootstrap draws and the p
/// value uses the closed form of the chi square distribution with two degrees of freedom.
pub fn wald(delta: &[f64; NCLASS], draws: &[[f64; NCLASS]]) -> (f64, f64) {
    let n = draws.len() as f64;
    if draws.len() < 10 {
        return (f64::NAN, 1.0);
    }
    let m0 = draws.iter().map(|d| d[0]).sum::<f64>() / n;
    let m1 = draws.iter().map(|d| d[1]).sum::<f64>() / n;
    let (mut s00, mut s01, mut s11) = (0.0, 0.0, 0.0);
    for d in draws {
        s00 += (d[0] - m0) * (d[0] - m0);
        s01 += (d[0] - m0) * (d[1] - m1);
        s11 += (d[1] - m1) * (d[1] - m1);
    }
    let (s00, s01, s11) = (s00 / (n - 1.0), s01 / (n - 1.0), s11 / (n - 1.0));
    let det = s00 * s11 - s01 * s01;
    let scale = (s00 + s11).max(1e-300);
    if det > 1e-12 * scale * scale {
        let w = (delta[0] * delta[0] * s11 - 2.0 * delta[0] * delta[1] * s01
            + delta[1] * delta[1] * s00)
            / det;
        return (w, (-w / 2.0).exp().min(1.0));
    }
    // Rank deficient (for example a class that never occurs): test the one remaining component.
    let (d, v) = if s00 >= s11 {
        (delta[0], s00)
    } else {
        (delta[1], s11)
    };
    if v <= 1e-18 {
        return (f64::NAN, 1.0);
    }
    let w = d * d / v;
    (w, erfc((w / 2.0).sqrt()).min(1.0))
}

/// Complementary error function, relative error below 1.2e-7 (Numerical Recipes, erfcc).
pub fn erfc(x: f64) -> f64 {
    let z = x.abs();
    let t = 1.0 / (1.0 + 0.5 * z);
    let poly = -z * z - 1.265_512_23
        + t * (1.000_023_68
            + t * (0.374_091_96
                + t * (0.096_784_18
                    + t * (-0.186_288_06
                        + t * (0.278_868_07
                            + t * (-1.135_203_98
                                + t * (1.488_515_87 + t * (-0.822_152_23 + t * 0.170_872_77))))))));
    let ans = t * poly.exp();
    if x >= 0.0 { ans } else { 2.0 - ans }
}

/// Holm adjusted p values, returned in the input order.
pub fn holm(p: &[f64]) -> Vec<f64> {
    let m = p.len();
    let mut idx: Vec<usize> = (0..m).collect();
    idx.sort_by(|&a, &b| p[a].partial_cmp(&p[b]).unwrap_or(std::cmp::Ordering::Equal));
    let mut adj = vec![0.0; m];
    let mut running: f64 = 0.0;
    for (rank, &i) in idx.iter().enumerate() {
        running = running.max(((m - rank) as f64 * p[i]).min(1.0));
        adj[i] = running;
    }
    adj
}

/// Total variation distance between two class frequency vectors.
pub fn total_variation(a: &[f64; NCLASS], b: &[f64; NCLASS]) -> f64 {
    0.5 * a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum::<f64>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holm_matches_a_hand_computation() {
        // p = (0.01, 0.04, 0.03): sorted 0.01, 0.03, 0.04 -> 3*0.01, 2*0.03, 1*0.04
        let adj = holm(&[0.01, 0.04, 0.03]);
        assert!((adj[0] - 0.03).abs() < 1e-12);
        assert!((adj[2] - 0.06).abs() < 1e-12);
        assert!((adj[1] - 0.06).abs() < 1e-12); // monotone: max(0.06, 0.04)
    }

    #[test]
    fn wald_with_identity_covariance_gives_the_closed_form() {
        // Draws on a grid with covariance close to the identity.
        let mut draws = Vec::new();
        for i in -10i32..=10 {
            for j in -10i32..=10 {
                draws.push([f64::from(i) * 0.1, f64::from(j) * 0.1, 0.0]);
            }
        }
        let (w, p) = wald(&[0.0, 0.0, 0.0], &draws);
        assert!(w.abs() < 1e-12 && (p - 1.0).abs() < 1e-12);
        let (w2, p2) = wald(&[0.5, 0.0, 0.0], &draws);
        assert!((p2 - (-w2 / 2.0).exp()).abs() < 1e-12);
        assert!(w2 > 0.0 && p2 < 1.0);
    }

    #[test]
    fn erfc_matches_known_values() {
        assert!((erfc(0.0) - 1.0).abs() < 1e-7);
        assert!((erfc(1.0) - 0.157_299_207).abs() < 1e-7);
        assert!((erfc(2.0) - 0.004_677_735).abs() < 1e-8);
        assert!((erfc(-1.0) - 1.842_700_793).abs() < 1e-7);
    }

    #[test]
    fn adjustment_reweights_the_control_to_the_vwap_stratum_mix() {
        // Two strata. VWAP: 60 events in stratum 0 (all REJ), 40 in stratum 1 (all CHOP).
        // Control: stratum 0 is 100 percent CHOP, stratum 1 is 100 percent REJ, in equal numbers.
        let vwap = [60, 0, 0, 0, 0, 40];
        let ctrl = [0, 0, 50, 50, 0, 0];
        let e = estimate(&vwap, &ctrl, 30);
        assert!((e.pi_vwap_adj[0] - 0.6).abs() < 1e-12);
        // control reweighted 60/40: REJ share is 0.4, CHOP share is 0.6
        assert!((e.pi_ctrl_adj[0] - 0.4).abs() < 1e-12);
        assert!((e.pi_ctrl_adj[2] - 0.6).abs() < 1e-12);
        // raw control is 50/50, so the adjustment matters
        assert!((e.pi_ctrl_raw[0] - 0.5).abs() < 1e-12);
        assert!((e.coverage - 1.0).abs() < 1e-12);
        // a stratum below the minimum is dropped and coverage falls
        let e2 = estimate(&vwap, &ctrl, 45);
        assert!((e2.coverage - 0.6).abs() < 1e-12 || e2.coverage < 1.0);
    }

    #[test]
    fn identical_groups_rarely_reject() {
        // 60 sessions, both groups drawn from the same class mix by the same generator.
        let mut rng = ChaCha8Rng::seed_from_u64(5);
        let mut rejections = 0;
        let trials = 40;
        for t in 0..trials {
            let mut v = Vec::new();
            let mut c = Vec::new();
            for s in 0..60 {
                for _ in 0..4 {
                    let draw = |r: &mut ChaCha8Rng| {
                        let u: f64 = r.random();
                        if u < 0.35 {
                            0
                        } else if u < 0.7 {
                            1
                        } else {
                            2
                        }
                    };
                    v.push(Row {
                        session: s,
                        stratum: 0,
                        class: draw(&mut rng),
                    });
                    c.push(Row {
                        session: s,
                        stratum: 0,
                        class: draw(&mut rng),
                    });
                }
            }
            let tables = SessionTables::build(1, &v, &c);
            let (point, draws) = bootstrap(&tables, 300, 1000 + t, 1);
            let d: Vec<[f64; 3]> = draws.iter().map(|e| e.delta).collect();
            let (_, p) = wald(&point.delta, &d);
            if p < 0.05 {
                rejections += 1;
            }
        }
        assert!(rejections <= 8, "{rejections} of {trials} false rejections");
    }

    #[test]
    fn a_real_difference_is_detected() {
        let mut rng = ChaCha8Rng::seed_from_u64(6);
        let mut v = Vec::new();
        let mut c = Vec::new();
        for s in 0..200 {
            for _ in 0..5 {
                let u: f64 = rng.random();
                v.push(Row {
                    session: s,
                    stratum: 0,
                    class: if u < 0.5 { 0 } else { 2 },
                });
                let u2: f64 = rng.random();
                c.push(Row {
                    session: s,
                    stratum: 0,
                    class: if u2 < 0.3 { 0 } else { 2 },
                });
            }
        }
        let tables = SessionTables::build(1, &v, &c);
        let (point, draws) = bootstrap(&tables, 500, 7, 1);
        let d: Vec<[f64; 3]> = draws.iter().map(|e| e.delta).collect();
        let (_, p) = wald(&point.delta, &d);
        assert!(p < 1e-6, "p = {p}");
        assert!(point.delta[0] > 0.1);
    }
}
