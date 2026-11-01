//! Cost aware trade simulation. Entry at the open of the bar after the touch, exits at the
//! labeling barriers, slippage on every fill, one contract. See `docs/BACKTEST.md`.
#![cfg_attr(
    test,
    allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)
)]

pub mod metrics;
pub mod sim;

pub use metrics::{
    ExitMix, Metrics, SweepCell, break_even_slippage, daily_pnl, expectancy_interval, metrics,
    sessions_in, sweep,
};
pub use sim::{ExitReason, RunOutcome, Signal, Trade, run, simulate};
