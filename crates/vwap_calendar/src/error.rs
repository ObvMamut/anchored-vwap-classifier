use thiserror::Error;

#[derive(Debug, Error)]
pub enum CalendarError {
    #[error("could not fetch {url}: {reason}")]
    Fetch { url: String, reason: String },
    #[error("could not parse {url}: {reason}")]
    Parse { url: String, reason: String },
    #[error("calendar validation failed: {0}")]
    Validation(String),
    #[error("snapshot does not match its lock: {0}")]
    Lock(String),
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}
