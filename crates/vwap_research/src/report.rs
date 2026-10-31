//! The fixed analysis report written for every card run.

use crate::card::{Card, RuleKind};
use crate::runner::CardResult;

fn pct(x: f64) -> String {
    format!("{:.2} percent", x * 100.0)
}

fn ticks_or_pct(card: &Card, x: f64) -> String {
    match card.outcome {
        crate::card::Outcome::Rejection => pct(x),
        crate::card::Outcome::NetTicks => format!("{x:.2} ticks"),
    }
}

pub fn render(card: &Card, r: &CardResult, view: &str) -> String {
    let mut s = String::new();
    s.push_str(&format!("# {} ({} view)\n\n", card.id, view));
    s.push_str(&format!(
        "Family: {}. Mechanism: {}\n\n",
        card.family, card.mechanism
    ));
    s.push_str(&format!(
        "Feature {}, rule {:?}, predicted direction {:?}, outcome {:?}. Card alpha {:.4}, minimum effect {}.\n\n",
        card.feature,
        card.rule,
        card.direction,
        card.outcome,
        r.alpha_card,
        ticks_or_pct(card, r.min_effect)
    ));
    s.push_str(&format!(
        "## Verdict: {:?}\n\nCombined p {:.4} (the larger of the timeframes), smallest effect {}, largest minimum detectable effect {}.\n",
        r.verdict,
        r.p,
        ticks_or_pct(card, r.delta),
        ticks_or_pct(card, r.mde)
    ));
    for reason in &r.reasons {
        s.push_str(&format!("* {reason}\n"));
    }
    for w in &r.widths {
        s.push_str(&format!("\n## {} minute bars\n\n", w.width));
        s.push_str(&format!(
            "Eligible events {}, missing value rate {}, cut points {:?}.\n",
            w.n_eligible,
            pct(w.nan_rate),
            w.cut_points
        ));
        if card.rule == RuleKind::Groups {
            for (g, m, n) in &w.groups {
                s.push_str(&format!(
                    "* group {}: mean {} over {} events\n",
                    g,
                    ticks_or_pct(card, *m),
                    n
                ));
            }
            s.push_str(&format!(
                "Largest group deviation {}, p {:.4}.\n",
                ticks_or_pct(card, w.delta),
                w.p
            ));
        } else {
            s.push_str(&format!(
                "Subset {} events with mean {}, complement {} events with mean {}. Difference {}, 95 percent interval {} to {}, one sided p {:.4}. Design effect {:.2}, minimum detectable effect {}.\n",
                w.n_subset,
                ticks_or_pct(card, w.mean_subset),
                w.n_comp,
                ticks_or_pct(card, w.mean_comp),
                ticks_or_pct(card, w.delta),
                ticks_or_pct(card, w.ci95[0]),
                ticks_or_pct(card, w.ci95[1]),
                w.p,
                w.deff,
                ticks_or_pct(card, w.mde)
            ));
            s.push_str(&format!(
                "By volatility tercile: {:?}. By side (above, below): {:?}.\n",
                w.by_vol_tercile
                    .iter()
                    .map(|x| format!("{:.4}", x))
                    .collect::<Vec<_>>(),
                w.by_side
                    .iter()
                    .map(|x| format!("{:.4}", x))
                    .collect::<Vec<_>>()
            ));
        }
        s.push_str(&format!(
            "Years with enough data {}, same sign as predicted in {}.\n",
            w.years_with_data, w.years_same_sign
        ));
        for (y, d, n) in &w.years {
            s.push_str(&format!(
                "* {}: {} (smaller arm {} events)\n",
                y,
                ticks_or_pct(card, *d),
                n
            ));
        }
    }
    s.push_str("\nThe difference in differences against control levels is not computed in this version of the harness; it is a diagnostic only and becomes required before any confirmation.\n");
    s
}
