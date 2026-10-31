//! Anchored VWAP, ATR, coarse bar sampling and touch detection.
#![cfg_attr(
    test,
    allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)
)]

pub mod bars;
pub mod detect;
pub mod error;
pub mod io;

pub use bars::{CoarseBars, build};
pub use detect::{DropCounts, Event, EventParams, detect};
pub use error::EventsError;
pub use io::{EventsSummary, event_params, run_events};
