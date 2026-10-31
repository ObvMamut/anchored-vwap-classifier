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
    /// Data pipeline stages.
    Data {
        #[command(subcommand)]
        stage: DataStage,
    },
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
