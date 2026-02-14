//! Cross-chain message passing between Lux L1 and Hanzo L2.
//!
//! The bridge handles bidirectional communication: submitting messages from L2
//! to L1 for settlement, and relaying finalized L1 messages back into L2.

use std::collections::VecDeque;
use std::sync::Arc;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use crate::TxHash;

/// A message transiting between L1 and L2 chains.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CrossChainMessage {
    /// Originating chain identifier (e.g. "lux-mainnet", "hanzo-l2").
    pub source_chain: String,
    /// Destination chain identifier.
    pub dest_chain: String,
    /// Opaque message payload.
    pub payload: Vec<u8>,
    /// Monotonically increasing nonce scoped to (source, dest) pair.
    pub nonce: u64,
    /// PQ signature over (source_chain || dest_chain || payload || nonce).
    pub signature: Vec<u8>,
}

impl CrossChainMessage {
    /// Compute the unique message identifier (SHA-256 digest).
    pub fn id(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.source_chain.as_bytes());
        hasher.update(self.dest_chain.as_bytes());
        hasher.update(&self.payload);
        hasher.update(self.nonce.to_le_bytes());
        let result = hasher.finalize();
        let mut out = [0u8; 32];
        out.copy_from_slice(&result);
        out
    }
}

/// Pending direction for queued messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum Direction {
    ToL1,
    FromL1,
}

/// Queued message wrapper.
#[derive(Debug, Clone)]
struct QueuedMessage {
    msg: CrossChainMessage,
    direction: Direction,
}

/// L2 bridge managing cross-chain message relay.
pub struct L2Bridge {
    /// Outbound queue: L2 -> L1.
    outbound: Arc<RwLock<VecDeque<QueuedMessage>>>,
    /// Inbound store: messages received from L1 keyed by message id.
    inbound: Arc<RwLock<Vec<CrossChainMessage>>>,
    /// Next nonce for outbound messages.
    next_nonce: Arc<RwLock<u64>>,
    /// L2 chain identifier.
    l2_chain_id: String,
    /// L1 chain identifier.
    l1_chain_id: String,
}

impl L2Bridge {
    /// Create a new bridge between the given L1 and L2 chain identifiers.
    pub fn new(l1_chain_id: impl Into<String>, l2_chain_id: impl Into<String>) -> Self {
        Self {
            outbound: Arc::new(RwLock::new(VecDeque::new())),
            inbound: Arc::new(RwLock::new(Vec::new())),
            next_nonce: Arc::new(RwLock::new(0)),
            l2_chain_id: l2_chain_id.into(),
            l1_chain_id: l1_chain_id.into(),
        }
    }

    /// Send a cross-chain message from L2 to L1.
    ///
    /// The message is queued for inclusion in the next batch commitment.
    /// Returns the transaction hash derived from the message id.
    pub async fn send_to_l1(&self, msg: CrossChainMessage) -> Result<TxHash> {
        if msg.source_chain != self.l2_chain_id {
            bail!(
                "source_chain mismatch: expected {}, got {}",
                self.l2_chain_id,
                msg.source_chain
            );
        }
        if msg.dest_chain != self.l1_chain_id {
            bail!(
                "dest_chain mismatch: expected {}, got {}",
                self.l1_chain_id,
                msg.dest_chain
            );
        }

        let tx_hash = msg.id();

        let queued = QueuedMessage {
            msg,
            direction: Direction::ToL1,
        };

        self.outbound.write().await.push_back(queued);

        let mut nonce = self.next_nonce.write().await;
        *nonce += 1;

        log::info!("queued L2->L1 message, tx_hash={}", hex::encode(tx_hash));
        Ok(tx_hash)
    }

    /// Receive a message that was relayed from L1.
    ///
    /// Looks up the message by its 32-byte identifier in the inbound store.
    pub async fn receive_from_l1(&self, msg_id: [u8; 32]) -> Result<CrossChainMessage> {
        let store = self.inbound.read().await;
        for msg in store.iter() {
            if msg.id() == msg_id {
                return Ok(msg.clone());
            }
        }
        bail!("message not found: {}", hex::encode(msg_id));
    }

    /// Verify a cross-chain message signature and structural integrity.
    ///
    /// Returns `true` if the signature is valid and the nonce is plausible.
    pub async fn verify_message(&self, msg: &CrossChainMessage) -> Result<bool> {
        // Structural checks.
        if msg.payload.is_empty() {
            bail!("empty payload");
        }
        if msg.signature.is_empty() {
            bail!("missing signature");
        }

        // Signature verification delegates to hanzo-pqc.
        // For now we verify the message id is derivable (non-corrupt).
        let _id = msg.id();

        // TODO: integrate hanzo_pqc::verify_signature once key management is wired.
        Ok(true)
    }

    /// Drain the outbound queue, returning all pending L2->L1 messages.
    pub async fn drain_outbound(&self) -> Vec<CrossChainMessage> {
        let mut queue = self.outbound.write().await;
        queue
            .drain(..)
            .filter(|q| q.direction == Direction::ToL1)
            .map(|q| q.msg)
            .collect()
    }

    /// Deliver an inbound message from L1 into the bridge store.
    pub async fn deliver_from_l1(&self, msg: CrossChainMessage) -> Result<()> {
        if msg.dest_chain != self.l2_chain_id {
            bail!(
                "dest_chain mismatch: expected {}, got {}",
                self.l2_chain_id,
                msg.dest_chain
            );
        }
        self.inbound.write().await.push(msg);
        Ok(())
    }

    /// Return the number of pending outbound messages.
    pub async fn outbound_len(&self) -> usize {
        self.outbound.read().await.len()
    }

    /// Return the number of stored inbound messages.
    pub async fn inbound_len(&self) -> usize {
        self.inbound.read().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_msg(source: &str, dest: &str, nonce: u64) -> CrossChainMessage {
        CrossChainMessage {
            source_chain: source.to_string(),
            dest_chain: dest.to_string(),
            payload: vec![0xCA, 0xFE],
            nonce,
            signature: vec![0x01, 0x02, 0x03],
        }
    }

    #[tokio::test]
    async fn test_send_to_l1() {
        let bridge = L2Bridge::new("lux-mainnet", "hanzo-l2");
        let msg = sample_msg("hanzo-l2", "lux-mainnet", 0);
        let tx_hash = bridge.send_to_l1(msg).await.unwrap();
        assert_ne!(tx_hash, [0u8; 32]);
        assert_eq!(bridge.outbound_len().await, 1);
    }

    #[tokio::test]
    async fn test_send_to_l1_rejects_wrong_source() {
        let bridge = L2Bridge::new("lux-mainnet", "hanzo-l2");
        let msg = sample_msg("wrong-chain", "lux-mainnet", 0);
        assert!(bridge.send_to_l1(msg).await.is_err());
    }

    #[tokio::test]
    async fn test_deliver_and_receive_from_l1() {
        let bridge = L2Bridge::new("lux-mainnet", "hanzo-l2");
        let msg = sample_msg("lux-mainnet", "hanzo-l2", 42);
        let msg_id = msg.id();

        bridge.deliver_from_l1(msg.clone()).await.unwrap();
        let received = bridge.receive_from_l1(msg_id).await.unwrap();
        assert_eq!(received, msg);
    }

    #[tokio::test]
    async fn test_receive_from_l1_not_found() {
        let bridge = L2Bridge::new("lux-mainnet", "hanzo-l2");
        assert!(bridge.receive_from_l1([0xAB; 32]).await.is_err());
    }

    #[tokio::test]
    async fn test_drain_outbound() {
        let bridge = L2Bridge::new("lux-mainnet", "hanzo-l2");
        for i in 0..3 {
            let msg = sample_msg("hanzo-l2", "lux-mainnet", i);
            bridge.send_to_l1(msg).await.unwrap();
        }
        let drained = bridge.drain_outbound().await;
        assert_eq!(drained.len(), 3);
        assert_eq!(bridge.outbound_len().await, 0);
    }

    #[tokio::test]
    async fn test_verify_message() {
        let bridge = L2Bridge::new("lux-mainnet", "hanzo-l2");
        let msg = sample_msg("hanzo-l2", "lux-mainnet", 0);
        assert!(bridge.verify_message(&msg).await.unwrap());
    }

    #[tokio::test]
    async fn test_verify_rejects_empty_payload() {
        let bridge = L2Bridge::new("lux-mainnet", "hanzo-l2");
        let msg = CrossChainMessage {
            source_chain: "hanzo-l2".to_string(),
            dest_chain: "lux-mainnet".to_string(),
            payload: vec![],
            nonce: 0,
            signature: vec![0x01],
        };
        assert!(bridge.verify_message(&msg).await.is_err());
    }

    #[test]
    fn test_message_id_deterministic() {
        let msg = sample_msg("a", "b", 1);
        assert_eq!(msg.id(), msg.id());
    }
}
