//! The harness controls. A planted effect must be promoted and shuffled labels must not.
//! The runner refuses to touch real data until both pass.

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use vwap_core::{Class, ResearchConfig};
use vwap_features::feature_names;

use crate::card::{Card, Direction, Outcome, RuleKind};
use crate::error::ResearchError;
use crate::ledger::Verdict;
use crate::runner::run_card;
use crate::split::Sample;

/// Synthetic samples. The feature `vslope_align` is uniform on [0, 1] and the top tercile
/// has rejection probability `0.5 + effect`.
pub fn synthetic_samples(
    effect: f64,
    sessions: usize,
    events_per_session: usize,
    seed: u64,
) -> Vec<Sample> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let n_feat = feature_names().len();
    let mut out = Vec::new();
    for s in 0..sessions {
        for w in [5u32, 15] {
            for k in 0..events_per_session {
                let x: f64 = rng.random();
                let p = 0.5 + if x > 2.0 / 3.0 { effect } else { 0.0 };
                let class = if rng.random::<f64>() < p {
                    Class::Rejection
                } else {
                    Class::Breakthrough
                };
                let mut features = vec![0.0; n_feat];
                if let Some(f) = features.first_mut() {
                    *f = x;
                }
                out.push(Sample {
                    width: w,
                    event_id: k as u32,
                    session: s as i32,
                    year: 2017 + (s % 5) as i32,
                    t_close_us: 0,
                    t_end_us: 0,
                    side: if k % 2 == 0 { 1 } else { -1 },
                    class: Some(class),
                    ambiguous: false,
                    outcome_ticks: 0.0,
                    sigma_over_price: rng.random::<f64>() * 0.01,
                    features,
                });
            }
        }
    }
    out
}

pub fn control_card() -> Card {
    Card {
        id: "SELFTEST".into(),
        family: "selftest".into(),
        mechanism: "synthetic planted effect used to validate the harness".into(),
        parent: None,
        outcome: Outcome::Rejection,
        feature: "vslope_align".into(),
        rule: RuleKind::TopTercile,
        direction: Direction::Positive,
        threshold: None,
        timeframes: vec![5, 15],
        min_effect: None,
        alpha: None,
    }
}

/// Runs the positive and negative controls. Returns an error describing any failure.
pub fn selftest(cfg: &ResearchConfig) -> Result<(), ResearchError> {
    let card = control_card();
    let mut c = cfg.clone();
    c.bootstrap_draws = c.bootstrap_draws.clamp(300, 2000);
    // positive control: a planted effect of 8 points is promoted
    let planted = synthetic_samples(0.08, 800, 5, 11);
    let r = run_card(&card, &planted, &c);
    if r.verdict != Verdict::Promote {
        return Err(ResearchError::SelfTest(format!(
            "planted effect not promoted: verdict {:?}, p {:.4}, reasons {:?}",
            r.verdict, r.p, r.reasons
        )));
    }
    // negative control: no effect, many seeds; promotions must be (almost) absent and the
    // raw one sided rate must be close to the nominal level
    let (mut promotes, mut raw_rejects) = (0, 0);
    let trials = 60;
    for seed in 0..trials {
        let null = synthetic_samples(0.0, 300, 5, 1000 + seed);
        let r = run_card(&card, &null, &c);
        promotes += usize::from(r.verdict == Verdict::Promote);
        raw_rejects += usize::from(r.p <= 0.05);
    }
    if promotes > 1 {
        return Err(ResearchError::SelfTest(format!(
            "{promotes} of {trials} null runs were promoted"
        )));
    }
    if raw_rejects > 10 {
        return Err(ResearchError::SelfTest(format!(
            "{raw_rejects} of {trials} null runs had p at or below 0.05, expected about 3"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ResearchConfig {
        vwap_core::load_research_config(std::path::Path::new("../../config")).unwrap()
    }

    #[test]
    fn the_harness_controls_pass() {
        selftest(&cfg()).unwrap();
    }

    #[test]
    fn a_group_test_detects_a_planted_group_difference_and_a_null_does_not() {
        // 4 groups on the feature value 0..3, group 2 has +10 points
        let mut rng = ChaCha8Rng::seed_from_u64(3);
        let n_feat = feature_names().len();
        let mut make = |effect: f64| -> Vec<Sample> {
            let mut out = Vec::new();
            for s in 0..600 {
                for w in [5u32, 15] {
                    for _ in 0..4 {
                        let g = rng.random_range(0..4usize);
                        let p = 0.5 + if g == 2 { effect } else { 0.0 };
                        let class = if rng.random::<f64>() < p {
                            Class::Rejection
                        } else {
                            Class::Breakthrough
                        };
                        let mut features = vec![0.0; n_feat];
                        features[16] = g as f64; // time_block
                        out.push(Sample {
                            width: w,
                            event_id: 0,
                            session: s,
                            year: 2017 + s % 5,
                            t_close_us: 0,
                            t_end_us: 0,
                            side: 1,
                            class: Some(class),
                            ambiguous: false,
                            outcome_ticks: 0.0,
                            sigma_over_price: 0.005,
                            features,
                        });
                    }
                }
            }
            out
        };
        let mut card = control_card();
        card.id = "G".into();
        card.feature = "time_block".into();
        card.rule = RuleKind::Groups;
        card.direction = Direction::Any;
        let mut c = cfg();
        c.bootstrap_draws = 600;
        let planted = run_card(&card, &make(0.10), &c);
        assert!(
            planted.p < 1e-3 && planted.delta > 0.05,
            "p {} delta {}",
            planted.p,
            planted.delta
        );
        let null = run_card(&card, &make(0.0), &c);
        assert!(null.p > 0.01, "null p {}", null.p);
    }
}
