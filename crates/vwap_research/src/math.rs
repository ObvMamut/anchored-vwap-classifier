//! Small numeric helpers: normal quantile, chi square tail, matrix inverse.
#![allow(clippy::indexing_slicing)]

/// Standard normal quantile function (Acklam's rational approximation, relative error 1.2e-9).
pub fn norm_ppf(p: f64) -> f64 {
    const A: [f64; 6] = [
        -3.969_683_028_665_376e1,
        2.209_460_984_245_205e2,
        -2.759_285_104_469_687e2,
        1.383_577_518_672_69e2,
        -3.066_479_806_614_716e1,
        2.506_628_277_459_239,
    ];
    const B: [f64; 5] = [
        -5.447_609_879_822_406e1,
        1.615_858_368_580_409e2,
        -1.556_989_798_598_866e2,
        6.680_131_188_771_972e1,
        -1.328_068_155_288_572e1,
    ];
    const C: [f64; 6] = [
        -7.784_894_002_430_293e-3,
        -3.223_964_580_411_365e-1,
        -2.400_758_277_161_838,
        -2.549_732_539_343_734,
        4.374_664_141_464_968,
        2.938_163_982_698_783,
    ];
    const D: [f64; 4] = [
        7.784_695_709_041_462e-3,
        3.224_671_290_700_398e-1,
        2.445_134_137_142_996,
        3.754_408_661_907_416,
    ];
    let p = p.clamp(1e-300, 1.0 - 1e-16);
    let lo = 0.02425;
    if p < lo {
        let q = (-2.0 * p.ln()).sqrt();
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    } else if p <= 1.0 - lo {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    } else {
        -norm_ppf(1.0 - p)
    }
}

fn ln_gamma(x: f64) -> f64 {
    // Lanczos approximation, g = 7
    const G: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];
    let x = x - 1.0;
    let mut a = G[0];
    let t = x + 7.5;
    for (i, g) in G.iter().enumerate().skip(1) {
        a += g / (x + i as f64);
    }
    0.5 * (2.0 * std::f64::consts::PI).ln() + (x + 0.5) * t.ln() - t + a.ln()
}

/// Regularised upper incomplete gamma function Q(a, x).
fn gammq(a: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 1.0;
    }
    if x < a + 1.0 {
        // series for P, then Q = 1 - P
        let (mut sum, mut term, mut ap) = (1.0 / a, 1.0 / a, a);
        for _ in 0..500 {
            ap += 1.0;
            term *= x / ap;
            sum += term;
            if term.abs() < sum.abs() * 1e-16 {
                break;
            }
        }
        1.0 - sum * (-x + a * x.ln() - ln_gamma(a)).exp()
    } else {
        // continued fraction for Q (modified Lentz)
        let tiny = 1e-300;
        let mut b = x + 1.0 - a;
        let mut c = 1.0 / tiny;
        let mut d = 1.0 / b;
        let mut h = d;
        for i in 1..500 {
            let an = -f64::from(i) * (f64::from(i) - a);
            b += 2.0;
            d = an * d + b;
            if d.abs() < tiny {
                d = tiny;
            }
            c = b + an / c;
            if c.abs() < tiny {
                c = tiny;
            }
            d = 1.0 / d;
            let del = d * c;
            h *= del;
            if (del - 1.0).abs() < 1e-16 {
                break;
            }
        }
        (-x + a * x.ln() - ln_gamma(a)).exp() * h
    }
}

/// Survival function of the chi square distribution with `df` degrees of freedom.
pub fn chi2_sf(x: f64, df: usize) -> f64 {
    gammq(df as f64 / 2.0, x / 2.0).clamp(0.0, 1.0)
}

/// Inverse of a small square matrix by Gauss Jordan elimination with pivoting.
pub fn invert(m: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    let mut a: Vec<Vec<f64>> = m
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut row = r.clone();
            row.extend((0..n).map(|j| f64::from(u8::from(i == j))));
            row
        })
        .collect();
    for col in 0..n {
        let piv = (col..n).max_by(|&i, &j| {
            a[i][col]
                .abs()
                .partial_cmp(&a[j][col].abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })?;
        if a[piv][col].abs() < 1e-14 {
            return None;
        }
        a.swap(col, piv);
        let d = a[col][col];
        for v in &mut a[col] {
            *v /= d;
        }
        for r in 0..n {
            if r != col {
                let f = a[r][col];
                if f != 0.0 {
                    let pivot_row = a[col].clone();
                    for (x, p) in a[r].iter_mut().zip(&pivot_row) {
                        *x -= f * p;
                    }
                }
            }
        }
    }
    Some(a.into_iter().map(|r| r[n..].to_vec()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_quantiles_match_known_values() {
        assert!((norm_ppf(0.975) - 1.959_963_985).abs() < 1e-7);
        assert!((norm_ppf(0.5)).abs() < 1e-9);
        assert!((norm_ppf(0.8) - 0.841_621_234).abs() < 1e-7);
        assert!((norm_ppf(0.001) + 3.090_232_306).abs() < 1e-6);
        assert!((norm_ppf(0.01) + 2.326_347_874).abs() < 1e-6);
    }

    #[test]
    fn chi_square_tails_match_known_values() {
        assert!((chi2_sf(5.991_464_5, 2) - 0.05).abs() < 1e-6);
        assert!((chi2_sf(7.814_727_9, 3) - 0.05).abs() < 1e-6);
        assert!((chi2_sf(3.841_458_8, 1) - 0.05).abs() < 1e-6);
        assert!((chi2_sf(20.0, 3) - 1.68e-4).abs() < 1e-5);
        // df 2 closed form exp(-x/2)
        assert!((chi2_sf(4.0, 2) - (-2.0f64).exp()).abs() < 1e-10);
    }

    #[test]
    #[allow(clippy::needless_range_loop)]
    fn matrix_inverse_times_matrix_is_identity() {
        let m = vec![
            vec![4.0, 1.0, 0.5],
            vec![1.0, 3.0, 0.2],
            vec![0.5, 0.2, 2.0],
        ];
        let inv = invert(&m).unwrap();
        for i in 0..3 {
            for j in 0..3 {
                let s: f64 = (0..3).map(|k| m[i][k] * inv[k][j]).sum();
                assert!((s - f64::from(u8::from(i == j))).abs() < 1e-12);
            }
        }
        assert!(invert(&[vec![1.0, 2.0], vec![2.0, 4.0]]).is_none());
    }
}
