//! The `vwap` binary: one subcommand per pipeline stage.

mod ctx;
mod research;
mod stages;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "vwap", about = "NQ anchored VWAP touch study")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Data stages.
    Data {
        #[command(subcommand)]
        stage: DataStage,
    },
    /// Event detection.
    Events {
        #[command(subcommand)]
        stage: EventsStage,
    },
    /// Triple barrier labels.
    Labels {
        #[command(subcommand)]
        stage: LabelsStage,
    },
    /// Controls and the gate 3a equivalence report.
    Baselines {
        #[command(subcommand)]
        stage: BaselinesStage,
    },
    /// Render the random day charts for the visual check.
    Spotcheck { config: PathBuf },
    /// The closed research loop.
    Research {
        #[command(subcommand)]
        stage: ResearchStage,
    },
}

#[derive(Subcommand)]
enum DataStage {
    /// Clean the raw file and write yearly parquet plus reports.
    Clean { config: PathBuf },
    /// Print yearly first and last closes of the cleaned partitions.
    Levels { config: PathBuf },
    /// Print a first look at the raw file.
    Inspect { config: PathBuf },
}

#[derive(Subcommand)]
enum EventsStage {
    /// Build coarse bars, VWAP, ATR and touch events.
    Detect { config: PathBuf },
}

#[derive(Subcommand)]
enum LabelsStage {
    /// Build labeled events for every timeframe.
    Build { config: PathBuf },
}

#[derive(Subcommand)]
enum BaselinesStage {
    /// Run the controls and the equivalence verdicts.
    Run { config: PathBuf },
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
    /// Check the ledger against the protocol and the repository.
    Verify { config: PathBuf },
    /// Recompute a card (or `all`) and compare with the recorded result.
    Replay { id: String, config: PathBuf },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let text = match cli.command {
        Command::Data { stage } => match stage {
            DataStage::Clean { config } => stages::data_clean(&config)?,
            DataStage::Levels { config } => stages::data_levels(&config)?,
            DataStage::Inspect { config } => stages::data_inspect(&config)?,
        },
        Command::Events {
            stage: EventsStage::Detect { config },
        } => stages::events_detect(&config)?,
        Command::Labels {
            stage: LabelsStage::Build { config },
        } => stages::labels_build(&config)?,
        Command::Baselines {
            stage: BaselinesStage::Run { config },
        } => stages::baselines_run(&config)?,
        Command::Spotcheck { config } => stages::spotcheck(&config)?,
        Command::Research { stage } => match stage {
            ResearchStage::Register { id, config } => research::register(&id, &config)?,
            ResearchStage::Run { id, config } => research::run(&id, &config)?,
            ResearchStage::Status { config } => research::status(&config)?,
            ResearchStage::Omnibus { config } => research::omnibus(&config)?,
            ResearchStage::Diagnose { config } => research::diagnose(&config)?,
            ResearchStage::Verify { config } => research::verify(&config)?,
            ResearchStage::Replay { id, config } => research::replay(&id, &config)?,
        },
    };
    println!("{text}");
    Ok(())
}
