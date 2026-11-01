//! Library behaviour tests: what the model libraries do, so an upgrade that changes it is noticed.
//!
//! Finding recorded in docs/DECISIONS.md: `lightgbm3` 1.0.11 sizes the
//! `predict_contrib` output buffer as `n_rows * (n_features + 1)`, which is too
//! small for a multiclass model (`n_rows * n_classes * (n_features + 1)`). It is
//! therefore only exercised here on a binary model, and the project computes
//! multiclass TreeSHAP itself (see ARCHITECTURE.md).
#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)]

use lightgbm3::{Booster, Dataset};
use linfa::prelude::*;
use linfa_logistic::MultiLogisticRegression;
use ndarray::{Array1, Array2};
use serde_json::json;

/// Small deterministic generator so the tests need no randomness crate.
fn lcg(state: &mut u64) -> f64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    ((*state >> 11) as f64) / ((1u64 << 53) as f64)
}

fn make_data(n: usize, classes: usize, seed: u64) -> (Vec<f64>, Vec<f32>) {
    let mut s = seed;
    let f = 4;
    let mut x = Vec::with_capacity(n * f);
    let mut y = Vec::with_capacity(n);
    for _ in 0..n {
        let row: Vec<f64> = (0..f).map(|_| lcg(&mut s) * 2.0 - 1.0).collect();
        let score = row[0] * 2.0 + row[1] - row[2] * 0.5 + (lcg(&mut s) - 0.5) * 0.5;
        let class = if classes == 2 {
            usize::from(score > 0.0)
        } else if score < -0.5 {
            0
        } else if score < 0.5 {
            1
        } else {
            2
        };
        x.extend(row);
        y.push(class as f32);
    }
    (x, y)
}

fn deterministic_params(objective: &str, classes: usize) -> serde_json::Value {
    let mut p = json!({
        "objective": objective,
        "num_iterations": 20,
        "learning_rate": 0.1,
        "num_leaves": 7,
        "min_data_in_leaf": 5,
        "verbose": -1,
        "num_threads": 1,
        "deterministic": true,
        "force_row_wise": true,
        "seed": 7,
        "bagging_seed": 7,
        "feature_fraction_seed": 7,
        "data_random_seed": 7,
    });
    if classes > 2 {
        p["num_class"] = json!(classes);
    }
    p
}

#[test]
fn lightgbm_multiclass_is_deterministic_and_probabilities_sum_to_one() {
    let (x, y) = make_data(600, 3, 1);
    let train = |x: &[f64], y: &[f32]| {
        let ds = Dataset::from_slice(x, y, 4, true).unwrap();
        Booster::train(ds, &deterministic_params("multiclass", 3)).unwrap()
    };
    let a = train(&x, &y);
    let b = train(&x, &y);
    let pa = a.predict(&x, 4, true).unwrap();
    let pb = b.predict(&x, 4, true).unwrap();
    assert_eq!(pa.len(), 600 * 3);
    assert!(pa.iter().zip(&pb).all(|(u, v)| u.to_bits() == v.to_bits()));
    for row in pa.chunks(3) {
        assert!((row.iter().sum::<f64>() - 1.0).abs() < 1e-9);
    }
    assert_eq!(a.num_classes(), 3);
}

#[test]
fn lightgbm_early_stopping_on_a_validation_set_is_available() {
    let (x, y) = make_data(600, 3, 2);
    let (xv, yv) = make_data(200, 3, 3);
    let train = Dataset::from_slice(&x, &y, 4, true).unwrap();
    let valid = Dataset::from_slice_with_reference(&xv, &yv, 4, true, Some(&train)).unwrap();
    let mut p = deterministic_params("multiclass", 3);
    p["num_iterations"] = json!(500);
    p["early_stopping_rounds"] = json!(10);
    p["metric"] = json!("multi_logloss");
    let bst = Booster::train_with_valid(train, Some(valid), &p).unwrap();
    assert!(bst.num_iterations() < 500, "early stopping did not trigger");
}

#[test]
fn lightgbm_binary_treeshap_is_additive() {
    let (x, y) = make_data(400, 2, 4);
    let ds = Dataset::from_slice(&x, &y, 4, true).unwrap();
    let bst = Booster::train(ds, &deterministic_params("binary", 2)).unwrap();
    let contrib = bst.predict_contrib(&x, 4, true).unwrap();
    let raw = bst.raw_scores(&x, 4, true).unwrap();
    assert_eq!(contrib.len(), 400 * 5);
    for (row, r) in contrib.chunks(5).zip(&raw) {
        assert!((row.iter().sum::<f64>() - r).abs() < 1e-9);
    }
}

#[test]
fn linfa_multinomial_logistic_regression_works() {
    let (x, y) = make_data(600, 3, 5);
    let xs = Array2::from_shape_vec((600, 4), x).unwrap();
    let ys: Array1<usize> = y.iter().map(|&v| v as usize).collect();
    let ds = linfa::Dataset::new(xs.clone(), ys.clone());
    let model = MultiLogisticRegression::default()
        .alpha(1.0)
        .max_iterations(200)
        .fit(&ds)
        .unwrap();
    let p = model.predict_probabilities(&xs);
    assert_eq!(p.dim(), (600, 3));
    for row in p.rows() {
        assert!((row.sum() - 1.0).abs() < 1e-9);
    }
    let acc = p
        .rows()
        .into_iter()
        .zip(ys.iter())
        .filter(|(r, t)| {
            let arg = r
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                .unwrap()
                .0;
            arg == **t
        })
        .count() as f64
        / 600.0;
    assert!(acc > 0.7, "accuracy {acc}");
}
