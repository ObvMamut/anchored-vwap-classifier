use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "vwap")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Label events with the triple barrier.
    Labels {
        #[command(subcommand)]
        stage: LabelsStage,
    },
    /// The closed research loop.
    Research {
        #[command(subcommand)]
        stage: ResearchStage,
    },
    /// Controls and the Phase 3 gate.
    Baselines {
        #[command(subcommand)]
        stage: BaselinesStage,
    },
    /// Render the random day charts for the visual check.
    Spotcheck { config: PathBuf },
    /// Event detection stages.
    Events {
        #[command(subcommand)]
        stage: EventsStage,
    },
    /// Data pipeline stages.
    Data {
        #[command(subcommand)]
        stage: DataStage,
    },
}

#[derive(Subcommand)]
enum ResearchStage {
    /// Register a committed hypothesis card.
    Register { id: String, config: PathBuf },
    /// Run a registered card on the discovery segment.
    Run { id: String, config: PathBuf },
    /// Show the ledger and the remaining budget.
    Status { config: PathBuf },
    /// Cycle 0: the omnibus predictive structure screen.
    Omnibus { config: PathBuf },
    /// Descriptive decomposition of the omnibus by outcome (spends no alpha).
    Diagnose { config: PathBuf },
}

fn git(args: &[&str]) -> Result<String> {
    let out = std::process::Command::new("git").args(args).output()?;
    anyhow::ensure!(out.status.success(), "git {:?} failed", args);
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The card file must be tracked and have no uncommitted changes.
fn committed_card(
    cfg: &vwap_core::ResearchConfig,
    id: &str,
) -> Result<(std::path::PathBuf, String)> {
    let path = std::path::Path::new(&cfg.cards_dir).join(format!("{id}.toml"));
    let p = path.to_string_lossy().to_string();
    git(&["ls-files", "--error-unmatch", &p])
        .map_err(|_| anyhow::anyhow!("{p} is not committed; commit the card first"))?;
    anyhow::ensure!(
        git(&["status", "--porcelain", "--", &p])?.is_empty(),
        "{p} has uncommitted changes; commit the card first"
    );
    Ok((path, git(&["rev-parse", "HEAD"])?))
}

#[derive(Subcommand)]
enum LabelsStage {
    /// Build labeled events for every timeframe.
    Build { config: PathBuf },
}

#[derive(Subcommand)]
enum BaselinesStage {
    /// Run the controls and the gate.
    Run { config: PathBuf },
}

#[derive(Subcommand)]
enum EventsStage {
    /// Build coarse bars, VWAP, ATR and touch events.
    Detect { config: PathBuf },
}

#[derive(Subcommand)]
enum DataStage {
    /// Print yearly first and last closes of the cleaned partitions.
    Levels { config: PathBuf },
    /// Clean the raw file and write yearly parquet plus reports.
    Clean { config: PathBuf },
    /// Print a first look at the raw file.
    Inspect { config: PathBuf },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Research { stage } => match stage {
            ResearchStage::Status { config } => {
                let cfg = vwap_core::load_research_config(&config)?;
                print!("{}", vwap_research::cycle::status(&cfg)?);
            }
            ResearchStage::Omnibus { config } => {
                let cfg = vwap_core::load_research_config(&config)?;
                let commit = git(&["rev-parse", "HEAD"])?;
                let data = vwap_core::load_data_config(&config)?;
                let ev = vwap_core::load_events_config(&config)?;
                let lab = vwap_core::load_labels_config(&config)?;
                let bars =
                    vwap_data::write::read_partitioned(std::path::Path::new(&data.output_dir))?;
                let samples = vwap_research::data::assemble(&data, &ev, &lab, &bars);
                let r = vwap_research::cycle::execute_omnibus(&cfg, &commit, &samples, |w| {
                    lab.horizon(w)
                })?;
                println!("{}", serde_json::to_string_pretty(&r)?);
            }
            ResearchStage::Diagnose { config } => {
                let cfg = vwap_core::load_research_config(&config)?;
                let commit = git(&["rev-parse", "HEAD"])?;
                let data = vwap_core::load_data_config(&config)?;
                let ev = vwap_core::load_events_config(&config)?;
                let lab = vwap_core::load_labels_config(&config)?;
                let bars =
                    vwap_data::write::read_partitioned(std::path::Path::new(&data.output_dir))?;
                let samples = vwap_research::data::assemble(&data, &ev, &lab, &bars);
                let tasks = [
                    vwap_research::omnibus::Task::Direction,
                    vwap_research::omnibus::Task::Chop,
                ];
                let r = vwap_research::cycle::execute_diagnostic(
                    &cfg,
                    &commit,
                    &samples,
                    |w| lab.horizon(w),
                    &tasks,
                    500,
                )?;
                println!("{}", serde_json::to_string_pretty(&r)?);
            }
            ResearchStage::Register { id, config } => {
                let cfg = vwap_core::load_research_config(&config)?;
                let (path, commit) = committed_card(&cfg, &id)?;
                let hash = vwap_research::cycle::register(&cfg, &path, &commit)?;
                println!("registered {id} sha256 {hash} at commit {commit}");
            }
            ResearchStage::Run { id, config } => {
                let cfg = vwap_core::load_research_config(&config)?;
                let (path, commit) = committed_card(&cfg, &id)?;
                let data = vwap_core::load_data_config(&config)?;
                let ev = vwap_core::load_events_config(&config)?;
                let lab = vwap_core::load_labels_config(&config)?;
                let bars =
                    vwap_data::write::read_partitioned(std::path::Path::new(&data.output_dir))?;
                let samples = vwap_research::data::assemble(&data, &ev, &lab, &bars);
                let r = vwap_research::cycle::execute(&cfg, &path, &commit, &samples)?;
                println!("{}", serde_json::to_string_pretty(&r)?);
            }
        },
        Command::Labels {
            stage: LabelsStage::Build { config },
        } => {
            let data = vwap_core::load_data_config(&config)?;
            let ev = vwap_core::load_events_config(&config)?;
            let lab = vwap_core::load_labels_config(&config)?;
            let bars = vwap_data::write::read_partitioned(std::path::Path::new(&data.output_dir))?;
            let params = vwap_events::event_params(&data, &ev);
            for &w in &ev.timeframes {
                let coarse = vwap_events::build(&bars, &data, w, ev.atr_length);
                let (events, _) = vwap_events::detect(&coarse, &params);
                let lp = vwap_labels::LabelParams {
                    m1: lab.m1,
                    m2: lab.m2,
                    horizon: lab.horizon(w),
                };
                let (labeled, drops) = vwap_labels::label_events(&events, &coarse, &bars, &lp);
                let mut counts = [0usize; 3];
                let mut by_year = std::collections::BTreeMap::new();
                for l in &labeled {
                    if let Some(x) = l.class.and_then(|c| counts.get_mut(c.index())) {
                        *x += 1;
                    }
                    let year = vwap_data::diag::utc(l.event.open_us)
                        .format("%Y")
                        .to_string()
                        .parse::<i32>()
                        .unwrap_or(0);
                    let e: &mut (usize, usize) = by_year.entry(year).or_default();
                    e.0 += 1;
                    e.1 += usize::from(l.ambiguous);
                }
                let report = vwap_labels::LabelReport {
                    width: w,
                    m1: lab.m1,
                    m2: lab.m2,
                    horizon: lab.horizon(w),
                    drops,
                    class_counts: counts,
                    ambiguous_by_year: by_year,
                };
                vwap_labels::write_labels(
                    &labeled,
                    &report,
                    std::path::Path::new(&lab.output_dir),
                )?;
                println!("{}", serde_json::to_string(&report)?);
            }
        }
        Command::Baselines {
            stage: BaselinesStage::Run { config },
        } => {
            let data = vwap_core::load_data_config(&config)?;
            let ev = vwap_core::load_events_config(&config)?;
            let lab = vwap_core::load_labels_config(&config)?;
            let base = vwap_core::load_baselines_config(&config)?;
            let res = vwap_core::load_research_config(&config)?;
            let bars = vwap_data::write::read_partitioned(std::path::Path::new(&data.output_dir))?;
            // gate 3a is computed on the discovery segment only
            let window = (
                vwap_core::ResearchConfig::date_us(&res.discovery_start),
                vwap_core::ResearchConfig::date_us(&res.confirm_start),
            );
            let r = vwap_baselines::run_baselines(&data, &ev, &lab, &base, window, &bars)?;
            println!("{}", vwap_baselines::run::render_markdown(&r));
        }
        Command::Spotcheck { config } => {
            let data = vwap_core::load_data_config(&config)?;
            let ev = vwap_core::load_events_config(&config)?;
            let val = vwap_core::load_validation_config(&config)?;
            let bars = vwap_data::write::read_partitioned(std::path::Path::new(&data.output_dir))?;
            let params = vwap_events::event_params(&data, &ev);
            let mut all = Vec::new();
            let mut chosen: Option<Vec<i32>> = None;
            for &w in &ev.timeframes {
                let coarse = vwap_events::build(&bars, &data, w, ev.atr_length);
                let (events, _) = vwap_events::detect(&coarse, &params);
                let sessions = match &chosen {
                    Some(s) => s.clone(),
                    None => {
                        let eligible =
                            vwap_report::spotcheck::eligible_sessions(&coarse, val.holdout_months);
                        let s = vwap_report::spotcheck::pick_sessions(
                            &eligible,
                            ev.spotcheck_days,
                            ev.spotcheck_seed,
                        );
                        chosen = Some(s.clone());
                        s
                    }
                };
                let entries = vwap_report::spotcheck::spotcheck(
                    &coarse,
                    &events,
                    &sessions,
                    ev.exclude_minutes,
                    &ev.font_path,
                    std::path::Path::new(&ev.spotcheck_dir),
                )?;
                all.extend(entries);
            }
            println!("{}", serde_json::to_string_pretty(&all)?);
        }
        Command::Events { stage } => match stage {
            EventsStage::Detect { config } => {
                let data = vwap_core::load_data_config(&config)?;
                let ev = vwap_core::load_events_config(&config)?;
                let bars =
                    vwap_data::write::read_partitioned(std::path::Path::new(&data.output_dir))?;
                let s = vwap_events::run_events(&data, &ev, &bars)?;
                println!("{}", serde_json::to_string_pretty(&s)?);
            }
        },
        Command::Data { stage } => match stage {
            DataStage::Levels { config } => {
                let cfg = vwap_core::config::load_data_config(&config)?;
                let b = vwap_data::write::read_partitioned(std::path::Path::new(&cfg.output_dir))?;
                let mut last_year = i32::MIN;
                for (i, t) in b.ts_us.iter().enumerate() {
                    let y = vwap_data::diag::utc(*t)
                        .format("%Y")
                        .to_string()
                        .parse::<i32>()
                        .unwrap_or(0);
                    if y != last_year {
                        println!("{y} first close {}", b.close.get(i).copied().unwrap_or(0.0));
                        last_year = y;
                    }
                }
                println!("rows {} last close {:?}", b.len(), b.close.last());
            }
            DataStage::Clean { config } => {
                let cfg = vwap_core::config::load_data_config(&config)?;
                let s = vwap_data::run_clean(&cfg)?;
                println!("{}", serde_json::to_string_pretty(&s)?);
            }
            DataStage::Inspect { config } => {
                let cfg = vwap_core::config::load_data_config(&config)?;
                let raw = vwap_data::read_raw(std::path::Path::new(&cfg.input_path))?;
                let d = vwap_data::diag::raw_diagnostics(&raw);
                println!("{}", serde_json::to_string_pretty(&d)?);
            }
        },
    }
    Ok(())
}
