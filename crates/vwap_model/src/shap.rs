//! Exact path dependent TreeSHAP for multiclass LightGBM, in safe Rust.
//!
//! `lightgbm3` 1.0.11 sizes the multiclass `predict_contrib` buffer as `n (F + 1)` instead of
//! `n K (F + 1)`, so the contributions are computed here from the text model
//! (`Booster::save_string`). The algorithm is Algorithm 2 of Lundberg, Erion and Lee (2018)
//! as implemented by LightGBM itself, with covers taken from the node data counts. The
//! verification against LightGBM's own `predict_contrib` on binary models is in the tests.
//!
//! For every class $k$ the contributions satisfy
//! $\phi_{k,0}+\sum_f\phi_{k,f}=s_k(x)$ with $s_k$ the raw score of class $k$.
// Indices come from arrays whose lengths and child ranges `TreeModel::parse` has validated.
#![allow(clippy::indexing_slicing)]

use std::collections::HashMap;

use crate::error::ModelError;

const ZERO_THRESHOLD: f64 = 1e-35;

#[derive(Debug, Clone)]
struct Tree {
    split_feature: Vec<usize>,
    threshold: Vec<f64>,
    decision_type: Vec<u8>,
    left: Vec<i32>,
    right: Vec<i32>,
    leaf_value: Vec<f64>,
    leaf_count: Vec<f64>,
    internal_count: Vec<f64>,
}

#[derive(Debug, Clone)]
pub struct TreeModel {
    n_class: usize,
    n_features: usize,
    trees: Vec<Tree>,
}

fn bad(msg: impl Into<String>) -> ModelError {
    ModelError::Fit(format!("model text: {}", msg.into()))
}

fn nums<T: std::str::FromStr>(m: &HashMap<&str, &str>, key: &str) -> Result<Vec<T>, ModelError> {
    match m.get(key) {
        None => Ok(Vec::new()),
        Some(v) => v
            .split_whitespace()
            .map(|s| {
                s.parse::<T>()
                    .map_err(|_| bad(format!("bad number in {key}")))
            })
            .collect(),
    }
}

impl TreeModel {
    /// Parses the text model of a LightGBM booster.
    pub fn parse(text: &str) -> Result<Self, ModelError> {
        let mut header: HashMap<&str, &str> = HashMap::new();
        let mut blocks: Vec<HashMap<&str, &str>> = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with("end of trees") {
                break;
            }
            if line.starts_with("Tree=") {
                blocks.push(HashMap::new());
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                match blocks.last_mut() {
                    Some(b) => {
                        b.insert(k, v);
                    }
                    None => {
                        header.insert(k, v);
                    }
                }
            }
        }
        let scalar = |m: &HashMap<&str, &str>, k: &str| -> Result<usize, ModelError> {
            m.get(k)
                .and_then(|v| v.trim().parse().ok())
                .ok_or_else(|| bad(format!("missing {k}")))
        };
        // a binary model has no num_class line and one tree per iteration
        let n_class = scalar(&header, "num_class")?;
        let per_iter = scalar(&header, "num_tree_per_iteration")?;
        if n_class != per_iter {
            return Err(bad("num_class and num_tree_per_iteration differ"));
        }
        let n_features = scalar(&header, "max_feature_idx")? + 1;
        let mut trees = Vec::with_capacity(blocks.len());
        for b in &blocks {
            if scalar(b, "num_cat")? != 0 {
                return Err(bad("categorical splits are not supported"));
            }
            let n_leaves = scalar(b, "num_leaves")?;
            let t = Tree {
                split_feature: nums(b, "split_feature")?,
                threshold: nums(b, "threshold")?,
                decision_type: nums(b, "decision_type")?,
                left: nums(b, "left_child")?,
                right: nums(b, "right_child")?,
                leaf_value: nums(b, "leaf_value")?,
                leaf_count: nums(b, "leaf_count")?,
                internal_count: nums(b, "internal_count")?,
            };
            let n_int = n_leaves.saturating_sub(1);
            let ok = t.leaf_value.len() == n_leaves
                && t.split_feature.len() == n_int
                && t.threshold.len() == n_int
                && t.decision_type.len() == n_int
                && t.left.len() == n_int
                && t.right.len() == n_int
                && t.internal_count.len() == n_int
                && (n_leaves == 1 || t.leaf_count.len() == n_leaves)
                && t.split_feature.iter().all(|&f| f < n_features)
                && t.left.iter().chain(&t.right).all(|&c| {
                    if c >= 0 {
                        (c as usize) < n_int
                    } else {
                        ((!c) as usize) < n_leaves
                    }
                });
            if !ok {
                return Err(bad("inconsistent tree arrays"));
            }
            trees.push(t);
        }
        if n_class == 0 || trees.is_empty() || !trees.len().is_multiple_of(n_class) {
            return Err(bad("tree count does not match the number of classes"));
        }
        Ok(Self {
            n_class,
            n_features,
            trees,
        })
    }

    pub fn n_classes(&self) -> usize {
        self.n_class
    }

    pub fn n_features(&self) -> usize {
        self.n_features
    }

    /// Raw scores of one row, one per class.
    pub fn raw_scores(&self, x: &[f64]) -> Vec<f64> {
        let mut s = vec![0.0; self.n_class];
        for (i, t) in self.trees.iter().enumerate() {
            s[i % self.n_class] += t.predict(x);
        }
        s
    }

    /// Contributions of one row: for class $k$ the entries `k * (F + 1) .. (k + 1) * (F + 1)`
    /// hold $\phi_{k,f}$ for each feature followed by the bias term (expected raw score).
    pub fn shap(&self, x: &[f64]) -> Vec<f64> {
        let w = self.n_features + 1;
        let mut phi = vec![0.0; self.n_class * w];
        for (i, t) in self.trees.iter().enumerate() {
            let k = i % self.n_class;
            t.shap(x, &mut phi[k * w..(k + 1) * w], self.n_features);
        }
        phi
    }
}

#[derive(Debug, Clone, Copy)]
struct PathEl {
    feature: i64,
    zero: f64,
    one: f64,
    pweight: f64,
}

fn extend(path: &mut Vec<PathEl>, d: usize, z: f64, o: f64, f: i64) {
    path.push(PathEl {
        feature: f,
        zero: z,
        one: o,
        pweight: if d == 0 { 1.0 } else { 0.0 },
    });
    let d1 = (d + 1) as f64;
    for i in (0..d).rev() {
        path[i + 1].pweight += o * path[i].pweight * (i + 1) as f64 / d1;
        path[i].pweight = z * path[i].pweight * (d - i) as f64 / d1;
    }
}

fn unwind(path: &mut Vec<PathEl>, d: usize, idx: usize) {
    let (o, z) = (path[idx].one, path[idx].zero);
    let mut next = path[d].pweight;
    let d1 = (d + 1) as f64;
    for i in (0..d).rev() {
        if o != 0.0 {
            let tmp = path[i].pweight;
            path[i].pweight = next * d1 / ((i + 1) as f64 * o);
            next = tmp - path[i].pweight * z * (d - i) as f64 / d1;
        } else {
            path[i].pweight = path[i].pweight * d1 / (z * (d - i) as f64);
        }
    }
    for i in idx..d {
        path[i].feature = path[i + 1].feature;
        path[i].zero = path[i + 1].zero;
        path[i].one = path[i + 1].one;
    }
    path.pop();
}

fn unwound_sum(path: &[PathEl], d: usize, idx: usize) -> f64 {
    let (o, z) = (path[idx].one, path[idx].zero);
    let mut next = path[d].pweight;
    let d1 = (d + 1) as f64;
    let mut total = 0.0;
    for i in (0..d).rev() {
        if o != 0.0 {
            let tmp = next * d1 / ((i + 1) as f64 * o);
            total += tmp;
            next = path[i].pweight - tmp * z * (d - i) as f64 / d1;
        } else {
            total += (path[i].pweight / z) / ((d - i) as f64 / d1);
        }
    }
    total
}

impl Tree {
    /// LightGBM's numerical decision: missing handling by the `decision_type` bits.
    fn go_left(&self, node: usize, x: &[f64]) -> bool {
        let dt = self.decision_type[node];
        let default_left = (dt >> 1) & 1 == 1;
        let missing = (dt >> 2) & 3;
        let mut v = x[self.split_feature[node]];
        if v.is_nan() && missing != 2 {
            v = 0.0;
        }
        if (missing == 1 && v.abs() <= ZERO_THRESHOLD) || (missing == 2 && v.is_nan()) {
            return default_left;
        }
        v <= self.threshold[node]
    }

    fn count(&self, child: i32) -> f64 {
        if child >= 0 {
            self.internal_count[child as usize]
        } else {
            self.leaf_count[(!child) as usize]
        }
    }

    fn predict(&self, x: &[f64]) -> f64 {
        if self.split_feature.is_empty() {
            return self.leaf_value[0];
        }
        let mut node = 0usize;
        loop {
            let c = if self.go_left(node, x) {
                self.left[node]
            } else {
                self.right[node]
            };
            if c < 0 {
                return self.leaf_value[(!c) as usize];
            }
            node = c as usize;
        }
    }

    fn expected(&self) -> f64 {
        if self.split_feature.is_empty() {
            return self.leaf_value[0];
        }
        let total = self.internal_count[0];
        self.leaf_value
            .iter()
            .zip(&self.leaf_count)
            .map(|(v, c)| v * c)
            .sum::<f64>()
            / total
    }

    fn shap(&self, x: &[f64], phi: &mut [f64], n_features: usize) {
        phi[n_features] += self.expected();
        if self.split_feature.is_empty() {
            return;
        }
        self.recurse(x, phi, 0, 0, &[], 1.0, 1.0, -1);
    }

    #[allow(clippy::too_many_arguments)]
    fn recurse(
        &self,
        x: &[f64],
        phi: &mut [f64],
        node: i32,
        depth: usize,
        parent: &[PathEl],
        pz: f64,
        po: f64,
        pf: i64,
    ) {
        let mut path = parent.to_vec();
        extend(&mut path, depth, pz, po, pf);
        if node < 0 {
            let leaf = self.leaf_value[(!node) as usize];
            for i in 1..=depth {
                let w = unwound_sum(&path, depth, i);
                let el = path[i];
                phi[el.feature as usize] += w * (el.one - el.zero) * leaf;
            }
            return;
        }
        let n = node as usize;
        let (hot, cold) = if self.go_left(n, x) {
            (self.left[n], self.right[n])
        } else {
            (self.right[n], self.left[n])
        };
        let total = self.internal_count[n];
        let (hot_zero, cold_zero) = (self.count(hot) / total, self.count(cold) / total);
        let (mut iz, mut io) = (1.0, 1.0);
        let mut d = depth;
        let feat = self.split_feature[n] as i64;
        if let Some(k) = (0..=d).find(|&k| path[k].feature == feat) {
            iz = path[k].zero;
            io = path[k].one;
            unwind(&mut path, d, k);
            d -= 1;
        }
        self.recurse(x, phi, hot, d + 1, &path, hot_zero * iz, io, feat);
        self.recurse(x, phi, cold, d + 1, &path, cold_zero * iz, 0.0, feat);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classifier::Classifier;
    use crate::lgbm::Lgbm;
    use crate::matrix::Matrix;
    use lightgbm3::{Booster, Dataset};
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha8Rng;
    use serde_json::json;
    use vwap_core::StrategyConfig;

    fn cfg() -> StrategyConfig {
        vwap_core::load_strategy_config(std::path::Path::new("../../config")).unwrap()
    }

    fn data(n: usize, seed: u64, classes: usize) -> (Matrix, Vec<usize>) {
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let mut rows = Vec::new();
        let mut y = Vec::new();
        for _ in 0..n {
            let a: f64 = rng.random::<f64>() * 2.0 - 1.0;
            let b: f64 = rng.random::<f64>() * 2.0 - 1.0;
            let score = a * 2.0 + b - 0.4 * rng.random::<f64>();
            y.push(if classes == 2 {
                usize::from(score > 0.0)
            } else if score < -0.6 {
                0
            } else if score < 0.6 {
                1
            } else {
                2
            });
            rows.push(vec![
                a,
                b,
                rng.random::<f64>(),
                // a column with missing values and a column with exact zeros
                if rng.random::<f64>() < 0.2 {
                    f64::NAN
                } else {
                    a * b
                },
                if rng.random::<f64>() < 0.3 {
                    0.0
                } else {
                    rng.random::<f64>()
                },
            ]);
        }
        (Matrix::from_rows(&rows), y)
    }

    #[test]
    fn it_matches_lightgbm_predict_contrib_on_a_binary_model() {
        let (x, y) = data(500, 1, 2);
        let labels: Vec<f32> = y.iter().map(|&k| k as f32).collect();
        let ds = Dataset::from_slice(&x.data, &labels, x.p as i32, true).unwrap();
        let p = json!({
            "objective": "binary", "num_iterations": 25, "learning_rate": 0.1,
            "num_leaves": 9, "min_data_in_leaf": 10, "verbose": -1, "num_threads": 1,
            "deterministic": true, "force_row_wise": true, "seed": 3,
        });
        let bst = Booster::train(ds, &p).unwrap();
        let contrib = bst.predict_contrib(&x.data, x.p as i32, true).unwrap();
        let raw = bst.raw_scores(&x.data, x.p as i32, true).unwrap();
        let model = TreeModel::parse(&bst.save_string().unwrap()).unwrap();
        assert_eq!(model.n_classes(), 1);
        let w = x.p + 1;
        for i in 0..x.n {
            let mine = model.shap(x.row(i));
            for j in 0..w {
                assert!(
                    (mine[j] - contrib[i * w + j]).abs() < 1e-9,
                    "row {i} col {j}: {} vs {}",
                    mine[j],
                    contrib[i * w + j]
                );
            }
            assert!((model.raw_scores(x.row(i))[0] - raw[i]).abs() < 1e-9);
        }
    }

    #[test]
    fn multiclass_contributions_add_up_to_the_raw_score_of_every_class() {
        let c = cfg();
        let (x, y) = data(600, 2, 3);
        let mut m = Lgbm::new(&c.lgbm_grid[2], 40, c.seed);
        m.fit(&x, &y).unwrap();
        let model = TreeModel::parse(&m.model_string().unwrap()).unwrap();
        assert_eq!(model.n_classes(), 3);
        let lgbm_raw = m.raw_scores(&x).unwrap();
        let w = x.p + 1;
        for (i, expect) in lgbm_raw.iter().enumerate() {
            let phi = model.shap(x.row(i));
            let mine = model.raw_scores(x.row(i));
            for k in 0..3 {
                let sum: f64 = phi[k * w..(k + 1) * w].iter().sum();
                assert!((sum - expect[k]).abs() < 1e-9, "row {i} class {k}");
                assert!((mine[k] - expect[k]).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn a_model_without_splits_has_only_a_bias() {
        let c = cfg();
        let (x, y) = data(300, 4, 3);
        // min_data_in_leaf above the sample size: no tree can split
        let mut point = c.lgbm_grid[0].clone();
        point.min_data_in_leaf = 10_000;
        let mut m = Lgbm::new(&point, 5, c.seed);
        m.fit(&x, &y).unwrap();
        let model = TreeModel::parse(&m.model_string().unwrap()).unwrap();
        let phi = model.shap(x.row(0));
        let w = x.p + 1;
        for k in 0..3 {
            assert!(phi[k * w..k * w + x.p].iter().all(|&v| v == 0.0));
        }
    }

    #[test]
    fn broken_model_text_is_rejected() {
        assert!(TreeModel::parse("nonsense").is_err());
        assert!(
            TreeModel::parse("num_class=3\nnum_tree_per_iteration=3\nmax_feature_idx=1\n").is_err()
        );
    }
}
