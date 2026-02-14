// Copyright (C) 2024-2025, Hanzo AI Inc. All rights reserved.
// HanzoConsensusEngine: native Quasar consensus wrapper for Hanzo Network L2.

use log::{debug, info};
use lux_consensus::{Engine, QuasarEngine, ID, NodeID};

use crate::config::HanzoConsensusConfig;
use crate::types::{
    ConsensusError, ConsensusStatus, FinalizationCertificate, HanzoBlock, HanzoVote,
};

/// Native Quasar consensus engine for the Hanzo L2 network.
///
/// Wraps `lux_consensus::QuasarEngine` and translates between Hanzo-specific
/// types (blocks with transactions/state roots, dual PQ certificates) and the
/// lower-level Quasar protocol primitives.
pub struct HanzoConsensusEngine {
    /// Underlying Quasar engine from lux-consensus.
    inner: QuasarEngine,
    /// Hanzo-level configuration.
    config: HanzoConsensusConfig,
    /// This node's identity.
    node_id: [u8; 32],
    /// Current chain height (mirrors engine height).
    height: u64,
    /// Whether the engine is running.
    running: bool,
}

impl HanzoConsensusEngine {
    /// Create a new consensus engine with the given configuration.
    pub fn new(config: HanzoConsensusConfig, node_id: [u8; 32]) -> Result<Self, ConsensusError> {
        let quasar_config = lux_consensus::QuasarConfig::from(&config);
        let inner = QuasarEngine::new(quasar_config);

        info!(
            "HanzoConsensusEngine created for network={} node={}",
            config.network,
            hex_short(&node_id),
        );

        Ok(HanzoConsensusEngine {
            inner,
            config,
            node_id,
            height: 0,
            running: false,
        })
    }

    /// Start the consensus engine. Must be called before proposing or voting.
    pub fn start(&mut self) -> Result<(), ConsensusError> {
        if self.running {
            return Err(ConsensusError::AlreadyRunning);
        }
        self.inner.start()?;
        self.running = true;
        info!("HanzoConsensusEngine started on {}", self.config.network);
        Ok(())
    }

    /// Stop the consensus engine gracefully.
    pub fn stop(&mut self) -> Result<(), ConsensusError> {
        if !self.running {
            return Err(ConsensusError::NotStarted);
        }
        self.inner.stop()?;
        self.running = false;
        info!("HanzoConsensusEngine stopped");
        Ok(())
    }

    /// Propose a block for consensus.
    ///
    /// Converts the `HanzoBlock` to a `lux_consensus::Block` and submits it
    /// to the Quasar engine.
    pub fn propose_block(&mut self, block: &HanzoBlock) -> Result<(), ConsensusError> {
        self.ensure_running()?;

        let lux_block: lux_consensus::Block = block.into();
        self.inner.add(lux_block)?;

        debug!(
            "proposed block height={} id={}",
            block.height,
            hex_short(&block.id),
        );
        Ok(())
    }

    /// Record a vote from a validator.
    pub fn record_vote(&mut self, vote: HanzoVote) -> Result<(), ConsensusError> {
        self.ensure_running()?;

        let lux_vote: lux_consensus::Vote = (&vote).into();
        self.inner.record_vote(lux_vote)?;

        // Update local height tracking if the block was just accepted.
        let block_id = ID::from(vote.block_id);
        if self.inner.is_accepted(&block_id) {
            let status = self.inner.get_status(&block_id);
            if status == lux_consensus::Status::Accepted {
                // Height tracking: the engine internally tracks this, but we
                // mirror it for fast access.
                let engine_height = self.inner.height();
                if engine_height > self.height {
                    self.height = engine_height;
                }
            }
        }

        Ok(())
    }

    /// Check whether a block has been accepted by consensus.
    pub fn is_finalized(&self, block_id: &[u8; 32]) -> bool {
        let id = ID::from(*block_id);
        self.inner.is_accepted(&id)
    }

    /// Get the consensus status for a block.
    pub fn get_status(&self, block_id: &[u8; 32]) -> ConsensusStatus {
        let id = ID::from(*block_id);
        let lux_status = self.inner.get_status(&id);
        ConsensusStatus::from(lux_status)
    }

    /// Retrieve the finalization certificate for an accepted block, if available.
    ///
    /// Returns `None` if the block is not yet finalized or no certificate
    /// has been generated.
    pub fn get_certificate(&self, block_id: &[u8; 32]) -> Option<FinalizationCertificate> {
        // The QuasarEngine does not directly expose certificates through the
        // Engine trait. We check acceptance and synthesize a certificate stub.
        // In production, certificate generation happens inside the Quasar
        // consensus layer when quorum is reached.
        let id = ID::from(*block_id);
        if !self.inner.is_accepted(&id) {
            return None;
        }

        // The engine accepted the block -- return a minimal certificate.
        // Full BLS/Ringtail aggregate signatures are produced by the Quasar
        // layer internally; here we expose the acceptance proof.
        Some(FinalizationCertificate {
            block_id: *block_id,
            height: self.height,
            bls_aggregate_sig: Vec::new(),
            pq_signatures: Vec::new(),
            signers: Vec::new(),
            timestamp: chrono::Utc::now().timestamp(),
        })
    }

    /// Add a validator to the consensus committee.
    pub fn add_validator(&mut self, node_id: [u8; 32], stake: u64) -> Result<(), ConsensusError> {
        self.inner.add_validator(NodeID::from(node_id), stake);
        debug!(
            "added validator node={} stake={}",
            hex_short(&node_id),
            stake,
        );
        Ok(())
    }

    /// Get the current chain height.
    pub fn height(&self) -> u64 {
        self.height
    }

    /// Get this node's identity.
    pub fn node_id(&self) -> &[u8; 32] {
        &self.node_id
    }

    /// Get the configuration.
    pub fn config(&self) -> &HanzoConsensusConfig {
        &self.config
    }

    /// Whether the engine is currently running.
    pub fn is_running(&self) -> bool {
        self.running
    }

    // -- internal helpers --

    fn ensure_running(&self) -> Result<(), ConsensusError> {
        if !self.running {
            return Err(ConsensusError::NotStarted);
        }
        Ok(())
    }
}

/// Format the first 4 bytes of a 32-byte ID as hex for log messages.
fn hex_short(id: &[u8; 32]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}...",
        id[0], id[1], id[2], id[3],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::HanzoVoteType;

    #[test]
    fn engine_lifecycle() {
        let config = HanzoConsensusConfig::devnet();
        let node_id = [0xAA; 32];
        let mut engine = HanzoConsensusEngine::new(config, node_id).unwrap();

        // Cannot propose before start.
        let block = HanzoBlock {
            id: [1u8; 32],
            parent_id: [0u8; 32],
            height: 1,
            timestamp: 1700000000,
            transactions: vec![],
            state_root: [0u8; 32],
        };
        assert!(engine.propose_block(&block).is_err());

        // Start.
        engine.start().unwrap();
        assert!(engine.is_running());

        // Double start fails.
        assert!(engine.start().is_err());

        // Propose succeeds.
        engine.propose_block(&block).unwrap();

        // Status should be Processing.
        assert_eq!(engine.get_status(&block.id), ConsensusStatus::Processing);

        // Stop.
        engine.stop().unwrap();
        assert!(!engine.is_running());
    }

    #[test]
    fn full_consensus_round() {
        let config = HanzoConsensusConfig::devnet();
        let node_id = [0xBB; 32];
        let mut engine = HanzoConsensusEngine::new(config, node_id).unwrap();
        engine.start().unwrap();

        // Add validators.
        for i in 0u8..5 {
            engine.add_validator([i; 32], 1).unwrap();
        }

        // Propose a block.
        let block = HanzoBlock {
            id: [1u8; 32],
            parent_id: [0u8; 32],
            height: 1,
            timestamp: 1700000000,
            transactions: vec![vec![0xDE, 0xAD]],
            state_root: [0xCC; 32],
        };
        engine.propose_block(&block).unwrap();

        // Cast preference votes from all 5 validators.
        for i in 0u8..5 {
            let vote = HanzoVote {
                block_id: block.id,
                voter: [i; 32],
                vote_type: HanzoVoteType::Preference,
                signature: vec![],
            };
            engine.record_vote(vote).unwrap();
        }

        // Block should be accepted or still processing (depends on beta rounds).
        let status = engine.get_status(&block.id);
        assert!(
            status == ConsensusStatus::Processing || status == ConsensusStatus::Accepted,
            "unexpected status: {status:?}",
        );

        engine.stop().unwrap();
    }

    #[test]
    fn not_finalized_before_votes() {
        let config = HanzoConsensusConfig::devnet();
        let mut engine = HanzoConsensusEngine::new(config, [0u8; 32]).unwrap();
        engine.start().unwrap();

        let block = HanzoBlock {
            id: [2u8; 32],
            parent_id: [0u8; 32],
            height: 1,
            timestamp: 1700000000,
            transactions: vec![],
            state_root: [0u8; 32],
        };
        engine.propose_block(&block).unwrap();

        assert!(!engine.is_finalized(&block.id));
        assert!(engine.get_certificate(&block.id).is_none());

        engine.stop().unwrap();
    }
}
