//! Hypothesis cards: TOML files written and committed before they are run.

use std::path::Path;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::ResearchError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Indicator of the REJECTION class. Also moves with the chop rate, so it is a poor
    /// directional outcome; see `docs/PROTOCOL.md`, section 3.
    Rejection,
    /// Indicator of REJECTION among events that hit a barrier (CHOP events are left out).
    RejectionGivenResolved,
    /// Outcome in ticks under the barrier rule minus the round trip cost.
    NetTicks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    TopTercile,
    BottomTercile,
    Above,
    Below,
    /// The feature is a 0 or 1 flag: flag equal to 1 versus 0.
    Flag,
    /// The feature is a group id: test that the group means are equal.
    Groups,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Subset mean minus complement mean is predicted to be positive.
    Positive,
    Negative,
    /// For group tests.
    Any,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Card {
    pub id: String,
    pub family: String,
    pub mechanism: String,
    #[serde(default)]
    pub parent: Option<String>,
    pub outcome: Outcome,
    pub feature: String,
    pub rule: RuleKind,
    pub direction: Direction,
    /// Required for `above` and `below`.
    #[serde(default)]
    pub threshold: Option<f64>,
    pub timeframes: Vec<u32>,
    #[serde(default)]
    pub min_effect: Option<f64>,
    #[serde(default)]
    pub alpha: Option<f64>,
}

impl Card {
    pub fn validate(&self, known_features: &[&str]) -> Result<(), ResearchError> {
        let bad = |m: &str| Err(ResearchError::InvalidCard(format!("{}: {m}", self.id)));
        if !known_features.contains(&self.feature.as_str()) {
            return bad(&format!("unknown feature {}", self.feature));
        }
        if matches!(self.rule, RuleKind::Above | RuleKind::Below) && self.threshold.is_none() {
            return bad("rules above and below need a threshold");
        }
        if self.rule == RuleKind::Groups && self.direction != Direction::Any {
            return bad("a groups rule needs direction = any");
        }
        if self.rule != RuleKind::Groups && self.direction == Direction::Any {
            return bad("direction = any is only for groups rules");
        }
        if self.timeframes.is_empty() || self.timeframes.iter().any(|w| ![5, 15].contains(w)) {
            return bad("timeframes must be a non empty subset of 5 and 15");
        }
        if self.mechanism.trim().len() < 20 {
            return bad("the mechanism must be written out");
        }
        Ok(())
    }
}

/// Reads a card and its SHA256.
pub fn load_card(path: &Path) -> Result<(Card, String), ResearchError> {
    let text = std::fs::read_to_string(path).map_err(|source| ResearchError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let card: Card = toml::from_str(&text).map_err(|source| ResearchError::Toml {
        path: path.display().to_string(),
        source,
    })?;
    let hash = Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok((card, hash))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = r#"
id = "T001"
family = "regime"
mechanism = "pullbacks into a rising VWAP find buyers"
outcome = "rejection"
feature = "vslope_align"
rule = "top_tercile"
direction = "positive"
timeframes = [5, 15]
"#;

    #[test]
    fn a_card_parses_and_validates() {
        let c: Card = toml::from_str(TEXT).unwrap();
        assert!(c.validate(&["vslope_align"]).is_ok());
        assert!(c.validate(&["other"]).is_err());
    }

    #[test]
    fn unknown_fields_and_inconsistent_rules_are_rejected() {
        assert!(toml::from_str::<Card>(&format!("{TEXT}\nextra = 1")).is_err());
        let mut c: Card = toml::from_str(TEXT).unwrap();
        c.rule = RuleKind::Above;
        assert!(c.validate(&["vslope_align"]).is_err()); // no threshold
        c.rule = RuleKind::Groups;
        assert!(c.validate(&["vslope_align"]).is_err()); // direction must be any
    }
}
