//! Models, calibration and scoring for the strategy stages. Everything that scikit learn would
//! provide is written here and tested against hand computed values. See `docs/MODELING.md`.
#![cfg_attr(
    test,
    allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)
)]

pub mod calibrate;
pub mod classifier;
pub mod error;
pub mod matrix;
pub mod metrics;
pub mod prep;

pub use calibrate::{Calibrator, Isotonic, MultiCalibrator, Platt, pav};
pub use classifier::{Classifier, Logistic, Majority};
pub use error::ModelError;
pub use matrix::{Matrix, Probs};
pub use metrics::{Bin, auc, brier, bss, ece, log_loss, precision_at, reliability};
pub use prep::Preprocessor;
