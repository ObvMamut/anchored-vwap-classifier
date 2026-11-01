//! Triple barrier labels from the touch bar close.
#![cfg_attr(
    test,
    allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)
)]

pub mod io;
pub mod label;

pub use io::{LabelIoError, LabelReport, write_labels};
pub use label::{Hit, LabelDrops, LabelParams, Labeled, Scan, label_events, scan_barriers};
