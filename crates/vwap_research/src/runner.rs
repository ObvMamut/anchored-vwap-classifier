//! Subset contrast runner and the verdict rules of `docs/PROTOCOL.md`.
#![allow(clippy::indexing_slicing)]

use std::collections::BTreeMap;

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::Serialize;
use vwap_baselines::stats::{percentile, sorted};
use vwap_core::{Class, ResearchConfig};
use vwap_features::feature_names;

use crate::card::{Card, Direction, Outcome, RuleKind};
use crate::ledger::Verdict;
use crate::math::{chi2_sf, invert, norm_ppf};
use crate::split::Sample;

#[derive(Debug, Clone, Serialize)]
pub struct YearStat {
    pub year: i32,
    pub delta: f64,
    pub n_subset: usize,
    pub n_comp: usize,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct WidthResult {
    pub width: u32,
    pub n_eligible: usize,
    pub nan_rate: f64,
    pub cut_points: Vec<f64>,
    pub n_subset: usize,
    pub n_comp: usize,
    pub mean_subset: f64,
    pub mean_comp: f64,
    /// Subset minus complement, or the largest absolute group deviation for group tests.
    pub delta: f64,
    pub ci95: [f64; 2],
    pub p: f64,
    pub deff: f64,
    pub mde: f64,
    pub years: Vec<(i32, f64, usize)>,
    pub years_same_sign: usize,
    pub years_with_data: usize,
    pub by_vol_tercile: Vec<f64>,
    pub by_side: Vec<f64>,
    /// `(group id, mean, n)` for group tests.
    pub groups: Vec<(f64, f64, usize)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CardResult {
    pub card_id: String,
    pub alpha_card: f64,
    pub min_effect: f64,
    pub widths: Vec<WidthResult>,
    pub p: f64,
    pub delta: f64,
    pub mde: f64,
    pub verdict: Verdict,
    pub reasons: Vec<String>,
}

fn hash_str(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    })
}

fn outcome_of(s: &Sample, o: Outcome, cost: f64) -> Option<f64> {
    if s.ambiguous {
        return None;
    }
    match o {
        Outcome::Rejection => s.class.map(|c| f64::from(u8::from(c == Class::Rejection))),
        Outcome::RejectionGivenResolved => match s.class {
            Some(Class::Chop) | None => None,
            Some(c) => Some(f64::from(u8::from(c == Class::Rejection))),
        },
        Outcome::NetTicks => Some(s.outcome_ticks - cost),
    }
}

fn quantile(v: &[f64], q: f64) -> f64 {
    percentile(&sorted(v.to_vec()), q)
}

fn mean_var(v: &[f64]) -> (f64, f64) {
    let n = v.len() as f64;
    if v.is_empty() {
        return (f64::NAN, f64::NAN);
    }
    let m = v.iter().sum::<f64>() / n;
    let var = if v.len() > 1 {
        v.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / (n - 1.0)
    } else {
        0.0
    };
    (m, var)
}

/// One row used by the contrast: arm (0, 1 or a group index), outcome, session, year, side, vol.
struct Row {
    arm: usize,
    y: f64,
    session: i32,
    year: i32,
    side: i8,
    vol: f64,
}

fn arm_delta(rows: &[&Row]) -> Option<(f64, usize, usize)> {
    let (mut n1, mut s1, mut n0, mut s0) = (0usize, 0.0, 0usize, 0.0);
    for r in rows {
        if r.arm == 1 {
            n1 += 1;
            s1 += r.y;
        } else {
            n0 += 1;
            s0 += r.y;
        }
    }
    (n1 >= 30 && n0 >= 30).then(|| (s1 / n1 as f64 - s0 / n0 as f64, n1, n0))
}

fn width_result(
    card: &Card,
    samples: &[&Sample],
    width: u32,
    cfg: &ResearchConfig,
    alpha: f64,
) -> WidthResult {
    let fidx = feature_names().iter().position(|n| *n == card.feature);
    let mut res = WidthResult {
        width,
        n_eligible: samples.len(),
        ..WidthResult::default()
    };
    let Some(fidx) = fidx else {
        res.p = 1.0;
        return res;
    };
    let vals: Vec<(&Sample, f64)> = samples
        .iter()
        .filter_map(|s| s.features.get(fidx).copied().map(|x| (*s, x)))
        .collect();
    res.nan_rate =
        1.0 - vals.iter().filter(|(_, x)| x.is_finite()).count() as f64 / vals.len().max(1) as f64;
    let finite: Vec<(&Sample, f64)> = vals.into_iter().filter(|(_, x)| x.is_finite()).collect();
    let xs: Vec<f64> = finite.iter().map(|(_, x)| *x).collect();
    let (lo, hi) = (quantile(&xs, 1.0 / 3.0), quantile(&xs, 2.0 / 3.0));
    let arm_of = |x: f64| -> Option<usize> {
        match card.rule {
            RuleKind::TopTercile => Some(usize::from(x > hi)),
            RuleKind::BottomTercile => Some(usize::from(x < lo)),
            RuleKind::Above => card.threshold.map(|t| usize::from(x > t)),
            RuleKind::Below => card.threshold.map(|t| usize::from(x < t)),
            RuleKind::Flag => {
                if x == 1.0 {
                    Some(1)
                } else if x == 0.0 {
                    Some(0)
                } else {
                    None
                }
            }
            RuleKind::Groups => Some(x as usize),
        }
    };
    res.cut_points = match card.rule {
        RuleKind::TopTercile => vec![hi],
        RuleKind::BottomTercile => vec![lo],
        RuleKind::Above | RuleKind::Below => card.threshold.into_iter().collect(),
        _ => vec![],
    };
    let vol_edges = {
        let v: Vec<f64> = finite.iter().map(|(s, _)| s.sigma_over_price).collect();
        (quantile(&v, 1.0 / 3.0), quantile(&v, 2.0 / 3.0))
    };
    let rows: Vec<Row> = finite
        .iter()
        .filter_map(|(s, x)| {
            let y = outcome_of(s, card.outcome, cfg.cost_ticks)?;
            let arm = arm_of(*x)?;
            Some(Row {
                arm,
                y,
                session: s.session,
                year: s.year,
                side: s.side,
                vol: s.sigma_over_price,
            })
        })
        .collect();
    if rows.len() < 60 {
        res.p = 1.0;
        return res;
    }
    let mut seed_rng = ChaCha8Rng::seed_from_u64(cfg.seed ^ hash_str(&card.id) ^ u64::from(width));
    let mut by_session: BTreeMap<i32, Vec<&Row>> = BTreeMap::new();
    for r in &rows {
        by_session.entry(r.session).or_default().push(r);
    }
    let sessions: Vec<&Vec<&Row>> = by_session.values().collect();

    if card.rule == RuleKind::Groups {
        group_test(&mut res, &rows, &sessions, cfg, &mut seed_rng);
    } else {
        two_arm(&mut res, card, &rows, &sessions, cfg, alpha, &mut seed_rng);
    }
    // breakdowns for the report and the stability rule
    let by_year: BTreeMap<i32, Vec<&Row>> = rows.iter().fold(BTreeMap::new(), |mut m, r| {
        m.entry(r.year).or_default().push(r);
        m
    });
    if card.rule == RuleKind::Groups {
        let extreme = res
            .groups
            .iter()
            .max_by(|a, b| {
                (a.1 - overall(&rows))
                    .abs()
                    .partial_cmp(&(b.1 - overall(&rows)).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|g| g.0 as usize);
        let sign_overall = res
            .groups
            .iter()
            .find(|g| Some(g.0 as usize) == extreme)
            .map_or(0.0, |g| g.1 - overall(&rows));
        for (y, rs) in &by_year {
            let g: Vec<f64> = rs
                .iter()
                .filter(|r| Some(r.arm) == extreme)
                .map(|r| r.y)
                .collect();
            if g.len() >= 30 {
                let d = mean_var(&g).0 - mean_var(&rs.iter().map(|r| r.y).collect::<Vec<_>>()).0;
                res.years.push((*y, d, g.len()));
                res.years_with_data += 1;
                if d.signum() == sign_overall.signum() {
                    res.years_same_sign += 1;
                }
            }
        }
    } else {
        let predicted = if card.direction == Direction::Positive {
            1.0
        } else {
            -1.0
        };
        for (y, rs) in &by_year {
            if let Some((d, n1, n0)) = arm_delta(rs) {
                res.years.push((*y, d, n1.min(n0)));
                res.years_with_data += 1;
                if d.signum() == predicted {
                    res.years_same_sign += 1;
                }
            }
        }
        res.by_vol_tercile = (0..3)
            .map(|t| {
                let rs: Vec<&Row> = rows
                    .iter()
                    .filter(|r| {
                        usize::from(r.vol > vol_edges.0) + usize::from(r.vol > vol_edges.1) == t
                    })
                    .collect();
                arm_delta(&rs).map_or(f64::NAN, |x| x.0)
            })
            .collect();
        res.by_side = [1i8, -1]
            .iter()
            .map(|sd| {
                let rs: Vec<&Row> = rows.iter().filter(|r| r.side == *sd).collect();
                arm_delta(&rs).map_or(f64::NAN, |x| x.0)
            })
            .collect();
    }
    res
}

fn overall(rows: &[Row]) -> f64 {
    mean_var(&rows.iter().map(|r| r.y).collect::<Vec<_>>()).0
}

fn two_arm(
    res: &mut WidthResult,
    card: &Card,
    rows: &[Row],
    sessions: &[&Vec<&Row>],
    cfg: &ResearchConfig,
    alpha: f64,
    rng: &mut ChaCha8Rng,
) {
    let y1: Vec<f64> = rows.iter().filter(|r| r.arm == 1).map(|r| r.y).collect();
    let y0: Vec<f64> = rows.iter().filter(|r| r.arm == 0).map(|r| r.y).collect();
    let ((m1, v1), (m0, v0)) = (mean_var(&y1), mean_var(&y0));
    res.n_subset = y1.len();
    res.n_comp = y0.len();
    res.mean_subset = m1;
    res.mean_comp = m0;
    res.delta = m1 - m0;
    // per session sums
    let sums: Vec<[f64; 4]> = sessions
        .iter()
        .map(|rs| {
            let mut a = [0.0; 4];
            for r in rs.iter() {
                if r.arm == 1 {
                    a[0] += 1.0;
                    a[1] += r.y;
                } else {
                    a[2] += 1.0;
                    a[3] += r.y;
                }
            }
            a
        })
        .collect();
    let n = sums.len();
    let mut draws = Vec::with_capacity(cfg.bootstrap_draws);
    for _ in 0..cfg.bootstrap_draws {
        let mut t = [0.0; 4];
        for _ in 0..n {
            let s = &sums[rng.random_range(0..n)];
            for (x, y) in t.iter_mut().zip(s) {
                *x += y;
            }
        }
        if t[0] > 0.0 && t[2] > 0.0 {
            draws.push(t[1] / t[0] - t[3] / t[2]);
        }
    }
    let b = draws.len() as f64;
    let against = draws
        .iter()
        .filter(|d| {
            if card.direction == Direction::Positive {
                **d <= 0.0
            } else {
                **d >= 0.0
            }
        })
        .count() as f64;
    res.p = (1.0 + against) / (b + 1.0);
    let s = sorted(draws.clone());
    res.ci95 = [percentile(&s, 0.025), percentile(&s, 0.975)];
    let (_, var_boot) = mean_var(&draws);
    let var_srs = v1 / y1.len() as f64 + v0 / y0.len() as f64;
    res.deff = if var_srs > 0.0 {
        var_boot / var_srs
    } else {
        f64::NAN
    };
    res.mde = (norm_ppf(1.0 - alpha) + norm_ppf(0.8)) * var_boot.sqrt();
}

fn group_test(
    res: &mut WidthResult,
    rows: &[Row],
    sessions: &[&Vec<&Row>],
    cfg: &ResearchConfig,
    rng: &mut ChaCha8Rng,
) {
    let mut ids: Vec<usize> = rows.iter().map(|r| r.arm).collect();
    ids.sort_unstable();
    ids.dedup();
    let g = ids.len();
    if g < 2 {
        res.p = 1.0;
        return;
    }
    let pos = |arm: usize| ids.binary_search(&arm).unwrap_or(0);
    let mut sums = vec![vec![0.0; 2 * g]; sessions.len()]; // [n_g, s_g] per group
    for (k, rs) in sessions.iter().enumerate() {
        for r in rs.iter() {
            sums[k][2 * pos(r.arm)] += 1.0;
            sums[k][2 * pos(r.arm) + 1] += r.y;
        }
    }
    let total: Vec<f64> = (0..2 * g)
        .map(|j| sums.iter().map(|s| s[j]).sum())
        .collect();
    let mu: Vec<f64> = (0..g).map(|i| total[2 * i + 1] / total[2 * i]).collect();
    let om = overall(rows);
    res.groups = ids
        .iter()
        .enumerate()
        .map(|(i, id)| (*id as f64, mu[i], total[2 * i] as usize))
        .collect();
    res.delta = mu.iter().map(|m| (m - om).abs()).fold(0.0, f64::max);
    res.n_subset = rows.len();
    let n = sums.len();
    let mut diffs: Vec<Vec<f64>> = Vec::new();
    for _ in 0..cfg.bootstrap_draws {
        let mut t = vec![0.0; 2 * g];
        for _ in 0..n {
            let s = &sums[rng.random_range(0..n)];
            for (x, y) in t.iter_mut().zip(s) {
                *x += y;
            }
        }
        if (0..g).all(|i| t[2 * i] > 0.0) {
            let m: Vec<f64> = (0..g).map(|i| t[2 * i + 1] / t[2 * i]).collect();
            diffs.push((0..g - 1).map(|i| m[i] - m[g - 1]).collect());
        }
    }
    let b = diffs.len() as f64;
    if b < 10.0 {
        res.p = 1.0;
        return;
    }
    let k = g - 1;
    let means: Vec<f64> = (0..k)
        .map(|i| diffs.iter().map(|d| d[i]).sum::<f64>() / b)
        .collect();
    let mut cov = vec![vec![0.0; k]; k];
    for d in &diffs {
        for i in 0..k {
            for j in 0..k {
                cov[i][j] += (d[i] - means[i]) * (d[j] - means[j]) / (b - 1.0);
            }
        }
    }
    let point: Vec<f64> = (0..k).map(|i| mu[i] - mu[g - 1]).collect();
    res.p = match invert(&cov) {
        Some(inv) => {
            let w: f64 = (0..k)
                .map(|i| (0..k).map(|j| point[i] * inv[i][j] * point[j]).sum::<f64>())
                .sum();
            chi2_sf(w, k)
        }
        None => 1.0,
    };
}

/// Applies the verdict rules of `docs/PROTOCOL.md`, section 6.
pub fn decide(
    card: &Card,
    widths: &[WidthResult],
    cfg: &ResearchConfig,
) -> (Verdict, Vec<String>, f64, f64, f64) {
    let alpha = card.alpha.unwrap_or_else(|| cfg.alpha_per_card());
    let min_effect = card.min_effect.unwrap_or(cfg.default_min_effect);
    let p = widths.iter().map(|w| w.p).fold(0.0, f64::max);
    let mde = widths.iter().map(|w| w.mde).fold(0.0, f64::max);
    let mut reasons = Vec::new();
    let predicted = match card.direction {
        Direction::Positive => 1.0,
        Direction::Negative => -1.0,
        Direction::Any => 0.0,
    };
    let sign_ok = widths
        .iter()
        .all(|w| predicted == 0.0 || w.delta.signum() == predicted);
    if !sign_ok {
        reasons.push("sign differs from the prediction on at least one timeframe".into());
    }
    let effect_ok = widths.iter().all(|w| w.delta.abs() >= min_effect);
    if !effect_ok {
        reasons.push(format!(
            "effect below the minimum {min_effect:.3} on at least one timeframe"
        ));
    }
    let arms_ok = widths.iter().all(|w| {
        let small = if card.rule == RuleKind::Groups {
            w.n_subset
        } else {
            w.n_subset.min(w.n_comp)
        };
        small >= cfg.min_events_per_arm
    });
    if !arms_ok {
        reasons.push("an arm has fewer than the minimum number of events".into());
    }
    let years_ok = widths.iter().all(|w| {
        w.years_with_data >= 3
            && w.years_same_sign >= cfg.promote_min_years_same_sign.min(w.years_with_data)
    });
    if !years_ok {
        reasons.push("sign is not stable across discovery years".into());
    }
    let p_ok = p <= alpha;
    if !p_ok {
        reasons.push(format!("p = {p:.4} above the card alpha {alpha:.4}"));
    }
    let verdict = if p_ok && sign_ok && effect_ok && arms_ok && years_ok {
        Verdict::Promote
    } else if sign_ok && p < cfg.refine_p_max && p > alpha {
        Verdict::Refine
    } else {
        Verdict::Drop
    };
    let delta = widths
        .iter()
        .map(|w| w.delta)
        .fold(f64::INFINITY, |a, b| if b.abs() < a.abs() { b } else { a });
    (verdict, reasons, p, delta, mde)
}

/// Runs a card on a set of samples (a discovery or confirmation view's contents).
pub fn run_card(card: &Card, samples: &[Sample], cfg: &ResearchConfig) -> CardResult {
    let alpha = card.alpha.unwrap_or_else(|| cfg.alpha_per_card());
    let widths: Vec<WidthResult> = card
        .timeframes
        .iter()
        .map(|&w| {
            let sub: Vec<&Sample> = samples.iter().filter(|s| s.width == w).collect();
            width_result(card, &sub, w, cfg, alpha)
        })
        .collect();
    let (verdict, reasons, p, delta, mde) = decide(card, &widths, cfg);
    CardResult {
        card_id: card.id.clone(),
        alpha_card: alpha,
        min_effect: card.min_effect.unwrap_or(cfg.default_min_effect),
        widths,
        p,
        delta,
        mde,
        verdict,
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selftest::{control_card, synthetic_samples};

    fn cfg() -> ResearchConfig {
        let mut c = vwap_core::load_research_config(std::path::Path::new("../../config")).unwrap();
        c.bootstrap_draws = 800;
        c
    }

    /// Samples where the top tercile of the feature changes only the chop rate.
    fn chop_only(seed: u64) -> Vec<Sample> {
        use rand::{Rng, SeedableRng};
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let mut out = synthetic_samples(0.0, 700, 5, seed);
        for s in &mut out {
            let x = s.features.first().copied().unwrap_or(0.0);
            // top tercile events become chop far more often, direction stays 50/50
            if x > 2.0 / 3.0 && rng.random::<f64>() < 0.5 {
                s.class = Some(Class::Chop);
            }
        }
        out
    }

    #[test]
    fn chop_events_are_left_out_of_the_conditional_outcome() {
        let mut s = synthetic_samples(0.0, 1, 1, 1).remove(0);
        s.class = Some(Class::Chop);
        assert_eq!(outcome_of(&s, Outcome::RejectionGivenResolved, 3.0), None);
        assert_eq!(outcome_of(&s, Outcome::Rejection, 3.0), Some(0.0));
        s.class = Some(Class::Rejection);
        assert_eq!(
            outcome_of(&s, Outcome::RejectionGivenResolved, 3.0),
            Some(1.0)
        );
    }

    #[test]
    fn a_chop_only_effect_fools_the_plain_indicator_but_not_the_conditional_outcome() {
        let c = cfg();
        let samples = chop_only(5);
        let mut card = control_card();
        card.direction = crate::card::Direction::Negative;
        card.outcome = Outcome::Rejection;
        let plain = run_card(&card, &samples, &c);
        assert!(plain.p < 0.01, "plain indicator p {}", plain.p);
        card.outcome = Outcome::RejectionGivenResolved;
        let cond = run_card(&card, &samples, &c);
        assert!(cond.p > 0.01, "conditional p {}", cond.p);
        assert_ne!(cond.verdict, Verdict::Promote);
    }

    #[test]
    fn a_real_directional_effect_is_still_promoted_on_the_conditional_outcome() {
        let c = cfg();
        let mut card = control_card();
        card.outcome = Outcome::RejectionGivenResolved;
        let r = run_card(&card, &synthetic_samples(0.08, 800, 5, 21), &c);
        assert_eq!(r.verdict, Verdict::Promote, "{:?} p {}", r.reasons, r.p);
    }
}
