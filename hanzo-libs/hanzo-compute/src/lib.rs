//! Hanzo Compute - BitTorrent-style Decentralized Compute Protocol
//!
//! This crate implements a BitTorrent-inspired protocol for distributing AI compute
//! tasks across a decentralized network of peers. Key features:
//!
//! - **Swarm Management**: Peer discovery, connection management, and reputation tracking
//! - **Piece Distribution**: Task decomposition into verifiable pieces
//! - **Rarest-First Scheduling**: Prioritize pieces that are least available in the swarm
//! - **Result Verification**: Multi-peer consensus and TEE attestation support
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                     ComputeSwarm                                │
//! │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐             │
//! │  │   Peers     │  │   Tasks     │  │  Scheduler  │             │
//! │  │  (DashMap)  │  │  (DashMap)  │  │ (rarest-1st)│             │
//! │  └─────────────┘  └─────────────┘  └─────────────┘             │
//! │         │                │                │                     │
//! │         ▼                ▼                ▼                     │
//! │  ┌─────────────────────────────────────────────┐               │
//! │  │              PieceManager                   │               │
//! │  │  - Split tasks into pieces                  │               │
//! │  │  - Track piece availability                 │               │
//! │  │  - Manage piece completion                  │               │
//! │  └─────────────────────────────────────────────┘               │
//! │                        │                                        │
//! │                        ▼                                        │
//! │  ┌─────────────────────────────────────────────┐               │
//! │  │              ResultVerifier                 │               │
//! │  │  - Hash verification                        │               │
//! │  │  - Multi-peer consensus                     │               │
//! │  │  - TEE attestation (optional)               │               │
//! │  └─────────────────────────────────────────────┘               │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Example
//!
//! ```rust,no_run
//! use hanzo_compute::{ComputeSwarm, SwarmConfig, ComputeTask, TaskType};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Create a swarm with default config
//!     let config = SwarmConfig::default();
//!     let swarm = ComputeSwarm::new(config).await?;
//!
//!     // Submit a compute task
//!     let task = ComputeTask::new(
//!         TaskType::Inference {
//!             model: "llama-3.1-8b".to_string(),
//!             prompt: "Hello, world!".to_string(),
//!             max_tokens: 100,
//!         },
//!         1.0, // reward in AI coins
//!     );
//!
//!     let task_id = swarm.submit_task(task).await?;
//!
//!     // Wait for result
//!     let result = swarm.await_result(&task_id).await?;
//!     println!("Result: {:?}", result);
//!
//!     Ok(())
//! }
//! ```

pub mod error;
pub mod peer;
pub mod piece;
pub mod scheduler;
pub mod swarm;
pub mod verifier;

// Re-export main types
pub use error::ComputeError;
pub use peer::{Peer, PeerCapabilities, PeerState, PeerId};
pub use piece::{Piece, PieceId, PieceState, PieceManager};
pub use scheduler::{Scheduler, SchedulingStrategy};
pub use swarm::{ComputeSwarm, SwarmConfig, SwarmStats};
pub use verifier::{ResultVerifier, VerificationMethod, VerificationResult};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for a compute task
pub type TaskId = String;

/// A compute task to be distributed across the swarm
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputeTask {
    /// Unique task identifier
    pub id: TaskId,
    /// Type of computation
    pub task_type: TaskType,
    /// Reward in AI coins for completing this task
    pub reward: f64,
    /// Minimum reputation score required to accept this task
    pub min_reputation: f64,
    /// Task deadline (Unix timestamp)
    pub deadline: Option<u64>,
    /// Number of pieces this task is split into
    pub num_pieces: usize,
    /// Number of redundant executions required for verification
    pub redundancy: usize,
    /// Creator's peer ID
    pub creator: PeerId,
    /// Creation timestamp
    pub created_at: u64,
    /// Input data hash (for verification)
    pub input_hash: String,
}

impl ComputeTask {
    /// Create a new compute task
    pub fn new(task_type: TaskType, reward: f64) -> Self {
        let id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp() as u64;

        Self {
            id,
            task_type,
            reward,
            min_reputation: 0.0,
            deadline: None,
            num_pieces: 1,
            redundancy: 1,
            creator: String::new(),
            created_at: now,
            input_hash: String::new(),
        }
    }

    /// Set the minimum reputation required
    pub fn with_min_reputation(mut self, min_rep: f64) -> Self {
        self.min_reputation = min_rep;
        self
    }

    /// Set the deadline
    pub fn with_deadline(mut self, deadline: u64) -> Self {
        self.deadline = Some(deadline);
        self
    }

    /// Set the number of pieces
    pub fn with_pieces(mut self, num_pieces: usize) -> Self {
        self.num_pieces = num_pieces;
        self
    }

    /// Set redundancy level for verification
    pub fn with_redundancy(mut self, redundancy: usize) -> Self {
        self.redundancy = redundancy;
        self
    }
}

/// Types of compute tasks supported
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskType {
    /// Generate embeddings for text
    Embedding {
        model: String,
        texts: Vec<String>,
        batch_size: usize,
    },
    /// Rerank documents
    Reranking {
        model: String,
        query: String,
        documents: Vec<String>,
        top_k: usize,
    },
    /// Run inference
    Inference {
        model: String,
        prompt: String,
        max_tokens: usize,
    },
    /// Train/fine-tune a model
    Training {
        model: String,
        dataset_url: String,
        epochs: usize,
    },
    /// Custom compute job
    Custom {
        /// WASM module hash
        wasm_hash: String,
        /// Input data
        input: Vec<u8>,
    },
}

/// Result of a completed compute task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputeResult {
    /// Task this result belongs to
    pub task_id: TaskId,
    /// Piece index (if task was split)
    pub piece_index: usize,
    /// Result data
    pub data: Vec<u8>,
    /// Hash of the result (for verification)
    pub result_hash: String,
    /// Peer who computed this result
    pub computed_by: PeerId,
    /// Computation time in milliseconds
    pub compute_time_ms: u64,
    /// Verification status
    pub verified: bool,
}

impl ComputeResult {
    /// Create a new compute result
    pub fn new(task_id: TaskId, piece_index: usize, data: Vec<u8>, peer_id: PeerId) -> Self {
        let result_hash = blake3::hash(&data).to_hex().to_string();

        Self {
            task_id,
            piece_index,
            data,
            result_hash,
            computed_by: peer_id,
            compute_time_ms: 0,
            verified: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_task_creation() {
        let task = ComputeTask::new(
            TaskType::Inference {
                model: "test-model".to_string(),
                prompt: "Hello".to_string(),
                max_tokens: 100,
            },
            1.0,
        );

        assert!(!task.id.is_empty());
        assert_eq!(task.reward, 1.0);
        assert_eq!(task.num_pieces, 1);
        assert_eq!(task.redundancy, 1);
    }

    #[test]
    fn test_task_builder() {
        let task = ComputeTask::new(
            TaskType::Embedding {
                model: "nomic-embed".to_string(),
                texts: vec!["test".to_string()],
                batch_size: 32,
            },
            2.0,
        )
        .with_min_reputation(50.0)
        .with_pieces(4)
        .with_redundancy(3);

        assert_eq!(task.min_reputation, 50.0);
        assert_eq!(task.num_pieces, 4);
        assert_eq!(task.redundancy, 3);
    }

    #[test]
    fn test_compute_result() {
        let result = ComputeResult::new(
            "task-123".to_string(),
            0,
            vec![1, 2, 3, 4],
            "peer-456".to_string(),
        );

        assert_eq!(result.task_id, "task-123");
        assert!(!result.result_hash.is_empty());
        assert!(!result.verified);
    }
}
