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
    /// SHA256 of the raw file. When set, `data clean` refuses a file that differs, so every
    /// recorded result stays tied to one dataset.
    #[serde(default)]
    pub input_sha256: Option<String>,
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
    /// Bars of a normal full Globex session, used to flag thin sessions.
    pub expected_session_bars: f64,
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

/// Reference VWAP used in the touch test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VwapReference {
    /// VWAP at the close of the touch bar (default).
    BarClose,
    /// VWAP at the close of the previous bar, a level known before the bar starts.
    PriorBarClose,
}

/// `config/events.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventsConfig {
    pub timeframes: Vec<u32>,
    pub atr_length: usize,
    pub rearm_k: f64,
    pub exclude_minutes: u32,
    pub vwap_reference: VwapReference,
    pub output_dir: String,
    pub spotcheck_days: usize,
    pub spotcheck_seed: u64,
    pub spotcheck_dir: String,
    /// Candidate font files for the charts, tried in order.
    pub font_paths: Vec<String>,
    /// Sessions shorter than this many minutes are not drawn.
    pub spotcheck_min_session_minutes: u32,
}

impl EventsConfig {
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.timeframes.iter().any(|w| ![5, 15].contains(w)) || self.timeframes.is_empty() {
            return Err(CoreError::ConfigInvalid(
                "timeframes must be a non empty subset of 5 and 15".into(),
            ));
        }
        if self.atr_length < 2 || self.rearm_k < 0.0 {
            return Err(CoreError::ConfigInvalid(
                "atr_length must be at least 2 and rearm_k non negative".into(),
            ));
        }
        Ok(())
    }
}

fn read_toml<T: serde::de::DeserializeOwned>(dir: &Path, name: &str) -> Result<T, CoreError> {
    let path = dir.join(name);
    let text = std::fs::read_to_string(&path).map_err(|source| CoreError::ConfigIo {
        path: path.display().to_string(),
        source,
    })?;
    toml::from_str(&text).map_err(|source| CoreError::ConfigParse {
        path: path.display().to_string(),
        source,
    })
}

/// Reads and validates `<dir>/events.toml`.
pub fn load_events_config(dir: &Path) -> Result<EventsConfig, CoreError> {
    let cfg: EventsConfig = read_toml(dir, "events.toml")?;
    cfg.validate()?;
    Ok(cfg)
}

/// Volatility scale for the barriers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SigmaKind {
    Atr,
}

/// Sensitivity grid, descriptive statistics only.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LabelGrid {
    pub m1: Vec<f64>,
    pub m2: Vec<f64>,
    pub horizon_5m: Vec<usize>,
    pub horizon_15m: Vec<usize>,
}

/// `config/labels.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LabelsConfig {
    pub sigma: SigmaKind,
    pub m1: f64,
    pub m2: f64,
    pub horizon_5m: usize,
    pub horizon_15m: usize,
    pub max_ambiguous_share: f64,
    pub output_dir: String,
    pub grid: LabelGrid,
}

impl LabelsConfig {
    pub fn horizon(&self, width: u32) -> usize {
        if width == 5 {
            self.horizon_5m
        } else {
            self.horizon_15m
        }
    }

    pub fn validate(&self) -> Result<(), CoreError> {
        if self.m1 <= 0.0 || self.m2 <= 0.0 || self.horizon_5m == 0 || self.horizon_15m == 0 {
            return Err(CoreError::ConfigInvalid(
                "m1, m2 and the horizons must be positive".into(),
            ));
        }
        Ok(())
    }
}

pub fn load_labels_config(dir: &Path) -> Result<LabelsConfig, CoreError> {
    let cfg: LabelsConfig = read_toml(dir, "labels.toml")?;
    cfg.validate()?;
    Ok(cfg)
}

/// `config/baselines.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaselinesConfig {
    pub seed: u64,
    pub random_levels_per_session: u32,
    pub random_level_z_max: f64,
    pub shifted_anchor_minutes: Vec<u32>,
    /// Width of the time of day strata, in minutes (60 gives 24 buckets).
    pub stratum_minutes: u32,
    pub min_stratum_events: usize,
    pub bootstrap_draws: usize,
    pub permutations: usize,
    pub delta_min: f64,
    pub alpha: f64,
    pub output_dir: String,
    pub report_dir: String,
}

pub fn load_baselines_config(dir: &Path) -> Result<BaselinesConfig, CoreError> {
    read_toml(dir, "baselines.toml")
}

/// `config/research.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchConfig {
    pub discovery_start: String,
    pub discovery_end: String,
    pub confirm_start: String,
    pub confirm_end: String,
    pub holdout_start: String,
    pub cards_dir: String,
    pub ledger_path: String,
    pub report_dir: String,
    pub alpha_total: f64,
    pub alpha_omnibus: f64,
    pub max_cards: usize,
    pub default_min_effect: f64,
    pub refine_p_max: f64,
    pub promote_min_years_same_sign: usize,
    pub min_events_per_arm: usize,
    /// Round trip cost in ticks used by `net_ticks` outcomes.
    pub cost_ticks: f64,
    pub bootstrap_draws: usize,
    pub omnibus_permutations: usize,
    pub seed: u64,
    /// Fewer discovery samples than this make a card run refuse to start.
    pub min_discovery_samples: usize,
    /// Harness controls: size of the planted effect, null trials, and how many false
    /// promotions or raw rejections the null trials may show.
    pub selftest_planted_effect: f64,
    pub selftest_null_trials: usize,
    pub selftest_max_null_promotes: usize,
    pub selftest_max_null_raw_rejects: usize,
}

impl ResearchConfig {
    /// Alpha share of one targeted card.
    pub fn alpha_per_card(&self) -> f64 {
        (self.alpha_total - self.alpha_omnibus) / self.max_cards as f64
    }

    pub fn validate(&self) -> Result<(), CoreError> {
        if self.alpha_omnibus >= self.alpha_total || self.max_cards == 0 {
            return Err(CoreError::ConfigInvalid(
                "alpha_omnibus must be below alpha_total and max_cards positive".into(),
            ));
        }
        let parse = |s: &str| {
            chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .map_err(|e| CoreError::ConfigInvalid(format!("bad date {s:?}: {e}")))
        };
        let (a, b, c, d, e) = (
            parse(&self.discovery_start)?,
            parse(&self.discovery_end)?,
            parse(&self.confirm_start)?,
            parse(&self.confirm_end)?,
            parse(&self.holdout_start)?,
        );
        if !(a < b && b < c && c < d && d < e) {
            return Err(CoreError::ConfigInvalid(
                "split dates must be strictly increasing: discovery, confirmation, holdout".into(),
            ));
        }
        Ok(())
    }

    /// UTC microseconds at 00:00 of a split date.
    pub fn date_us(s: &str) -> i64 {
        chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .ok()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map_or(0, |d| d.and_utc().timestamp_micros())
    }
}

pub fn load_research_config(dir: &Path) -> Result<ResearchConfig, CoreError> {
    let cfg: ResearchConfig = read_toml(dir, "research.toml")?;
    cfg.validate()?;
    Ok(cfg)
}

/// `config/features.toml`. The defaults are the values used for every recorded result, and a
/// test checks that the file in `config/` equals them.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeaturesConfig {
    /// Window of the VWAP slope feature, in minutes.
    pub slope_minutes: usize,
    /// Distance in bars of the approach speed feature.
    pub speed_bars: usize,
    /// Earlier sessions used for relative volume.
    pub rel_volume_sessions: usize,
    /// Fewer earlier sessions than this make relative volume undefined.
    pub rel_volume_min_sessions: usize,
    /// A level counts as coinciding with the VWAP within this many sigma.
    pub confluence_band_sigma: f64,
    /// Regular trading hours, New York minutes of day.
    pub rth_open_minute: u16,
    pub rth_close_minute: u16,
    /// The Globex anchor, New York minute of day.
    pub globex_open_minute: u32,
    /// Four edges of the three daytime time blocks; everything else is block 3.
    pub time_block_edges: [u32; 4],
    /// Scale of the volatility ratio feature.
    pub vol_ratio_scale: f64,
    /// Where `vwap features build` writes the exported feature tables.
    pub output_dir: String,
}

impl Default for FeaturesConfig {
    fn default() -> Self {
        Self {
            slope_minutes: 60,
            speed_bars: 3,
            rel_volume_sessions: 20,
            rel_volume_min_sessions: 10,
            confluence_band_sigma: 0.5,
            rth_open_minute: 570,
            rth_close_minute: 960,
            globex_open_minute: 1080,
            time_block_edges: [570, 630, 840, 1020],
            vol_ratio_scale: 10_000.0,
            output_dir: "artifacts/features".into(),
        }
    }
}

impl FeaturesConfig {
    pub fn validate(&self) -> Result<(), CoreError> {
        let e = &self.time_block_edges;
        if self.slope_minutes == 0
            || self.speed_bars == 0
            || self.rel_volume_min_sessions > self.rel_volume_sessions
            || !(e[0] < e[1] && e[1] < e[2] && e[2] < e[3])
            || self.rth_open_minute >= self.rth_close_minute
        {
            return Err(CoreError::ConfigInvalid(
                "features.toml: windows must be positive, minimum sessions at most the window, and time block edges increasing".into(),
            ));
        }
        Ok(())
    }
}

pub fn load_features_config(dir: &Path) -> Result<FeaturesConfig, CoreError> {
    let cfg: FeaturesConfig = read_toml(dir, "features.toml")?;
    cfg.validate()?;
    Ok(cfg)
}

/// `config/model.toml`: the fixed LightGBM settings of the omnibus screen. Deterministic
/// execution settings (single thread, fixed seeds) are policy and stay in code.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelConfig {
    /// Blocks of the expanding blocked cross validation.
    pub blocks: usize,
    pub rounds: i64,
    pub learning_rate: f64,
    pub num_leaves: i64,
    pub min_data_in_leaf: i64,
    pub feature_fraction: f64,
    pub lambda_l2: f64,
    /// Fewer usable rows than this make the screen refuse to run.
    pub min_rows: usize,
    /// A fold with fewer training rows than this is skipped.
    pub min_train_rows: usize,
    /// Permutations for the descriptive decomposition.
    pub diagnostic_permutations: usize,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            blocks: 6,
            rounds: 60,
            learning_rate: 0.05,
            num_leaves: 7,
            min_data_in_leaf: 40,
            feature_fraction: 0.8,
            lambda_l2: 5.0,
            min_rows: 1500,
            min_train_rows: 300,
            diagnostic_permutations: 500,
        }
    }
}

pub fn load_model_config(dir: &Path) -> Result<ModelConfig, CoreError> {
    let cfg: ModelConfig = read_toml(dir, "model.toml")?;
    if cfg.blocks < 3 || cfg.rounds < 1 || cfg.num_leaves < 2 {
        return Err(CoreError::ConfigInvalid(
            "model.toml: at least 3 blocks, 1 round and 2 leaves are required".into(),
        ));
    }
    Ok(cfg)
}

#[cfg(test)]
mod config_file_tests {
    use super::*;

    fn dir() -> &'static Path {
        Path::new("../../config")
    }

    #[test]
    fn every_config_file_in_the_repository_loads_and_validates() {
        load_data_config(dir()).unwrap();
        load_events_config(dir()).unwrap();
        load_labels_config(dir()).unwrap();
        load_baselines_config(dir()).unwrap();
        load_research_config(dir()).unwrap();
        load_features_config(dir()).unwrap();
        load_model_config(dir()).unwrap();
    }

    #[test]
    fn features_and_model_files_equal_the_values_behind_the_recorded_results() {
        assert_eq!(
            load_features_config(dir()).unwrap(),
            FeaturesConfig::default()
        );
        assert_eq!(load_model_config(dir()).unwrap(), ModelConfig::default());
    }

    #[test]
    fn invalid_values_are_rejected() {
        let f = FeaturesConfig {
            time_block_edges: [570, 570, 840, 1020],
            ..FeaturesConfig::default()
        };
        assert!(f.validate().is_err());
        let f = FeaturesConfig {
            rel_volume_min_sessions: 30,
            ..FeaturesConfig::default()
        };
        assert!(f.validate().is_err());
        let r = load_research_config(dir()).unwrap();
        assert!((r.alpha_per_card() - 0.04 / 12.0).abs() < 1e-12);
    }
}
