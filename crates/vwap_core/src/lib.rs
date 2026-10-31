//! Shared types, typed configuration, errors and cost constants.
#![cfg_attr(
    test,
    allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)
)]

pub mod config;
pub mod consts;
pub mod error;
pub mod types;

pub use config::{
    BarLabel, DataConfig, EventsConfig, HhMm, SessionAnchor, TimestampKind, ValidationConfig,
    VwapReference, WeightColumn, load_data_config, load_events_config, load_validation_config,
};
pub use consts::{POINT_VALUE_USD, TICK_SIZE, TICK_VALUE_USD};
pub use error::CoreError;
pub use types::{Class, Side, Timeframe};
