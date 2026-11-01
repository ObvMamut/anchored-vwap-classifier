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
use crate::runner::{CardResult, decide_confirm, run_card, run_card_with};
use crate::selftest::selftest;
use crate::split::{Bounds, ConfirmView, DiscoveryView, HoldoutPermit, HoldoutView, Sample};

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
    fingerprint: &str,
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
    if view.samples().len() < cfg.min_discovery_samples {
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
        mde: result.mde.is_finite().then_some(result.mde),
        data_fingerprint: Some(fingerprint.to_string()),
        verdict: result.verdict,
        time_utc: now(),
    })?;
    Ok(result)
}

/// Cycle 0: the omnibus screen on the discovery view. Allowed once.
pub fn execute_omnibus(
    cfg: &ResearchConfig,
    model: &vwap_core::ModelConfig,
    git_commit: &str,
    fingerprint: &str,
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
    let r = crate::omnibus::omnibus(view.samples(), cfg, model, horizon_bars)?;
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
        data_fingerprint: Some(fingerprint.to_string()),
        verdict: r.verdict,
        time_utc: now(),
    })?;
    Ok(r)
}

/// Descriptive decomposition of the omnibus. Logged as a diagnostic, spends no alpha.
pub fn execute_diagnostic(
    cfg: &ResearchConfig,
    model: &vwap_core::ModelConfig,
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
        let d = crate::omnibus::decompose(view.samples(), cfg, model, &horizon_bars, t, perms)?;
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
            Entry::Holdout {
                card_id,
                delta,
                p,
                verdict,
                ..
            } => {
                s.push_str(&format!(
                    "holdout: {card_id}: delta {delta:.4}, p {p:.4}, {verdict:?}\n"
                ));
            }
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

/// Result of checking the structure of the ledger.
#[derive(Debug, serde::Serialize)]
pub struct VerifyReport {
    pub entries: usize,
    pub cards_run: usize,
    pub trials: usize,
    pub alpha_spent: f64,
    /// Every git commit named by the ledger, for the caller to check against the repository.
    pub commits: Vec<String>,
    pub problems: Vec<String>,
}

/// Checks the ledger against the protocol: strictly increasing cycles, every run registered
/// first with an unchanged card, budget respected, confirmations only after a PROMOTE.
pub fn verify(cfg: &ResearchConfig) -> Result<VerifyReport, ResearchError> {
    let ledger = Ledger::open(Path::new(&cfg.ledger_path));
    let entries = ledger.read_all()?;
    let mut problems = Vec::new();
    let mut registered: std::collections::BTreeMap<String, String> = Default::default();
    let mut promoted: std::collections::BTreeSet<String> = Default::default();
    let mut commits = Vec::new();
    let (mut expected_cycle, mut alpha, mut omnibus) = (1usize, 0.0f64, 0usize);
    for e in &entries {
        match e {
            Entry::Registered {
                card_id,
                sha256,
                git_commit,
                ..
            } => {
                commits.push(git_commit.clone());
                if registered.insert(card_id.clone(), sha256.clone()).is_some() {
                    problems.push(format!("{card_id} is registered twice"));
                }
            }
            Entry::Run {
                cycle,
                card_id,
                sha256,
                git_commit,
                view,
                alpha_spent,
                verdict,
                ..
            } => {
                commits.push(git_commit.clone());
                if *cycle != expected_cycle {
                    problems.push(format!(
                        "cycle {cycle} of {card_id} breaks the sequence (expected {expected_cycle})"
                    ));
                }
                expected_cycle = cycle + 1;
                alpha += alpha_spent;
                match registered.get(card_id) {
                    None => problems.push(format!("{card_id} ran without being registered first")),
                    Some(h) if h != sha256 => problems.push(format!(
                        "{card_id} ran with a hash that differs from its registration"
                    )),
                    Some(_) => {}
                }
                if view == "discovery" && *verdict == Verdict::Promote {
                    promoted.insert(card_id.clone());
                }
            }
            Entry::Omnibus {
                cycle,
                git_commit,
                alpha_spent,
                ..
            } => {
                commits.push(git_commit.clone());
                omnibus += 1;
                if *cycle != expected_cycle {
                    problems.push(format!(
                        "omnibus cycle {cycle} breaks the sequence (expected {expected_cycle})"
                    ));
                }
                expected_cycle = cycle + 1;
                alpha += alpha_spent;
            }
            Entry::Confirm {
                cycle,
                card_id,
                git_commit,
                ..
            } => {
                commits.push(git_commit.clone());
                if *cycle != expected_cycle {
                    problems.push(format!(
                        "confirmation cycle {cycle} breaks the sequence (expected {expected_cycle})"
                    ));
                }
                expected_cycle = cycle + 1;
                if !promoted.contains(card_id) {
                    problems.push(format!("{card_id} was confirmed without a PROMOTE verdict"));
                }
            }
            Entry::Holdout { git_commit, .. } => commits.push(git_commit.clone()),
            Entry::Diagnostic { git_commit, .. } => commits.push(git_commit.clone()),
        }
    }
    let cards_run = ledger.cards_run()?;
    if cards_run.len() > cfg.max_cards {
        problems.push(format!(
            "{} cards run, budget is {}",
            cards_run.len(),
            cfg.max_cards
        ));
    }
    if omnibus > 1 {
        problems.push(format!(
            "the omnibus cycle ran {omnibus} times, once is allowed"
        ));
    }
    if alpha > cfg.alpha_total + 1e-9 {
        problems.push(format!(
            "alpha spent {alpha:.4} exceeds the total {:.4}",
            cfg.alpha_total
        ));
    }
    // registered cards must still match the committed files
    for (id, hash) in &registered {
        let path = Path::new(&cfg.cards_dir).join(format!("{id}.toml"));
        match load_card(&path) {
            Ok((_, current)) if &current == hash => {}
            Ok((_, current)) => problems.push(format!(
                "card {id} changed after registration ({hash} then {current})"
            )),
            Err(e) => problems.push(format!("card {id} cannot be read: {e}")),
        }
    }
    commits.sort();
    commits.dedup();
    Ok(VerifyReport {
        entries: entries.len(),
        cards_run: cards_run.len(),
        trials: ledger.trials()?,
        alpha_spent: alpha,
        commits,
        problems,
    })
}

/// The recorded and the recomputed result of one card.
#[derive(Debug, serde::Serialize)]
pub struct ReplayResult {
    pub card_id: String,
    pub recorded_delta: f64,
    pub replayed_delta: f64,
    pub recorded_p: f64,
    pub replayed_p: f64,
    pub recorded_verdict: Verdict,
    pub replayed_verdict: Verdict,
    pub data_fingerprint_matches: Option<bool>,
    pub matches: bool,
}

/// Re-runs a registered card on the discovery view without touching the ledger, and compares
/// the numbers with the last recorded run. The whole chain is deterministic, so any
/// difference means the data, the code or the configuration changed.
pub fn replay(
    cfg: &ResearchConfig,
    card_path: &Path,
    fingerprint: &str,
    samples: &[Sample],
) -> Result<ReplayResult, ResearchError> {
    const TOL: f64 = 1e-12;
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
    let recorded = ledger
        .read_all()?
        .into_iter()
        .rev()
        .find_map(|e| match e {
            Entry::Run {
                card_id,
                view,
                delta,
                p,
                verdict,
                data_fingerprint,
                ..
            } if card_id == card.id && view == "discovery" => {
                Some((delta, p, verdict, data_fingerprint))
            }
            _ => None,
        })
        .ok_or_else(|| ResearchError::NotRegistered {
            id: format!("{} (no recorded run)", card.id),
        })?;
    let view = DiscoveryView::select(samples, &Bounds::from_config(cfg));
    let r = run_card(&card, view.samples(), cfg);
    let same = |a: f64, b: f64| (a - b).abs() <= TOL || (a.is_nan() && b.is_nan());
    let fp_match = recorded.3.as_deref().map(|f| f == fingerprint);
    Ok(ReplayResult {
        card_id: card.id,
        recorded_delta: recorded.0,
        replayed_delta: r.delta,
        recorded_p: recorded.1,
        replayed_p: r.p,
        recorded_verdict: recorded.2,
        replayed_verdict: r.verdict,
        data_fingerprint_matches: fp_match,
        matches: same(recorded.0, r.delta) && same(recorded.1, r.p) && recorded.2 == r.verdict,
    })
}

#[cfg(test)]
mod verify_tests {
    use super::*;

    fn cfg_in(dir: &Path) -> ResearchConfig {
        let mut c = vwap_core::load_research_config(Path::new("../../config")).unwrap();
        c.ledger_path = dir.join("ledger.jsonl").display().to_string();
        c.cards_dir = dir.join("cards").display().to_string();
        c
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("vwap_verify_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("cards")).unwrap();
        d
    }

    const CARD: &str = "id = \"T1\"\nfamily = \"regime\"\nmechanism = \"a long enough mechanism text for the test\"\noutcome = \"rejection\"\nfeature = \"vslope_align\"\nrule = \"top_tercile\"\ndirection = \"positive\"\ntimeframes = [5]\n";

    fn run_entry(cycle: usize, sha: &str) -> Entry {
        Entry::Run {
            cycle,
            card_id: "T1".into(),
            sha256: sha.into(),
            git_commit: "c".into(),
            view: "discovery".into(),
            timeframes: vec![5],
            trials_added: 1,
            alpha_spent: 0.003,
            delta: 0.0,
            p: 0.5,
            mde: None,
            data_fingerprint: None,
            verdict: Verdict::Drop,
            time_utc: "t".into(),
        }
    }

    #[test]
    fn a_consistent_ledger_verifies_and_each_kind_of_tampering_is_reported() {
        let dir = tmp("ok");
        let cfg = cfg_in(&dir);
        fs::write(dir.join("cards/T1.toml"), CARD).unwrap();
        let hash = load_card(&dir.join("cards/T1.toml")).unwrap().1;
        let l = Ledger::open(Path::new(&cfg.ledger_path));
        l.append(&Entry::Registered {
            card_id: "T1".into(),
            sha256: hash.clone(),
            git_commit: "c".into(),
            time_utc: "t".into(),
        })
        .unwrap();
        l.append(&run_entry(1, &hash)).unwrap();
        let r = verify(&cfg).unwrap();
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        assert_eq!(r.cards_run, 1);

        // a card edited after registration
        fs::write(
            dir.join("cards/T1.toml"),
            format!("{CARD}min_effect = 0.5\n"),
        )
        .unwrap();
        assert!(
            verify(&cfg)
                .unwrap()
                .problems
                .iter()
                .any(|p| p.contains("changed after registration"))
        );
        fs::write(dir.join("cards/T1.toml"), CARD).unwrap();

        // a skipped cycle number
        l.append(&run_entry(5, &hash)).unwrap();
        assert!(
            verify(&cfg)
                .unwrap()
                .problems
                .iter()
                .any(|p| p.contains("breaks the sequence"))
        );
    }

    #[test]
    fn a_run_without_registration_and_a_confirmation_without_promote_are_reported() {
        let dir = tmp("bad");
        let cfg = cfg_in(&dir);
        let l = Ledger::open(Path::new(&cfg.ledger_path));
        l.append(&run_entry(1, "h")).unwrap();
        l.append(&Entry::Confirm {
            cycle: 2,
            card_id: "T1".into(),
            sha256: "h".into(),
            git_commit: "c".into(),
            delta: 0.0,
            p: 0.5,
            verdict: Verdict::NotConfirmed,
            time_utc: "t".into(),
        })
        .unwrap();
        let p = verify(&cfg).unwrap().problems;
        assert!(p.iter().any(|x| x.contains("without being registered")));
        assert!(p.iter().any(|x| x.contains("confirmed without a PROMOTE")));
    }
}

/// The cut points a card used on discovery, per timeframe. Confirmation and holdout reuse them.
fn discovery_cuts(
    card: &crate::card::Card,
    cfg: &ResearchConfig,
    samples: &[Sample],
) -> std::collections::BTreeMap<u32, f64> {
    let view = DiscoveryView::select(samples, &Bounds::from_config(cfg));
    run_card(card, view.samples(), cfg)
        .widths
        .iter()
        .filter_map(|w| w.cut_points.first().map(|c| (w.width, *c)))
        .collect()
}

fn write_report(cfg: &ResearchConfig, name: &str, text: &str) -> Result<(), ResearchError> {
    let dir = Path::new(&cfg.report_dir);
    fs::create_dir_all(dir).map_err(|source| ResearchError::Io {
        path: dir.display().to_string(),
        source,
    })?;
    let rp = dir.join(name);
    fs::write(&rp, text).map_err(|source| ResearchError::Io {
        path: rp.display().to_string(),
        source,
    })
}

fn registered_card(
    cfg: &ResearchConfig,
    ledger: &Ledger,
    card_path: &Path,
) -> Result<(crate::card::Card, String), ResearchError> {
    let (card, hash) = load_card(card_path)?;
    card.validate(feature_names())?;
    match ledger.registered_hash(&card.id)? {
        None => Err(ResearchError::NotRegistered { id: card.id }),
        Some(h) if h != hash => Err(ResearchError::CardChanged {
            id: card.id,
            registered: h,
            current: hash,
        }),
        Some(_) => {
            let _ = cfg;
            Ok((card, hash))
        }
    }
}

/// One shot confirmation of a promoted card on the confirmation segment, with the cut points
/// frozen on discovery. The confirmation alpha is the total alpha shared Bonferroni style over
/// the promoted cards.
pub fn execute_confirm(
    cfg: &ResearchConfig,
    card_path: &Path,
    git_commit: &str,
    samples: &[Sample],
) -> Result<CardResult, ResearchError> {
    selftest(cfg)?;
    let ledger = Ledger::open(Path::new(&cfg.ledger_path));
    let (card, hash) = registered_card(cfg, &ledger, card_path)?;
    let permit = ledger.confirm_permit(&card.id)?;
    let promoted = ledger.promoted_cards()?.len().max(1);
    let alpha = cfg.alpha_total / promoted as f64;
    let cuts = discovery_cuts(&card, cfg, samples);
    let bounds = Bounds::from_config(cfg);
    let view = ConfirmView::select(samples, &bounds, &permit);
    if view.samples().len() < cfg.min_discovery_samples / 5 {
        return Err(ResearchError::NotEnoughData(format!(
            "only {} confirmation samples",
            view.samples().len()
        )));
    }
    let mut result = run_card_with(&card, view.samples(), cfg, &cuts, Some(alpha));
    let (verdict, reasons, p) = decide_confirm(&card, &result.widths, alpha, cfg);
    result.verdict = verdict;
    result.reasons = reasons;
    result.p = p;
    write_report(
        cfg,
        &format!("{}_confirm.md", card.id),
        &render(&card, &result, "confirmation"),
    )?;
    ledger.append(&Entry::Confirm {
        cycle: ledger.next_cycle()?,
        card_id: card.id.clone(),
        sha256: hash,
        git_commit: git_commit.to_string(),
        delta: result.delta,
        p: result.p,
        verdict: result.verdict,
        time_utc: now(),
    })?;
    Ok(result)
}

/// The freeze file naming the one strategy allowed onto the holdout.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Freeze {
    pub card_id: String,
    pub note: String,
}

/// The single run on the holdout segment. It needs a freeze file, a confirmed card and an unused
/// lock, and it creates the lock before computing anything.
pub fn execute_holdout(
    cfg: &ResearchConfig,
    freeze_path: &Path,
    git_commit: &str,
    samples: &[Sample],
) -> Result<CardResult, ResearchError> {
    let text = fs::read_to_string(freeze_path).map_err(|_| {
        ResearchError::HoldoutLocked(format!(
            "{} does not exist; the holdout needs a freeze file naming a confirmed card",
            freeze_path.display()
        ))
    })?;
    let freeze: Freeze = toml::from_str(&text).map_err(|source| ResearchError::Toml {
        path: freeze_path.display().to_string(),
        source,
    })?;
    let ledger = Ledger::open(Path::new(&cfg.ledger_path));
    let entries = ledger.read_all()?;
    let confirmed = entries.iter().any(|e| {
        matches!(e, Entry::Confirm { card_id, verdict: Verdict::Confirmed, .. } if *card_id == freeze.card_id)
    });
    if !confirmed {
        return Err(ResearchError::HoldoutLocked(format!(
            "{} has no Confirmed entry in the ledger",
            freeze.card_id
        )));
    }
    if entries.iter().any(|e| matches!(e, Entry::Holdout { .. })) {
        return Err(ResearchError::HoldoutLocked(
            "the holdout was already run".into(),
        ));
    }
    selftest(cfg)?;
    let card_path = Path::new(&cfg.cards_dir).join(format!("{}.toml", freeze.card_id));
    let (card, hash) = registered_card(cfg, &ledger, &card_path)?;
    let lock = Path::new(&cfg.ledger_path).parent().map_or_else(
        || Path::new("holdout.lock").to_path_buf(),
        |d| d.join("holdout.lock"),
    );
    let permit = HoldoutPermit::acquire(
        &lock,
        &format!("{} at {git_commit}: {}", freeze.card_id, freeze.note),
    )?;
    let cuts = discovery_cuts(&card, cfg, samples);
    let view = HoldoutView::select(samples, &Bounds::from_config(cfg), &permit);
    let mut result = run_card_with(&card, view.samples(), cfg, &cuts, Some(cfg.alpha_total));
    let (verdict, reasons, p) = decide_confirm(&card, &result.widths, cfg.alpha_total, cfg);
    result.verdict = verdict;
    result.reasons = reasons;
    result.p = p;
    write_report(
        cfg,
        &format!("{}_holdout.md", card.id),
        &render(&card, &result, "holdout"),
    )?;
    ledger.append(&Entry::Holdout {
        card_id: card.id.clone(),
        sha256: hash,
        git_commit: git_commit.to_string(),
        delta: result.delta,
        p: result.p,
        verdict: result.verdict,
        time_utc: now(),
    })?;
    Ok(result)
}

#[cfg(test)]
mod confirm_tests {
    use super::*;
    use crate::selftest::synthetic_samples;

    fn setup(name: &str) -> (ResearchConfig, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("vwap_confirm_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("cards")).unwrap();
        let mut c = vwap_core::load_research_config(Path::new("../../config")).unwrap();
        c.ledger_path = dir.join("ledger.jsonl").display().to_string();
        c.cards_dir = dir.join("cards").display().to_string();
        c.report_dir = dir.join("reports").display().to_string();
        c.selftest_null_trials = 6;
        c.bootstrap_draws = 400;
        (c, dir)
    }

    const CARD: &str = "id = \"T1\"\nfamily = \"regime\"\nmechanism = \"a planted effect used to test the whole loop\"\noutcome = \"rejection_given_resolved\"\nfeature = \"vslope_align\"\nrule = \"top_tercile\"\ndirection = \"positive\"\ntimeframes = [5, 15]\n";

    /// Planted samples placed in the three segments of the split.
    fn segments(cfg: &ResearchConfig, effect: f64) -> Vec<Sample> {
        let b = Bounds::from_config(cfg);
        let mut out = Vec::new();
        for (k, start) in [b.discovery_start, b.confirm_start, b.holdout_start]
            .into_iter()
            .enumerate()
        {
            let mut s = synthetic_samples(effect, 700, 5, 100 + k as u64);
            for (i, x) in s.iter_mut().enumerate() {
                x.t_close_us = start + 3_600_000_000 * (i as i64 % 1000);
                x.t_end_us = x.t_close_us + 3_600_000_000;
                x.session += 10_000 * k as i32;
            }
            out.extend(s);
        }
        out
    }

    #[test]
    fn promote_then_confirm_then_one_holdout_run() {
        let (cfg, dir) = setup("flow");
        let card_path = dir.join("cards/T1.toml");
        fs::write(&card_path, CARD).unwrap();
        let samples = segments(&cfg, 0.10);
        register(&cfg, &card_path, "c0").unwrap();
        let r = execute(&cfg, &card_path, "c1", "fp", &samples).unwrap();
        assert_eq!(r.verdict, Verdict::Promote, "{:?}", r.reasons);

        // holdout needs a freeze file and a confirmed card
        let freeze = dir.join("FREEZE.toml");
        assert!(execute_holdout(&cfg, &freeze, "c2", &samples).is_err());
        fs::write(&freeze, "card_id = \"T1\"\nnote = \"test\"\n").unwrap();
        assert!(execute_holdout(&cfg, &freeze, "c2", &samples).is_err()); // not confirmed yet

        let c = execute_confirm(&cfg, &card_path, "c3", &samples).unwrap();
        assert_eq!(c.verdict, Verdict::Confirmed, "{:?}", c.reasons);
        // confirmation is one shot
        assert!(execute_confirm(&cfg, &card_path, "c3", &samples).is_err());

        let h = execute_holdout(&cfg, &freeze, "c4", &samples).unwrap();
        assert_eq!(h.verdict, Verdict::Confirmed, "{:?}", h.reasons);
        // the holdout runs once
        assert!(matches!(
            execute_holdout(&cfg, &freeze, "c5", &samples),
            Err(ResearchError::HoldoutLocked(_))
        ));
        // and the whole record still verifies
        let v = verify(&cfg).unwrap();
        assert!(v.problems.is_empty(), "{:?}", v.problems);
    }

    #[test]
    fn a_card_that_is_not_promoted_cannot_be_confirmed() {
        let (cfg, dir) = setup("noprom");
        let card_path = dir.join("cards/T1.toml");
        fs::write(&card_path, CARD).unwrap();
        let samples = segments(&cfg, 0.0);
        register(&cfg, &card_path, "c0").unwrap();
        let r = execute(&cfg, &card_path, "c1", "fp", &samples).unwrap();
        assert_ne!(r.verdict, Verdict::Promote);
        assert!(matches!(
            execute_confirm(&cfg, &card_path, "c2", &samples),
            Err(ResearchError::ConfirmNotAllowed(_))
        ));
    }

    #[test]
    fn a_confirmation_does_not_pass_when_the_effect_disappears_out_of_sample() {
        let (cfg, dir) = setup("fade");
        let card_path = dir.join("cards/T1.toml");
        fs::write(&card_path, CARD).unwrap();
        let b = Bounds::from_config(&cfg);
        let mut samples = segments(&cfg, 0.10);
        // replace the confirmation segment with a null one
        samples.retain(|s| s.t_close_us < b.confirm_start || s.t_close_us >= b.holdout_start);
        let mut null = synthetic_samples(0.0, 700, 5, 777);
        for (i, x) in null.iter_mut().enumerate() {
            x.t_close_us = b.confirm_start + 3_600_000_000 * (i as i64 % 1000);
            x.t_end_us = x.t_close_us + 3_600_000_000;
            x.session += 50_000;
        }
        samples.extend(null);
        register(&cfg, &card_path, "c0").unwrap();
        let r = execute(&cfg, &card_path, "c1", "fp", &samples).unwrap();
        assert_eq!(r.verdict, Verdict::Promote);
        let c = execute_confirm(&cfg, &card_path, "c2", &samples).unwrap();
        assert_eq!(c.verdict, Verdict::NotConfirmed);
    }
}
