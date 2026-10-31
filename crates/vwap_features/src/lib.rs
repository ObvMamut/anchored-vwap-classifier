//! Features known at the touch. Every feature function receives a `CausalView`
//! that cannot read any bar after the touch bar.
#![cfg_attr(
    test,
    allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)
)]

pub mod features;
pub mod levels;
pub mod view;

pub use features::{FeatureTable, build_features, feature_names};
pub use levels::LevelTable;
pub use view::{CausalView, SessionIndex};
