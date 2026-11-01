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
    data: vwap_core::DataConfig,
    events: vwap_core::EventsConfig,
    features: vwap_core::FeaturesConfig,
    calendar: vwap_features::CalendarTable,
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
        data,
        events: ev,
        features: feat,
        calendar,
    })
}

/// The labels and samples a strategy card runs on: `labels.toml` as it is, or with the barrier
/// shape of the card (then the samples are built again for that shape).
struct Cell {
    labels: vwap_core::LabelsConfig,
    own: Option<Vec<vwap_research::Sample>>,
}

impl World {
    fn env<'a>(&'a self, config: &'a Path) -> Env<'a> {
        self.env_with(config, &self.labels)
    }

    fn env_with<'a>(&'a self, config: &'a Path, labels: &'a vwap_core::LabelsConfig) -> Env<'a> {
        Env {
            research: &self.research,
            strategy: &self.strategy,
            validation: &self.validation,
            backtest: &self.backtest,
            labels,
            config_dir: config,
            coarse: &self.coarse,
            one: &self.clean.bars,
        }
    }

    fn samples_for(&self, labels: &vwap_core::LabelsConfig) -> Vec<vwap_research::Sample> {
        vwap_dataset::assemble(
            &self.data,
            &self.events,
            labels,
            &self.features,
            &self.calendar,
            &self.clean.bars,
        )
    }

    /// The cell of the strategy card in `path`.
    fn cell(&self, path: &Path) -> Result<Cell> {
        let (card, _) = vwap_research::strategy::load_strategy_card(path)?;
        let labels = card.label_config(&self.labels);
        let own = card.has_label_override().then(|| self.samples_for(&labels));
        Ok(Cell { labels, own })
    }
}

impl Cell {
    fn samples<'a>(&'a self, w: &'a World) -> &'a [vwap_research::Sample] {
        self.own.as_deref().unwrap_or(&w.samples)
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
    let cell = w.cell(&path)?;
    let runs = vwap_research::strategy::execute_strategy(
        &w.env_with(config, &cell.labels),
        &path,
        &commit,
        &w.clean.fingerprint,
        cell.samples(&w),
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

/// What taking every touch toward rejection would have earned on the walk forward test windows
/// of the discovery segment. Descriptive: no model, no ledger entry, no alpha.
pub fn benchmark(config: &Path, grid: bool) -> Result<String> {
    if grid {
        return benchmark_grid(config);
    }
    let cfg = vwap_core::load_research_config(config)?;
    guard_ledger(&cfg)?;
    let w = world(config)?;
    let res = vwap_research::strategy::run_benchmark(&w.env(config), &w.samples)?;
    let dir =
        std::path::Path::new(&vwap_core::load_report_config(config)?.output_dir).to_path_buf();
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("benchmark.json"),
        serde_json::to_string_pretty(&res)?,
    )?;
    let f = |x: f64, d: usize| {
        if x.is_finite() {
            format!("{x:.d$}")
        } else {
            "n/a".into()
        }
    };
    let mut out = String::new();
    for b in &res {
        out.push_str(&format!(
            "{}m, round trip cost {:.1} ticks\n",
            b.width, b.round_trip_ticks
        ));
        for p in &b.results {
            out.push_str(&format!("  {}\n", p.policy));
            out.push_str("    fold  touches trades  per week  rej/resolved  win  win net  avg gross  avg net  target  breakeven  edge\n");
            for r in p.folds.iter().chain(std::iter::once(&p.pooled)) {
                let label = r.fold.map_or("all".to_string(), |k| (k + 1).to_string());
                out.push_str(&format!(
                    "    {label:>4}  {:>7} {:>6}  {:>8}  {:>12}  {:>5} {:>7}  {:>9} {:>8}  {:>7} {:>9} {:>6}\n",
                    r.touches,
                    r.trades,
                    f(r.trades_per_week, 1),
                    f(100.0 * r.rejection_share_resolved, 1),
                    f(100.0 * r.win_rate, 1),
                    f(100.0 * r.win_rate_net, 1),
                    f(r.avg_gross_ticks, 2),
                    f(r.avg_net_ticks, 2),
                    f(r.mean_target_ticks, 0),
                    f(100.0 * r.breakeven_win_rate, 1),
                    f(100.0 * r.edge, 1),
                ));
            }
            out.push_str(&format!(
                "    pooled net ticks per trade, interval [{}, {}]\n",
                f(p.net_interval.0, 2),
                f(p.net_interval.1, 2)
            ));
        }
    }
    Ok(out)
}

#[derive(serde::Serialize)]
struct GridRow {
    width: u32,
    m1: f64,
    m2: f64,
    horizon: usize,
    trades: usize,
    win_rate: f64,
    breakeven_win_rate: f64,
    edge: f64,
    avg_net_ticks: f64,
    net_interval: (f64, f64),
}

/// Every cell of the label grid, taking every touch toward rejection. Descriptive, but the cells
/// examined are counted as trials in the ledger (once), because choosing one later is a selection.
fn benchmark_grid(config: &Path) -> Result<String> {
    let cfg = vwap_core::load_research_config(config)?;
    guard_ledger(&cfg)?;
    let w = world(config)?;
    let g = w.labels.grid.clone();
    let mut rows: Vec<GridRow> = Vec::new();
    for width in [5u32, 15] {
        let horizons = if width == 5 {
            &g.horizon_5m
        } else {
            &g.horizon_15m
        };
        for &m1 in &g.m1 {
            for &m2 in &g.m2 {
                for &h in horizons {
                    let mut labels = w.labels.clone();
                    labels.m1 = m1;
                    labels.m2 = m2;
                    if width == 5 {
                        labels.horizon_5m = h;
                    } else {
                        labels.horizon_15m = h;
                    }
                    let samples = w.samples_for(&labels);
                    let res = vwap_research::strategy::run_benchmark(
                        &w.env_with(config, &labels),
                        &samples,
                    )?;
                    let Some(p) = res
                        .iter()
                        .find(|b| b.width == width)
                        .and_then(|b| b.results.first())
                    else {
                        continue;
                    };
                    rows.push(GridRow {
                        width,
                        m1,
                        m2,
                        horizon: h,
                        trades: p.pooled.trades,
                        win_rate: p.pooled.win_rate,
                        breakeven_win_rate: p.pooled.breakeven_win_rate,
                        edge: p.pooled.edge,
                        avg_net_ticks: p.pooled.avg_net_ticks,
                        net_interval: p.net_interval,
                    });
                }
            }
        }
    }
    let dir = std::path::PathBuf::from(&vwap_core::load_report_config(config)?.output_dir);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("benchmark_grid.json"),
        serde_json::to_string_pretty(&rows)?,
    )?;
    let ledger = vwap_research::Ledger::open(Path::new(&cfg.ledger_path));
    let positive = rows.iter().filter(|r| r.edge > 0.0).count();
    let counted = ledger.record_search(
        "label_grid_benchmark",
        rows.len(),
        &format!(
            "take every touch over {} label grid cells: {positive} with a positive win rate edge",
            rows.len()
        ),
        &head(),
    )?;
    let mut sorted: Vec<&GridRow> = rows.iter().collect();
    sorted.sort_by(|a, b| b.edge.total_cmp(&a.edge));
    let mut out = format!(
        "{} cells, {positive} with a positive edge ({} trials {})\n  tf   m1    m2  horizon  trades   win  break even   edge  net ticks (interval)\n",
        rows.len(),
        rows.len(),
        if counted {
            "added to the ledger"
        } else {
            "already counted"
        }
    );
    for r in sorted.iter().take(12) {
        out.push_str(&format!(
            "  {:>2}m {:>4} {:>5} {:>7} {:>7} {:>5.1} {:>10.1} {:>6.1}  {:.2} ({:.2} to {:.2})\n",
            r.width,
            r.m1,
            r.m2,
            r.horizon,
            r.trades,
            100.0 * r.win_rate,
            100.0 * r.breakeven_win_rate,
            100.0 * r.edge,
            r.avg_net_ticks,
            r.net_interval.0,
            r.net_interval.1
        ));
    }
    out.push_str(
        "(the 12 cells with the highest edge; all cells are in reports/benchmark_grid.json)\n",
    );
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
        let cell = w.cell(&path)?;
        let env = w.env_with(config, &cell.labels);
        let mut out = String::new();
        for width in widths {
            let r = vwap_research::strategy::execute_strategy_confirm(
                &env,
                &path,
                width,
                &commit,
                cell.samples(&w),
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
        let card_path = Path::new(&cfg.cards_dir).join(format!("{}.toml", f.card_id));
        let cell = w.cell(&card_path)?;
        let r = vwap_research::strategy::execute_strategy_holdout(
            &w.env_with(config, &cell.labels),
            &f.card_id,
            width,
            &f.note,
            &head(),
            cell.samples(&w),
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
            let cell = w.cell(&path)?;
            let env = w.env_with(config, &cell.labels);
            for width in widths {
                let r = vwap_research::strategy::replay_strategy(
                    &env,
                    &path,
                    width,
                    &w.clean.fingerprint,
                    cell.samples(&w),
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
