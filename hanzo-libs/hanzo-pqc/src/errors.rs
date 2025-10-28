//! Error types for PQC operations

use thiserror::Error;

#[derive(Error, Debug)]
pub enum PqcError {
    #[error("KEM operation failed: {0}")]
    KemError(String),
    
    #[error("Signature operation failed: {0}")]
    SignatureError(String),
    
    #[error("KDF operation failed: {0}")]
    KdfError(String),
    
    #[error("Unsupported algorithm: {0}")]
    UnsupportedAlgorithm(String),
    
    #[error("Attestation failed: {0}")]
    AttestationError(String),
    
    #[error("Wire protocol error: {0}")]
    WireProtocolError(String),
    
    #[error("RNG error: {0}")]
    RngError(String),
    
    #[error("Invalid key size: expected {expected}, got {actual}")]
    InvalidKeySize { expected: usize, actual: usize },
    
    #[error("Policy violation: {0}")]
    PolicyViolation(String),
    
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    
    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),
    
    #[error("Other error: {0}")]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, PqcError>;