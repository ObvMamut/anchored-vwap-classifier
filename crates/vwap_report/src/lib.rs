//! Charts and reports.
#![cfg_attr(
    test,
    allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)
)]

pub mod error;
pub mod spotcheck;

pub use error::ReportError;
