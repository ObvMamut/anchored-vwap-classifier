//! The closed research loop harness: split views, hypothesis cards, the ledger,
//! subset contrast runners and reports. See `docs/PROTOCOL.md`.
#![cfg_attr(
    test,
    allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)
)]

pub mod card;
pub mod cycle;
pub mod data;
pub mod error;
pub mod ledger;
pub mod omnibus;
pub mod report;
pub mod runner;
pub mod selftest;
pub mod split;

pub use card::{Card, Outcome, RuleKind};
pub use error::ResearchError;
pub use ledger::{Entry, Ledger, Verdict};
pub use runner::{CardResult, WidthResult, run_card};
pub use split::{
    Bounds, ConfirmPermit, ConfirmView, DiscoveryView, HoldoutPermit, HoldoutView, Sample,
};
pub use vwap_validation::math;
