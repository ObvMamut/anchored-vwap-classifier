//! Models, calibration and scoring for the strategy stages. Everything that scikit learn would
//! provide is written here and tested against hand computed values. See `docs/MODELING.md`.
#![cfg_attr(
    test,
    allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)
)]

pub mod calibrate;
pub mod classifier;
pub mod error;
pub mod importance;
pub mod lgbm;
pub mod matrix;
pub mod metrics;
pub mod prep;
pub mod scheme;
pub mod shap;
pub mod threshold;

pub use calibrate::{Calibrator, Isotonic, MultiCalibrator, Platt, pav};
pub use classifier::{Classifier, Logistic, Majority};
pub use error::ModelError;
pub use importance::{Stability, spearman, stability, top_m};
pub use lgbm::{Lgbm, fit_checkpoints};
pub use matrix::{Matrix, Probs};
pub use metrics::{Bin, auc, brier, bss, ece, log_loss, precision_at, reliability};
pub use prep::Preprocessor;
pub use scheme::{Family, FoldFit, Params, Table, child_seed, run_fold};
pub use shap::TreeModel;
pub use threshold::{TauChoice, TauRow, tune_tau};
