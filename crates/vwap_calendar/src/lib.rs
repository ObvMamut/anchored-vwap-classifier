//! The frozen calendar of scheduled CPI, NFP and FOMC releases. This is the only crate that
//! may use the network, and only through `vwap calendar fetch`. Every other stage reads the
//! snapshot it writes. See `docs/FEATURES.md`, section 6.
#![cfg_attr(
    test,
    allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)
)]

pub mod error;
pub mod fetch;
pub mod parse;
pub mod snapshot;
pub mod types;

pub use error::CalendarError;
pub use fetch::{Fetcher, FixtureFetcher, HttpFetcher};
pub use snapshot::{
    Counts, Fetched, Lock, PageRecord, fetch_all, import_csv, read_snapshot, validate,
    write_snapshot,
};
pub use types::{CalendarEvent, Kind};
