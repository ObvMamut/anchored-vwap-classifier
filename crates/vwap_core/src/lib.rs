//! Shared types, typed configuration, errors and cost constants.
#![cfg_attr(
    test,
    allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)
)]

pub mod config;
pub mod consts;
pub mod error;
pub mod manifest;
pub mod types;

pub use config::{
    BarLabel, BaselinesConfig, DataConfig, EventsConfig, HhMm, LabelGrid, LabelsConfig,
    ResearchConfig, SessionAnchor, SigmaKind, TimestampKind, VwapReference, WeightColumn,
    load_baselines_config, load_data_config, load_events_config, load_labels_config,
    load_research_config,
};
pub use consts::{POINT_VALUE_USD, TICK_SIZE, TICK_VALUE_USD};
pub use error::CoreError;
pub use types::{Class, Side, Timeframe};
