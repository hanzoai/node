//! DID Error types

use thiserror::Error;

#[derive(Error, Debug)]
pub enum DIDError {
    #[error("Invalid DID format: {0}")]
    InvalidFormat(String),

    #[error("Unknown chain: {0}")]
    UnknownChain(String),

    #[error("DID resolution failed: {0}")]
    ResolutionFailed(String),

    #[error("DID document validation failed: {0}")]
    ValidationFailed(String),

    #[error("Verification failed: {0}")]
    VerificationFailed(String),

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, DIDError>;