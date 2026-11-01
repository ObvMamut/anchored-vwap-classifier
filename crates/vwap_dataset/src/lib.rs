//! The joined table of events, labels and features that every later stage reads: the research
//! harness, the models and the backtest. It is built from the cleaned bars and can be exported
//! to parquet. See `docs/FEATURES.md`, section on the export.
#![cfg_attr(
    test,
    allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)
)]

mod assemble;
mod error;
mod export;
mod sample;

pub use assemble::assemble;
pub use error::DatasetError;
pub use export::{read_samples, samples_frame, write_features};
pub use sample::Sample;
