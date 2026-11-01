//! The `vwap` binary: one subcommand per pipeline stage.

mod calendar;
mod ctx;
mod ops;
mod report;
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
    /// The frozen calendar snapshot for the K1 to K3 features.
    Calendar {
        #[command(subcommand)]
        stage: CalendarStage,
    },
    /// Export the joined event, label and feature tables.
    Features {
        #[command(subcommand)]
        stage: FeaturesStage,
    },
    /// The Markdown summary and the plots of the research record.
    Report {
        #[command(subcommand)]
        stage: ReportStage,
    },
    /// Render the random day charts for the visual check.
    Spotcheck { config: PathBuf },
    /// The closed research loop.
    Research {
        #[command(subcommand)]
        stage: ResearchStage,
    },
    /// The single, locked run on the holdout segment.
    Holdout {
        #[command(subcommand)]
        stage: HoldoutStage,
    },
    /// Preflight checks: configuration, data checksum, toolchain, disk, git.
    Doctor { config: PathBuf },
    /// Run the deterministic stages in order (not the alpha spending research runs).
    Pipeline { config: PathBuf },
}

#[derive(Subcommand)]
enum HoldoutStage {
    /// Needs research/FREEZE.toml and a confirmed card; runs once.
    Run { config: PathBuf },
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
enum CalendarStage {
    /// Download the official schedules, validate them and write the snapshot (needs internet).
    Fetch { config: PathBuf },
    /// Freeze a hand supplied CSV of `kind,date` rows with the same validation.
    Import { csv: PathBuf, config: PathBuf },
}

#[derive(Subcommand)]
enum FeaturesStage {
    /// Write features_<w>m.parquet for every timeframe.
    Build { config: PathBuf },
}

#[derive(Subcommand)]
enum ReportStage {
    /// Write reports/summary.md and the strategy plots.
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
    /// Run a registered strategy card (a model plus the backtest) on the discovery segment.
    Strategy { id: String, config: PathBuf },
    /// DSR and PBO of every strategy trial, with the trial count of the ledger.
    Dsr { config: PathBuf },
    /// Show the ledger and the remaining budget.
    Status { config: PathBuf },
    /// Cycle 0: the omnibus predictive structure screen.
    Omnibus { config: PathBuf },
    /// Descriptive decomposition of the omnibus by outcome (spends no alpha).
    Diagnose { config: PathBuf },
    /// One shot confirmation of a promoted card on the confirmation segment.
    Confirm { id: String, config: PathBuf },
    /// Check the ledger against the protocol and the repository.
    Verify { config: PathBuf },
    /// Recompute a card (or `all`) and compare with the recorded result.
    Replay { id: String, config: PathBuf },
}

fn main() {
    if let Err(e) = run() {
        // a clear message and a non zero exit code, without a backtrace
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
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
        Command::Calendar { stage } => match stage {
            CalendarStage::Fetch { config } => calendar::fetch(&config)?,
            CalendarStage::Import { csv, config } => calendar::import(&csv, &config)?,
        },
        Command::Features {
            stage: FeaturesStage::Build { config },
        } => stages::features_build(&config)?,
        Command::Report {
            stage: ReportStage::Build { config },
        } => report::build(&config)?,
        Command::Baselines {
            stage: BaselinesStage::Run { config },
        } => stages::baselines_run(&config)?,
        Command::Spotcheck { config } => stages::spotcheck(&config)?,
        Command::Doctor { config } => ops::doctor(&config)?,
        Command::Pipeline { config } => ops::pipeline(&config)?,
        Command::Holdout {
            stage: HoldoutStage::Run { config },
        } => research::holdout(&config)?,
        Command::Research { stage } => match stage {
            ResearchStage::Register { id, config } => research::register(&id, &config)?,
            ResearchStage::Run { id, config } => research::run(&id, &config)?,
            ResearchStage::Strategy { id, config } => research::strategy(&id, &config)?,
            ResearchStage::Dsr { config } => research::dsr(&config)?,
            ResearchStage::Status { config } => research::status(&config)?,
            ResearchStage::Omnibus { config } => research::omnibus(&config)?,
            ResearchStage::Diagnose { config } => research::diagnose(&config)?,
            ResearchStage::Confirm { id, config } => research::confirm(&id, &config)?,
            ResearchStage::Verify { config } => research::verify(&config)?,
            ResearchStage::Replay { id, config } => research::replay(&id, &config)?,
        },
    };
    println!("{text}");
    Ok(())
}
