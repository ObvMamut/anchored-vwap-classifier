use thiserror::Error;
use vwap_core::CoreError;

#[derive(Debug, Error)]
pub enum DataError {
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("polars error: {0}")]
    Polars(#[from] polars::error::PolarsError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("schema error: {0}")]
    Schema(String),
    #[error("data validation failed: {0}")]
    Validation(String),
}
