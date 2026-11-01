use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("cannot read config {path}: {source}")]
    ConfigIo {
        path: String,
        source: std::io::Error,
    },
    #[error("cannot parse config {path}: {source}")]
    ConfigParse {
        path: String,
        source: toml::de::Error,
    },
    #[error("invalid config: {0}")]
    ConfigInvalid(String),
}
