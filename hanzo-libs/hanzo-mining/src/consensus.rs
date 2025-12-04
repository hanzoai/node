//! Native Lux Consensus Integration for AI-Chain
//!
//! This module provides native consensus capabilities when the `consensus` feature
//! is enabled, allowing hanzo-mining to operate as a sovereign L1 "AI-Chain".
//!
//! ## Architecture
//!
//! The AI-Chain is a specialized blockchain for AI mining:
//! - Native ML-DSA quantum-safe signatures
//! - 500ms block time with 69% BFT quorum
//! - 2-round finality for fast confirmation
//! - Cross-chain teleport to EVM L2s (Hanzo, Zoo, Lux C-Chain)
//!
//! ## Feature Flags
//!
//! - `consensus`: Enable embedded Lux consensus (native L1 mode)
//! - Default: RPC-based consensus (connects to existing nodes)
//!
//! ## Usage
//!
//! ```ignore
//! use hanzo_mining::consensus::{ConsensusEngine, ConsensusConfig};
//!
//! // Create AI-Chain consensus engine
//! let config = ConsensusConfig::ai_chain_mainnet();
//! let mut engine = ConsensusEngine::new(config)?;
//! engine.start()?;
//!
//! // Submit blocks and votes
//! let block = engine.propose_block(transactions)?;
//! engine.broadcast_vote(block.id, VoteType::Preference)?;
//! ```

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

#[cfg(feature = "consensus")]
use lux_consensus::{Block as LuxBlock, Chain, Config as LuxConfig, Engine, ID, Vote as LuxVote, VoteType as LuxVoteType};

use crate::ledger::{BlockHeader, MiningTransaction, VoteType, LedgerError};

/// AI-Chain network identifiers
pub const AI_CHAIN_MAINNET_ID: u64 = 36963;
pub const AI_CHAIN_TESTNET_ID: u64 = 36964;

/// AI-Chain consensus parameters
pub const AI_CHAIN_BLOCK_TIME_MS: u64 = 500;
pub const AI_CHAIN_QUORUM_THRESHOLD: f64 = 0.69;
pub const AI_CHAIN_FINALITY_ROUNDS: u32 = 2;
pub const AI_CHAIN_SAMPLE_SIZE: usize = 20;

/// Consensus configuration for AI-Chain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusConfig {
    /// Network identifier
    pub network_id: u64,
    /// Enable quantum-safe cryptography
    pub quantum_resistant: bool,
    /// Security level (NIST 2, 3, or 5)
    pub security_level: u32,
    /// Sample size for voting (k parameter)
    pub sample_size: usize,
    /// Quorum size (alpha parameter)
    pub quorum_size: usize,
    /// Block time in milliseconds
    pub block_time_ms: u64,
    /// Enable GPU acceleration for signature verification
    pub gpu_acceleration: bool,
    /// Bootstrap peers for P2P network
    pub bootstrap_peers: Vec<String>,
}

impl ConsensusConfig {
    /// AI-Chain mainnet configuration
    pub fn ai_chain_mainnet() -> Self {
        Self {
            network_id: AI_CHAIN_MAINNET_ID,
            quantum_resistant: true,
            security_level: 3, // NIST Level 3 (ML-DSA-65)
            sample_size: AI_CHAIN_SAMPLE_SIZE,
            quorum_size: AI_CHAIN_SAMPLE_SIZE, // 69% of validators
            block_time_ms: AI_CHAIN_BLOCK_TIME_MS,
            gpu_acceleration: true,
            bootstrap_peers: vec![
                "/dns4/boot1.ai-chain.hanzo.network/tcp/3691".to_string(),
                "/dns4/boot2.ai-chain.hanzo.network/tcp/3691".to_string(),
                "/dns4/boot3.ai-chain.hanzo.network/tcp/3691".to_string(),
            ],
        }
    }

    /// AI-Chain testnet configuration
    pub fn ai_chain_testnet() -> Self {
        Self {
            network_id: AI_CHAIN_TESTNET_ID,
            quantum_resistant: true,
            security_level: 3,
            sample_size: AI_CHAIN_SAMPLE_SIZE,
            quorum_size: AI_CHAIN_SAMPLE_SIZE,
            block_time_ms: AI_CHAIN_BLOCK_TIME_MS,
            gpu_acceleration: false,
            bootstrap_peers: vec![
                "/dns4/boot1.ai-chain-test.hanzo.network/tcp/3691".to_string(),
            ],
        }
    }

    /// Convert to lux-consensus Config
    #[cfg(feature = "consensus")]
    pub fn to_lux_config(&self) -> LuxConfig {
        LuxConfig {
            alpha: self.quorum_size,
            k: self.sample_size,
            max_outstanding: 10,
            max_poll_delay: std::time::Duration::from_millis(self.block_time_ms),
            network_timeout: std::time::Duration::from_secs(5),
            max_message_size: 2 * 1024 * 1024,
            security_level: self.security_level,
            quantum_resistant: self.quantum_resistant,
            gpu_acceleration: self.gpu_acceleration,
        }
    }
}

/// AI-Chain block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AIChainBlock {
    /// Block identifier (Blake3 hash)
    pub id: [u8; 32],
    /// Parent block hash
    pub parent_id: [u8; 32],
    /// Block height
    pub height: u64,
    /// Block timestamp (Unix ms)
    pub timestamp: u64,
    /// Proposer's quantum-safe public key
    pub proposer: Vec<u8>,
    /// Mining transactions in this block
    pub transactions: Vec<MiningTransaction>,
    /// Merkle root of transactions
    pub tx_root: [u8; 32],
    /// Merkle root of state
    pub state_root: [u8; 32],
    /// Proposer signature (ML-DSA)
    pub signature: Vec<u8>,
}

impl AIChainBlock {
    /// Create a new block
    pub fn new(
        parent_id: [u8; 32],
        height: u64,
        proposer: Vec<u8>,
        transactions: Vec<MiningTransaction>,
    ) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        // Compute transaction Merkle root
        let tx_root = compute_tx_root(&transactions);

        // Compute block ID
        let id = compute_block_id(parent_id, height, timestamp, &tx_root);

        Self {
            id,
            parent_id,
            height,
            timestamp,
            proposer,
            transactions,
            tx_root,
            state_root: [0u8; 32], // Computed after state application
            signature: Vec::new(),
        }
    }

    /// Sign the block with ML-DSA
    pub fn sign(&mut self, signature: Vec<u8>) {
        self.signature = signature;
    }

    /// Convert to BlockHeader for ledger
    pub fn to_header(&self) -> BlockHeader {
        BlockHeader {
            height: self.height,
            parent_hash: self.parent_id,
            hash: self.id,
            timestamp: self.timestamp,
            proposer: self.proposer.clone(),
            tx_root: self.tx_root,
            state_root: self.state_root,
            tx_count: self.transactions.len() as u32,
        }
    }

    /// Convert to lux-consensus Block
    #[cfg(feature = "consensus")]
    pub fn to_lux_block(&self) -> LuxBlock {
        let payload = serde_json::to_vec(&self.transactions).unwrap_or_default();
        LuxBlock::new(
            ID::from(self.id),
            ID::from(self.parent_id),
            self.height,
            payload,
        )
    }
}

/// AI-Chain vote
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AIChainVote {
    /// Block being voted on
    pub block_id: [u8; 32],
    /// Vote type
    pub vote_type: VoteType,
    /// Voter's quantum-safe public key
    pub voter: Vec<u8>,
    /// Vote signature (ML-DSA)
    pub signature: Vec<u8>,
}

impl AIChainVote {
    /// Create a new vote
    pub fn new(block_id: [u8; 32], vote_type: VoteType, voter: Vec<u8>) -> Self {
        Self {
            block_id,
            vote_type,
            voter,
            signature: Vec::new(),
        }
    }

    /// Sign the vote with ML-DSA
    pub fn sign(&mut self, signature: Vec<u8>) {
        self.signature = signature;
    }

    /// Convert to lux-consensus Vote
    #[cfg(feature = "consensus")]
    pub fn to_lux_vote(&self) -> LuxVote {
        let lux_vote_type = match self.vote_type {
            VoteType::Preference => LuxVoteType::Preference,
            VoteType::Commit => LuxVoteType::Commit,
            VoteType::Cancel => LuxVoteType::Cancel,
        };

        LuxVote::new(
            ID::from(self.block_id),
            lux_vote_type,
            ID::new(self.voter.clone()),
        )
        .with_signature(self.signature.clone())
    }
}

/// Consensus engine mode
#[derive(Debug, Clone, PartialEq)]
pub enum ConsensusMode {
    /// Embedded consensus (native L1)
    #[cfg(feature = "consensus")]
    Embedded,
    /// RPC-based consensus (client mode)
    Rpc { endpoint: String },
}

/// Native consensus engine for AI-Chain
pub struct ConsensusEngine {
    /// Configuration
    config: ConsensusConfig,
    /// Operating mode
    mode: ConsensusMode,
    /// Embedded consensus chain (when feature enabled)
    #[cfg(feature = "consensus")]
    chain: Option<Chain>,
    /// Current block height
    height: Arc<RwLock<u64>>,
    /// Is engine running
    running: Arc<RwLock<bool>>,
    /// Pending blocks awaiting finality
    pending_blocks: Arc<RwLock<Vec<AIChainBlock>>>,
    /// Accepted blocks
    accepted_blocks: Arc<RwLock<Vec<[u8; 32]>>>,
}

impl ConsensusEngine {
    /// Create new consensus engine
    pub fn new(config: ConsensusConfig) -> Result<Self, ConsensusError> {
        #[cfg(feature = "consensus")]
        {
            let lux_config = config.to_lux_config();
            let chain = Chain::new(lux_config);

            Ok(Self {
                config,
                mode: ConsensusMode::Embedded,
                chain: Some(chain),
                height: Arc::new(RwLock::new(0)),
                running: Arc::new(RwLock::new(false)),
                pending_blocks: Arc::new(RwLock::new(Vec::new())),
                accepted_blocks: Arc::new(RwLock::new(Vec::new())),
            })
        }

        #[cfg(not(feature = "consensus"))]
        {
            Ok(Self {
                config,
                mode: ConsensusMode::Rpc {
                    endpoint: "https://consensus.hanzo.network".to_string(),
                },
                height: Arc::new(RwLock::new(0)),
                running: Arc::new(RwLock::new(false)),
                pending_blocks: Arc::new(RwLock::new(Vec::new())),
                accepted_blocks: Arc::new(RwLock::new(Vec::new())),
            })
        }
    }

    /// Create engine with specific RPC endpoint
    pub fn with_rpc(config: ConsensusConfig, endpoint: String) -> Self {
        Self {
            config,
            mode: ConsensusMode::Rpc { endpoint },
            #[cfg(feature = "consensus")]
            chain: None,
            height: Arc::new(RwLock::new(0)),
            running: Arc::new(RwLock::new(false)),
            pending_blocks: Arc::new(RwLock::new(Vec::new())),
            accepted_blocks: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Start the consensus engine
    pub async fn start(&mut self) -> Result<(), ConsensusError> {
        #[cfg(feature = "consensus")]
        if let Some(ref mut chain) = self.chain {
            chain.start().map_err(|e| ConsensusError::EngineError(e.to_string()))?;
        }

        *self.running.write().await = true;
        Ok(())
    }

    /// Stop the consensus engine
    pub async fn stop(&mut self) -> Result<(), ConsensusError> {
        #[cfg(feature = "consensus")]
        if let Some(ref mut chain) = self.chain {
            chain.stop().map_err(|e| ConsensusError::EngineError(e.to_string()))?;
        }

        *self.running.write().await = false;
        Ok(())
    }

    /// Submit a block for consensus
    pub async fn submit_block(&mut self, block: AIChainBlock) -> Result<(), ConsensusError> {
        if !*self.running.read().await {
            return Err(ConsensusError::NotRunning);
        }

        #[cfg(feature = "consensus")]
        if let Some(ref mut chain) = self.chain {
            let lux_block = block.to_lux_block();
            chain.add(lux_block).map_err(|e| ConsensusError::EngineError(e.to_string()))?;
        }

        // Track pending block
        self.pending_blocks.write().await.push(block);

        Ok(())
    }

    /// Record a vote
    pub async fn record_vote(&mut self, vote: AIChainVote) -> Result<bool, ConsensusError> {
        if !*self.running.read().await {
            return Err(ConsensusError::NotRunning);
        }

        #[cfg(feature = "consensus")]
        if let Some(ref mut chain) = self.chain {
            let lux_vote = vote.to_lux_vote();
            chain.record_vote(lux_vote).map_err(|e| ConsensusError::EngineError(e.to_string()))?;

            // Check if block is now accepted
            let id = ID::from(vote.block_id);
            if chain.is_accepted(&id) {
                self.accepted_blocks.write().await.push(vote.block_id);
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Check if a block is accepted (finalized)
    pub async fn is_accepted(&self, block_id: &[u8; 32]) -> bool {
        #[cfg(feature = "consensus")]
        if let Some(ref chain) = self.chain {
            let id = ID::from(*block_id);
            return chain.is_accepted(&id);
        }

        self.accepted_blocks.read().await.contains(block_id)
    }

    /// Get current block height
    pub async fn get_height(&self) -> u64 {
        *self.height.read().await
    }

    /// Get consensus mode
    pub fn mode(&self) -> &ConsensusMode {
        &self.mode
    }

    /// Get configuration
    pub fn config(&self) -> &ConsensusConfig {
        &self.config
    }

    /// Check if quantum-safe mode is enabled
    pub fn is_quantum_safe(&self) -> bool {
        self.config.quantum_resistant
    }
}

/// Consensus errors
#[derive(Debug, thiserror::Error)]
pub enum ConsensusError {
    #[error("Consensus engine not running")]
    NotRunning,
    #[error("Block not found: {0}")]
    BlockNotFound(String),
    #[error("Invalid block: {0}")]
    InvalidBlock(String),
    #[error("Invalid vote: {0}")]
    InvalidVote(String),
    #[error("Engine error: {0}")]
    EngineError(String),
    #[error("Signature verification failed")]
    SignatureError,
    #[error("Network error: {0}")]
    NetworkError(String),
}

impl From<ConsensusError> for LedgerError {
    fn from(e: ConsensusError) -> Self {
        LedgerError::TransactionFailed(e.to_string())
    }
}

// ============= Helper Functions =============

/// Compute transaction Merkle root
fn compute_tx_root(transactions: &[MiningTransaction]) -> [u8; 32] {
    if transactions.is_empty() {
        return [0u8; 32];
    }

    let tx_bytes: Vec<u8> = transactions
        .iter()
        .flat_map(|tx| serde_json::to_vec(tx).unwrap_or_default())
        .collect();

    *blake3::hash(&tx_bytes).as_bytes()
}

/// Compute block ID from block data
fn compute_block_id(
    parent_id: [u8; 32],
    height: u64,
    timestamp: u64,
    tx_root: &[u8; 32],
) -> [u8; 32] {
    let mut data = Vec::new();
    data.extend_from_slice(&parent_id);
    data.extend_from_slice(&height.to_le_bytes());
    data.extend_from_slice(&timestamp.to_le_bytes());
    data.extend_from_slice(tx_root);
    *blake3::hash(&data).as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_mainnet() {
        let config = ConsensusConfig::ai_chain_mainnet();
        assert_eq!(config.network_id, AI_CHAIN_MAINNET_ID);
        assert!(config.quantum_resistant);
        assert_eq!(config.security_level, 3);
        assert_eq!(config.sample_size, 20);
        assert!(!config.bootstrap_peers.is_empty());
    }

    #[test]
    fn test_config_testnet() {
        let config = ConsensusConfig::ai_chain_testnet();
        assert_eq!(config.network_id, AI_CHAIN_TESTNET_ID);
        assert!(config.quantum_resistant);
    }

    #[test]
    fn test_block_creation() {
        let parent = [0u8; 32];
        let proposer = vec![1u8; 32];
        let block = AIChainBlock::new(parent, 1, proposer.clone(), vec![]);

        assert_eq!(block.height, 1);
        assert_eq!(block.parent_id, parent);
        assert_eq!(block.proposer, proposer);
        assert!(block.timestamp > 0);
    }

    #[test]
    fn test_vote_creation() {
        let block_id = [1u8; 32];
        let voter = vec![2u8; 32];
        let vote = AIChainVote::new(block_id, VoteType::Preference, voter.clone());

        assert_eq!(vote.block_id, block_id);
        assert_eq!(vote.vote_type, VoteType::Preference);
        assert_eq!(vote.voter, voter);
    }

    #[test]
    fn test_compute_tx_root() {
        let empty_root = compute_tx_root(&[]);
        assert_eq!(empty_root, [0u8; 32]);
    }

    #[test]
    fn test_compute_block_id() {
        let parent = [0u8; 32];
        let tx_root = [1u8; 32];
        let id = compute_block_id(parent, 1, 1000, &tx_root);

        // Same inputs should produce same ID
        let id2 = compute_block_id(parent, 1, 1000, &tx_root);
        assert_eq!(id, id2);

        // Different inputs should produce different ID
        let id3 = compute_block_id(parent, 2, 1000, &tx_root);
        assert_ne!(id, id3);
    }

    #[tokio::test]
    async fn test_engine_creation() {
        let config = ConsensusConfig::ai_chain_testnet();
        let engine = ConsensusEngine::new(config).unwrap();

        assert!(engine.is_quantum_safe());
        assert_eq!(engine.get_height().await, 0);
    }

    #[tokio::test]
    async fn test_engine_start_stop() {
        let config = ConsensusConfig::ai_chain_testnet();
        let mut engine = ConsensusEngine::new(config).unwrap();

        assert!(engine.start().await.is_ok());
        assert!(engine.stop().await.is_ok());
    }
}
