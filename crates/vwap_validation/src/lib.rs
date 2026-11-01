//! Validation maths: purged walk forward splits, Sharpe statistics, the Deflated Sharpe Ratio
//! and the Probability of Backtest Overfitting. Everything is hand written and tested against
//! reference values; nothing here knows about NQ or about the data layout.
#![cfg_attr(
    test,
    allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)
)]

pub mod bootstrap;
pub mod dsr;
pub mod math;
pub mod pbo;
pub mod splits;
pub mod stats;

pub use bootstrap::{cluster_draws, interval, percentile};
pub use dsr::{dsr, expected_max_sharpe, probabilistic_sharpe};
pub use pbo::{PboResult, pbo};
pub use splits::{
    EventSpan, InnerFold, Mode, Split, WalkForwardConfig, blocked_expanding, shift_months,
    walk_forward,
};
pub use stats::{kurtosis, mean, sample_sd, sharpe, skewness};
