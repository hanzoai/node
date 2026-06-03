//! State commitment to L1 (Lux).
//!
//! Produces Merkle-root state commitments that anchor L2 state on the L1 chain.
//! Supports batch commitment for amortised gas cost.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Represents an L2 block for commitment purposes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    /// Block height.
    pub height: u64,
    /// Ordered transaction data in this block.
    pub transactions: Vec<Vec<u8>>,
    /// Ordered receipt data in this block.
    pub receipts: Vec<Vec<u8>>,
    /// Serialised state trie (or state diff).
    pub state_data: Vec<u8>,
    /// Unix timestamp (seconds).
    pub timestamp: i64,
}

/// A state commitment anchoring L2 state to L1.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StateCommitment {
    /// L2 block height this commitment covers.
    pub block_height: u64,
    /// Merkle root of the state trie.
    pub state_root: [u8; 32],
    /// Merkle root of the transaction list.
    pub tx_root: [u8; 32],
    /// Merkle root of the receipt list.
    pub receipt_root: [u8; 32],
    /// Unix timestamp of the committed block.
    pub timestamp: i64,
}

/// Merkle proof for verifying a state commitment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommitmentProof {
    /// Sibling hashes along the Merkle path.
    pub siblings: Vec<[u8; 32]>,
    /// Index of the leaf in the tree.
    pub leaf_index: usize,
    /// The leaf hash being proved.
    pub leaf_hash: [u8; 32],
    /// The expected root.
    pub root: [u8; 32],
}

/// Create a [`StateCommitment`] from a block.
pub fn create_commitment(block: &Block) -> Result<StateCommitment> {
    if block.transactions.is_empty() && block.state_data.is_empty() {
        bail!("block has no transactions and no state data");
    }

    let state_root = sha256_hash(&block.state_data);
    let tx_root = compute_merkle_root(&block.transactions);
    let receipt_root = compute_merkle_root(&block.receipts);

    Ok(StateCommitment {
        block_height: block.height,
        state_root,
        tx_root,
        receipt_root,
        timestamp: block.timestamp,
    })
}

/// Verify a [`StateCommitment`] against a [`CommitmentProof`].
///
/// Walks the Merkle path from the leaf to the root and checks equality.
pub fn verify_commitment(
    commitment: &StateCommitment,
    proof: &CommitmentProof,
) -> Result<bool> {
    if proof.siblings.is_empty() {
        // Single-leaf tree: the leaf hash must equal the root.
        return Ok(proof.leaf_hash == proof.root);
    }

    let mut current = proof.leaf_hash;
    let mut index = proof.leaf_index;

    for sibling in &proof.siblings {
        current = if index % 2 == 0 {
            hash_pair(&current, sibling)
        } else {
            hash_pair(sibling, &current)
        };
        index /= 2;
    }

    // The reconstructed root must match the proof root and the commitment's tx_root.
    Ok(current == proof.root && proof.root == commitment.tx_root)
}

/// Create commitments for a batch of blocks in one pass.
///
/// Returns a list of commitments and a single aggregate root covering all of them.
pub fn batch_commit(blocks: &[Block]) -> Result<(Vec<StateCommitment>, [u8; 32])> {
    if blocks.is_empty() {
        bail!("empty block batch");
    }

    let mut commitments = Vec::with_capacity(blocks.len());
    let mut commitment_hashes = Vec::with_capacity(blocks.len());

    for block in blocks {
        let c = create_commitment(block)?;
        let h = hash_commitment(&c);
        commitment_hashes.push(h.to_vec());
        commitments.push(c);
    }

    let aggregate_root = compute_merkle_root(&commitment_hashes);

    Ok((commitments, aggregate_root))
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// SHA-256 hash of arbitrary bytes.
fn sha256_hash(data: &[u8]) -> [u8; 32] {
    let d = Sha256::digest(data);
    let mut out = [0u8; 32];
    out.copy_from_slice(&d);
    out
}

/// Hash two 32-byte nodes together (left || right).
fn hash_pair(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(left);
    hasher.update(right);
    let d = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&d);
    out
}

/// Compute a Merkle root from a list of byte slices.
///
/// Leaves are SHA-256 hashed individually, then paired bottom-up.
/// If the number of nodes at any level is odd, the last node is duplicated.
fn compute_merkle_root(items: &[Vec<u8>]) -> [u8; 32] {
    if items.is_empty() {
        return [0u8; 32];
    }

    let mut level: Vec<[u8; 32]> = items.iter().map(|item| sha256_hash(item)).collect();

    while level.len() > 1 {
        if level.len() % 2 != 0 {
            let last = *level.last().unwrap();
            level.push(last);
        }

        let mut next = Vec::with_capacity(level.len() / 2);
        for pair in level.chunks_exact(2) {
            next.push(hash_pair(&pair[0], &pair[1]));
        }
        level = next;
    }

    level[0]
}

/// Hash a state commitment for use in aggregate Merkle trees.
fn hash_commitment(c: &StateCommitment) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(c.block_height.to_le_bytes());
    hasher.update(c.state_root);
    hasher.update(c.tx_root);
    hasher.update(c.receipt_root);
    hasher.update(c.timestamp.to_le_bytes());
    let d = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&d);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_block(height: u64) -> Block {
        Block {
            height,
            transactions: vec![vec![1, 2, 3], vec![4, 5, 6]],
            receipts: vec![vec![0xA], vec![0xB]],
            state_data: vec![0xFF; 64],
            timestamp: 1700000000,
        }
    }

    #[test]
    fn test_create_commitment() {
        let block = sample_block(1);
        let commitment = create_commitment(&block).unwrap();

        assert_eq!(commitment.block_height, 1);
        assert_ne!(commitment.state_root, [0u8; 32]);
        assert_ne!(commitment.tx_root, [0u8; 32]);
        assert_ne!(commitment.receipt_root, [0u8; 32]);
        assert_eq!(commitment.timestamp, 1700000000);
    }

    #[test]
    fn test_create_commitment_rejects_empty_block() {
        let block = Block {
            height: 0,
            transactions: vec![],
            receipts: vec![],
            state_data: vec![],
            timestamp: 0,
        };
        assert!(create_commitment(&block).is_err());
    }

    #[test]
    fn test_merkle_root_deterministic() {
        let items = vec![vec![1, 2], vec![3, 4], vec![5, 6]];
        let r1 = compute_merkle_root(&items);
        let r2 = compute_merkle_root(&items);
        assert_eq!(r1, r2);
    }

    #[test]
    fn test_merkle_root_empty() {
        assert_eq!(compute_merkle_root(&[]), [0u8; 32]);
    }

    #[test]
    fn test_merkle_root_single_item() {
        let items = vec![vec![42]];
        let root = compute_merkle_root(&items);
        assert_eq!(root, sha256_hash(&[42]));
    }

    #[test]
    fn test_verify_commitment_single_leaf() {
        let block = Block {
            height: 1,
            transactions: vec![vec![0xCA, 0xFE]],
            receipts: vec![],
            state_data: vec![0xFF],
            timestamp: 100,
        };
        let commitment = create_commitment(&block).unwrap();
        let leaf_hash = sha256_hash(&block.transactions[0]);

        let proof = CommitmentProof {
            siblings: vec![],
            leaf_index: 0,
            leaf_hash,
            root: commitment.tx_root,
        };

        assert!(verify_commitment(&commitment, &proof).unwrap());
    }

    #[test]
    fn test_verify_commitment_two_leaves() {
        let block = Block {
            height: 1,
            transactions: vec![vec![1], vec![2]],
            receipts: vec![],
            state_data: vec![0xFF],
            timestamp: 100,
        };
        let commitment = create_commitment(&block).unwrap();

        let leaf_hash = sha256_hash(&block.transactions[0]);
        let sibling = sha256_hash(&block.transactions[1]);

        let proof = CommitmentProof {
            siblings: vec![sibling],
            leaf_index: 0,
            leaf_hash,
            root: commitment.tx_root,
        };

        assert!(verify_commitment(&commitment, &proof).unwrap());
    }

    #[test]
    fn test_batch_commit() {
        let blocks: Vec<Block> = (0..4).map(sample_block).collect();
        let (commitments, aggregate_root) = batch_commit(&blocks).unwrap();

        assert_eq!(commitments.len(), 4);
        assert_ne!(aggregate_root, [0u8; 32]);

        for (i, c) in commitments.iter().enumerate() {
            assert_eq!(c.block_height, i as u64);
        }
    }

    #[test]
    fn test_batch_commit_empty() {
        assert!(batch_commit(&[]).is_err());
    }

    #[test]
    fn test_commitment_deterministic() {
        let block = sample_block(5);
        let c1 = create_commitment(&block).unwrap();
        let c2 = create_commitment(&block).unwrap();
        assert_eq!(c1, c2);
    }
}
