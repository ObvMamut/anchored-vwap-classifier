//! The Markdown summary of the whole research record and the plain inputs of the strategy plots,
//! both read from the ledger and the tracked summaries. Nothing here computes a result.

use std::path::Path;

use vwap_core::ResearchConfig;

use crate::error::ResearchError;
use crate::ledger::{Entry, Ledger, Verdict};

fn f(x: Option<f64>) -> String {
    x.map_or_else(|| "n/a".into(), |v| format!("{v:.4}"))
}

fn verdict(v: Verdict) -> String {
    format!("{v:?}").to_uppercase()
}

/// A report of every card and strategy in the ledger: p values, effects, trial counts, gate
/// outcomes, DSR and PBO. Written in plain sentences and tables.
pub fn build(cfg: &ResearchConfig) -> Result<String, ResearchError> {
    let ledger = Ledger::open(Path::new(&cfg.ledger_path));
    let entries = ledger.read_all()?;
    let spent: f64 = entries
        .iter()
        .map(|e| match e {
            Entry::Run { alpha_spent, .. } | Entry::Omnibus { alpha_spent, .. } => *alpha_spent,
            _ => 0.0,
        })
        .sum();
    let mut s = String::from("# Research summary\n\n");
    s.push_str(&format!(
        "Counted trials: {}. Hypothesis cards run: {} of {}. Alpha spent: {:.4} of {:.4}. Strategies run: {}.\n\n",
        ledger.trials()?,
        ledger.cards_run()?.len(),
        cfg.max_cards,
        spent,
        cfg.alpha_total,
        ledger.strategies_run()?.len()
    ));

    s.push_str("## Hypothesis cards\n\n");
    s.push_str("| cycle | card | view | effect | p value | verdict |\n|---|---|---|---|---|---|\n");
    for e in &entries {
        match e {
            Entry::Omnibus {
                cycle,
                statistic,
                p,
                verdict: v,
                ..
            } => s.push_str(&format!(
                "| {cycle} | omnibus | discovery | {statistic:.5} | {p:.4} | {} |\n",
                verdict(*v)
            )),
            Entry::Run {
                cycle,
                card_id,
                view,
                delta,
                p,
                verdict: v,
                ..
            } => s.push_str(&format!(
                "| {cycle} | {card_id} | {view} | {delta:.4} | {p:.4} | {} |\n",
                verdict(*v)
            )),
            Entry::Confirm {
                cycle,
                card_id,
                delta,
                p,
                verdict: v,
                ..
            } => s.push_str(&format!(
                "| {cycle} | {card_id} | confirmation | {delta:.4} | {p:.4} | {} |\n",
                verdict(*v)
            )),
            Entry::Holdout {
                card_id,
                delta,
                p,
                verdict: v,
                ..
            } => s.push_str(&format!(
                "| | {card_id} | holdout | {delta:.4} | {p:.4} | {} |\n",
                verdict(*v)
            )),
            _ => {}
        }
    }

    s.push_str("\n## Strategies\n\n");
    s.push_str("| cycle | strategy | timeframe | trades | net ticks per trade | BSS | DSR | PBO | verdict |\n|---|---|---|---|---|---|---|---|---|\n");
    let mut failures: Vec<String> = Vec::new();
    for e in &entries {
        if let Entry::Strategy {
            cycle,
            card_id,
            width,
            n_trades,
            bss,
            net_expectancy_ticks,
            dsr,
            pbo,
            verdict: v,
            failed,
            ..
        } = e
        {
            s.push_str(&format!(
                "| {cycle} | {card_id} | {width}m | {n_trades} | {} | {} | {} | {} | {} |\n",
                f(*net_expectancy_ticks),
                f(*bss),
                f(*dsr),
                f(*pbo),
                verdict(*v)
            ));
            for x in failed {
                failures.push(format!("* {card_id} on {width}m: {x}\n"));
            }
        }
    }
    if !failures.is_empty() {
        s.push_str("\nGates that failed:\n\n");
        for x in &failures {
            s.push_str(x);
        }
    }
    let mut later = String::new();
    for e in &entries {
        match e {
            Entry::StrategyConfirm {
                card_id,
                width,
                net_expectancy_ticks,
                expectancy_lower,
                verdict: v,
                ..
            } => later.push_str(&format!(
                "* Confirmation of {card_id} on {width}m: {} (net {} ticks per trade, lower bound {}).\n",
                verdict(*v),
                f(*net_expectancy_ticks),
                f(*expectancy_lower)
            )),
            Entry::StrategyHoldout {
                card_id,
                width,
                net_expectancy_ticks,
                expectancy_lower,
                verdict: v,
                ..
            } => later.push_str(&format!(
                "* Holdout of {card_id} on {width}m: {} (net {} ticks per trade, lower bound {}).\n",
                verdict(*v),
                f(*net_expectancy_ticks),
                f(*expectancy_lower)
            )),
            _ => {}
        }
    }
    if !later.is_empty() {
        s.push('\n');
        s.push_str(&later);
    }
    s.push_str(
        "\nThe DSR and the PBO use the trial count of the ledger; hypothesis cards count in the trial count but have no PnL series, which is conservative.\n",
    );
    Ok(s)
}

/// Plain inputs of the four plots of one strategy summary.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlotInputs {
    /// Per class, `(mean predicted probability, observed frequency, events)` per bin.
    pub reliability: Vec<Vec<(f64, f64, usize)>>,
    pub daily_pnl: Vec<f64>,
    pub fold_first_day: Vec<usize>,
    pub feature_names: Vec<String>,
    pub importance_by_fold: Vec<Vec<f64>>,
    pub shap_values: Vec<Vec<f64>>,
    pub shap_phi: Vec<Vec<f64>>,
}

fn nums(v: Option<&serde_json::Value>) -> Vec<f64> {
    v.and_then(serde_json::Value::as_array)
        .map(|a| a.iter().map(|x| x.as_f64().unwrap_or(f64::NAN)).collect())
        .unwrap_or_default()
}

fn rows(v: Option<&serde_json::Value>) -> Vec<Vec<f64>> {
    v.and_then(serde_json::Value::as_array)
        .map(|a| a.iter().map(|r| nums(Some(r))).collect())
        .unwrap_or_default()
}

impl PlotInputs {
    /// Reads the `result` of a tracked summary.
    pub fn from_summary(summary: &serde_json::Value) -> Self {
        let r = summary.get("result");
        let g = |k: &str| r.and_then(|r| r.get(k));
        let reliability = g("reliability")
            .and_then(serde_json::Value::as_array)
            .map(|classes| {
                classes
                    .iter()
                    .map(|bins| {
                        bins.as_array()
                            .map(|b| {
                                b.iter()
                                    .map(|x| {
                                        (
                                            x.get("mean_pred")
                                                .and_then(|v| v.as_f64())
                                                .unwrap_or(f64::NAN),
                                            x.get("freq")
                                                .and_then(|v| v.as_f64())
                                                .unwrap_or(f64::NAN),
                                            x.get("n")
                                                .and_then(serde_json::Value::as_u64)
                                                .unwrap_or(0)
                                                as usize,
                                        )
                                    })
                                    .collect()
                            })
                            .unwrap_or_default()
                    })
                    .collect()
            })
            .unwrap_or_default();
        let sample = g("shap_sample").and_then(serde_json::Value::as_array);
        let (mut shap_values, mut shap_phi) = (Vec::new(), Vec::new());
        for row in sample.into_iter().flatten() {
            shap_values.push(nums(row.get("values")));
            shap_phi.push(nums(row.get("phi")));
        }
        Self {
            reliability,
            daily_pnl: nums(g("daily_pnl")),
            fold_first_day: nums(g("fold_first_day"))
                .iter()
                .map(|x| *x as usize)
                .collect(),
            feature_names: g("feature_names")
                .and_then(serde_json::Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            importance_by_fold: rows(g("importance_by_fold")),
            shap_values,
            shap_phi,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plot_inputs_are_read_from_a_summary() {
        let v = serde_json::json!({"result": {
            "reliability": [[{"n": 4, "mean_pred": 0.2, "freq": 0.25}], [], []],
            "daily_pnl": [1.0, -2.0],
            "fold_first_day": [0, 1],
            "feature_names": ["a", "b"],
            "importance_by_fold": [[1.0, 2.0]],
            "shap_sample": [{"values": [1.0, null], "phi": [0.1, 0.2]}]
        }});
        let p = PlotInputs::from_summary(&v);
        assert_eq!(p.reliability[0], vec![(0.2, 0.25, 4)]);
        assert_eq!(p.daily_pnl, vec![1.0, -2.0]);
        assert_eq!(p.fold_first_day, vec![0, 1]);
        assert_eq!(p.feature_names, vec!["a", "b"]);
        assert_eq!(p.importance_by_fold, vec![vec![1.0, 2.0]]);
        assert!(p.shap_values[0][1].is_nan());
        assert_eq!(p.shap_phi[0], vec![0.1, 0.2]);
        assert_eq!(
            PlotInputs::from_summary(&serde_json::json!({})),
            PlotInputs::default()
        );
    }
}
