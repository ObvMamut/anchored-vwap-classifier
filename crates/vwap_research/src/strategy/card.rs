//! Strategy cards: a model plus the backtest, frozen before it runs. See `docs/PROTOCOL.md`.
//! Everything numeric (grids, costs, folds, gate limits) lives in `config/*.toml`; the run entry
//! in the ledger records the SHA256 of each of those files, so a changed configuration is
//! detected by `verify` and `replay`.

use std::path::Path;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use vwap_core::LabelsConfig;
use vwap_model::Family;

use crate::error::ResearchError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelKind {
    Majority,
    Logistic,
    Lgbm,
}

impl ModelKind {
    pub fn family(self) -> Family {
        match self {
            ModelKind::Majority => Family::Majority,
            ModelKind::Logistic => Family::Logistic,
            ModelKind::Lgbm => Family::Lgbm,
        }
    }
}

/// What the models are trained and scored on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    /// Rejection, breakthrough and chop.
    #[default]
    ThreeClass,
    /// Rejection against breakthrough among the touches that resolve (chop is still traded).
    Direction,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategyCard {
    pub id: String,
    /// Always `"strategy"`; it tells the register command which kind of card this is.
    pub kind: String,
    pub mechanism: String,
    pub model: ModelKind,
    /// Each timeframe is a separate counted trial.
    pub timeframes: Vec<u32>,
    /// Feature names; empty means all of them.
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub target: Target,
    /// Barrier shape overrides. A card that sets none uses `labels.toml` as it is.
    #[serde(default)]
    pub m1: Option<f64>,
    #[serde(default)]
    pub m2: Option<f64>,
    #[serde(default)]
    pub horizon_5m: Option<usize>,
    #[serde(default)]
    pub horizon_15m: Option<usize>,
    /// Average trades per calendar week the strategy must make over its test windows. Unset
    /// means no requirement (the behaviour of the first three strategy cards).
    #[serde(default)]
    pub min_trades_per_week: Option<f64>,
}

impl StrategyCard {
    pub fn validate(&self, known_features: &[&str]) -> Result<(), ResearchError> {
        let bad = |m: &str| Err(ResearchError::InvalidCard(format!("{}: {m}", self.id)));
        if self.kind != "strategy" {
            return bad("kind must be \"strategy\"");
        }
        if !self.id.starts_with('S') {
            return bad("strategy ids start with S");
        }
        if self.timeframes.is_empty() || self.timeframes.iter().any(|w| ![5, 15].contains(w)) {
            return bad("timeframes must be a non empty subset of 5 and 15");
        }
        if let Some(f) = self
            .features
            .iter()
            .find(|f| !known_features.contains(&f.as_str()))
        {
            return bad(&format!("unknown feature {f}"));
        }
        if self.mechanism.trim().len() < 20 {
            return bad("the mechanism must be written out");
        }
        if self.m1.is_some_and(|v| v <= 0.0)
            || self.m2.is_some_and(|v| v <= 0.0)
            || self.horizon_5m == Some(0)
            || self.horizon_15m == Some(0)
        {
            return bad("barrier multiples and horizons must be positive");
        }
        if self
            .min_trades_per_week
            .is_some_and(|r| r.is_nan() || r <= 0.0)
        {
            return bad("min_trades_per_week must be positive");
        }
        Ok(())
    }

    pub fn direction(&self) -> bool {
        self.target == Target::Direction
    }

    /// Whether the card changes the barrier shape.
    pub fn has_label_override(&self) -> bool {
        self.m1.is_some()
            || self.m2.is_some()
            || self.horizon_5m.is_some()
            || self.horizon_15m.is_some()
    }

    /// `base` with the card's barrier shape applied.
    pub fn label_config(&self, base: &LabelsConfig) -> LabelsConfig {
        let mut l = base.clone();
        l.m1 = self.m1.unwrap_or(l.m1);
        l.m2 = self.m2.unwrap_or(l.m2);
        l.horizon_5m = self.horizon_5m.unwrap_or(l.horizon_5m);
        l.horizon_15m = self.horizon_15m.unwrap_or(l.horizon_15m);
        l
    }

    /// An error if the labels the run was built with do not carry the card's barrier shape.
    pub fn check_labels(&self, labels: &LabelsConfig) -> Result<(), ResearchError> {
        let ok = self.m1.is_none_or(|v| v == labels.m1)
            && self.m2.is_none_or(|v| v == labels.m2)
            && self.horizon_5m.is_none_or(|v| v == labels.horizon_5m)
            && self.horizon_15m.is_none_or(|v| v == labels.horizon_15m);
        if ok {
            Ok(())
        } else {
            Err(ResearchError::Strategy(format!(
                "{}: the samples were not built with the barrier shape of the card",
                self.id
            )))
        }
    }

    /// Column indices of the feature list in `known_features` order; all columns when empty.
    pub fn feature_indices(&self, known_features: &[&str]) -> Vec<usize> {
        if self.features.is_empty() {
            return (0..known_features.len()).collect();
        }
        self.features
            .iter()
            .filter_map(|f| known_features.iter().position(|k| k == f))
            .collect()
    }
}

/// True when the card text declares `kind = "strategy"`.
pub fn is_strategy_text(text: &str) -> bool {
    toml::from_str::<toml::Table>(text)
        .ok()
        .and_then(|t| t.get("kind").and_then(|k| k.as_str().map(str::to_string)))
        .is_some_and(|k| k == "strategy")
}

/// Reads a strategy card and its SHA256.
pub fn load_strategy_card(path: &Path) -> Result<(StrategyCard, String), ResearchError> {
    let text = std::fs::read_to_string(path).map_err(|source| ResearchError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let card: StrategyCard = toml::from_str(&text).map_err(|source| ResearchError::Toml {
        path: path.display().to_string(),
        source,
    })?;
    Ok((card, sha_hex(text.as_bytes())))
}

pub fn sha_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// SHA256 of a card file of either kind.
pub fn card_file_hash(path: &Path) -> Result<String, ResearchError> {
    let text = std::fs::read(path).map_err(|source| ResearchError::Io {
        path: path.display().to_string(),
        source,
    })?;
    Ok(sha_hex(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = r#"
id = "S001"
kind = "strategy"
mechanism = "calibrated logistic regression on all features, rejection above a tuned threshold"
model = "logistic"
timeframes = [5, 15]
"#;

    #[test]
    fn a_strategy_card_parses_and_validates() {
        let c: StrategyCard = toml::from_str(TEXT).unwrap();
        assert!(c.validate(&["a", "b"]).is_ok());
        assert_eq!(c.feature_indices(&["a", "b"]), vec![0, 1]);
        assert!(is_strategy_text(TEXT));
        assert!(!is_strategy_text("id = \"H001\"\nfamily = \"x\""));
    }

    #[test]
    fn a_card_can_set_the_target_and_the_barrier_shape() {
        let text =
            format!("{TEXT}\ntarget = \"direction\"\nm1 = 0.75\nm2 = 1.5\nhorizon_15m = 16\n");
        let c: StrategyCard = toml::from_str(&text).unwrap();
        assert!(c.validate(&["a"]).is_ok());
        assert!(c.direction() && c.has_label_override());
        let base = vwap_core::load_labels_config(std::path::Path::new("../../config")).unwrap();
        let l = c.label_config(&base);
        assert_eq!((l.m1, l.m2, l.horizon_15m), (0.75, 1.5, 16));
        assert_eq!(l.horizon_5m, base.horizon_5m);
        assert!(c.check_labels(&l).is_ok());
        // samples built with another shape are refused
        assert!(c.check_labels(&base).is_err());
        // a plain card changes nothing and defaults to three classes
        let plain: StrategyCard = toml::from_str(TEXT).unwrap();
        assert!(!plain.direction() && !plain.has_label_override());
        assert_eq!(plain.label_config(&base).m1, base.m1);
        assert!(plain.check_labels(&base).is_ok());
        // nonsense shapes are rejected
        let mut bad = c.clone();
        bad.m1 = Some(0.0);
        assert!(bad.validate(&["a"]).is_err());
    }

    #[test]
    fn bad_cards_are_rejected() {
        assert!(toml::from_str::<StrategyCard>(&format!("{TEXT}\nextra = 1")).is_err());
        let mut c: StrategyCard = toml::from_str(TEXT).unwrap();
        c.features = vec!["zzz".into()];
        assert!(c.validate(&["a"]).is_err());
        c.features = vec!["b".into()];
        assert_eq!(c.feature_indices(&["a", "b"]), vec![1]);
        c.timeframes = vec![30];
        assert!(c.validate(&["b"]).is_err());
        c.timeframes = vec![5];
        c.id = "H001".into();
        assert!(c.validate(&["b"]).is_err());
    }
}
