//! Global AI Mining Ledger - Powered by Lux Consensus
//!
//! This module implements the global ledger for AI coin mining, using Lux's
//! quantum-safe consensus protocol for transaction finality. The ledger tracks:
//!
//! - Mining rewards earned across the network
//! - Data sharing contributions
//! - Compute provisioning
//! - Model hosting rewards
//! - Cross-chain teleport transfers
//!
//! ## Architecture
//!
//! The mining ledger operates as an overlay on the Lux consensus layer:
//! - Lux provides BFT consensus with 2-round finality
//! - Quantum-safe signatures (ML-DSA) for all transactions
//! - Sub-second block times with 69% quorum threshold
//!
//! ## Usage
//!
//! ```ignore
//! use hanzo_mining::ledger::{MiningLedger, LedgerConfig};
//! use hanzo_mining::wallet::MiningWallet;
//!
//! // Create quantum-safe wallet
//! let wallet = MiningWallet::generate_quantum_safe().await?;
//!
//! // Connect to global ledger
//! let ledger = MiningLedger::connect(LedgerConfig::mainnet()).await?;
//!
//! // Submit mining reward claim
//! let tx = ledger.claim_reward(&wallet, reward_proof).await?;
//! ```

use crate::{NetworkType, PerformanceStats, MiningRewardType};
use crate::evm::{TeleportDestination, TeleportTransfer, TeleportStatus};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Lux consensus network IDs
pub const LUX_MAINNET_NETWORK_ID: u32 = 1;
pub const LUX_TESTNET_NETWORK_ID: u32 = 5;

/// AI Protocol network IDs (overlay on Lux)
pub const AI_MAINNET_ID: u32 = 36963;
pub const AI_TESTNET_ID: u32 = 36964;

/// Block production parameters
pub const BLOCK_TIME_MS: u64 = 500;      // 500ms block time
pub const QUORUM_THRESHOLD: f64 = 0.69;   // 69% BFT threshold
pub const FINALITY_ROUNDS: u32 = 2;       // 2-round finality

/// Ledger configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerConfig {
    /// Network type
    pub network: NetworkType,
    /// Lux consensus RPC endpoint
    pub consensus_rpc: String,
    /// P2P bootstrap peers
    pub bootstrap_peers: Vec<String>,
    /// Block time in milliseconds
    pub block_time_ms: u64,
    /// Enable quantum-safe mode
    pub quantum_safe: bool,
    /// Security level (NIST 2, 3, or 5)
    pub security_level: u8,
}

impl LedgerConfig {
    pub fn mainnet() -> Self {
        Self {
            network: NetworkType::HanzoMainnet,
            consensus_rpc: "https://consensus.hanzo.network".to_string(),
            bootstrap_peers: vec![
                "/dns4/boot1.hanzo.network/tcp/3691".to_string(),
                "/dns4/boot2.hanzo.network/tcp/3691".to_string(),
                "/dns4/boot3.hanzo.network/tcp/3691".to_string(),
            ],
            block_time_ms: BLOCK_TIME_MS,
            quantum_safe: true,
            security_level: 3, // NIST Level 3 (ML-DSA-65)
        }
    }

    pub fn testnet() -> Self {
        Self {
            network: NetworkType::HanzoTestnet,
            consensus_rpc: "https://consensus.hanzo-test.network".to_string(),
            bootstrap_peers: vec![
                "/dns4/boot1.hanzo-test.network/tcp/3691".to_string(),
            ],
            block_time_ms: BLOCK_TIME_MS,
            quantum_safe: true,
            security_level: 3,
        }
    }

    pub fn zoo_mainnet() -> Self {
        Self {
            network: NetworkType::ZooMainnet,
            consensus_rpc: "https://consensus.zoo.network".to_string(),
            bootstrap_peers: vec![
                "/dns4/boot1.zoo.network/tcp/3691".to_string(),
                "/dns4/boot2.zoo.network/tcp/3691".to_string(),
            ],
            block_time_ms: BLOCK_TIME_MS,
            quantum_safe: true,
            security_level: 3,
        }
    }
}

/// Block header in the mining ledger
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockHeader {
    /// Block height
    pub height: u64,
    /// Previous block hash (Blake3)
    pub parent_hash: [u8; 32],
    /// Block hash
    pub hash: [u8; 32],
    /// Timestamp (Unix ms)
    pub timestamp: u64,
    /// Proposer's quantum-safe public key
    pub proposer: Vec<u8>,
    /// Merkle root of transactions
    pub tx_root: [u8; 32],
    /// Merkle root of state
    pub state_root: [u8; 32],
    /// Number of transactions
    pub tx_count: u32,
}

/// Mining transaction types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MiningTransaction {
    /// Register as a miner on the network
    RegisterMiner {
        /// Quantum-safe public key
        public_key: Vec<u8>,
        /// Miner capabilities
        capabilities: Vec<u8>,
        /// Performance stats
        stats: PerformanceStats,
        /// Signature (ML-DSA)
        signature: Vec<u8>,
    },
    /// Submit proof of work/contribution
    SubmitProof {
        /// Miner public key
        miner: Vec<u8>,
        /// Type of contribution
        reward_type: MiningRewardType,
        /// Proof data (varies by type)
        proof: Vec<u8>,
        /// Signature
        signature: Vec<u8>,
    },
    /// Claim accumulated rewards
    ClaimReward {
        /// Miner public key
        miner: Vec<u8>,
        /// Reward amount (in wei)
        amount: u128,
        /// Recipient address
        recipient: String,
        /// Merkle proof of rewards
        proof: Vec<u8>,
        /// Signature
        signature: Vec<u8>,
    },
    /// Teleport AI coins to EVM chain
    TeleportOut {
        /// Source miner
        from: Vec<u8>,
        /// Destination chain
        destination: TeleportDestination,
        /// Destination address
        to_address: String,
        /// Amount to teleport
        amount: u128,
        /// Signature
        signature: Vec<u8>,
    },
    /// Receive teleported coins from EVM
    TeleportIn {
        /// Source chain
        source_chain: u64,
        /// Source transaction hash
        source_tx: String,
        /// Recipient public key
        recipient: Vec<u8>,
        /// Amount received
        amount: u128,
        /// Relay signature
        relay_signature: Vec<u8>,
    },
    /// Update miner status/capabilities
    UpdateMiner {
        /// Miner public key
        miner: Vec<u8>,
        /// New capabilities
        capabilities: Option<Vec<u8>>,
        /// New performance stats
        stats: Option<PerformanceStats>,
        /// Signature
        signature: Vec<u8>,
    },
    /// Validator vote for consensus
    Vote {
        /// Voter's public key
        voter: Vec<u8>,
        /// Block being voted on
        block_hash: [u8; 32],
        /// Vote type (preference, commit, cancel)
        vote_type: VoteType,
        /// Quantum-safe signature
        signature: Vec<u8>,
    },
}

/// Vote types for consensus
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum VoteType {
    /// Preference vote
    Preference,
    /// Commit vote (finalizes block)
    Commit,
    /// Cancel vote (rejects block)
    Cancel,
}

/// Transaction status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TransactionStatus {
    /// Transaction is pending
    Pending,
    /// Transaction is in mempool
    InMempool,
    /// Transaction is included in block
    Included { block_height: u64 },
    /// Transaction is finalized
    Finalized { block_height: u64 },
    /// Transaction failed
    Failed { reason: String },
}

/// Miner state in the ledger
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinerState {
    /// Quantum-safe public key
    pub public_key: Vec<u8>,
    /// Address (derived from public key)
    pub address: String,
    /// Total rewards earned (in wei)
    pub total_earned: u128,
    /// Pending rewards (claimable)
    pub pending_rewards: u128,
    /// Registration block
    pub registered_at: u64,
    /// Last active block
    pub last_active: u64,
    /// Reputation score (0-100)
    pub reputation: u64,
    /// Jobs completed
    pub jobs_completed: u64,
    /// Performance stats
    pub stats: PerformanceStats,
    /// Active capabilities
    pub capabilities: Vec<u8>,
}

/// Global mining ledger client
pub struct MiningLedger {
    /// Configuration
    config: LedgerConfig,
    /// Current block height
    current_height: Arc<RwLock<u64>>,
    /// Local miner states cache
    miners: Arc<RwLock<HashMap<String, MinerState>>>,
    /// Pending transactions
    pending_txs: Arc<RwLock<Vec<(String, MiningTransaction)>>>,
    /// Connected to network
    connected: Arc<RwLock<bool>>,
    /// HTTP client for RPC
    http_client: reqwest::Client,
}

impl MiningLedger {
    /// Connect to the global mining ledger
    pub async fn connect(config: LedgerConfig) -> Result<Self, LedgerError> {
        let ledger = Self {
            config,
            current_height: Arc::new(RwLock::new(0)),
            miners: Arc::new(RwLock::new(HashMap::new())),
            pending_txs: Arc::new(RwLock::new(Vec::new())),
            connected: Arc::new(RwLock::new(false)),
            http_client: reqwest::Client::new(),
        };

        // Connect to consensus network
        ledger.sync_state().await?;
        *ledger.connected.write().await = true;

        Ok(ledger)
    }

    /// Sync local state with network
    async fn sync_state(&self) -> Result<(), LedgerError> {
        // Get current block height from consensus RPC
        let height = self.get_block_height().await?;
        *self.current_height.write().await = height;
        Ok(())
    }

    /// Get current block height
    pub async fn get_block_height(&self) -> Result<u64, LedgerError> {
        let response = self.rpc_call("ledger.getHeight", serde_json::json!({})).await?;
        let height = response.get("height")
            .and_then(|h| h.as_u64())
            .unwrap_or(0);
        Ok(height)
    }

    /// Register a new miner
    pub async fn register_miner(
        &self,
        public_key: &[u8],
        capabilities: &[u8],
        stats: &PerformanceStats,
        signature: &[u8],
    ) -> Result<String, LedgerError> {
        let tx = MiningTransaction::RegisterMiner {
            public_key: public_key.to_vec(),
            capabilities: capabilities.to_vec(),
            stats: stats.clone(),
            signature: signature.to_vec(),
        };

        self.submit_transaction(tx).await
    }

    /// Submit a mining proof
    pub async fn submit_proof(
        &self,
        miner: &[u8],
        reward_type: MiningRewardType,
        proof: &[u8],
        signature: &[u8],
    ) -> Result<String, LedgerError> {
        let tx = MiningTransaction::SubmitProof {
            miner: miner.to_vec(),
            reward_type,
            proof: proof.to_vec(),
            signature: signature.to_vec(),
        };

        self.submit_transaction(tx).await
    }

    /// Claim accumulated rewards
    pub async fn claim_rewards(
        &self,
        miner: &[u8],
        amount: u128,
        recipient: &str,
        proof: &[u8],
        signature: &[u8],
    ) -> Result<String, LedgerError> {
        let tx = MiningTransaction::ClaimReward {
            miner: miner.to_vec(),
            amount,
            recipient: recipient.to_string(),
            proof: proof.to_vec(),
            signature: signature.to_vec(),
        };

        self.submit_transaction(tx).await
    }

    /// Teleport AI coins to an EVM chain
    pub async fn teleport_out(
        &self,
        from: &[u8],
        destination: TeleportDestination,
        to_address: &str,
        amount: u128,
        signature: &[u8],
    ) -> Result<TeleportTransfer, LedgerError> {
        let tx = MiningTransaction::TeleportOut {
            from: from.to_vec(),
            destination: destination.clone(),
            to_address: to_address.to_string(),
            amount,
            signature: signature.to_vec(),
        };

        let tx_hash = self.submit_transaction(tx).await?;

        Ok(TeleportTransfer {
            teleport_id: tx_hash.clone(),
            amount,
            from_address: hex::encode(from),
            to_address: to_address.to_string(),
            destination,
            status: TeleportStatus::Initiated,
            protocol_tx: Some(tx_hash),
            evm_tx: None,
            initiated_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            completed_at: None,
        })
    }

    /// Get miner state from ledger
    pub async fn get_miner_state(&self, address: &str) -> Result<Option<MinerState>, LedgerError> {
        // Check local cache first
        if let Some(state) = self.miners.read().await.get(address) {
            return Ok(Some(state.clone()));
        }

        // Fetch from network
        let response = self.rpc_call(
            "ledger.getMiner",
            serde_json::json!({ "address": address }),
        ).await?;

        if response.is_null() {
            return Ok(None);
        }

        let state: MinerState = serde_json::from_value(response)
            .map_err(|e| LedgerError::InvalidResponse(e.to_string()))?;

        // Cache locally
        self.miners.write().await.insert(address.to_string(), state.clone());

        Ok(Some(state))
    }

    /// Get pending rewards for a miner
    pub async fn get_pending_rewards(&self, miner: &[u8]) -> Result<u128, LedgerError> {
        let address = derive_address_from_pubkey(miner);

        let response = self.rpc_call(
            "ledger.getPendingRewards",
            serde_json::json!({ "address": address }),
        ).await?;

        let rewards = response.get("rewards")
            .and_then(|r| r.as_str())
            .and_then(|s| s.parse::<u128>().ok())
            .unwrap_or(0);

        Ok(rewards)
    }

    /// Get teleport transfer status
    pub async fn get_teleport_status(&self, teleport_id: &str) -> Result<TeleportStatus, LedgerError> {
        let response = self.rpc_call(
            "ledger.getTeleportStatus",
            serde_json::json!({ "id": teleport_id }),
        ).await?;

        let status_str = response.get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("unknown");

        let status = match status_str {
            "initiated" => TeleportStatus::Initiated,
            "pending_confirmation" => TeleportStatus::PendingConfirmation,
            "processing" => TeleportStatus::Processing,
            "minting" => TeleportStatus::Minting,
            "completed" => TeleportStatus::Completed,
            other => TeleportStatus::Failed(format!("Unknown status: {}", other)),
        };

        Ok(status)
    }

    /// Submit a consensus vote
    pub async fn submit_vote(
        &self,
        voter: &[u8],
        block_hash: [u8; 32],
        vote_type: VoteType,
        signature: &[u8],
    ) -> Result<(), LedgerError> {
        let tx = MiningTransaction::Vote {
            voter: voter.to_vec(),
            block_hash,
            vote_type,
            signature: signature.to_vec(),
        };

        self.submit_transaction(tx).await?;
        Ok(())
    }

    /// Get transaction status
    pub async fn get_transaction_status(&self, tx_hash: &str) -> Result<TransactionStatus, LedgerError> {
        let response = self.rpc_call(
            "ledger.getTransactionStatus",
            serde_json::json!({ "hash": tx_hash }),
        ).await?;

        let status_str = response.get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("unknown");

        let status = match status_str {
            "pending" => TransactionStatus::Pending,
            "mempool" => TransactionStatus::InMempool,
            "included" => {
                let height = response.get("block_height")
                    .and_then(|h| h.as_u64())
                    .unwrap_or(0);
                TransactionStatus::Included { block_height: height }
            }
            "finalized" => {
                let height = response.get("block_height")
                    .and_then(|h| h.as_u64())
                    .unwrap_or(0);
                TransactionStatus::Finalized { block_height: height }
            }
            _ => {
                let reason = response.get("reason")
                    .and_then(|r| r.as_str())
                    .unwrap_or("Unknown error")
                    .to_string();
                TransactionStatus::Failed { reason }
            }
        };

        Ok(status)
    }

    /// Subscribe to new blocks
    pub async fn subscribe_blocks(&self, callback: impl Fn(BlockHeader) + Send + Sync + 'static) {
        let connected = self.connected.clone();
        let height = self.current_height.clone();
        let rpc_url = self.config.consensus_rpc.clone();

        tokio::spawn(async move {
            let client = reqwest::Client::new();
            let mut last_height = 0u64;

            while *connected.read().await {
                // Poll for new blocks
                tokio::time::sleep(tokio::time::Duration::from_millis(BLOCK_TIME_MS)).await;

                let current = *height.read().await;
                if current > last_height {
                    // Fetch new block headers
                    if let Ok(response) = client.post(&rpc_url)
                        .json(&serde_json::json!({
                            "jsonrpc": "2.0",
                            "method": "ledger.getBlock",
                            "params": { "height": current },
                            "id": 1
                        }))
                        .send()
                        .await
                    {
                        if let Ok(json) = response.json::<serde_json::Value>().await {
                            if let Ok(header) = serde_json::from_value::<BlockHeader>(
                                json.get("result").cloned().unwrap_or_default()
                            ) {
                                callback(header);
                            }
                        }
                    }
                    last_height = current;
                }
            }
        });
    }

    /// Internal: Submit transaction to network
    async fn submit_transaction(&self, tx: MiningTransaction) -> Result<String, LedgerError> {
        let tx_bytes = serde_json::to_vec(&tx)
            .map_err(|e| LedgerError::SerializationError(e.to_string()))?;

        let tx_hash = hex::encode(blake3::hash(&tx_bytes).as_bytes());

        // Add to pending
        self.pending_txs.write().await.push((tx_hash.clone(), tx.clone()));

        // Submit to network
        let response = self.rpc_call(
            "ledger.submitTransaction",
            serde_json::json!({
                "tx": hex::encode(&tx_bytes),
            }),
        ).await?;

        let network_hash = response.get("hash")
            .and_then(|h| h.as_str())
            .unwrap_or(&tx_hash)
            .to_string();

        Ok(network_hash)
    }

    /// Internal: Make RPC call
    async fn rpc_call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, LedgerError> {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": 1
        });

        let response = self.http_client
            .post(&self.config.consensus_rpc)
            .json(&request)
            .send()
            .await
            .map_err(|e| LedgerError::NetworkError(e.to_string()))?;

        let json: serde_json::Value = response.json().await
            .map_err(|e| LedgerError::NetworkError(e.to_string()))?;

        if let Some(error) = json.get("error") {
            return Err(LedgerError::RpcError(error.to_string()));
        }

        Ok(json.get("result").cloned().unwrap_or_default())
    }

    /// Get ledger statistics
    pub async fn get_stats(&self) -> LedgerStats {
        LedgerStats {
            network: self.config.network.clone(),
            current_height: *self.current_height.read().await,
            connected: *self.connected.read().await,
            miners_cached: self.miners.read().await.len(),
            pending_txs: self.pending_txs.read().await.len(),
            quantum_safe: self.config.quantum_safe,
        }
    }
}

/// Ledger statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerStats {
    pub network: NetworkType,
    pub current_height: u64,
    pub connected: bool,
    pub miners_cached: usize,
    pub pending_txs: usize,
    pub quantum_safe: bool,
}

/// Ledger errors
#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    #[error("Network error: {0}")]
    NetworkError(String),
    #[error("RPC error: {0}")]
    RpcError(String),
    #[error("Invalid response: {0}")]
    InvalidResponse(String),
    #[error("Serialization error: {0}")]
    SerializationError(String),
    #[error("Transaction failed: {0}")]
    TransactionFailed(String),
    #[error("Not connected to network")]
    NotConnected,
    #[error("Invalid signature")]
    InvalidSignature,
    #[error("Insufficient balance")]
    InsufficientBalance,
}

/// Derive address from quantum-safe public key
pub fn derive_address_from_pubkey(pubkey: &[u8]) -> String {
    let hash = blake3::hash(pubkey);
    format!("0x{}", hex::encode(&hash.as_bytes()[..20]))
}

/// Verify a quantum-safe signature (placeholder - actual impl in hanzo-pqc)
pub fn verify_quantum_signature(pubkey: &[u8], message: &[u8], signature: &[u8]) -> bool {
    // This would use hanzo_pqc::signature::MlDsa for actual verification
    // Placeholder for now
    !pubkey.is_empty() && !message.is_empty() && !signature.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ledger_config() {
        let config = LedgerConfig::mainnet();
        assert!(config.quantum_safe);
        assert_eq!(config.security_level, 3);
        assert!(!config.bootstrap_peers.is_empty());
    }

    #[test]
    fn test_derive_address() {
        let pubkey = vec![1u8; 32];
        let address = derive_address_from_pubkey(&pubkey);
        assert!(address.starts_with("0x"));
        assert_eq!(address.len(), 42); // 0x + 40 hex chars
    }

    #[test]
    fn test_vote_types() {
        assert_eq!(VoteType::Preference, VoteType::Preference);
        assert_ne!(VoteType::Commit, VoteType::Cancel);
    }

    #[test]
    fn test_transaction_status() {
        let status = TransactionStatus::Finalized { block_height: 100 };
        match status {
            TransactionStatus::Finalized { block_height } => {
                assert_eq!(block_height, 100);
            }
            _ => panic!("Wrong status"),
        }
    }
}
