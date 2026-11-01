use thiserror::Error;

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("model fit failed: {0}")]
    Fit(String),
    #[error("shape mismatch: {0}")]
    Shape(String),
    #[error("the model has not been fit")]
    NotFit,
}
