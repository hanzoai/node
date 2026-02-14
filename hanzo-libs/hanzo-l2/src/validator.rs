//! Validator set management for Hanzo L2.
//!
//! Tracks active validators, their stakes, and public keys. Provides
//! stake-weighted committee selection for each epoch.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

/// Information about a single validator.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ValidatorInfo {
    /// Unique node identifier.
    pub node_id: String,
    /// Staked amount (in smallest unit).
    pub stake: u64,
    /// BLS public key for aggregate signatures.
    pub bls_pubkey: Vec<u8>,
    /// Post-quantum public key (ML-DSA via hanzo-pqc).
    pub pq_pubkey: Vec<u8>,
    /// Whether this validator is currently active.
    pub active: bool,
}

/// Manages the full validator set for the L2 network.
pub struct ValidatorSet {
    validators: Arc<RwLock<HashMap<String, ValidatorInfo>>>,
}

impl ValidatorSet {
    /// Create an empty validator set.
    pub fn new() -> Self {
        Self {
            validators: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Add a validator to the set.
    ///
    /// Returns an error if a validator with the same `node_id` already exists.
    pub async fn add_validator(&self, info: ValidatorInfo) -> Result<()> {
        let mut set = self.validators.write().await;
        if set.contains_key(&info.node_id) {
            bail!("validator already exists: {}", info.node_id);
        }
        log::info!(
            "validator added: node_id={}, stake={}",
            info.node_id,
            info.stake
        );
        set.insert(info.node_id.clone(), info);
        Ok(())
    }

    /// Remove a validator by node id.
    ///
    /// Returns the removed validator info, or an error if not found.
    pub async fn remove_validator(&self, node_id: &str) -> Result<ValidatorInfo> {
        let mut set = self.validators.write().await;
        match set.remove(node_id) {
            Some(info) => {
                log::info!("validator removed: node_id={}", node_id);
                Ok(info)
            }
            None => bail!("validator not found: {node_id}"),
        }
    }

    /// Return all currently active validators.
    pub async fn get_active_set(&self) -> Vec<ValidatorInfo> {
        self.validators
            .read()
            .await
            .values()
            .filter(|v| v.active)
            .cloned()
            .collect()
    }

    /// Return total number of validators (active and inactive).
    pub async fn len(&self) -> usize {
        self.validators.read().await.len()
    }

    /// Check if the set is empty.
    pub async fn is_empty(&self) -> bool {
        self.validators.read().await.is_empty()
    }

    /// Compute the committee for a given epoch using stake-weighted selection.
    ///
    /// The committee is selected deterministically: validators are sorted by a
    /// hash of (node_id || epoch), then the top `committee_size` by stake from
    /// the active set are chosen. If there are fewer active validators than
    /// `committee_size`, all active validators are returned.
    pub async fn compute_committee(&self, epoch: u64) -> Vec<ValidatorInfo> {
        let active = self.get_active_set().await;
        if active.is_empty() {
            return Vec::new();
        }

        // Target committee size: at most 2/3 + 1 of active set for BFT.
        let committee_size = (active.len() * 2 / 3) + 1;

        let mut scored: Vec<(ValidatorInfo, Vec<u8>)> = active
            .into_iter()
            .map(|v| {
                let seed = Self::selection_seed(&v.node_id, epoch);
                (v, seed)
            })
            .collect();

        // Sort by (stake descending, then seed ascending for tie-breaking).
        scored.sort_by(|(a, seed_a), (b, seed_b)| {
            b.stake.cmp(&a.stake).then_with(|| seed_a.cmp(seed_b))
        });

        scored
            .into_iter()
            .take(committee_size)
            .map(|(v, _)| v)
            .collect()
    }

    /// Deterministic seed for committee selection.
    fn selection_seed(node_id: &str, epoch: u64) -> Vec<u8> {
        let mut hasher = Sha256::new();
        hasher.update(node_id.as_bytes());
        hasher.update(epoch.to_le_bytes());
        hasher.finalize().to_vec()
    }

    /// Look up a validator by node id.
    pub async fn get_validator(&self, node_id: &str) -> Option<ValidatorInfo> {
        self.validators.read().await.get(node_id).cloned()
    }

    /// Total stake across all active validators.
    pub async fn total_active_stake(&self) -> u64 {
        self.validators
            .read()
            .await
            .values()
            .filter(|v| v.active)
            .map(|v| v.stake)
            .sum()
    }
}

impl Default for ValidatorSet {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_validator(id: &str, stake: u64, active: bool) -> ValidatorInfo {
        ValidatorInfo {
            node_id: id.to_string(),
            stake,
            bls_pubkey: vec![0x01],
            pq_pubkey: vec![0x02],
            active,
        }
    }

    #[tokio::test]
    async fn test_add_and_get_validator() {
        let vs = ValidatorSet::new();
        let v = make_validator("node-1", 1000, true);
        vs.add_validator(v.clone()).await.unwrap();

        let got = vs.get_validator("node-1").await.unwrap();
        assert_eq!(got, v);
    }

    #[tokio::test]
    async fn test_add_duplicate_rejected() {
        let vs = ValidatorSet::new();
        vs.add_validator(make_validator("node-1", 1000, true))
            .await
            .unwrap();
        assert!(vs
            .add_validator(make_validator("node-1", 2000, true))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_remove_validator() {
        let vs = ValidatorSet::new();
        vs.add_validator(make_validator("node-1", 1000, true))
            .await
            .unwrap();
        let removed = vs.remove_validator("node-1").await.unwrap();
        assert_eq!(removed.node_id, "node-1");
        assert!(vs.is_empty().await);
    }

    #[tokio::test]
    async fn test_remove_nonexistent() {
        let vs = ValidatorSet::new();
        assert!(vs.remove_validator("ghost").await.is_err());
    }

    #[tokio::test]
    async fn test_get_active_set() {
        let vs = ValidatorSet::new();
        vs.add_validator(make_validator("a", 100, true))
            .await
            .unwrap();
        vs.add_validator(make_validator("b", 200, false))
            .await
            .unwrap();
        vs.add_validator(make_validator("c", 300, true))
            .await
            .unwrap();

        let active = vs.get_active_set().await;
        assert_eq!(active.len(), 2);
        assert!(active.iter().all(|v| v.active));
    }

    #[tokio::test]
    async fn test_total_active_stake() {
        let vs = ValidatorSet::new();
        vs.add_validator(make_validator("a", 100, true))
            .await
            .unwrap();
        vs.add_validator(make_validator("b", 200, false))
            .await
            .unwrap();
        vs.add_validator(make_validator("c", 300, true))
            .await
            .unwrap();

        assert_eq!(vs.total_active_stake().await, 400);
    }

    #[tokio::test]
    async fn test_compute_committee() {
        let vs = ValidatorSet::new();
        for i in 0..6 {
            vs.add_validator(make_validator(
                &format!("node-{i}"),
                (i + 1) as u64 * 100,
                true,
            ))
            .await
            .unwrap();
        }

        let committee = vs.compute_committee(1).await;
        // 6 active -> committee_size = (6*2/3)+1 = 5
        assert_eq!(committee.len(), 5);
        // Highest staked should be first.
        assert!(committee[0].stake >= committee[1].stake);
    }

    #[tokio::test]
    async fn test_compute_committee_empty() {
        let vs = ValidatorSet::new();
        let committee = vs.compute_committee(0).await;
        assert!(committee.is_empty());
    }

    #[tokio::test]
    async fn test_compute_committee_deterministic() {
        let vs = ValidatorSet::new();
        for i in 0..4 {
            vs.add_validator(make_validator(
                &format!("node-{i}"),
                (i + 1) as u64 * 50,
                true,
            ))
            .await
            .unwrap();
        }
        let c1 = vs.compute_committee(10).await;
        let c2 = vs.compute_committee(10).await;
        assert_eq!(c1, c2);
    }
}
