//! Control levels, class frequencies and the Phase 3 gate.
#![cfg_attr(
    test,
    allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)
)]

pub mod controls;
pub mod error;
pub mod run;
pub mod stats;

pub use error::BaselinesError;
pub use run::{BaselineReport, run_baselines};
