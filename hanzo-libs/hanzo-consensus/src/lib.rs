// Copyright (C) 2024-2025, Hanzo AI Inc. All rights reserved.
//
//! # hanzo-consensus
//!
//! Native Quasar consensus engine for the Hanzo Network L2.
//!
//! Replaces the legacy Cap'n Proto FFI integration with a direct Rust
//! dependency on `lux-consensus`, providing the full Quasar protocol
//! stack: Wave voting, FPC adaptive thresholds, Photon sampling, Focus
//! confidence accumulation, and post-quantum finality certificates.
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use hanzo_consensus::{HanzoConsensusEngine, HanzoConsensusConfig, HanzoBlock, HanzoVote, HanzoVoteType};
//!
//! let config = HanzoConsensusConfig::devnet();
//! let mut engine = HanzoConsensusEngine::new(config, [0xAAu8; 32]).unwrap();
//! engine.start().unwrap();
//!
//! // Add validators, propose blocks, record votes...
//! engine.stop().unwrap();
//! ```

pub mod config;
pub mod engine;
pub mod types;

// Re-export key types at crate root for ergonomic imports.
pub use config::HanzoConsensusConfig;
pub use engine::HanzoConsensusEngine;
pub use types::{
    ConsensusError, ConsensusStatus, FinalizationCertificate, HanzoBlock, HanzoVote,
    HanzoVoteType,
};
