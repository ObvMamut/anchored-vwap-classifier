//! Strategy cards, the walk forward evaluation, the gates and the DSR and PBO bookkeeping.
//! See `docs/PROTOCOL.md`, section on strategy cards.

pub mod card;
pub mod eval;
pub mod exec;
pub mod gates;

pub use card::{ModelKind, StrategyCard, card_file_hash, is_strategy_text, load_strategy_card};
pub use eval::{Configs, FoldReport, Geometry, Pooled, StrategyResult, evaluate};
pub use exec::{
    Env, SegmentRun, StrategyReplay, StrategyRun, dsr_report, execute_strategy,
    execute_strategy_confirm, execute_strategy_holdout, replay_strategy, verify_strategy_files,
};
pub use gates::{DsrPbo, Series, decide, dsr_pbo, own_failures, read_series};

#[cfg(test)]
pub mod testutil;
