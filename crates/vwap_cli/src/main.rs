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
