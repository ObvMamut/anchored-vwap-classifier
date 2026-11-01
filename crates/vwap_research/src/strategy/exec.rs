//! Running strategy cards under the protocol: registration check, the discovery run, the DSR
//! and PBO over the counted trials, the tracked summary file and the ledger entry; then the
//! one shot confirmation, the single holdout run, replay and the file checks of `verify`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use vwap_core::{
    BacktestConfig, LabelsConfig, ResearchConfig, StrategyConfig, ValidationConfig, manifest,
};
use vwap_data::CleanBars;
use vwap_events::CoarseBars;
use vwap_features::feature_names;
use vwap_validation::shift_months;

use crate::error::ResearchError;
use crate::ledger::{Entry, Ledger, Verdict};
use crate::split::{Bounds, ConfirmView, DiscoveryView, HoldoutPermit, HoldoutView, Sample};
use crate::strategy::card::{StrategyCard, load_strategy_card, sha_hex};
use crate::strategy::eval::{Configs, Geometry, StrategyResult, evaluate};
use crate::strategy::gates::{DsrPbo, Series, decide, dsr_pbo, own_failures, read_series};

/// Everything a strategy run reads besides the samples.
pub struct Env<'a> {
    pub research: &'a ResearchConfig,
    pub strategy: &'a StrategyConfig,
    pub validation: &'a ValidationConfig,
    pub backtest: &'a BacktestConfig,
    pub labels: &'a LabelsConfig,
    /// The configuration directory, whose files are hashed into the ledger entry.
    pub config_dir: &'a Path,
    /// Coarse bars per timeframe and the 1 minute bars underneath.
    pub coarse: &'a BTreeMap<u32, CoarseBars>,
    pub one: &'a CleanBars,
}

/// Configuration files whose hashes are recorded with a run.
const CONFIG_FILES: [&str; 6] = [
    "research.toml",
    "labels.toml",
    "features.toml",
    "strategy.toml",
    "validation.toml",
    "backtest.toml",
];

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn io_err(path: &Path) -> impl Fn(std::io::Error) -> ResearchError + '_ {
    move |source| ResearchError::Io {
        path: path.display().to_string(),
        source,
    }
}

fn config_hashes(env: &Env) -> Result<BTreeMap<String, String>, ResearchError> {
    Ok(manifest::config_hashes(env.config_dir, &CONFIG_FILES)?)
}

fn summary_path(env: &Env, id: &str, width: u32) -> PathBuf {
    Path::new(&env.research.strategies_dir).join(format!("{id}_{width}m.json"))
}

fn sub_summary_path(env: &Env, sub: &str, id: &str, width: u32) -> PathBuf {
    Path::new(&env.research.strategies_dir)
        .join(sub)
        .join(format!("{id}_{width}m.json"))
}

fn registered_strategy(
    ledger: &Ledger,
    card_path: &Path,
    labels: &LabelsConfig,
) -> Result<(StrategyCard, String), ResearchError> {
    let (card, hash) = load_strategy_card(card_path)?;
    card.validate(feature_names())?;
    card.check_labels(labels)?;
    match ledger.registered_hash(&card.id)? {
        None => Err(ResearchError::NotRegistered { id: card.id }),
        Some(h) if h != hash => Err(ResearchError::CardChanged {
            id: card.id,
            registered: h,
            current: hash,
        }),
        Some(_) => Ok((card, hash)),
    }
}

fn configs<'a>(env: &'a Env, card: &StrategyCard) -> Configs<'a> {
    Configs {
        strategy: env.strategy,
        validation: env.validation,
        backtest: env.backtest,
        direction: card.direction(),
        min_trades_per_week: card.min_trades_per_week,
    }
}

fn bars_for<'a>(env: &'a Env, width: u32) -> Result<&'a CoarseBars, ResearchError> {
    env.coarse
        .get(&width)
        .ok_or_else(|| ResearchError::Strategy(format!("no {width} minute bars were built")))
}

#[derive(Serialize)]
struct Summary<'a> {
    card_id: &'a str,
    card_sha256: &'a str,
    width: u32,
    view: &'a str,
    data_fingerprint: Option<&'a str>,
    config_sha256: &'a BTreeMap<String, String>,
    verdict: Verdict,
    failed: &'a [String],
    dsr_pbo: Option<&'a DsrPbo>,
    result: &'a StrategyResult,
}

/// The outcome of one (card, timeframe) discovery run.
#[derive(Debug)]
pub struct StrategyRun {
    pub width: u32,
    pub result: StrategyResult,
    pub dsr_pbo: DsrPbo,
    pub verdict: Verdict,
    pub failed: Vec<String>,
}

fn discovery_geometry(env: &Env, width: u32) -> Geometry {
    Geometry {
        range: (
            ResearchConfig::date_us(&env.research.discovery_start),
            ResearchConfig::date_us(&env.research.confirm_start),
        ),
        horizon_bars: env.labels.horizon(width),
    }
}

/// Runs a registered strategy card on the discovery view, once per timeframe. A timeframe that
/// already has a ledger entry is refused: use replay to check it.
pub fn execute_strategy(
    env: &Env,
    card_path: &Path,
    git_commit: &str,
    fingerprint: &str,
    samples: &[Sample],
) -> Result<Vec<StrategyRun>, ResearchError> {
    let ledger = Ledger::open(Path::new(&env.research.ledger_path));
    let (card, hash) = registered_strategy(&ledger, card_path, env.labels)?;
    let view = DiscoveryView::select(samples, &Bounds::from_config(env.research));
    if view.samples().len() < env.research.min_discovery_samples {
        return Err(ResearchError::NotEnoughData(format!(
            "only {} discovery samples",
            view.samples().len()
        )));
    }
    let idx = card.feature_indices(feature_names());
    let hashes = config_hashes(env)?;
    let mut widths = card.timeframes.clone();
    widths.sort_unstable();
    let mut runs = Vec::new();
    for width in widths {
        if ledger.strategies_run()?.contains(&(card.id.clone(), width)) {
            return Err(ResearchError::Strategy(format!(
                "{} on {width}m was already run; strategy runs are not repeated, use replay",
                card.id
            )));
        }
        let sp = summary_path(env, &card.id, width);
        if sp.exists() {
            return Err(ResearchError::Strategy(format!(
                "{} exists without a ledger entry; remove it before running",
                sp.display()
            )));
        }
        let result = evaluate(
            &card.id,
            card.model.family(),
            width,
            &idx,
            view.samples(),
            bars_for(env, width)?,
            env.one,
            &discovery_geometry(env, width),
            &configs(env, &card),
        )?;
        let own = Series {
            key: format!("{}_{width}m", card.id),
            daily_pnl: result.daily_pnl.clone(),
        };
        let others = read_series(Path::new(&env.research.strategies_dir))?;
        let n_trials = ledger.trials()? + 1;
        let d = dsr_pbo(&own, &others, n_trials, env.validation);
        let (verdict, failed) = decide(
            own_failures(&result, env.validation, card.min_trades_per_week),
            &d,
            env.validation,
        );
        let summary = Summary {
            card_id: &card.id,
            card_sha256: &hash,
            width,
            view: "discovery",
            data_fingerprint: Some(fingerprint),
            config_sha256: &hashes,
            verdict,
            failed: &failed,
            dsr_pbo: Some(&d),
            result: &result,
        };
        let text = serde_json::to_string_pretty(&summary)?;
        if let Some(dir) = sp.parent() {
            fs::create_dir_all(dir).map_err(io_err(dir))?;
        }
        fs::write(&sp, &text).map_err(io_err(&sp))?;
        ledger.append(&Entry::Strategy {
            cycle: ledger.next_cycle()?,
            card_id: card.id.clone(),
            sha256: hash.clone(),
            git_commit: git_commit.to_string(),
            width,
            trials_added: 1,
            n_trials: d.n_trials,
            config_sha256: hashes.clone(),
            summary_sha256: sha_hex(text.as_bytes()),
            data_fingerprint: Some(fingerprint.to_string()),
            n_trades: result.backtest.n_trades,
            bss: result.pooled.as_ref().map(|p| p.bss),
            net_expectancy_ticks: Some(result.backtest.expectancy_ticks).filter(|x| x.is_finite()),
            dsr: d.dsr,
            pbo: d.pbo,
            verdict,
            failed: failed.clone(),
            time_utc: now(),
        })?;
        runs.push(StrategyRun {
            width,
            result,
            dsr_pbo: d,
            verdict,
            failed,
        });
    }
    Ok(runs)
}

/// The outcome of a confirmation or a holdout run.
#[derive(Debug)]
pub struct SegmentRun {
    pub width: u32,
    pub result: StrategyResult,
    pub verdict: Verdict,
    pub failed: Vec<String>,
}

/// Validation settings with the interval level of a one sided test at `alpha`:
/// $1 - 2\alpha$.
fn at_alpha(v: &ValidationConfig, alpha: f64) -> ValidationConfig {
    ValidationConfig {
        interval_level: 1.0 - 2.0 * alpha,
        ..v.clone()
    }
}

fn evaluate_segment(
    env: &Env,
    card: &StrategyCard,
    width: u32,
    pool: &[Sample],
    start_us: i64,
    end_us: i64,
    alpha: f64,
) -> Result<StrategyResult, ResearchError> {
    let from = shift_months(start_us, -i64::from(env.validation.train_months))
        .ok_or_else(|| ResearchError::Strategy("the segment start is out of range".into()))?;
    let v = at_alpha(env.validation, alpha);
    evaluate(
        &card.id,
        card.model.family(),
        width,
        &card.feature_indices(feature_names()),
        pool,
        bars_for(env, width)?,
        env.one,
        &Geometry {
            range: (from, end_us),
            horizon_bars: env.labels.horizon(width),
        },
        &Configs {
            strategy: env.strategy,
            validation: &v,
            backtest: env.backtest,
            direction: card.direction(),
            min_trades_per_week: card.min_trades_per_week,
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn write_sub_summary(
    env: &Env,
    sub: &str,
    card: &StrategyCard,
    hash: &str,
    width: u32,
    view: &str,
    verdict: Verdict,
    failed: &[String],
    result: &StrategyResult,
) -> Result<String, ResearchError> {
    let hashes = config_hashes(env)?;
    let summary = Summary {
        card_id: &card.id,
        card_sha256: hash,
        width,
        view,
        data_fingerprint: None,
        config_sha256: &hashes,
        verdict,
        failed,
        dsr_pbo: None,
        result,
    };
    let text = serde_json::to_string_pretty(&summary)?;
    let path = sub_summary_path(env, sub, &card.id, width);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(io_err(dir))?;
    }
    fs::write(&path, &text).map_err(io_err(&path))?;
    Ok(sha_hex(text.as_bytes()))
}

/// One shot confirmation of a promoted strategy on one timeframe, on the confirmation segment.
/// The configuration is frozen; the models are refit on the rolling pool of earlier events
/// (discovery and the confirmation events before each test window, purged). The same gates as
/// on discovery apply except the DSR and PBO, with the interval level of a one sided test at
/// the total alpha shared over the promoted cards and strategies.
pub fn execute_strategy_confirm(
    env: &Env,
    card_path: &Path,
    width: u32,
    git_commit: &str,
    samples: &[Sample],
) -> Result<SegmentRun, ResearchError> {
    let ledger = Ledger::open(Path::new(&env.research.ledger_path));
    let (card, hash) = registered_strategy(&ledger, card_path, env.labels)?;
    let permit = ledger.strategy_confirm_permit(&card.id, width)?;
    let promoted = (ledger.promoted_cards()?.len() + ledger.promoted_strategies()?.len()).max(1);
    let alpha = env.research.alpha_total / promoted as f64;
    let b = Bounds::from_config(env.research);
    let disc = DiscoveryView::select(samples, &b);
    let conf = ConfirmView::select(samples, &b, &permit);
    let mut pool = disc.samples().to_vec();
    pool.extend_from_slice(conf.samples());
    let result = evaluate_segment(
        env,
        &card,
        width,
        &pool,
        b.confirm_start,
        b.holdout_start,
        alpha,
    )?;
    let failed = own_failures(
        &result,
        &at_alpha(env.validation, alpha),
        card.min_trades_per_week,
    );
    let verdict = if failed.is_empty() {
        Verdict::Confirmed
    } else {
        Verdict::NotConfirmed
    };
    let summary_sha256 = write_sub_summary(
        env,
        "confirm",
        &card,
        &hash,
        width,
        "confirmation",
        verdict,
        &failed,
        &result,
    )?;
    ledger.append(&Entry::StrategyConfirm {
        cycle: ledger.next_cycle()?,
        card_id: card.id.clone(),
        sha256: hash,
        git_commit: git_commit.to_string(),
        width,
        summary_sha256,
        net_expectancy_ticks: Some(result.backtest.expectancy_ticks).filter(|x| x.is_finite()),
        expectancy_lower: Some(result.expectancy_interval.0).filter(|x| x.is_finite()),
        bss: result.pooled.as_ref().map(|p| p.bss),
        verdict,
        failed: failed.clone(),
        time_utc: now(),
    })?;
    Ok(SegmentRun {
        width,
        result,
        verdict,
        failed,
    })
}

/// The single run of a confirmed strategy on the holdout segment. It needs a `Confirmed` entry,
/// no earlier holdout run, and it creates the lock before computing anything.
pub fn execute_strategy_holdout(
    env: &Env,
    card_id: &str,
    width: u32,
    note: &str,
    git_commit: &str,
    samples: &[Sample],
) -> Result<SegmentRun, ResearchError> {
    let ledger = Ledger::open(Path::new(&env.research.ledger_path));
    let entries = ledger.read_all()?;
    let confirmed = entries.iter().any(|e| {
        matches!(e, Entry::StrategyConfirm { card_id: c, width: w, verdict: Verdict::Confirmed, .. }
            if c == card_id && *w == width)
    });
    if !confirmed {
        return Err(ResearchError::HoldoutLocked(format!(
            "{card_id} on {width}m has no Confirmed entry in the ledger"
        )));
    }
    if entries
        .iter()
        .any(|e| matches!(e, Entry::Holdout { .. } | Entry::StrategyHoldout { .. }))
    {
        return Err(ResearchError::HoldoutLocked(
            "the holdout was already run".into(),
        ));
    }
    let card_path = Path::new(&env.research.cards_dir).join(format!("{card_id}.toml"));
    let (card, hash) = registered_strategy(&ledger, &card_path, env.labels)?;
    let lock = Path::new(&env.research.ledger_path).parent().map_or_else(
        || Path::new("holdout.lock").to_path_buf(),
        |d| d.join("holdout.lock"),
    );
    let permit = HoldoutPermit::acquire(
        &lock,
        &format!("{card_id} {width}m at {git_commit}: {note}"),
    )?;
    let b = Bounds::from_config(env.research);
    let permit_c = crate::split::ConfirmPermit {
        card_id: card_id.to_string(),
    };
    let disc = DiscoveryView::select(samples, &b);
    let conf = ConfirmView::select(samples, &b, &permit_c);
    let hold = HoldoutView::select(samples, &b, &permit);
    let end_us = hold
        .samples()
        .iter()
        .map(|s| s.t_end_us)
        .max()
        .map_or(b.holdout_start, |m| m + 1);
    let mut pool = disc.samples().to_vec();
    pool.extend_from_slice(conf.samples());
    pool.extend_from_slice(hold.samples());
    let alpha = env.research.alpha_total;
    let result = evaluate_segment(env, &card, width, &pool, b.holdout_start, end_us, alpha)?;
    let failed = own_failures(
        &result,
        &at_alpha(env.validation, alpha),
        card.min_trades_per_week,
    );
    let verdict = if failed.is_empty() {
        Verdict::Confirmed
    } else {
        Verdict::NotConfirmed
    };
    let summary_sha256 = write_sub_summary(
        env, "holdout", &card, &hash, width, "holdout", verdict, &failed, &result,
    )?;
    ledger.append(&Entry::StrategyHoldout {
        card_id: card.id.clone(),
        sha256: hash,
        git_commit: git_commit.to_string(),
        width,
        summary_sha256,
        net_expectancy_ticks: Some(result.backtest.expectancy_ticks).filter(|x| x.is_finite()),
        expectancy_lower: Some(result.expectancy_interval.0).filter(|x| x.is_finite()),
        bss: result.pooled.as_ref().map(|p| p.bss),
        verdict,
        failed: failed.clone(),
        time_utc: now(),
    })?;
    Ok(SegmentRun {
        width,
        result,
        verdict,
        failed,
    })
}

/// Current DSR and PBO of every strategy trial, with the trial count of the ledger.
pub fn dsr_report(env: &Env) -> Result<String, ResearchError> {
    let ledger = Ledger::open(Path::new(&env.research.ledger_path));
    let series = read_series(Path::new(&env.research.strategies_dir))?;
    let n = ledger.trials()?;
    let mut s = format!(
        "{} strategy series, {n} counted trials in the ledger (hypothesis cards count in N but have no PnL series, which is conservative)\n",
        series.len()
    );
    for own in &series {
        let d = dsr_pbo(own, &series, n, env.validation);
        s.push_str(&format!(
            "{}: daily Sharpe {:.4}, expected max Sharpe {:.4}, DSR {}, PBO {}\n",
            own.key,
            d.sharpe_daily,
            d.expected_max_sharpe,
            d.dsr.map_or("n/a".into(), |x| format!("{x:.4}")),
            d.pbo.map_or("n/a".into(), |x| format!("{x:.4}")),
        ));
        for note in &d.notes {
            s.push_str(&format!("  note: {note}\n"));
        }
    }
    Ok(s)
}

/// Compares two JSON values, floats within a relative tolerance, nulls equal to nulls.
pub fn json_close(
    a: &serde_json::Value,
    b: &serde_json::Value,
    tol: f64,
    path: &str,
    diffs: &mut Vec<String>,
) {
    use serde_json::Value::*;
    match (a, b) {
        (Number(x), Number(y)) => {
            let (x, y) = (
                x.as_f64().unwrap_or(f64::NAN),
                y.as_f64().unwrap_or(f64::NAN),
            );
            if (x - y).abs() > tol * (1.0 + x.abs().max(y.abs())) {
                diffs.push(format!("{path}: {x} versus {y}"));
            }
        }
        (Array(x), Array(y)) => {
            if x.len() != y.len() {
                diffs.push(format!("{path}: length {} versus {}", x.len(), y.len()));
            } else {
                for (i, (u, v)) in x.iter().zip(y).enumerate() {
                    json_close(u, v, tol, &format!("{path}[{i}]"), diffs);
                }
            }
        }
        (Object(x), Object(y)) => {
            for (k, u) in x {
                match y.get(k) {
                    Some(v) => json_close(u, v, tol, &format!("{path}.{k}"), diffs),
                    None => diffs.push(format!("{path}.{k}: missing")),
                }
            }
            // keys that only the fresh value has were added to the output after the run was
            // recorded; every recorded number must still match
        }
        _ => {
            if a != b {
                diffs.push(format!("{path}: {a} versus {b}"));
            }
        }
    }
}

#[derive(Debug, Serialize)]
pub struct StrategyReplay {
    pub card_id: String,
    pub width: u32,
    pub differences: Vec<String>,
    pub matches: bool,
}

/// Recomputes a recorded strategy run on the discovery view and compares it with the tracked
/// summary. The whole chain is deterministic, so a difference means the data, the code or a
/// configuration file changed.
pub fn replay_strategy(
    env: &Env,
    card_path: &Path,
    width: u32,
    fingerprint: &str,
    samples: &[Sample],
) -> Result<StrategyReplay, ResearchError> {
    const TOL: f64 = 1e-9;
    let ledger = Ledger::open(Path::new(&env.research.ledger_path));
    let (card, _) = registered_strategy(&ledger, card_path, env.labels)?;
    let entry = ledger
        .read_all()?
        .into_iter()
        .find(|e| matches!(e, Entry::Strategy { card_id, width: w, .. } if *card_id == card.id && *w == width))
        .ok_or_else(|| {
            ResearchError::Strategy(format!("{} on {width}m has no run in the ledger", card.id))
        })?;
    let Entry::Strategy {
        config_sha256,
        summary_sha256,
        data_fingerprint,
        ..
    } = entry
    else {
        return Err(ResearchError::Strategy("unexpected ledger entry".into()));
    };
    let mut diffs = Vec::new();
    let path = summary_path(env, &card.id, width);
    let text = fs::read_to_string(&path).map_err(io_err(&path))?;
    if sha_hex(text.as_bytes()) != summary_sha256 {
        diffs.push("the summary file differs from the ledger's hash".into());
    }
    if let Some(f) = data_fingerprint
        && f != fingerprint
    {
        diffs.push("the cleaned data differ from the recorded fingerprint".into());
    }
    let now_hashes = config_hashes(env)?;
    for (k, h) in &config_sha256 {
        if now_hashes.get(k) != Some(h) {
            diffs.push(format!("{k} changed since the run"));
        }
    }
    let view = DiscoveryView::select(samples, &Bounds::from_config(env.research));
    let result = evaluate(
        &card.id,
        card.model.family(),
        width,
        &card.feature_indices(feature_names()),
        view.samples(),
        bars_for(env, width)?,
        env.one,
        &discovery_geometry(env, width),
        &configs(env, &card),
    )?;
    let stored: serde_json::Value = serde_json::from_str(&text)?;
    let fresh = serde_json::to_value(&result)?;
    json_close(
        stored.get("result").unwrap_or(&serde_json::Value::Null),
        &fresh,
        TOL,
        "result",
        &mut diffs,
    );
    Ok(StrategyReplay {
        card_id: card.id,
        width,
        matches: diffs.is_empty(),
        differences: diffs,
    })
}

/// File level checks of the strategy entries for `verify`: the summary exists and matches its
/// recorded hash, the recorded configuration hashes still match the files, and the DSR the
/// ledger recorded follows from the series of the earlier trials.
pub fn verify_strategy_files(
    research: &ResearchConfig,
    validation: &ValidationConfig,
    config_dir: &Path,
) -> Result<Vec<String>, ResearchError> {
    let ledger = Ledger::open(Path::new(&research.ledger_path));
    let entries = ledger.read_all()?;
    let mut problems = Vec::new();
    let current = manifest::config_hashes(config_dir, &CONFIG_FILES)?;
    let dir = Path::new(&research.strategies_dir);
    let mut earlier: Vec<String> = Vec::new();
    for e in &entries {
        let Entry::Strategy {
            card_id,
            width,
            n_trials,
            config_sha256,
            summary_sha256,
            dsr,
            pbo,
            verdict,
            ..
        } = e
        else {
            continue;
        };
        let key = format!("{card_id}_{width}m");
        let path = dir.join(format!("{key}.json"));
        match fs::read(&path) {
            Err(_) => problems.push(format!("summary {} is missing", path.display())),
            Ok(b) if sha_hex(&b) != *summary_sha256 => {
                problems.push(format!("summary {key} differs from the recorded hash"))
            }
            Ok(_) => {}
        }
        for (k, h) in config_sha256 {
            if current.get(k) != Some(h) {
                problems.push(format!(
                    "{k} changed since {key} ran; a changed configuration is a new study"
                ));
            }
        }
        // the point in time DSR and PBO follow from the series that existed before this run
        let all = read_series(dir)?;
        if let Some(own) = all.iter().find(|s| s.key == key) {
            let before: Vec<Series> = all
                .iter()
                .filter(|s| earlier.contains(&s.key))
                .cloned()
                .collect();
            let d = dsr_pbo(own, &before, *n_trials, validation);
            let close = |a: Option<f64>, b: Option<f64>| match (a, b) {
                (Some(x), Some(y)) => (x - y).abs() < 1e-9,
                (None, None) => true,
                _ => false,
            };
            if !close(d.dsr, *dsr) || !close(d.pbo, *pbo) {
                problems.push(format!(
                    "the DSR or PBO recorded for {key} does not follow from its summary"
                ));
            }
            if *verdict == Verdict::Promote && !(d.dsr.is_some_and(|x| x >= validation.dsr_min)) {
                problems.push(format!("{key} is PROMOTE with a DSR below the limit"));
            }
        }
        earlier.push(key);
    }
    Ok(problems)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cycle::{register, status, verify};
    use crate::strategy::testutil::{Market, market};

    const CONFIG: &str = "../../config";

    struct World {
        research: ResearchConfig,
        strategy: StrategyConfig,
        validation: ValidationConfig,
        backtest: BacktestConfig,
        labels: LabelsConfig,
        m: Market,
        dir: PathBuf,
    }

    fn world(name: &str, signal: bool) -> World {
        let dir = std::env::temp_dir().join(format!("vwap_strat_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("cards")).unwrap();
        let cfg = Path::new(CONFIG);
        let mut research = vwap_core::load_research_config(cfg).unwrap();
        research.ledger_path = dir.join("ledger.jsonl").display().to_string();
        research.cards_dir = dir.join("cards").display().to_string();
        research.report_dir = dir.join("reports").display().to_string();
        research.strategies_dir = dir.join("strategies").display().to_string();
        let mut strategy = vwap_core::load_strategy_config(cfg).unwrap();
        strategy.tau_bootstrap_draws = 100;
        let mut validation = vwap_core::load_validation_config(cfg).unwrap();
        validation.bootstrap_draws = 300;
        let backtest = vwap_core::load_backtest_config(cfg).unwrap();
        let labels = vwap_core::load_labels_config(cfg).unwrap();
        let start = ResearchConfig::date_us("2016-11-15");
        let end = ResearchConfig::date_us("2025-10-01");
        let m = market(6000, signal, 11, start, end);
        World {
            research,
            strategy,
            validation,
            backtest,
            labels,
            m,
            dir,
        }
    }

    fn card_text(id: &str, model: &str, features: &[&str]) -> String {
        let list = features
            .iter()
            .map(|f| format!("\"{f}\""))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "id = \"{id}\"\nkind = \"strategy\"\nmechanism = \"{model} model on a feature subset, trade rejection above a tuned threshold\"\nmodel = \"{model}\"\ntimeframes = [15]\nfeatures = [{list}]\n"
        )
    }

    fn with_env<T>(w: &World, f: impl FnOnce(&Env) -> T) -> T {
        let mut coarse = BTreeMap::new();
        coarse.insert(15u32, w.m.bars.clone());
        let env = Env {
            research: &w.research,
            strategy: &w.strategy,
            validation: &w.validation,
            backtest: &w.backtest,
            labels: &w.labels,
            config_dir: Path::new(CONFIG),
            coarse: &coarse,
            one: &w.m.one,
        };
        f(&env)
    }

    fn add_card(w: &World, id: &str, model: &str, features: &[&str]) -> PathBuf {
        let p = w.dir.join("cards").join(format!("{id}.toml"));
        fs::write(&p, card_text(id, model, features)).unwrap();
        register(&w.research, &p, "abc123").unwrap();
        p
    }

    fn names() -> Vec<&'static str> {
        feature_names().to_vec()
    }

    #[test]
    fn a_planted_strategy_is_promoted_confirmed_and_held_out_once_and_noise_is_dropped() {
        let w = world("planted", true);
        let n = names();
        // three trials that cannot see the signal (feature 0), then the one that can
        let c1 = add_card(&w, "S001", "majority", &[]);
        let c2 = add_card(&w, "S002", "logistic", &[n[1], n[2], n[3]]);
        let c3 = add_card(&w, "S003", "logistic", &[n[4], n[5], n[6]]);
        let c4 = add_card(&w, "S004", "logistic", &[]);
        let fp = "fp";
        let samples = &w.m.samples;
        with_env(&w, |env| {
            for c in [&c1, &c2, &c3] {
                let r = execute_strategy(env, c, "abc123", fp, samples).unwrap();
                assert_eq!(r[0].verdict, Verdict::Drop);
            }
            let first = execute_strategy(env, &c1, "abc123", fp, samples);
            assert!(first.is_err(), "a strategy run is not repeated");
            let r = execute_strategy(env, &c4, "abc123", fp, samples).unwrap();
            let run = &r[0];
            eprintln!("{:?} {:?}\n{:?}", run.verdict, run.failed, run.dsr_pbo);
            assert_eq!(run.verdict, Verdict::Promote, "{:?}", run.failed);
            assert!(run.dsr_pbo.pbo.unwrap() <= 0.2);

            // the ledger counts the four trials, no alpha was spent
            let s = status(&w.research).unwrap();
            assert!(s.contains("trials 4"), "{s}");
            assert!(s.contains("alpha spent 0.0000"));

            // verify is clean, including the summary files and the DSR bookkeeping
            let v = verify(&w.research).unwrap();
            assert!(v.problems.is_empty(), "{:?}", v.problems);
            let fp_problems =
                verify_strategy_files(&w.research, &w.validation, Path::new(CONFIG)).unwrap();
            assert!(fp_problems.is_empty(), "{fp_problems:?}");

            // the DSR report lists every series
            let rep = dsr_report(env).unwrap();
            assert!(
                rep.contains("S004_15m") && rep.contains("S001_15m"),
                "{rep}"
            );

            // a promoted strategy is replayed exactly; a dropped one too
            let rp = replay_strategy(env, &c4, 15, fp, samples).unwrap();
            assert!(rp.matches, "{:?}", rp.differences);

            // confirmation: only the promoted one, once
            assert!(execute_strategy_confirm(env, &c2, 15, "abc123", samples).is_err());
            let conf = execute_strategy_confirm(env, &c4, 15, "abc123", samples).unwrap();
            assert_eq!(conf.verdict, Verdict::Confirmed, "{:?}", conf.failed);
            assert!(execute_strategy_confirm(env, &c4, 15, "abc123", samples).is_err());

            // the holdout runs once and then the lock refuses
            let h = execute_strategy_holdout(env, "S004", 15, "frozen", "abc123", samples).unwrap();
            assert_eq!(h.verdict, Verdict::Confirmed, "{:?}", h.failed);
            let again = execute_strategy_holdout(env, "S004", 15, "frozen", "abc123", samples);
            assert!(matches!(again, Err(ResearchError::HoldoutLocked(_))));

            let v = verify(&w.research).unwrap();
            assert!(v.problems.is_empty(), "{:?}", v.problems);

            // the summary names every strategy, the promotion, the confirmation and the holdout
            let md = crate::strategy::summary::build(&w.research).unwrap();
            assert!(md.contains("| S004 | 15m |"), "{md}");
            assert!(
                md.contains("PROMOTE") && md.contains("Gates that failed"),
                "{md}"
            );
            assert!(
                md.contains("Confirmation of S004 on 15m: CONFIRMED"),
                "{md}"
            );
            assert!(md.contains("Holdout of S004 on 15m: CONFIRMED"), "{md}");
            assert!(!md.contains('\u{2014}') && !md.contains('\u{2013}'));
            // the tracked summary feeds the plots
            let text =
                fs::read_to_string(Path::new(&w.research.strategies_dir).join("S004_15m.json"))
                    .unwrap();
            let pi = crate::strategy::summary::PlotInputs::from_summary(
                &serde_json::from_str(&text).unwrap(),
            );
            assert_eq!(pi.reliability.len(), 3);
            assert!(!pi.daily_pnl.is_empty() && !pi.shap_phi.is_empty());
            assert_eq!(pi.feature_names.len(), feature_names().len());
        });
    }

    #[test]
    fn noise_is_never_promoted_and_cannot_be_confirmed() {
        let w = world("noise", false);
        let n = names();
        let cards = [
            add_card(&w, "S001", "majority", &[]),
            add_card(&w, "S002", "logistic", &[n[1], n[2], n[3]]),
            add_card(&w, "S003", "logistic", &[n[4], n[5], n[6]]),
            add_card(&w, "S004", "logistic", &[]),
        ];
        with_env(&w, |env| {
            for c in &cards {
                let r = execute_strategy(env, c, "abc123", "fp", &w.m.samples).unwrap();
                assert_eq!(r[0].verdict, Verdict::Drop);
            }
            let err = execute_strategy_confirm(env, &cards[3], 15, "abc123", &w.m.samples);
            assert!(matches!(err, Err(ResearchError::ConfirmNotAllowed(_))));
            let err = execute_strategy_holdout(env, "S004", 15, "x", "abc123", &w.m.samples);
            assert!(matches!(err, Err(ResearchError::HoldoutLocked(_))));
            assert!(
                !Path::new(&w.research.ledger_path)
                    .parent()
                    .unwrap()
                    .join("holdout.lock")
                    .exists()
            );
        });
    }

    #[test]
    fn an_unregistered_or_changed_card_does_not_run() {
        let w = world("registration", true);
        let p = w.dir.join("cards").join("S001.toml");
        fs::write(&p, card_text("S001", "majority", &[])).unwrap();
        with_env(&w, |env| {
            let r = execute_strategy(env, &p, "c", "fp", &w.m.samples);
            assert!(matches!(r, Err(ResearchError::NotRegistered { .. })));
            register(&w.research, &p, "c").unwrap();
            fs::write(&p, card_text("S001", "logistic", &[])).unwrap();
            let r = execute_strategy(env, &p, "c", "fp", &w.m.samples);
            assert!(matches!(r, Err(ResearchError::CardChanged { .. })));
        });
    }

    #[test]
    fn json_close_reports_the_path_of_a_difference() {
        let a: serde_json::Value = serde_json::json!({"x": [1.0, 2.0], "y": null});
        let b: serde_json::Value = serde_json::json!({"x": [1.0, 2.5], "y": null});
        let mut d = Vec::new();
        json_close(&a, &b, 1e-9, "r", &mut d);
        assert_eq!(d.len(), 1);
        assert!(d[0].starts_with("r.x[1]"));
        d.clear();
        json_close(&a, &a, 1e-9, "r", &mut d);
        assert!(d.is_empty());
    }
}
