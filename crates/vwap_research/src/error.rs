use thiserror::Error;

#[derive(Debug, Error)]
pub enum ResearchError {
    #[error(transparent)]
    Core(#[from] vwap_core::CoreError),
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("toml error in {path}: {source}")]
    Toml {
        path: String,
        source: toml::de::Error,
    },
    #[error(
        "card {id} is not registered; commit it and run `vwap research register {id} config` first"
    )]
    NotRegistered { id: String },
    #[error(
        "card {id} changed after registration (registered hash {registered}, current {current})"
    )]
    CardChanged {
        id: String,
        registered: String,
        current: String,
    },
    #[error("budget exhausted: {0}")]
    BudgetExhausted(String),
    #[error("invalid card: {0}")]
    InvalidCard(String),
    #[error("confirmation not allowed: {0}")]
    ConfirmNotAllowed(String),
    #[error("holdout is locked: {0}")]
    HoldoutLocked(String),
    #[error("harness self test failed: {0}")]
    SelfTest(String),
    #[error("not enough data: {0}")]
    NotEnoughData(String),
    #[error("strategy: {0}")]
    Strategy(String),
}
