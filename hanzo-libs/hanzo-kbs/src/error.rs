//! Error types for Hanzo Security

use thiserror::Error;

#[derive(Error, Debug)]
pub enum SecurityError {
    #[error("Key not found: {0}")]
    KeyNotFound(String),
    
    #[error("Invalid attestation: {0}")]
    InvalidAttestation(String),
    
    #[error("Policy violation: {0}")]
    PolicyViolation(String),
    
    #[error("Cryptographic error: {0}")]
    CryptoError(String),
    
    #[error("HSM error: {0}")]
    HsmError(String),
    
    #[error("KMS error: {0}")]
    KmsError(String),
    
    #[error("KBS error: {0}")]
    KbsError(String),
    
    #[error("Tier mismatch: requested {requested}, available {available}")]
    TierMismatch { requested: u8, available: u8 },
    
    #[error("Session expired")]
    SessionExpired,
    
    #[error("Rate limit exceeded")]
    RateLimitExceeded,
    
    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),
    
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    
    #[error("Other error: {0}")]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, SecurityError>;