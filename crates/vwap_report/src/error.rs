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
    #[error("no usable font among {0}; set font_paths in config/events.toml")]
    NoFont(String),
    #[error("no eligible sessions to draw")]
    NoSessions,
}
