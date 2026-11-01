//! The research loop commands.

use std::path::Path;

use anyhow::{Result, bail, ensure};
use vwap_core::ResearchConfig;
use vwap_research::strategy::Env;

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
    let calendar = crate::ctx::calendar_table(config, &feat)?;
    let s = vwap_research::data::assemble(&data, &ev, &lab, &feat, &calendar, &clean.bars);
    Ok((s, clean.fingerprint, lab))
}

/// Everything a strategy command needs: configuration, the samples and the bars.
struct World {
    research: ResearchConfig,
    strategy: vwap_core::StrategyConfig,
    validation: vwap_core::ValidationConfig,
    backtest: vwap_core::BacktestConfig,
    labels: vwap_core::LabelsConfig,
    coarse: std::collections::BTreeMap<u32, vwap_events::CoarseBars>,
    clean: crate::ctx::Clean,
    samples: Vec<vwap_research::Sample>,
}

fn world(config: &Path) -> Result<World> {
    let data = vwap_core::load_data_config(config)?;
    let ev = vwap_core::load_events_config(config)?;
    let labels = vwap_core::load_labels_config(config)?;
    let feat = vwap_core::load_features_config(config)?;
    let clean = load_clean(&data)?;
    let calendar = crate::ctx::calendar_table(config, &feat)?;
    let samples = vwap_dataset::assemble(&data, &ev, &labels, &feat, &calendar, &clean.bars);
    let coarse = ev
        .timeframes
        .iter()
        .map(|&w| (w, vwap_events::build(&clean.bars, &data, w, ev.atr_length)))
        .collect();
    Ok(World {
        research: vwap_core::load_research_config(config)?,
        strategy: vwap_core::load_strategy_config(config)?,
        validation: vwap_core::load_validation_config(config)?,
        backtest: vwap_core::load_backtest_config(config)?,
        labels,
        coarse,
        clean,
        samples,
    })
}

impl World {
    fn env<'a>(&'a self, config: &'a Path) -> Env<'a> {
        Env {
            research: &self.research,
            strategy: &self.strategy,
            validation: &self.validation,
            backtest: &self.backtest,
            labels: &self.labels,
            config_dir: config,
            coarse: &self.coarse,
            one: &self.clean.bars,
        }
    }
}

fn is_strategy_id(id: &str) -> bool {
    id.starts_with('S')
}

/// Timeframes of a strategy with a PROMOTE verdict in the ledger.
fn promoted_widths(cfg: &ResearchConfig, id: &str) -> Result<Vec<u32>> {
    let ledger = vwap_research::Ledger::open(Path::new(&cfg.ledger_path));
    let mut w: Vec<u32> = ledger
        .promoted_strategies()?
        .into_iter()
        .filter(|(c, _)| c == id)
        .map(|(_, w)| w)
        .collect();
    w.sort_unstable();
    Ok(w)
}

/// Runs a registered strategy card on the discovery view, once per timeframe.
pub fn strategy(id: &str, config: &Path) -> Result<String> {
    let cfg = vwap_core::load_research_config(config)?;
    guard_ledger(&cfg)?;
    ensure!(
        is_strategy_id(id),
        "{id} is not a strategy card (ids start with S)"
    );
    let (path, commit) = committed_card(&cfg, id)?;
    let w = world(config)?;
    let runs = vwap_research::strategy::execute_strategy(
        &w.env(config),
        &path,
        &commit,
        &w.clean.fingerprint,
        &w.samples,
    )?;
    let mut out = String::new();
    for r in &runs {
        out.push_str(&format!(
            "{id} on {}m: {:?}, {} trades, net {:.4} ticks, BSS {}, DSR {}, PBO {}\n",
            r.width,
            r.verdict,
            r.result.backtest.n_trades,
            r.result.backtest.expectancy_ticks,
            r.result
                .pooled
                .as_ref()
                .map_or("n/a".into(), |p| format!("{:.4}", p.bss)),
            r.dsr_pbo.dsr.map_or("n/a".into(), |x| format!("{x:.4}")),
            r.dsr_pbo.pbo.map_or("n/a".into(), |x| format!("{x:.4}")),
        ));
        for f in &r.failed {
            out.push_str(&format!("  failed: {f}\n"));
        }
    }
    Ok(out)
}

/// DSR and PBO of every strategy trial.
pub fn dsr(config: &Path) -> Result<String> {
    let cfg = vwap_core::load_research_config(config)?;
    guard_ledger(&cfg)?;
    let validation = vwap_core::load_validation_config(config)?;
    // the report reads only the ledger and the tracked summaries, so it needs no bars
    let coarse = std::collections::BTreeMap::new();
    let one = vwap_data::CleanBars::default();
    let strategy = vwap_core::load_strategy_config(config)?;
    let backtest = vwap_core::load_backtest_config(config)?;
    let labels = vwap_core::load_labels_config(config)?;
    let env = Env {
        research: &cfg,
        strategy: &strategy,
        validation: &validation,
        backtest: &backtest,
        labels: &labels,
        config_dir: config,
        coarse: &coarse,
        one: &one,
    };
    Ok(vwap_research::strategy::dsr_report(&env)?)
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
    ensure!(
        !is_strategy_id(id),
        "{id} is a strategy card; run it with `vwap research strategy {id} config`"
    );
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
    if is_strategy_id(id) {
        let widths = promoted_widths(&cfg, id)?;
        ensure!(
            !widths.is_empty(),
            "{id} has no PROMOTE verdict on any timeframe"
        );
        let w = world(config)?;
        let env = w.env(config);
        let mut out = String::new();
        for width in widths {
            let r = vwap_research::strategy::execute_strategy_confirm(
                &env, &path, width, &commit, &w.samples,
            )?;
            out.push_str(&format!("{id} on {width}m confirmation: {:?}\n", r.verdict));
            for f in &r.failed {
                out.push_str(&format!("  failed: {f}\n"));
            }
        }
        return Ok(out);
    }
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
    if let Ok(text) = std::fs::read_to_string(&freeze)
        && let Ok(f) = toml::from_str::<vwap_research::cycle::Freeze>(&text)
        && is_strategy_id(&f.card_id)
    {
        let width = f
            .width
            .ok_or_else(|| anyhow::anyhow!("the freeze file of a strategy needs a width"))?;
        let w = world(config)?;
        let r = vwap_research::strategy::execute_strategy_holdout(
            &w.env(config),
            &f.card_id,
            width,
            &f.note,
            &head(),
            &w.samples,
        )?;
        let mut out = format!("{} on {width}m holdout: {:?}\n", f.card_id, r.verdict);
        for x in &r.failed {
            out.push_str(&format!("  failed: {x}\n"));
        }
        return Ok(out);
    }
    let (s, _, _) = samples(config)?;
    let r = vwap_research::cycle::execute_holdout(&cfg, &freeze, &head(), &s)?;
    Ok(serde_json::to_string_pretty(&r)?)
}

/// Structure of the ledger plus a check that every commit it names exists.
pub fn verify(config: &Path) -> Result<String> {
    let cfg = vwap_core::load_research_config(config)?;
    guard_ledger(&cfg)?;
    let mut report = vwap_research::cycle::verify(&cfg)?;
    let validation = vwap_core::load_validation_config(config)?;
    report
        .problems
        .extend(vwap_research::strategy::verify_strategy_files(
            &cfg,
            &validation,
            config,
        )?);
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
    let w = world(config)?;
    let env = w.env(config);
    let mut out: Vec<serde_json::Value> = Vec::new();
    let mut bad: Vec<String> = Vec::new();
    for card_id in &ids {
        let path = Path::new(&cfg.cards_dir).join(format!("{card_id}.toml"));
        if is_strategy_id(card_id) {
            let widths: Vec<u32> = ledger
                .strategies_run()?
                .into_iter()
                .filter(|(c, _)| c == card_id)
                .map(|(_, w)| w)
                .collect();
            for width in widths {
                let r = vwap_research::strategy::replay_strategy(
                    &env,
                    &path,
                    width,
                    &w.clean.fingerprint,
                    &w.samples,
                )?;
                if !r.matches {
                    bad.push(format!("{card_id}/{width}m"));
                }
                out.push(serde_json::to_value(&r)?);
            }
        } else {
            let r = vwap_research::cycle::replay(&cfg, &path, &w.clean.fingerprint, &w.samples)?;
            if !r.matches {
                bad.push(r.card_id.clone());
            }
            out.push(serde_json::to_value(&r)?);
        }
    }
    let text = serde_json::to_string_pretty(&out)?;
    if !bad.is_empty() {
        bail!("replay does not reproduce the ledger for {bad:?}:\n{text}");
    }
    Ok(text)
}
