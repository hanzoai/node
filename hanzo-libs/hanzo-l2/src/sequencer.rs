//! Transaction sequencing for Hanzo L2.
//!
//! The sequencer collects transactions into an ordered mempool, then builds
//! batches at configurable intervals or size thresholds. Each batch is signed
//! by the active sequencer and submitted for state commitment.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use crate::TxHash;

/// Configuration for the sequencer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SequencerConfig {
    /// Maximum number of transactions per batch.
    pub batch_size: usize,
    /// Maximum time to wait before producing a batch, in milliseconds.
    pub batch_timeout_ms: u64,
    /// Sequencer's signing key (opaque bytes, consumed by hanzo-pqc).
    pub signing_key: Vec<u8>,
    /// Sequencer node identifier.
    pub node_id: String,
}

impl Default for SequencerConfig {
    fn default() -> Self {
        Self {
            batch_size: 100,
            batch_timeout_ms: 2000,
            signing_key: Vec::new(),
            node_id: String::new(),
        }
    }
}

/// A batch of ordered transactions ready for commitment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransactionBatch {
    /// Ordered list of raw transactions.
    pub txs: Vec<Vec<u8>>,
    /// Unique batch identifier (hash of contents).
    pub batch_id: [u8; 32],
    /// Unix timestamp (seconds) when the batch was sealed.
    pub timestamp: i64,
    /// PQ signature from the sequencer over the batch id.
    pub sequencer_signature: Vec<u8>,
}

/// The L2 transaction sequencer.
pub struct Sequencer {
    config: SequencerConfig,
    mempool: Arc<RwLock<VecDeque<Vec<u8>>>>,
    batch_count: Arc<RwLock<u64>>,
}

impl Sequencer {
    /// Create a new sequencer with the given configuration.
    pub fn new(config: SequencerConfig) -> Self {
        Self {
            config,
            mempool: Arc::new(RwLock::new(VecDeque::new())),
            batch_count: Arc::new(RwLock::new(0)),
        }
    }

    /// Submit a raw transaction to the mempool.
    ///
    /// Returns the SHA-256 hash of the transaction bytes.
    pub async fn submit_tx(&self, tx: Vec<u8>) -> Result<TxHash> {
        if tx.is_empty() {
            bail!("empty transaction");
        }

        let tx_hash = Self::hash_tx(&tx);
        self.mempool.write().await.push_back(tx);

        log::debug!("tx submitted, hash={}", hex::encode(tx_hash));
        Ok(tx_hash)
    }

    /// Build a batch from the front of the mempool.
    ///
    /// Takes up to `batch_size` transactions and seals them into a
    /// [`TransactionBatch`] with a timestamp and sequencer signature.
    pub async fn build_batch(&self) -> Result<TransactionBatch> {
        let mut pool = self.mempool.write().await;

        if pool.is_empty() {
            bail!("mempool is empty, nothing to batch");
        }

        let take = pool.len().min(self.config.batch_size);
        let txs: Vec<Vec<u8>> = pool.drain(..take).collect();

        let batch_id = Self::compute_batch_id(&txs);
        let timestamp = Utc::now().timestamp();

        // TODO: sign batch_id with hanzo_pqc using self.config.signing_key
        let sequencer_signature = Vec::new();

        let mut count = self.batch_count.write().await;
        *count += 1;

        let batch = TransactionBatch {
            txs,
            batch_id,
            timestamp,
            sequencer_signature,
        };

        log::info!(
            "batch sealed: id={}, txs={}, seq={}",
            hex::encode(batch.batch_id),
            batch.txs.len(),
            count
        );

        Ok(batch)
    }

    /// Return the current mempool depth.
    pub async fn mempool_len(&self) -> usize {
        self.mempool.read().await.len()
    }

    /// Return the number of batches produced so far.
    pub async fn batch_count(&self) -> u64 {
        *self.batch_count.read().await
    }

    /// Return the configured batch timeout as a [`Duration`].
    pub fn batch_timeout(&self) -> Duration {
        Duration::from_millis(self.config.batch_timeout_ms)
    }

    /// Hash a single transaction (SHA-256).
    fn hash_tx(tx: &[u8]) -> TxHash {
        let digest = Sha256::digest(tx);
        let mut out = [0u8; 32];
        out.copy_from_slice(&digest);
        out
    }

    /// Compute the batch id by hashing the concatenation of all tx hashes.
    fn compute_batch_id(txs: &[Vec<u8>]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        for tx in txs {
            hasher.update(Sha256::digest(tx));
        }
        let digest = hasher.finalize();
        let mut out = [0u8; 32];
        out.copy_from_slice(&digest);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> SequencerConfig {
        SequencerConfig {
            batch_size: 3,
            batch_timeout_ms: 1000,
            signing_key: vec![0xAA],
            node_id: "test-seq".to_string(),
        }
    }

    #[tokio::test]
    async fn test_submit_tx() {
        let seq = Sequencer::new(test_config());
        let hash = seq.submit_tx(vec![1, 2, 3]).await.unwrap();
        assert_ne!(hash, [0u8; 32]);
        assert_eq!(seq.mempool_len().await, 1);
    }

    #[tokio::test]
    async fn test_submit_empty_tx_rejected() {
        let seq = Sequencer::new(test_config());
        assert!(seq.submit_tx(vec![]).await.is_err());
    }

    #[tokio::test]
    async fn test_build_batch() {
        let seq = Sequencer::new(test_config());
        seq.submit_tx(vec![1]).await.unwrap();
        seq.submit_tx(vec![2]).await.unwrap();

        let batch = seq.build_batch().await.unwrap();
        assert_eq!(batch.txs.len(), 2);
        assert_ne!(batch.batch_id, [0u8; 32]);
        assert!(batch.timestamp > 0);
        assert_eq!(seq.mempool_len().await, 0);
        assert_eq!(seq.batch_count().await, 1);
    }

    #[tokio::test]
    async fn test_build_batch_respects_size_limit() {
        let seq = Sequencer::new(test_config()); // batch_size = 3
        for i in 0..5u8 {
            seq.submit_tx(vec![i]).await.unwrap();
        }

        let batch = seq.build_batch().await.unwrap();
        assert_eq!(batch.txs.len(), 3);
        assert_eq!(seq.mempool_len().await, 2);
    }

    #[tokio::test]
    async fn test_build_batch_empty_mempool() {
        let seq = Sequencer::new(test_config());
        assert!(seq.build_batch().await.is_err());
    }

    #[tokio::test]
    async fn test_batch_id_deterministic() {
        let txs = vec![vec![1, 2], vec![3, 4]];
        let id1 = Sequencer::compute_batch_id(&txs);
        let id2 = Sequencer::compute_batch_id(&txs);
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_default_config() {
        let cfg = SequencerConfig::default();
        assert_eq!(cfg.batch_size, 100);
        assert_eq!(cfg.batch_timeout_ms, 2000);
    }
}
