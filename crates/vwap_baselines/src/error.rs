use thiserror::Error;

#[derive(Debug, Error)]
pub enum BaselinesError {
    #[error(transparent)]
    Core(#[from] vwap_core::CoreError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("not enough data: {0}")]
    NotEnoughData(String),
}
