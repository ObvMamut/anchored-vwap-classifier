//! One research cycle: all the protocol checks, the run on the discovery view, the
//! report and the ledger entry.

use std::fs;
use std::path::Path;

use vwap_core::ResearchConfig;
use vwap_features::feature_names;

use crate::card::load_card;
use crate::error::ResearchError;
use crate::ledger::{Entry, Ledger, Verdict};
use crate::report::render;
use crate::runner::{CardResult, run_card};
use crate::selftest::selftest;
use crate::split::{Bounds, DiscoveryView, Sample};

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Registers a card: stores its hash and the commit it was committed in. Registering an
/// unchanged card again is a no op; a changed card is an error.
pub fn register(
    cfg: &ResearchConfig,
    card_path: &Path,
    git_commit: &str,
) -> Result<String, ResearchError> {
    let (card, hash) = load_card(card_path)?;
    card.validate(feature_names())?;
    let ledger = Ledger::open(Path::new(&cfg.ledger_path));
    match ledger.registered_hash(&card.id)? {
        Some(h) if h == hash => return Ok(hash),
        Some(h) => {
            return Err(ResearchError::CardChanged {
                id: card.id,
                registered: h,
                current: hash,
            });
        }
        None => {}
    }
    ledger.append(&Entry::Registered {
        card_id: card.id,
        sha256: hash.clone(),
        git_commit: git_commit.to_string(),
        time_utc: now(),
    })?;
    Ok(hash)
}

/// Runs a registered card on the discovery view.
pub fn execute(
    cfg: &ResearchConfig,
    card_path: &Path,
    git_commit: &str,
    samples: &[Sample],
) -> Result<CardResult, ResearchError> {
    selftest(cfg)?;
    let (card, hash) = load_card(card_path)?;
    card.validate(feature_names())?;
    let ledger = Ledger::open(Path::new(&cfg.ledger_path));
    match ledger.registered_hash(&card.id)? {
        None => return Err(ResearchError::NotRegistered { id: card.id }),
        Some(h) if h != hash => {
            return Err(ResearchError::CardChanged {
                id: card.id,
                registered: h,
                current: hash,
            });
        }
        Some(_) => {}
    }
    let alpha = card.alpha.unwrap_or_else(|| cfg.alpha_per_card());
    ledger.check_budget(cfg, &card.id, alpha)?;
    let first_run = !ledger.cards_run()?.contains(&card.id);
    let view = DiscoveryView::select(samples, &Bounds::from_config(cfg));
    if view.samples().len() < 500 {
        return Err(ResearchError::NotEnoughData(format!(
            "only {} discovery samples",
            view.samples().len()
        )));
    }
    let result = run_card(&card, view.samples(), cfg);
    let dir = Path::new(&cfg.report_dir);
    fs::create_dir_all(dir).map_err(|source| ResearchError::Io {
        path: dir.display().to_string(),
        source,
    })?;
    let rp = dir.join(format!("{}.md", card.id));
    fs::write(&rp, render(&card, &result, "discovery")).map_err(|source| ResearchError::Io {
        path: rp.display().to_string(),
        source,
    })?;
    ledger.append(&Entry::Run {
        cycle: ledger.next_cycle()?,
        card_id: card.id.clone(),
        sha256: hash,
        git_commit: git_commit.to_string(),
        view: "discovery".into(),
        timeframes: card.timeframes.clone(),
        trials_added: usize::from(first_run),
        alpha_spent: if first_run { alpha } else { 0.0 },
        delta: result.delta,
        p: result.p,
        mde: result.mde,
        verdict: result.verdict,
        time_utc: now(),
    })?;
    Ok(result)
}

/// Cycle 0: the omnibus screen on the discovery view. Allowed once.
pub fn execute_omnibus(
    cfg: &ResearchConfig,
    git_commit: &str,
    samples: &[Sample],
    horizon_bars: impl Fn(u32) -> usize,
) -> Result<crate::omnibus::OmnibusResult, ResearchError> {
    selftest(cfg)?;
    let ledger = Ledger::open(Path::new(&cfg.ledger_path));
    if ledger
        .read_all()?
        .iter()
        .any(|e| matches!(e, Entry::Omnibus { .. }))
    {
        return Err(ResearchError::BudgetExhausted(
            "the omnibus cycle was already run and is allowed once".into(),
        ));
    }
    let view = DiscoveryView::select(samples, &Bounds::from_config(cfg));
    let r = crate::omnibus::omnibus(view.samples(), cfg, horizon_bars)?;
    let dir = Path::new(&cfg.report_dir);
    fs::create_dir_all(dir).map_err(|source| ResearchError::Io {
        path: dir.display().to_string(),
        source,
    })?;
    let rp = dir.join("omnibus.json");
    fs::write(&rp, serde_json::to_string_pretty(&r)?).map_err(|source| ResearchError::Io {
        path: rp.display().to_string(),
        source,
    })?;
    ledger.append(&Entry::Omnibus {
        cycle: ledger.next_cycle()?,
        git_commit: git_commit.to_string(),
        trials_added: 1,
        alpha_spent: cfg.alpha_omnibus,
        statistic: r.statistic,
        p: r.p,
        verdict: r.verdict,
        time_utc: now(),
    })?;
    Ok(r)
}

/// Descriptive decomposition of the omnibus. Logged as a diagnostic, spends no alpha.
pub fn execute_diagnostic(
    cfg: &ResearchConfig,
    git_commit: &str,
    samples: &[Sample],
    horizon_bars: impl Fn(u32) -> usize,
    tasks: &[crate::omnibus::Task],
    perms: usize,
) -> Result<Vec<crate::omnibus::Decomposition>, ResearchError> {
    let ledger = Ledger::open(Path::new(&cfg.ledger_path));
    let view = DiscoveryView::select(samples, &Bounds::from_config(cfg));
    let mut out = Vec::new();
    for &t in tasks {
        let d = crate::omnibus::decompose(view.samples(), cfg, &horizon_bars, t, perms)?;
        let note = d
            .widths
            .iter()
            .map(|w| format!("{}m stat {:.5} p {:.4}", w.width, w.statistic, w.p))
            .collect::<Vec<_>>()
            .join("; ");
        ledger.append(&Entry::Diagnostic {
            name: format!("omnibus_decomposition_{t:?}"),
            git_commit: git_commit.to_string(),
            note,
            time_utc: now(),
        })?;
        out.push(d);
    }
    let dir = Path::new(&cfg.report_dir);
    fs::create_dir_all(dir).map_err(|source| ResearchError::Io {
        path: dir.display().to_string(),
        source,
    })?;
    let rp = dir.join("omnibus_decomposition.json");
    fs::write(&rp, serde_json::to_string_pretty(&out)?).map_err(|source| ResearchError::Io {
        path: rp.display().to_string(),
        source,
    })?;
    Ok(out)
}

/// Text for `vwap research status`.
pub fn status(cfg: &ResearchConfig) -> Result<String, ResearchError> {
    let ledger = Ledger::open(Path::new(&cfg.ledger_path));
    let entries = ledger.read_all()?;
    let run = ledger.cards_run()?;
    let spent: f64 = entries
        .iter()
        .map(|e| match e {
            Entry::Run { alpha_spent, .. } | Entry::Omnibus { alpha_spent, .. } => *alpha_spent,
            _ => 0.0,
        })
        .sum();
    let mut s = format!(
        "cards run {} of {}, trials {}, alpha spent {:.4} of {:.4}\n",
        run.len(),
        cfg.max_cards,
        ledger.trials()?,
        spent,
        cfg.alpha_total
    );
    for e in &entries {
        match e {
            Entry::Run {
                cycle,
                card_id,
                view,
                delta,
                p,
                verdict,
                ..
            } => s.push_str(&format!(
                "cycle {cycle}: {card_id} on {view}: delta {delta:.4}, p {p:.4}, {verdict:?}\n"
            )),
            Entry::Omnibus {
                cycle,
                statistic,
                p,
                verdict,
                ..
            } => s.push_str(&format!(
                "cycle {cycle}: omnibus: statistic {statistic:.5}, p {p:.4}, {verdict:?}\n"
            )),
            Entry::Confirm {
                cycle,
                card_id,
                delta,
                p,
                verdict,
                ..
            } => s.push_str(&format!(
                "cycle {cycle}: {card_id} confirmation: delta {delta:.4}, p {p:.4}, {verdict:?}\n"
            )),
            Entry::Diagnostic { name, note, .. } => {
                s.push_str(&format!("diagnostic {name}: {note}\n"));
            }
            Entry::Registered { .. } => {}
        }
    }
    let nulls = entries
        .iter()
        .rev()
        .take_while(|e| match e {
            Entry::Run { verdict, .. } => *verdict == Verdict::Drop,
            Entry::Omnibus { verdict, .. } => *verdict == Verdict::Null,
            Entry::Registered { .. } | Entry::Diagnostic { .. } => true,
            _ => false,
        })
        .filter(|e| !matches!(e, Entry::Registered { .. } | Entry::Diagnostic { .. }))
        .count();
    s.push_str(&format!(
        "consecutive nulls at the end of the ledger: {nulls}\n"
    ));
    Ok(s)
}
