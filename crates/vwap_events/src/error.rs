use thiserror::Error;

#[derive(Debug, Error)]
pub enum EventsError {
    #[error(transparent)]
    Core(#[from] vwap_core::CoreError),
    #[error(transparent)]
    Data(#[from] vwap_data::DataError),
    #[error("polars error: {0}")]
    Polars(#[from] polars::error::PolarsError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
}
