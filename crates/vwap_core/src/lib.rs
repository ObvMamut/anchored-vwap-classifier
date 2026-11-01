//! Shared types, typed configuration, errors and cost constants.
#![cfg_attr(
    test,
    allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)
)]

pub mod config;
pub mod consts;
pub mod error;
pub mod manifest;
pub mod strategy_config;
pub mod types;

pub use config::{
    BarLabel, BaselinesConfig, DataConfig, EventsConfig, FeaturesConfig, HhMm, LabelGrid,
    LabelsConfig, ModelConfig, ResearchConfig, SessionAnchor, SigmaKind, TimestampKind,
    VwapReference, WeightColumn, load_baselines_config, load_data_config, load_events_config,
    load_features_config, load_labels_config, load_model_config, load_research_config,
};
pub use consts::{POINT_VALUE_USD, TICK_SIZE, TICK_VALUE_USD};
pub use error::CoreError;
pub use strategy_config::{
    BacktestConfig, LgbmPoint, PositionPolicy, ReportConfig, StrategyConfig, ValidationConfig,
    WalkMode, load_backtest_config, load_report_config, load_strategy_config,
    load_validation_config, round_trip_ticks,
};
pub use types::{Class, Side, Timeframe};
