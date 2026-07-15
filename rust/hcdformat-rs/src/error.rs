//! Error type for the crate.
use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("XML error: {0}")]
    Xml(String),

    #[error("JSON error: {0}")]
    Json(String),

    #[error("schema sha mismatch: embedded bytes hash to {got}, expected pinned {expected}")]
    SchemaShaMismatch { expected: &'static str, got: String },

    #[error("unsupported HCDF version {found:?}: this reader supports {supported} (same-MAJOR minors load)")]
    UnsupportedVersion { found: String, supported: String },
}
