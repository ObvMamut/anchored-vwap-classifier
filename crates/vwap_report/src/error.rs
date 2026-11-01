use thiserror::Error;

#[derive(Debug, Error)]
pub enum ReportError {
    #[error("drawing error: {0}")]
    Draw(String),
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("no eligible sessions to draw")]
    NoSessions,
}
