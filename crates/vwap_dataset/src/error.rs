use thiserror::Error;

#[derive(Debug, Error)]
pub enum DatasetError {
    #[error("polars error: {0}")]
    Polars(#[from] polars::error::PolarsError),
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("column {0} is missing or has the wrong type")]
    Column(String),
}
