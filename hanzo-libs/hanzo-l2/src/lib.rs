//! Hanzo L2 - Bridge and Sequencing on Lux Network
//!
//! This crate implements L2 infrastructure for the Hanzo Network running on
//! Lux consensus. It provides:
//!
//! - **Bridge**: Cross-chain message passing between L1 (Lux) and L2 (Hanzo)
//! - **Sequencer**: Transaction ordering and batch construction
//! - **Validator**: Validator set management with stake-weighted selection
//! - **Commitment**: State commitment and Merkle proof generation for L1 anchoring
//!
//! # Architecture
//!
//! ```text
//! +------------------+      +------------------+
//! |   Lux L1         | <--> |   Hanzo L2       |
//! |  (Settlement)    |      |  (Execution)     |
//! +------------------+      +------------------+
//!         ^                         |
//!         |   StateCommitment       |
//!         +--- (Merkle root) -------+
//!         |                         |
//!         |   CrossChainMessage     |
//!         +-------------------------+
//! ```

pub mod bridge;
pub mod commitment;
pub mod sequencer;
pub mod validator;

// Re-export primary types for convenience.
pub use bridge::{CrossChainMessage, L2Bridge};
pub use commitment::{CommitmentProof, StateCommitment};
pub use sequencer::{Sequencer, SequencerConfig, TransactionBatch};
pub use validator::{ValidatorInfo, ValidatorSet};

/// 32-byte transaction hash used throughout the L2 system.
pub type TxHash = [u8; 32];
