// Copyright (C) 2024-2025, Hanzo AI Inc. All rights reserved.
// Native Quasar consensus types for Hanzo Network L2.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Hanzo block for consensus.
///
/// Wraps application-level block data (transactions, state root) and maps
/// to the lower-level `lux_consensus::Block` used by the Quasar engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HanzoBlock {
    /// BLAKE3 hash of the block header (32 bytes).
    pub id: [u8; 32],
    /// Parent block hash.
    pub parent_id: [u8; 32],
    /// Block height in the L2 chain.
    pub height: u64,
    /// Unix timestamp (seconds).
    pub timestamp: i64,
    /// Serialized transactions included in this block.
    pub transactions: Vec<Vec<u8>>,
    /// Merkle state root after applying transactions.
    pub state_root: [u8; 32],
}

impl HanzoBlock {
    /// Compute the BLAKE3 hash of the block's canonical payload.
    pub fn compute_id(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&self.parent_id);
        hasher.update(&self.height.to_le_bytes());
        hasher.update(&self.timestamp.to_le_bytes());
        hasher.update(&self.state_root);
        for tx in &self.transactions {
            hasher.update(tx);
        }
        *hasher.finalize().as_bytes()
    }
}

/// Convert a HanzoBlock into the lux_consensus::Block representation.
///
/// The payload is the serde_json serialization of transactions + state_root
/// so the Quasar engine can store it opaquely while Hanzo interprets it.
impl From<&HanzoBlock> for lux_consensus::Block {
    fn from(hb: &HanzoBlock) -> Self {
        let payload = serde_json::to_vec(&(&hb.transactions, &hb.state_root))
            .unwrap_or_default();
        lux_consensus::Block::new(
            lux_consensus::ID::from(hb.id),
            lux_consensus::ID::from(hb.parent_id),
            hb.height,
            payload,
        )
    }
}

/// Hanzo vote cast by a validator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HanzoVote {
    /// Block being voted on.
    pub block_id: [u8; 32],
    /// Voter node identity.
    pub voter: [u8; 32],
    /// Type of vote.
    pub vote_type: HanzoVoteType,
    /// BLS or hybrid signature bytes.
    pub signature: Vec<u8>,
}

/// Vote type mirroring `lux_consensus::VoteType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HanzoVoteType {
    Preference,
    Commit,
    Cancel,
}

impl From<HanzoVoteType> for lux_consensus::VoteType {
    fn from(vt: HanzoVoteType) -> Self {
        match vt {
            HanzoVoteType::Preference => lux_consensus::VoteType::Preference,
            HanzoVoteType::Commit => lux_consensus::VoteType::Commit,
            HanzoVoteType::Cancel => lux_consensus::VoteType::Cancel,
        }
    }
}

impl From<lux_consensus::VoteType> for HanzoVoteType {
    fn from(vt: lux_consensus::VoteType) -> Self {
        match vt {
            lux_consensus::VoteType::Preference => HanzoVoteType::Preference,
            lux_consensus::VoteType::Commit => HanzoVoteType::Commit,
            lux_consensus::VoteType::Cancel => HanzoVoteType::Cancel,
        }
    }
}

/// Convert a HanzoVote into lux_consensus::Vote.
impl From<&HanzoVote> for lux_consensus::Vote {
    fn from(hv: &HanzoVote) -> Self {
        lux_consensus::Vote::new(
            lux_consensus::ID::from(hv.block_id),
            hv.vote_type.into(),
            lux_consensus::NodeID::from(hv.voter),
        )
        .with_signature(hv.signature.clone())
    }
}

/// Finalization certificate with dual BLS + PQ signatures.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinalizationCertificate {
    /// Block that was finalized.
    pub block_id: [u8; 32],
    /// Height of the finalized block.
    pub height: u64,
    /// Aggregated BLS12-381 signature (48 bytes).
    pub bls_aggregate_sig: Vec<u8>,
    /// Individual post-quantum (Ringtail) signatures from each signer.
    pub pq_signatures: Vec<Vec<u8>>,
    /// Node IDs of validators who signed.
    pub signers: Vec<[u8; 32]>,
    /// Unix timestamp when the certificate was created (seconds).
    pub timestamp: i64,
}

/// Convert a lux_consensus::Certificate into our FinalizationCertificate.
impl From<&lux_consensus::Certificate> for FinalizationCertificate {
    fn from(cert: &lux_consensus::Certificate) -> Self {
        let signers = cert
            .signers
            .iter()
            .map(|id| *id.as_bytes())
            .collect();
        let timestamp = cert
            .timestamp
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        FinalizationCertificate {
            block_id: *cert.block_id.as_bytes(),
            height: cert.height,
            bls_aggregate_sig: cert.aggregated_sig.clone(),
            pq_signatures: cert.quantum_sigs.clone(),
            signers,
            timestamp,
        }
    }
}

/// Consensus status for a block from the Hanzo perspective.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConsensusStatus {
    /// Not yet seen by the engine.
    Pending,
    /// Actively being voted on.
    Processing,
    /// Accepted by Quasar consensus.
    Accepted,
    /// Rejected by Quasar consensus.
    Rejected,
    /// Accepted and has a finalization certificate.
    Finalized,
}

impl From<lux_consensus::Status> for ConsensusStatus {
    fn from(s: lux_consensus::Status) -> Self {
        match s {
            lux_consensus::Status::Unknown => ConsensusStatus::Pending,
            lux_consensus::Status::Processing => ConsensusStatus::Processing,
            lux_consensus::Status::Accepted => ConsensusStatus::Accepted,
            lux_consensus::Status::Rejected => ConsensusStatus::Rejected,
        }
    }
}

/// Errors produced by the Hanzo consensus engine.
#[derive(Debug, Error)]
pub enum ConsensusError {
    #[error("engine not started")]
    NotStarted,

    #[error("engine already running")]
    AlreadyRunning,

    #[error("block not found: {0}")]
    BlockNotFound(String),

    #[error("invalid block: {0}")]
    InvalidBlock(String),

    #[error("invalid vote: {0}")]
    InvalidVote(String),

    #[error("quasar engine error: {0}")]
    QuasarError(String),

    #[error("configuration error: {0}")]
    ConfigError(String),

    #[error("{0}")]
    Other(String),
}

/// Map lux_consensus errors into ConsensusError.
impl From<lux_consensus::ConsensusError> for ConsensusError {
    fn from(e: lux_consensus::ConsensusError) -> Self {
        ConsensusError::QuasarError(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hanzo_block_compute_id_deterministic() {
        let block = HanzoBlock {
            id: [0u8; 32],
            parent_id: [0u8; 32],
            height: 1,
            timestamp: 1700000000,
            transactions: vec![vec![1, 2, 3]],
            state_root: [0xAA; 32],
        };
        let id1 = block.compute_id();
        let id2 = block.compute_id();
        assert_eq!(id1, id2);
        assert_ne!(id1, [0u8; 32]);
    }

    #[test]
    fn vote_type_roundtrip() {
        let cases = [
            HanzoVoteType::Preference,
            HanzoVoteType::Commit,
            HanzoVoteType::Cancel,
        ];
        for vt in cases {
            let lux_vt: lux_consensus::VoteType = vt.into();
            let back: HanzoVoteType = lux_vt.into();
            assert_eq!(vt, back);
        }
    }

    #[test]
    fn consensus_status_mapping() {
        assert_eq!(
            ConsensusStatus::from(lux_consensus::Status::Unknown),
            ConsensusStatus::Pending
        );
        assert_eq!(
            ConsensusStatus::from(lux_consensus::Status::Accepted),
            ConsensusStatus::Accepted
        );
    }

    #[test]
    fn hanzo_block_to_lux_block() {
        let hb = HanzoBlock {
            id: [1u8; 32],
            parent_id: [0u8; 32],
            height: 42,
            timestamp: 1700000000,
            transactions: vec![vec![0xDE, 0xAD]],
            state_root: [0xBB; 32],
        };
        let lb: lux_consensus::Block = (&hb).into();
        assert_eq!(lb.id, lux_consensus::ID::from([1u8; 32]));
        assert_eq!(lb.height, 42);
    }
}
