//! Typed configuration deserialised from `config/*.toml`.

use std::path::Path;

use chrono::NaiveTime;
use serde::Deserialize;

use crate::error::CoreError;

/// Session anchor kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionAnchor {
    Globex,
    Rth,
}

/// How the `DateTime` column maps to real time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimestampKind {
    /// True UTC.
    Utc,
    /// Naive New York local time.
    NyLocal,
    /// Broker server time that equals New York local time plus a fixed number of
    /// hours, with DST changes aligned to the US rules.
    ServerNySynced,
}

/// Which column weights the VWAP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeightColumn {
    Volume,
    TickVolume,
}

/// Whether the `DateTime` column marks the bar open or the bar close.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BarLabel {
    Open,
    Close,
}

/// A clock time written as "HH:MM" in the config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct HhMm(pub NaiveTime);

impl TryFrom<String> for HhMm {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        NaiveTime::parse_from_str(&s, "%H:%M")
            .map(HhMm)
            .map_err(|e| format!("bad HH:MM value {s:?}: {e}"))
    }
}

/// `config/data.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataConfig {
    pub input_path: String,
    pub output_dir: String,
    pub artifact_dir: String,
    pub timestamp_kind: TimestampKind,
    pub server_offset_hours: i64,
    pub bar_label: BarLabel,
    pub session_anchor: SessionAnchor,
    pub weight_column: WeightColumn,
    pub price_grid: f64,
    pub drop_invalid_bars: bool,
    pub roll_offset_days: i64,
    pub roll_buffer_sessions: u32,
    pub roll_confirm_percentile: f64,
    pub roll_search_window_days: i64,
    pub max_gap_minutes_ok: u32,
    pub max_gap_minutes_event: u32,
    pub outlier_z: f64,
    pub outlier_window_bars: usize,
    pub max_range_median_multiple: f64,
    pub min_bars_fraction: f64,
    pub early_close_time: HhMm,
    pub flat_time_globex: HhMm,
    pub flat_time_rth: HhMm,
}

impl DataConfig {
    pub fn validate(&self) -> Result<(), CoreError> {
        if !(0.0..=1.0).contains(&self.min_bars_fraction) {
            return Err(CoreError::ConfigInvalid(
                "min_bars_fraction must be in [0, 1]".into(),
            ));
        }
        if !(0.0..=1.0).contains(&self.roll_confirm_percentile) {
            return Err(CoreError::ConfigInvalid(
                "roll_confirm_percentile must be in [0, 1]".into(),
            ));
        }
        if self.outlier_z <= 0.0 || self.outlier_window_bars < 30 {
            return Err(CoreError::ConfigInvalid(
                "outlier_z must be positive and outlier_window_bars at least 30".into(),
            ));
        }
        Ok(())
    }
}

/// Reads and validates `<dir>/data.toml`.
pub fn load_data_config(dir: &Path) -> Result<DataConfig, CoreError> {
    let path = dir.join("data.toml");
    let text = std::fs::read_to_string(&path).map_err(|source| CoreError::ConfigIo {
        path: path.display().to_string(),
        source,
    })?;
    let cfg: DataConfig = toml::from_str(&text).map_err(|source| CoreError::ConfigParse {
        path: path.display().to_string(),
        source,
    })?;
    cfg.validate()?;
    Ok(cfg)
}
