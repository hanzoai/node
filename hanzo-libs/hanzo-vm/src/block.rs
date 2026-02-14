//! Block and transaction types for the Hanzo L2.
//!
//! Block IDs are computed as the BLAKE3 hash of the serialized
//! [`BlockHeader`], giving 256-bit collision-resistant identifiers
//! without the overhead of SHA-256.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Transaction
// ---------------------------------------------------------------------------

/// A single EVM transaction within a block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    /// RLP-encoded raw transaction bytes.
    pub raw_bytes: Vec<u8>,
    /// Position within the block (0-indexed).
    pub tx_index: u32,
    /// Gas consumed by execution (filled after processing).
    pub gas_used: u64,
}

// ---------------------------------------------------------------------------
// BlockHeader
// ---------------------------------------------------------------------------

/// Immutable header fields whose hash constitutes the block ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockHeader {
    /// ID of the parent block.
    pub parent_id: [u8; 32],
    /// Zero-indexed height of this block.
    pub height: u64,
    /// Unix timestamp (seconds) when the block was produced.
    pub timestamp: u64,
    /// Number of transactions in the block.
    pub tx_count: u32,
    /// State root after applying all transactions.
    pub state_root: [u8; 32],
}

// ---------------------------------------------------------------------------
// Block
// ---------------------------------------------------------------------------

/// A complete block: header plus transaction list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    /// Block header.
    pub header: BlockHeader,
    /// Ordered list of transactions.
    pub transactions: Vec<Transaction>,
}

impl Block {
    /// Construct a new block.
    pub fn new(
        parent_id: [u8; 32],
        height: u64,
        timestamp: u64,
        transactions: Vec<Transaction>,
        state_root: [u8; 32],
    ) -> Self {
        let header = BlockHeader {
            parent_id,
            height,
            timestamp,
            tx_count: transactions.len() as u32,
            state_root,
        };
        Self {
            header,
            transactions,
        }
    }

    /// Create the genesis block (height 0, no parent, no transactions).
    pub fn genesis(chain_id: u64, timestamp: u64) -> Self {
        // Embed chain_id in the parent field of genesis as a fingerprint.
        let mut parent = [0u8; 32];
        parent[..8].copy_from_slice(&chain_id.to_le_bytes());

        Self::new(parent, 0, timestamp, Vec::new(), [0u8; 32])
    }

    /// Compute the block ID as the BLAKE3 hash of the header.
    pub fn id(&self) -> [u8; 32] {
        let header_bytes = serde_json::to_vec(&self.header).unwrap_or_default();
        *blake3::hash(&header_bytes).as_bytes()
    }

    /// Serialize the full block to bytes (JSON for dev, compact encoding later).
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// Deserialize a block from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genesis_block_has_height_zero() {
        let block = Block::genesis(31337, 1700000000);
        assert_eq!(block.header.height, 0);
        assert!(block.transactions.is_empty());
    }

    #[test]
    fn block_id_is_deterministic() {
        let block = Block::genesis(31337, 1700000000);
        assert_eq!(block.id(), block.id());
    }

    #[test]
    fn different_blocks_have_different_ids() {
        let a = Block::genesis(31337, 1700000000);
        let b = Block::genesis(31337, 1700000001);
        assert_ne!(a.id(), b.id());
    }

    #[test]
    fn serialization_round_trip() {
        let block = Block::new(
            [0xaa; 32],
            10,
            1700000000,
            vec![Transaction {
                raw_bytes: vec![0xde, 0xad],
                tx_index: 0,
                gas_used: 21000,
            }],
            [0xbb; 32],
        );

        let bytes = block.to_bytes();
        let parsed = Block::from_bytes(&bytes).unwrap();
        assert_eq!(block, parsed);
    }

    #[test]
    fn block_id_is_blake3_of_header() {
        let block = Block::genesis(1, 0);
        let header_bytes = serde_json::to_vec(&block.header).unwrap();
        let expected = *blake3::hash(&header_bytes).as_bytes();
        assert_eq!(block.id(), expected);
    }
}
