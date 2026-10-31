//! Cycle 0: is there any predictive structure in the features at all?
//!
//! A LightGBM multiclass model with fixed, pre registered settings is scored by purged
//! expanding blocked cross validation. The statistic is the log loss improvement over the
//! training frequencies. The null distribution comes from circularly shifting the label
//! sequence, which keeps the label dependence structure and breaks its alignment with the
//! features (a within session shuffle would keep between session associations).
#![allow(clippy::indexing_slicing)]

use lightgbm3::{Booster, Dataset};
use rayon::prelude::*;
use serde::Serialize;
use serde_json::json;
use vwap_core::{Class, ResearchConfig};
use vwap_features::feature_names;

use crate::error::ResearchError;
use crate::ledger::Verdict;
use crate::split::Sample;

const BLOCKS: usize = 6;
const ROUNDS: i64 = 60;

#[derive(Debug, Clone, Serialize)]
pub struct WidthOmnibus {
    pub width: u32,
    pub n: usize,
    pub statistic: f64,
    pub p: f64,
    pub permutations: usize,
    pub null_mean: f64,
    pub null_p95: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct OmnibusResult {
    pub widths: Vec<WidthOmnibus>,
    /// Smallest width p value times the number of timeframes.
    pub p: f64,
    pub statistic: f64,
    pub verdict: Verdict,
}

/// Time ordered rows ready for the cross validation.
struct Data {
    x: Vec<f64>,
    n_feat: usize,
    y: Vec<usize>,
    t_close: Vec<i64>,
    t_end: Vec<i64>,
}

fn prepare(samples: &[&Sample]) -> Data {
    let mut rows: Vec<&&Sample> = samples
        .iter()
        .filter(|s| !s.ambiguous && s.class.is_some())
        .collect();
    rows.sort_by_key(|s| (s.t_close_us, s.event_id));
    let n_feat = feature_names().len();
    let mut d = Data {
        x: Vec::with_capacity(rows.len() * n_feat),
        n_feat,
        y: Vec::new(),
        t_close: Vec::new(),
        t_end: Vec::new(),
    };
    for s in rows {
        d.x.extend(s.features.iter().copied());
        d.y.push(s.class.map_or(0, Class::index));
        d.t_close.push(s.t_close_us);
        d.t_end.push(s.t_end_us);
    }
    d
}

fn params(seed: u64) -> serde_json::Value {
    json!({
        "objective": "multiclass",
        "num_class": 3,
        "num_iterations": ROUNDS,
        "learning_rate": 0.05,
        "num_leaves": 7,
        "min_data_in_leaf": 40,
        "feature_fraction": 0.8,
        "lambda_l2": 5.0,
        "verbose": -1,
        "num_threads": 1,
        "deterministic": true,
        "force_row_wise": true,
        "seed": seed,
        "bagging_seed": seed,
        "feature_fraction_seed": seed,
        "data_random_seed": seed,
    })
}

/// Pooled log loss improvement over the training frequencies, for labels `y`.
fn statistic(d: &Data, y: &[usize], embargo_us: i64, seed: u64) -> Option<f64> {
    let n = y.len();
    let block = n / BLOCKS;
    let (mut gain, mut count) = (0.0, 0usize);
    for k in 1..BLOCKS {
        let (a, z) = (k * block, if k + 1 == BLOCKS { n } else { (k + 1) * block });
        let test_start = d.t_close[a];
        // purge: training events whose label span plus the embargo reaches the test start
        let train: Vec<usize> = (0..a)
            .filter(|&i| d.t_end[i] + embargo_us < test_start)
            .collect();
        if train.len() < 300 {
            continue;
        }
        let flat: Vec<f64> = train
            .iter()
            .flat_map(|&i| d.x[i * d.n_feat..(i + 1) * d.n_feat].iter().copied())
            .collect();
        let labels: Vec<f32> = train.iter().map(|&i| y[i] as f32).collect();
        let ds = Dataset::from_slice(&flat, &labels, d.n_feat as i32, true).ok()?;
        let booster = Booster::train(ds, &params(seed)).ok()?;
        let test_flat = &d.x[a * d.n_feat..z * d.n_feat];
        let p = booster.predict(test_flat, d.n_feat as i32, true).ok()?;
        let mut freq = [1.0f64; 3];
        for &i in &train {
            freq[y[i]] += 1.0;
        }
        let tot: f64 = freq.iter().sum();
        for (j, i) in (a..z).enumerate() {
            let pm = (p[j * 3 + y[i]]).clamp(1e-12, 1.0);
            let pb = freq[y[i]] / tot;
            gain += pb.ln().abs() - pm.ln().abs();
            count += 1;
        }
    }
    (count > 0).then(|| gain / count as f64)
}

fn width_omnibus(
    samples: &[&Sample],
    width: u32,
    horizon_bars: usize,
    cfg: &ResearchConfig,
) -> Result<WidthOmnibus, ResearchError> {
    let d = prepare(samples);
    let n = d.y.len();
    if n < 1500 {
        return Err(ResearchError::NotEnoughData(format!(
            "{n} rows for the omnibus on {width} minute bars"
        )));
    }
    let embargo_us = (horizon_bars as i64) * i64::from(width) * 60_000_000;
    let obs = statistic(&d, &d.y, embargo_us, cfg.seed)
        .ok_or_else(|| ResearchError::NotEnoughData("cross validation produced no folds".into()))?;
    let perms = cfg.omnibus_permutations;
    let nulls: Vec<f64> = (0..perms)
        .into_par_iter()
        .map(|b| {
            // circular shift of the label sequence by a seeded offset away from zero
            let h = (cfg.seed ^ (b as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
                .wrapping_mul(0xBF58_476D_1CE4_E5B9);
            let span = (n as u64) * 8 / 10;
            let off = n / 10 + (h >> 11) as usize % span.max(1) as usize;
            let y: Vec<usize> = (0..n).map(|i| d.y[(i + off) % n]).collect();
            statistic(&d, &y, embargo_us, cfg.seed).unwrap_or(f64::NAN)
        })
        .collect();
    let valid: Vec<f64> = nulls.into_iter().filter(|x| x.is_finite()).collect();
    let ge = valid.iter().filter(|&&s| s >= obs).count() as f64;
    let mean = valid.iter().sum::<f64>() / valid.len().max(1) as f64;
    let mut sorted_n = valid.clone();
    sorted_n.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p95 = sorted_n
        .get(sorted_n.len() * 95 / 100)
        .copied()
        .unwrap_or(f64::NAN);
    Ok(WidthOmnibus {
        width,
        n,
        statistic: obs,
        p: (1.0 + ge) / (1.0 + valid.len() as f64),
        permutations: valid.len(),
        null_mean: mean,
        null_p95: p95,
    })
}

/// Runs the omnibus screen on discovery samples. `horizon_bars` gives the label horizon per timeframe.
pub fn omnibus(
    samples: &[Sample],
    cfg: &ResearchConfig,
    horizon_bars: impl Fn(u32) -> usize,
) -> Result<OmnibusResult, ResearchError> {
    let mut widths = Vec::new();
    for w in [5u32, 15] {
        let sub: Vec<&Sample> = samples.iter().filter(|s| s.width == w).collect();
        if !sub.is_empty() {
            widths.push(width_omnibus(&sub, w, horizon_bars(w), cfg)?);
        }
    }
    let k = widths.len().max(1) as f64;
    let p = (widths.iter().map(|w| w.p).fold(1.0, f64::min) * k).min(1.0);
    let statistic = widths
        .iter()
        .map(|w| w.statistic)
        .fold(f64::NEG_INFINITY, f64::max);
    Ok(OmnibusResult {
        widths,
        p,
        statistic,
        verdict: if p <= cfg.alpha_omnibus {
            Verdict::Structure
        } else {
            Verdict::Null
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(signal: bool, seed: u64) -> Vec<Sample> {
        use rand::{Rng, SeedableRng};
        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(seed);
        let nf = feature_names().len();
        (0..2400usize)
            .map(|i| {
                let x: f64 = rng.random();
                let p = if signal { 0.2 + 0.6 * x } else { 0.5 };
                let class = if rng.random::<f64>() < p {
                    Class::Rejection
                } else {
                    Class::Breakthrough
                };
                let mut features = vec![0.0; nf];
                features[1] = x;
                features[4] = rng.random();
                Sample {
                    width: 15,
                    event_id: i as u32,
                    session: (i / 5) as i32,
                    year: 2018,
                    t_close_us: i as i64 * 900_000_000,
                    t_end_us: i as i64 * 900_000_000 + 7_200_000_000,
                    side: 1,
                    class: Some(class),
                    ambiguous: false,
                    outcome_ticks: 0.0,
                    sigma_over_price: 0.005,
                    features,
                }
            })
            .collect()
    }

    fn cfg() -> ResearchConfig {
        let mut c = vwap_core::load_research_config(std::path::Path::new("../../config")).unwrap();
        c.omnibus_permutations = 30;
        c
    }

    #[test]
    fn planted_structure_is_detected_and_a_null_is_not() {
        let c = cfg();
        let s = make(true, 1);
        let r = omnibus(&s, &c, |_| 8).unwrap();
        assert!(r.statistic > 0.02, "statistic {}", r.statistic);
        assert!(r.p <= 2.0 / 31.0 + 1e-9, "p {}", r.p);
        let n = make(false, 2);
        let r0 = omnibus(&n, &c, |_| 8).unwrap();
        assert!(r0.p > 0.05, "null p {}", r0.p);
    }
}
