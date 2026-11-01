//! The research loop commands.

use std::path::Path;

use anyhow::{Result, bail, ensure};
use vwap_core::ResearchConfig;

use crate::ctx::{git, guard_ledger, head, load_clean};

/// The card file must be tracked and have no uncommitted changes.
fn committed_card(cfg: &ResearchConfig, id: &str) -> Result<(std::path::PathBuf, String)> {
    let path = Path::new(&cfg.cards_dir).join(format!("{id}.toml"));
    let p = path.to_string_lossy().to_string();
    git(&["ls-files", "--error-unmatch", &p])
        .map_err(|_| anyhow::anyhow!("{p} is not committed; commit the card first"))?;
    ensure!(
        git(&["status", "--porcelain", "--", &p])?.is_empty(),
        "{p} has uncommitted changes; commit the card first"
    );
    Ok((path, head()))
}

fn samples(config: &Path) -> Result<(Vec<vwap_research::Sample>, String, vwap_core::LabelsConfig)> {
    let data = vwap_core::load_data_config(config)?;
    let ev = vwap_core::load_events_config(config)?;
    let lab = vwap_core::load_labels_config(config)?;
    let feat = vwap_core::load_features_config(config)?;
    let clean = load_clean(&data)?;
    let s = vwap_research::data::assemble(&data, &ev, &lab, &feat, &clean.bars);
    Ok((s, clean.fingerprint, lab))
}

pub fn status(config: &Path) -> Result<String> {
    let cfg = vwap_core::load_research_config(config)?;
    guard_ledger(&cfg)?;
    Ok(vwap_research::cycle::status(&cfg)?)
}

pub fn register(id: &str, config: &Path) -> Result<String> {
    let cfg = vwap_core::load_research_config(config)?;
    guard_ledger(&cfg)?;
    let (path, commit) = committed_card(&cfg, id)?;
    let hash = vwap_research::cycle::register(&cfg, &path, &commit)?;
    Ok(format!("registered {id} sha256 {hash} at commit {commit}"))
}

pub fn run(id: &str, config: &Path) -> Result<String> {
    let cfg = vwap_core::load_research_config(config)?;
    guard_ledger(&cfg)?;
    let (path, commit) = committed_card(&cfg, id)?;
    let (s, fingerprint, _) = samples(config)?;
    let r = vwap_research::cycle::execute(&cfg, &path, &commit, &fingerprint, &s)?;
    Ok(serde_json::to_string_pretty(&r)?)
}

pub fn omnibus(config: &Path) -> Result<String> {
    let cfg = vwap_core::load_research_config(config)?;
    guard_ledger(&cfg)?;
    let model = vwap_core::load_model_config(config)?;
    let (s, fingerprint, lab) = samples(config)?;
    let r = vwap_research::cycle::execute_omnibus(&cfg, &model, &head(), &fingerprint, &s, |w| {
        lab.horizon(w)
    })?;
    Ok(serde_json::to_string_pretty(&r)?)
}

pub fn diagnose(config: &Path) -> Result<String> {
    let cfg = vwap_core::load_research_config(config)?;
    guard_ledger(&cfg)?;
    let model = vwap_core::load_model_config(config)?;
    let (s, _, lab) = samples(config)?;
    let tasks = [
        vwap_research::omnibus::Task::Direction,
        vwap_research::omnibus::Task::Chop,
    ];
    let r = vwap_research::cycle::execute_diagnostic(
        &cfg,
        &model,
        &head(),
        &s,
        |w| lab.horizon(w),
        &tasks,
        model.diagnostic_permutations,
    )?;
    Ok(serde_json::to_string_pretty(&r)?)
}

/// One shot confirmation of a promoted card on the confirmation segment.
pub fn confirm(id: &str, config: &Path) -> Result<String> {
    let cfg = vwap_core::load_research_config(config)?;
    guard_ledger(&cfg)?;
    let (path, commit) = committed_card(&cfg, id)?;
    let (s, _, _) = samples(config)?;
    let r = vwap_research::cycle::execute_confirm(&cfg, &path, &commit, &s)?;
    Ok(serde_json::to_string_pretty(&r)?)
}

/// The single run on the holdout. Needs research/FREEZE.toml and a confirmed card.
pub fn holdout(config: &Path) -> Result<String> {
    let cfg = vwap_core::load_research_config(config)?;
    guard_ledger(&cfg)?;
    ensure!(
        git(&["status", "--porcelain"])?.is_empty(),
        "the git tree must be clean before the holdout run, so the run is tied to a commit"
    );
    let freeze = Path::new(&cfg.ledger_path).parent().map_or_else(
        || Path::new("FREEZE.toml").to_path_buf(),
        |d| d.join("FREEZE.toml"),
    );
    let (s, _, _) = samples(config)?;
    let r = vwap_research::cycle::execute_holdout(&cfg, &freeze, &head(), &s)?;
    Ok(serde_json::to_string_pretty(&r)?)
}

/// Structure of the ledger plus a check that every commit it names exists.
pub fn verify(config: &Path) -> Result<String> {
    let cfg = vwap_core::load_research_config(config)?;
    guard_ledger(&cfg)?;
    let mut report = vwap_research::cycle::verify(&cfg)?;
    if git(&["rev-parse", "--git-dir"]).is_ok() {
        for c in report.commits.clone() {
            if git(&["cat-file", "-e", &format!("{c}^{{commit}}")]).is_err() {
                report.problems.push(format!(
                    "the ledger names commit {c}, which is not in this repository"
                ));
            }
        }
    }
    let text = serde_json::to_string_pretty(&report)?;
    if !report.problems.is_empty() {
        bail!("ledger verification failed:\n{text}");
    }
    Ok(text)
}

/// Re-runs one card, or every registered card for `all`, and compares with the ledger.
pub fn replay(id: &str, config: &Path) -> Result<String> {
    let cfg = vwap_core::load_research_config(config)?;
    guard_ledger(&cfg)?;
    let ledger = vwap_research::Ledger::open(Path::new(&cfg.ledger_path));
    let ids: Vec<String> = if id == "all" {
        let mut v: Vec<String> = ledger
            .read_all()?
            .into_iter()
            .filter_map(|e| match e {
                vwap_research::Entry::Registered { card_id, .. } => Some(card_id),
                _ => None,
            })
            .collect();
        v.sort();
        v.dedup();
        v
    } else {
        vec![id.to_string()]
    };
    let (s, fingerprint, _) = samples(config)?;
    let mut out = Vec::new();
    for card_id in &ids {
        let path = Path::new(&cfg.cards_dir).join(format!("{card_id}.toml"));
        out.push(vwap_research::cycle::replay(&cfg, &path, &fingerprint, &s)?);
    }
    let text = serde_json::to_string_pretty(&out)?;
    let bad: Vec<&str> = out
        .iter()
        .filter(|r| !r.matches)
        .map(|r| r.card_id.as_str())
        .collect();
    if !bad.is_empty() {
        bail!("replay does not reproduce the ledger for {bad:?}:\n{text}");
    }
    Ok(text)
}
