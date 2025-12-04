//! Error types for hanzo-ai-format

use thiserror::Error;

/// Errors that can occur when working with AI artifacts
#[derive(Error, Debug)]
pub enum AiFormatError {
    #[error("Invalid artifact format: {0}")]
    InvalidFormat(String),

    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Invalid magic bytes")]
    InvalidMagic,

    #[error("Unsupported format version: {0}")]
    UnsupportedVersion(u32),

    #[error("Checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("HuggingFace API error: {0}")]
    HuggingFace(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("Peer not found: {0}")]
    PeerNotFound(String),

    #[error("Artifact not found: {0}")]
    ArtifactNotFound(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Zip error: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Invalid URL: {0}")]
    InvalidUrl(#[from] url::ParseError),

    #[error("Timeout: {0}")]
    Timeout(String),

    #[error("Rate limited: retry after {retry_after} seconds")]
    RateLimited { retry_after: u64 },

    #[error("Dependency not found: {0}")]
    DependencyNotFound(String),

    #[error("Circular dependency detected: {0}")]
    CircularDependency(String),

    #[error("License violation: {0}")]
    LicenseViolation(String),

    #[error("Compute requirements not met: {0}")]
    ComputeRequirementsNotMet(String),

    #[error("Signature verification failed: {0}")]
    SignatureVerification(String),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, AiFormatError>;

impl AiFormatError {
    pub fn invalid_format(msg: impl Into<String>) -> Self {
        Self::InvalidFormat(msg.into())
    }

    pub fn missing_field(field: impl Into<String>) -> Self {
        Self::MissingField(field.into())
    }

    pub fn storage(msg: impl Into<String>) -> Self {
        Self::Storage(msg.into())
    }

    pub fn artifact_not_found(id: impl Into<String>) -> Self {
        Self::ArtifactNotFound(id.into())
    }

    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }
}
