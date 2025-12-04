//! Error types for the hanzo-compute crate

use thiserror::Error;

/// Errors that can occur in the compute swarm
#[derive(Error, Debug)]
pub enum ComputeError {
    /// Peer not found in the swarm
    #[error("Peer not found: {0}")]
    PeerNotFound(String),

    /// Task not found
    #[error("Task not found: {0}")]
    TaskNotFound(String),

    /// Piece not found
    #[error("Piece not found: task={task_id}, piece={piece_index}")]
    PieceNotFound { task_id: String, piece_index: usize },

    /// No available peers for the task
    #[error("No available peers for task: {0}")]
    NoPeersAvailable(String),

    /// Peer rejected the task
    #[error("Peer {peer_id} rejected task {task_id}: {reason}")]
    TaskRejected {
        peer_id: String,
        task_id: String,
        reason: String,
    },

    /// Task deadline exceeded
    #[error("Task deadline exceeded: {0}")]
    DeadlineExceeded(String),

    /// Verification failed
    #[error("Verification failed for task {task_id}: {reason}")]
    VerificationFailed { task_id: String, reason: String },

    /// Hash mismatch during verification
    #[error("Hash mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },

    /// Consensus not reached
    #[error("Consensus not reached for task {task_id}: got {actual}/{required} matching results")]
    ConsensusNotReached {
        task_id: String,
        actual: usize,
        required: usize,
    },

    /// Insufficient reputation
    #[error("Insufficient reputation: peer {peer_id} has {current}, needs {required}")]
    InsufficientReputation {
        peer_id: String,
        current: f64,
        required: f64,
    },

    /// Capacity exceeded
    #[error("Peer {peer_id} at capacity: {current}/{max} concurrent tasks")]
    CapacityExceeded {
        peer_id: String,
        current: usize,
        max: usize,
    },

    /// Network error
    #[error("Network error: {0}")]
    NetworkError(String),

    /// Serialization error
    #[error("Serialization error: {0}")]
    SerializationError(String),

    /// Internal error
    #[error("Internal error: {0}")]
    InternalError(String),

    /// Task already exists
    #[error("Task already exists: {0}")]
    TaskAlreadyExists(String),

    /// Peer already exists
    #[error("Peer already exists: {0}")]
    PeerAlreadyExists(String),

    /// Invalid task configuration
    #[error("Invalid task configuration: {0}")]
    InvalidTaskConfig(String),

    /// Timeout waiting for result
    #[error("Timeout waiting for task {0}")]
    Timeout(String),

    /// Channel closed
    #[error("Channel closed")]
    ChannelClosed,
}

impl From<serde_json::Error> for ComputeError {
    fn from(err: serde_json::Error) -> Self {
        ComputeError::SerializationError(err.to_string())
    }
}

/// Result type alias for compute operations
pub type ComputeResult<T> = Result<T, ComputeError>;
