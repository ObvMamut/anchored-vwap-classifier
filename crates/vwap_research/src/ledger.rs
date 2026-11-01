//! The append only research ledger and the budget it enforces.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use vwap_core::ResearchConfig;

use crate::error::ResearchError;
use crate::split::ConfirmPermit;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Promote,
    Refine,
    Drop,
    /// Omnibus cycle found structure.
    Structure,
    /// Omnibus cycle found none.
    Null,
    Confirmed,
    NotConfirmed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Entry {
    Registered {
        card_id: String,
        sha256: String,
        git_commit: String,
        time_utc: String,
    },
    Run {
        cycle: usize,
        card_id: String,
        sha256: String,
        git_commit: String,
        view: String,
        timeframes: Vec<u32>,
        trials_added: usize,
        alpha_spent: f64,
        delta: f64,
        p: f64,
        /// `None` when no minimum detectable effect is defined (group tests).
        #[serde(default)]
        mde: Option<f64>,
        verdict: Verdict,
        time_utc: String,
    },
    Omnibus {
        cycle: usize,
        git_commit: String,
        trials_added: usize,
        alpha_spent: f64,
        statistic: f64,
        p: f64,
        verdict: Verdict,
        time_utc: String,
    },
    Confirm {
        cycle: usize,
        card_id: String,
        sha256: String,
        git_commit: String,
        delta: f64,
        p: f64,
        verdict: Verdict,
        time_utc: String,
    },
    /// A descriptive analysis that spends no alpha and cannot promote anything.
    Diagnostic {
        name: String,
        git_commit: String,
        note: String,
        time_utc: String,
    },
}

pub struct Ledger {
    path: PathBuf,
}

impl Ledger {
    pub fn open(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
        }
    }

    pub fn read_all(&self) -> Result<Vec<Entry>, ResearchError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let text = fs::read_to_string(&self.path).map_err(|source| ResearchError::Io {
            path: self.path.display().to_string(),
            source,
        })?;
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).map_err(ResearchError::from))
            .collect()
    }

    /// The only write operation: one JSON line appended at the end of the file.
    pub fn append(&self, e: &Entry) -> Result<(), ResearchError> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).map_err(|source| ResearchError::Io {
                path: dir.display().to_string(),
                source,
            })?;
        }
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|source| ResearchError::Io {
                path: self.path.display().to_string(),
                source,
            })?;
        writeln!(f, "{}", serde_json::to_string(e)?).map_err(|source| ResearchError::Io {
            path: self.path.display().to_string(),
            source,
        })
    }

    pub fn registered_hash(&self, id: &str) -> Result<Option<String>, ResearchError> {
        Ok(self.read_all()?.into_iter().find_map(|e| match e {
            Entry::Registered {
                card_id, sha256, ..
            } if card_id == id => Some(sha256),
            _ => None,
        }))
    }

    /// Card ids that have been run on discovery data.
    pub fn cards_run(&self) -> Result<BTreeSet<String>, ResearchError> {
        Ok(self
            .read_all()?
            .into_iter()
            .filter_map(|e| match e {
                Entry::Run { card_id, view, .. } if view == "discovery" => Some(card_id),
                _ => None,
            })
            .collect())
    }

    /// Number of counted trials: the omnibus cycle plus every distinct card run on discovery.
    pub fn trials(&self) -> Result<usize, ResearchError> {
        let entries = self.read_all()?;
        let omni = entries
            .iter()
            .filter(|e| matches!(e, Entry::Omnibus { .. }))
            .count();
        Ok(omni.min(1) + self.cards_run()?.len())
    }

    pub fn next_cycle(&self) -> Result<usize, ResearchError> {
        Ok(self
            .read_all()?
            .iter()
            .filter(|e| !matches!(e, Entry::Registered { .. } | Entry::Diagnostic { .. }))
            .count()
            + 1)
    }

    /// Errors if running `card_id` would exceed the card count or alpha budget.
    pub fn check_budget(
        &self,
        cfg: &ResearchConfig,
        card_id: &str,
        card_alpha: f64,
    ) -> Result<(), ResearchError> {
        let run = self.cards_run()?;
        if card_alpha > cfg.alpha_per_card() + 1e-12 {
            return Err(ResearchError::BudgetExhausted(format!(
                "card alpha {card_alpha} exceeds the per card share {}",
                cfg.alpha_per_card()
            )));
        }
        if !run.contains(card_id) && run.len() >= cfg.max_cards {
            return Err(ResearchError::BudgetExhausted(format!(
                "{} cards already run, the budget is {}",
                run.len(),
                cfg.max_cards
            )));
        }
        Ok(())
    }

    pub fn verdict_of(&self, card_id: &str) -> Result<Option<Verdict>, ResearchError> {
        Ok(self
            .read_all()?
            .into_iter()
            .filter_map(|e| match e {
                Entry::Run {
                    card_id: c,
                    view,
                    verdict,
                    ..
                } if c == card_id && view == "discovery" => Some(verdict),
                _ => None,
            })
            .next_back())
    }

    /// A one shot permit to confirm a promoted card.
    pub fn confirm_permit(&self, card_id: &str) -> Result<ConfirmPermit, ResearchError> {
        if self.verdict_of(card_id)? != Some(Verdict::Promote) {
            return Err(ResearchError::ConfirmNotAllowed(format!(
                "{card_id} has no PROMOTE verdict in the ledger"
            )));
        }
        let already = self
            .read_all()?
            .iter()
            .any(|e| matches!(e, Entry::Confirm { card_id: c, .. } if c == card_id));
        if already {
            return Err(ResearchError::ConfirmNotAllowed(format!(
                "{card_id} was already confirmed once; confirmation is one shot"
            )));
        }
        Ok(ConfirmPermit {
            card_id: card_id.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ResearchConfig {
        toml::from_str(
            r#"
discovery_start = "2016-11-15"
discovery_end = "2021-09-30"
confirm_start = "2021-10-01"
confirm_end = "2023-09-30"
holdout_start = "2023-10-01"
cards_dir = "h"
ledger_path = "l"
report_dir = "r"
alpha_total = 0.05
alpha_omnibus = 0.01
max_cards = 2
default_min_effect = 0.03
refine_p_max = 0.25
promote_min_years_same_sign = 4
min_events_per_arm = 150
cost_ticks = 3.0
bootstrap_draws = 100
omnibus_permutations = 10
seed = 1
"#,
        )
        .unwrap()
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vwap_ledger_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d.join("ledger.jsonl")
    }

    fn run_entry(id: &str, verdict: Verdict) -> Entry {
        Entry::Run {
            cycle: 1,
            card_id: id.into(),
            sha256: "h".into(),
            git_commit: "c".into(),
            view: "discovery".into(),
            timeframes: vec![5, 15],
            trials_added: 1,
            alpha_spent: 0.02,
            delta: 0.04,
            p: 0.001,
            mde: Some(0.03),
            verdict,
            time_utc: "t".into(),
        }
    }

    #[test]
    fn the_ledger_only_appends_and_survives_a_round_trip() {
        let path = tmp("append");
        let l = Ledger::open(&path);
        l.append(&run_entry("A", Verdict::Drop)).unwrap();
        l.append(&run_entry("B", Verdict::Refine)).unwrap();
        let before = fs::read_to_string(&path).unwrap();
        l.append(&run_entry("C", Verdict::Drop)).unwrap();
        let after = fs::read_to_string(&path).unwrap();
        assert!(
            after.starts_with(&before),
            "earlier lines must be untouched"
        );
        assert_eq!(l.read_all().unwrap().len(), 3);
        assert_eq!(l.trials().unwrap(), 3);
    }

    #[test]
    fn the_budget_blocks_a_new_card_but_not_a_repeat() {
        let path = tmp("budget");
        let l = Ledger::open(&path);
        let c = cfg();
        l.append(&run_entry("A", Verdict::Drop)).unwrap();
        l.append(&run_entry("B", Verdict::Drop)).unwrap();
        assert!(l.check_budget(&c, "C", 0.01).is_err()); // third distinct card
        assert!(l.check_budget(&c, "A", 0.01).is_ok()); // a repeat of a counted card
        assert!(l.check_budget(&c, "A", 0.03).is_err()); // alpha above the per card share
    }

    #[test]
    fn confirmation_needs_a_promote_verdict_and_works_once() {
        let path = tmp("confirm");
        let l = Ledger::open(&path);
        l.append(&run_entry("A", Verdict::Refine)).unwrap();
        assert!(l.confirm_permit("A").is_err());
        l.append(&run_entry("P", Verdict::Promote)).unwrap();
        assert!(l.confirm_permit("P").is_ok());
        l.append(&Entry::Confirm {
            cycle: 3,
            card_id: "P".into(),
            sha256: "h".into(),
            git_commit: "c".into(),
            delta: 0.03,
            p: 0.02,
            verdict: Verdict::Confirmed,
            time_utc: "t".into(),
        })
        .unwrap();
        assert!(l.confirm_permit("P").is_err());
    }
}

#[cfg(test)]
mod nan_tests {
    use super::*;

    #[test]
    fn an_entry_without_a_minimum_detectable_effect_round_trips_and_old_lines_still_parse() {
        let e = Entry::Run {
            cycle: 1,
            card_id: "G".into(),
            sha256: "h".into(),
            git_commit: "c".into(),
            view: "discovery".into(),
            timeframes: vec![5],
            trials_added: 1,
            alpha_spent: 0.003,
            delta: 0.02,
            p: 0.5,
            mde: None,
            verdict: Verdict::Drop,
            time_utc: "t".into(),
        };
        let line = serde_json::to_string(&e).unwrap();
        let back: Entry = serde_json::from_str(&line).unwrap();
        assert!(matches!(back, Entry::Run { mde: None, .. }));
        // a line written by the earlier version carried a number
        let old = line.replace("\"mde\":null", "\"mde\":0.0");
        assert!(serde_json::from_str::<Entry>(&old).is_ok());
    }
}
