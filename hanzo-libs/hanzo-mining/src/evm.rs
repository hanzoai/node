//! EVM Integration for AI Coin Mining Rewards
//!
//! AI Coin is the native currency mined on the open AI protocol (BitTorrent-style).
//! Mining rewards are earned by:
//! - Sharing training data
//! - Providing compute (GPU/CPU)
//! - Keeping AI models loaded (model hosting)
//! - Hosting specific registered models/embeddings
//!
//! Mined AI coins can be "teleported" via the Teleport Protocol to:
//! - Lux C-Chain (primary L1)
//! - Zoo EVM (chain ID 200200)
//! - Hanzo EVM (chain ID 36963)

use crate::{NetworkType, PerformanceStats};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

/// AI Mining Contract addresses per network
pub mod contracts {
    /// Hanzo EVM Mining Contract
    pub const HANZO_MAINNET_MINING: &str = "0x369000000000000000000000000000000000aAAI";
    /// Hanzo EVM Testnet Mining Contract
    pub const HANZO_TESTNET_MINING: &str = "0x369100000000000000000000000000000000aAAI";
    /// Zoo EVM Mining Contract
    pub const ZOO_MAINNET_MINING: &str = "0x200200000000000000000000000000000000aAAI";
    /// Zoo EVM Testnet Mining Contract
    pub const ZOO_TESTNET_MINING: &str = "0x200201000000000000000000000000000000aAAI";
    /// Lux C-Chain Mining Contract
    pub const LUX_MAINNET_MINING: &str = "0x4C5558000000000000000000000000000000aAAI";
    /// Lux C-Chain Testnet Mining Contract
    pub const LUX_TESTNET_MINING: &str = "0x4C5559000000000000000000000000000000aAAI";

    /// Teleport Protocol Contract - bridges AI coin to EVM chains
    pub const TELEPORT_CONTRACT: &str = "0xAI00000000000000000000000000000TELEPORT";
}

/// Lux C-Chain configuration
pub const LUX_MAINNET_CHAIN_ID: u64 = 96369;  // Lux C-Chain mainnet
pub const LUX_TESTNET_CHAIN_ID: u64 = 96368;  // Lux C-Chain testnet

/// EVM chain configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainConfig {
    pub chain_id: u64,
    pub rpc_url: String,
    pub mining_contract: String,
    pub token_symbol: String,
    pub token_decimals: u8,
    pub block_time_ms: u64,
}

impl ChainConfig {
    pub fn hanzo_mainnet() -> Self {
        Self {
            chain_id: 36963,  // Hanzo mainnet chain ID
            rpc_url: "https://rpc.hanzo.network".to_string(),
            mining_contract: contracts::HANZO_MAINNET_MINING.to_string(),
            token_symbol: "HAI".to_string(),
            token_decimals: 18,
            block_time_ms: 2000,
        }
    }

    pub fn hanzo_testnet() -> Self {
        Self {
            chain_id: 36964,  // Hanzo testnet chain ID
            rpc_url: "https://rpc.hanzo-test.network".to_string(),
            mining_contract: contracts::HANZO_TESTNET_MINING.to_string(),
            token_symbol: "HAI".to_string(),
            token_decimals: 18,
            block_time_ms: 2000,
        }
    }

    pub fn zoo_mainnet() -> Self {
        Self {
            chain_id: 200200,  // Zoo mainnet chain ID
            rpc_url: "https://rpc.zoo.network".to_string(),
            mining_contract: contracts::ZOO_MAINNET_MINING.to_string(),
            token_symbol: "ZOO".to_string(),
            token_decimals: 18,
            block_time_ms: 2000,
        }
    }

    pub fn zoo_testnet() -> Self {
        Self {
            chain_id: 200201,  // Zoo testnet chain ID
            rpc_url: "https://rpc.zoo-test.network".to_string(),
            mining_contract: contracts::ZOO_TESTNET_MINING.to_string(),
            token_symbol: "ZOO".to_string(),
            token_decimals: 18,
            block_time_ms: 2000,
        }
    }

    pub fn lux_mainnet() -> Self {
        Self {
            chain_id: LUX_MAINNET_CHAIN_ID,
            rpc_url: "https://api.lux.network/ext/bc/C/rpc".to_string(),
            mining_contract: contracts::LUX_MAINNET_MINING.to_string(),
            token_symbol: "LUX".to_string(),
            token_decimals: 18,
            block_time_ms: 2000,
        }
    }

    pub fn lux_testnet() -> Self {
        Self {
            chain_id: LUX_TESTNET_CHAIN_ID,
            rpc_url: "https://api.lux-test.network/ext/bc/C/rpc".to_string(),
            mining_contract: contracts::LUX_TESTNET_MINING.to_string(),
            token_symbol: "LUX".to_string(),
            token_decimals: 18,
            block_time_ms: 2000,
        }
    }

    pub fn from_network(network: &NetworkType) -> Self {
        match network {
            NetworkType::HanzoMainnet => Self::hanzo_mainnet(),
            NetworkType::HanzoTestnet => Self::hanzo_testnet(),
            NetworkType::ZooMainnet => Self::zoo_mainnet(),
            NetworkType::ZooTestnet => Self::zoo_testnet(),
            NetworkType::Custom(url) => Self {
                chain_id: 0,
                rpc_url: url.clone(),
                mining_contract: String::new(),
                token_symbol: "TOKEN".to_string(),
                token_decimals: 18,
                block_time_ms: 2000,
            },
        }
    }
}

/// Miner registration on-chain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinerRegistration {
    /// Miner's wallet address
    pub miner_address: String,
    /// Node's P2P peer ID
    pub peer_id: String,
    /// GPU compute power (TFLOPS)
    pub gpu_tflops: f32,
    /// CPU compute power (GFLOPS)
    pub cpu_gflops: f32,
    /// Available VRAM (GB)
    pub vram_gb: f32,
    /// Available RAM (GB)
    pub ram_gb: f32,
    /// Network bandwidth (Mbps)
    pub bandwidth_mbps: f32,
    /// Supported AI capabilities
    pub capabilities: Vec<MinerCapability>,
    /// Registration timestamp
    pub registered_at: u64,
    /// Last heartbeat timestamp
    pub last_heartbeat: u64,
    /// Total jobs completed
    pub jobs_completed: u64,
    /// Reputation score (0-100)
    pub reputation: u64,
}

/// AI compute capabilities a miner can offer
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MinerCapability {
    /// Text embeddings generation
    Embedding,
    /// Document reranking
    Reranking,
    /// LLM inference
    Inference,
    /// Model fine-tuning
    Training,
    /// GGUF quantization
    Quantization,
    /// Model storage/serving
    Storage,
    /// Custom compute
    Custom(String),
}

/// How AI coins are earned in the protocol
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MiningRewardType {
    /// Rewards for sharing training data
    DataSharing {
        dataset_id: String,
        bytes_shared: u64,
    },
    /// Rewards for providing GPU/CPU compute
    ComputeProvision {
        job_id: String,
        compute_units: u64,
    },
    /// Rewards for keeping models loaded/hosted
    ModelHosting {
        model_id: String,
        hosting_hours: f64,
    },
    /// Rewards for specific model/embedding registration
    ModelRegistration {
        model_hash: String,
        model_type: String,
    },
    /// Rewards for inference serving
    InferenceServing {
        model_id: String,
        tokens_served: u64,
    },
}

/// Destination chain for teleporting AI coins
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum TeleportDestination {
    /// Lux C-Chain (primary L1)
    LuxCChain,
    /// Zoo EVM
    ZooEvm,
    /// Hanzo EVM
    HanzoEvm,
}

impl TeleportDestination {
    pub fn chain_id(&self) -> u64 {
        match self {
            Self::LuxCChain => LUX_MAINNET_CHAIN_ID,
            Self::ZooEvm => 200200,
            Self::HanzoEvm => 36963,
        }
    }

    pub fn rpc_url(&self) -> &str {
        match self {
            Self::LuxCChain => "https://api.lux.network/ext/bc/C/rpc",
            Self::ZooEvm => "https://rpc.zoo.network",
            Self::HanzoEvm => "https://rpc.hanzo.network",
        }
    }
}

/// Teleport transfer - bridging AI coins from protocol to EVM chains
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeleportTransfer {
    /// Unique teleport ID
    pub teleport_id: String,
    /// Amount of AI coin to teleport (in wei)
    pub amount: u128,
    /// Source address (from AI protocol)
    pub from_address: String,
    /// Destination address (on EVM chain)
    pub to_address: String,
    /// Destination chain
    pub destination: TeleportDestination,
    /// Transfer status
    pub status: TeleportStatus,
    /// Protocol transaction hash
    pub protocol_tx: Option<String>,
    /// EVM transaction hash
    pub evm_tx: Option<String>,
    /// Timestamp initiated
    pub initiated_at: u64,
    /// Timestamp completed
    pub completed_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TeleportStatus {
    /// Transfer initiated on AI protocol
    Initiated,
    /// Waiting for protocol confirmations
    PendingConfirmation,
    /// Transfer being processed by relayers
    Processing,
    /// Minting on destination chain
    Minting,
    /// Transfer complete
    Completed,
    /// Transfer failed
    Failed(String),
}

/// Pending reward that can be claimed
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingReward {
    /// Job ID that generated this reward
    pub job_id: String,
    /// Amount in wei (1e18 = 1 token)
    pub amount: u128,
    /// Network where reward is claimable
    pub network: NetworkType,
    /// Block number when reward was allocated
    pub block_number: u64,
    /// Merkle proof for claiming (if applicable)
    pub proof: Option<Vec<String>>,
    /// Whether reward has been claimed
    pub claimed: bool,
}

/// Cross-chain bridge transfer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeTransfer {
    /// Unique transfer ID
    pub transfer_id: String,
    /// Source chain
    pub from_chain: NetworkType,
    /// Destination chain
    pub to_chain: NetworkType,
    /// Amount being bridged
    pub amount: u128,
    /// Sender address
    pub sender: String,
    /// Recipient address
    pub recipient: String,
    /// Transfer status
    pub status: BridgeStatus,
    /// Source chain tx hash
    pub source_tx: Option<String>,
    /// Destination chain tx hash
    pub dest_tx: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum BridgeStatus {
    Pending,
    SourceConfirmed,
    Bridging,
    DestConfirmed,
    Completed,
    Failed,
}

/// EVM client for interacting with Hanzo/Zoo chains
pub struct EvmClient {
    config: ChainConfig,
    http_client: reqwest::Client,
}

impl EvmClient {
    pub fn new(config: ChainConfig) -> Self {
        Self {
            config,
            http_client: reqwest::Client::new(),
        }
    }

    pub fn from_network(network: &NetworkType) -> Self {
        Self::new(ChainConfig::from_network(network))
    }

    /// Get current block number
    pub async fn get_block_number(&self) -> Result<u64, EvmError> {
        let result = self.rpc_call("eth_blockNumber", serde_json::json!([])).await?;
        let hex_str = result.as_str().ok_or(EvmError::InvalidResponse)?;
        let block = u64::from_str_radix(hex_str.trim_start_matches("0x"), 16)
            .map_err(|_| EvmError::InvalidResponse)?;
        Ok(block)
    }

    /// Get native token balance
    pub async fn get_balance(&self, address: &str) -> Result<u128, EvmError> {
        let result = self.rpc_call(
            "eth_getBalance",
            serde_json::json!([address, "latest"]),
        ).await?;
        let hex_str = result.as_str().ok_or(EvmError::InvalidResponse)?;
        let balance = u128::from_str_radix(hex_str.trim_start_matches("0x"), 16)
            .map_err(|_| EvmError::InvalidResponse)?;
        Ok(balance)
    }

    /// Get pending rewards for a miner
    pub async fn get_pending_rewards(&self, miner_address: &str) -> Result<u128, EvmError> {
        // Call pendingRewards(address) on mining contract
        let data = encode_function_call("pendingRewards(address)", &[miner_address]);
        let result = self.eth_call(&self.config.mining_contract, &data).await?;

        let hex_str = result.as_str().ok_or(EvmError::InvalidResponse)?;
        let rewards = u128::from_str_radix(hex_str.trim_start_matches("0x"), 16)
            .unwrap_or(0);
        Ok(rewards)
    }

    /// Register as a miner on the network
    pub async fn register_miner(
        &self,
        private_key: &str,
        stats: &PerformanceStats,
        capabilities: &[MinerCapability],
    ) -> Result<String, EvmError> {
        // Encode registerMiner(uint256 gpuTflops, uint256 cpuGflops, uint256 vram, uint256 ram, bytes capabilities)
        let caps_encoded = encode_capabilities(capabilities);
        let data = encode_function_call(
            "registerMiner(uint256,uint256,uint256,uint256,bytes)",
            &[
                &format!("{}", (stats.gpu_tflops * 1000.0) as u64),
                &format!("{}", (stats.cpu_gflops * 1000.0) as u64),
                &format!("{}", (stats.vram_gb * 1000.0) as u64),
                &format!("{}", (stats.ram_gb * 1000.0) as u64),
                &caps_encoded,
            ],
        );

        self.send_transaction(private_key, &self.config.mining_contract, &data, 0).await
    }

    /// Claim pending rewards
    pub async fn claim_rewards(&self, private_key: &str) -> Result<String, EvmError> {
        let data = encode_function_call("claimRewards()", &[]);
        self.send_transaction(private_key, &self.config.mining_contract, &data, 0).await
    }

    /// Submit job completion proof
    pub async fn submit_job_completion(
        &self,
        private_key: &str,
        job_id: &str,
        result_hash: &str,
    ) -> Result<String, EvmError> {
        let data = encode_function_call(
            "submitJobCompletion(bytes32,bytes32)",
            &[job_id, result_hash],
        );
        self.send_transaction(private_key, &self.config.mining_contract, &data, 0).await
    }

    /// Send heartbeat to maintain miner registration
    pub async fn send_heartbeat(&self, private_key: &str) -> Result<String, EvmError> {
        let data = encode_function_call("heartbeat()", &[]);
        self.send_transaction(private_key, &self.config.mining_contract, &data, 0).await
    }

    /// Initiate cross-chain bridge transfer
    pub async fn bridge_tokens(
        &self,
        private_key: &str,
        dest_chain: &NetworkType,
        amount: u128,
        recipient: &str,
    ) -> Result<String, EvmError> {
        let dest_chain_id = match dest_chain {
            NetworkType::HanzoMainnet => 36963u64,
            NetworkType::HanzoTestnet => 36964,
            NetworkType::ZooMainnet => 200200,
            NetworkType::ZooTestnet => 200201,
            NetworkType::Custom(_) => return Err(EvmError::UnsupportedChain),
        };

        let data = encode_function_call(
            "bridgeTokens(uint256,address,uint256)",
            &[
                &dest_chain_id.to_string(),
                recipient,
                &amount.to_string(),
            ],
        );
        self.send_transaction(private_key, contracts::TELEPORT_CONTRACT, &data, amount).await
    }

    /// Internal: Make RPC call
    async fn rpc_call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, EvmError> {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": 1
        });

        let response = self.http_client
            .post(&self.config.rpc_url)
            .json(&request)
            .send()
            .await
            .map_err(|e| EvmError::RpcError(e.to_string()))?;

        let json: serde_json::Value = response.json().await
            .map_err(|e| EvmError::RpcError(e.to_string()))?;

        if let Some(error) = json.get("error") {
            return Err(EvmError::RpcError(error.to_string()));
        }

        json.get("result")
            .cloned()
            .ok_or(EvmError::InvalidResponse)
    }

    /// Internal: eth_call
    async fn eth_call(&self, to: &str, data: &str) -> Result<serde_json::Value, EvmError> {
        self.rpc_call(
            "eth_call",
            serde_json::json!([
                {"to": to, "data": data},
                "latest"
            ]),
        ).await
    }

    /// Internal: Send transaction
    async fn send_transaction(
        &self,
        _private_key: &str,
        to: &str,
        data: &str,
        value: u128,
    ) -> Result<String, EvmError> {
        // In production, this would:
        // 1. Get nonce
        // 2. Estimate gas
        // 3. Build transaction
        // 4. Sign with private key
        // 5. Send via eth_sendRawTransaction

        // For now, return placeholder
        let _tx = serde_json::json!({
            "to": to,
            "data": data,
            "value": format!("0x{:x}", value),
            "chainId": format!("0x{:x}", self.config.chain_id),
        });

        // TODO: Implement actual transaction signing and sending
        Ok(format!("0x{:064x}", rand::random::<u64>()))
    }
}

/// Rewards manager for tracking and claiming rewards across chains
pub struct RewardsManager {
    /// Primary network for mining
    primary_network: NetworkType,
    /// EVM client for primary network
    primary_client: EvmClient,
    /// Optional secondary network (for cross-chain)
    secondary_client: Option<EvmClient>,
    /// Pending rewards
    pending_rewards: Arc<RwLock<Vec<PendingReward>>>,
    /// Total claimed rewards (in wei)
    total_claimed: Arc<RwLock<u128>>,
    /// Bridge transfers in progress
    bridge_transfers: Arc<RwLock<Vec<BridgeTransfer>>>,
}

impl RewardsManager {
    pub fn new(primary_network: NetworkType) -> Self {
        let primary_client = EvmClient::from_network(&primary_network);

        // Set up secondary client for cross-chain
        let secondary_client = match &primary_network {
            NetworkType::HanzoMainnet => Some(EvmClient::from_network(&NetworkType::ZooMainnet)),
            NetworkType::HanzoTestnet => Some(EvmClient::from_network(&NetworkType::ZooTestnet)),
            NetworkType::ZooMainnet => Some(EvmClient::from_network(&NetworkType::HanzoMainnet)),
            NetworkType::ZooTestnet => Some(EvmClient::from_network(&NetworkType::HanzoTestnet)),
            NetworkType::Custom(_) => None,
        };

        Self {
            primary_network,
            primary_client,
            secondary_client,
            pending_rewards: Arc::new(RwLock::new(Vec::new())),
            total_claimed: Arc::new(RwLock::new(0)),
            bridge_transfers: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Check and update pending rewards
    pub async fn refresh_pending_rewards(&self, miner_address: &str) -> Result<u128, EvmError> {
        let primary_rewards = self.primary_client.get_pending_rewards(miner_address).await?;

        let secondary_rewards = if let Some(client) = &self.secondary_client {
            client.get_pending_rewards(miner_address).await.unwrap_or(0)
        } else {
            0
        };

        Ok(primary_rewards + secondary_rewards)
    }

    /// Claim rewards from primary network
    pub async fn claim_primary_rewards(&self, private_key: &str) -> Result<String, EvmError> {
        let tx_hash = self.primary_client.claim_rewards(private_key).await?;
        Ok(tx_hash)
    }

    /// Claim rewards from secondary network
    pub async fn claim_secondary_rewards(&self, private_key: &str) -> Result<Option<String>, EvmError> {
        if let Some(client) = &self.secondary_client {
            let tx_hash = client.claim_rewards(private_key).await?;
            Ok(Some(tx_hash))
        } else {
            Ok(None)
        }
    }

    /// Claim rewards from all networks
    pub async fn claim_all_rewards(&self, private_key: &str) -> Result<Vec<String>, EvmError> {
        let mut tx_hashes = Vec::new();

        // Claim primary
        let primary_tx = self.primary_client.claim_rewards(private_key).await?;
        tx_hashes.push(primary_tx);

        // Claim secondary if available
        if let Some(client) = &self.secondary_client {
            if let Ok(tx) = client.claim_rewards(private_key).await {
                tx_hashes.push(tx);
            }
        }

        Ok(tx_hashes)
    }

    /// Bridge tokens from primary to secondary network
    pub async fn bridge_to_secondary(
        &self,
        private_key: &str,
        amount: u128,
        recipient: &str,
    ) -> Result<String, EvmError> {
        let dest_chain = match &self.primary_network {
            NetworkType::HanzoMainnet => NetworkType::ZooMainnet,
            NetworkType::HanzoTestnet => NetworkType::ZooTestnet,
            NetworkType::ZooMainnet => NetworkType::HanzoMainnet,
            NetworkType::ZooTestnet => NetworkType::HanzoTestnet,
            NetworkType::Custom(_) => return Err(EvmError::UnsupportedChain),
        };

        self.primary_client.bridge_tokens(private_key, &dest_chain, amount, recipient).await
    }

    /// Get total balance across all networks
    pub async fn get_total_balance(&self, address: &str) -> Result<u128, EvmError> {
        let primary_balance = self.primary_client.get_balance(address).await?;

        let secondary_balance = if let Some(client) = &self.secondary_client {
            client.get_balance(address).await.unwrap_or(0)
        } else {
            0
        };

        Ok(primary_balance + secondary_balance)
    }

    /// Get rewards summary
    pub async fn get_rewards_summary(&self, address: &str) -> Result<RewardsSummary, EvmError> {
        let primary_pending = self.primary_client.get_pending_rewards(address).await?;
        let primary_balance = self.primary_client.get_balance(address).await?;

        let (secondary_pending, secondary_balance) = if let Some(client) = &self.secondary_client {
            let pending = client.get_pending_rewards(address).await.unwrap_or(0);
            let balance = client.get_balance(address).await.unwrap_or(0);
            (pending, balance)
        } else {
            (0, 0)
        };

        Ok(RewardsSummary {
            primary_network: self.primary_network.clone(),
            primary_pending,
            primary_balance,
            secondary_pending,
            secondary_balance,
            total_pending: primary_pending + secondary_pending,
            total_balance: primary_balance + secondary_balance,
            total_claimed: *self.total_claimed.read().await,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewardsSummary {
    pub primary_network: NetworkType,
    pub primary_pending: u128,
    pub primary_balance: u128,
    pub secondary_pending: u128,
    pub secondary_balance: u128,
    pub total_pending: u128,
    pub total_balance: u128,
    pub total_claimed: u128,
}

impl RewardsSummary {
    /// Format amounts for display (convert wei to token units)
    pub fn format_primary_pending(&self) -> f64 {
        self.primary_pending as f64 / 1e18
    }

    pub fn format_total_balance(&self) -> f64 {
        self.total_balance as f64 / 1e18
    }
}

/// EVM-related errors
#[derive(Debug, thiserror::Error)]
pub enum EvmError {
    #[error("RPC error: {0}")]
    RpcError(String),
    #[error("Invalid response from RPC")]
    InvalidResponse,
    #[error("Transaction failed: {0}")]
    TransactionFailed(String),
    #[error("Insufficient balance")]
    InsufficientBalance,
    #[error("Unsupported chain for this operation")]
    UnsupportedChain,
    #[error("Contract error: {0}")]
    ContractError(String),
}

// Helper functions for ABI encoding (simplified)
fn encode_function_call(signature: &str, _params: &[&str]) -> String {
    // In production, use ethers-rs or alloy for proper ABI encoding
    // For now, return a placeholder
    let selector = keccak256_selector(signature);
    format!("0x{}", selector)
}

fn keccak256_selector(signature: &str) -> String {
    // Simplified: return first 8 hex chars of hash
    // In production, use actual keccak256
    let hash = blake3::hash(signature.as_bytes());
    hex::encode(&hash.as_bytes()[..4])
}

fn encode_capabilities(capabilities: &[MinerCapability]) -> String {
    // Encode capabilities as bytes
    let encoded: Vec<u8> = capabilities.iter().map(|c| {
        match c {
            MinerCapability::Embedding => 0x01,
            MinerCapability::Reranking => 0x02,
            MinerCapability::Inference => 0x03,
            MinerCapability::Training => 0x04,
            MinerCapability::Quantization => 0x05,
            MinerCapability::Storage => 0x06,
            MinerCapability::Custom(_) => 0xFF,
        }
    }).collect();
    hex::encode(&encoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chain_configs() {
        let hanzo = ChainConfig::hanzo_mainnet();
        assert_eq!(hanzo.chain_id, 36963);
        assert_eq!(hanzo.token_symbol, "HAI");

        let zoo = ChainConfig::zoo_mainnet();
        assert_eq!(zoo.chain_id, 200200);
        assert_eq!(zoo.token_symbol, "ZOO");
    }

    #[test]
    fn test_network_to_config() {
        let config = ChainConfig::from_network(&NetworkType::HanzoMainnet);
        assert_eq!(config.chain_id, 36963);

        let config = ChainConfig::from_network(&NetworkType::ZooMainnet);
        assert_eq!(config.chain_id, 200200);
    }

    #[test]
    fn test_encode_capabilities() {
        let caps = vec![
            MinerCapability::Embedding,
            MinerCapability::Inference,
        ];
        let encoded = encode_capabilities(&caps);
        assert_eq!(encoded, "0103"); // 0x01 for Embedding, 0x03 for Inference
    }

    #[tokio::test]
    async fn test_rewards_manager_creation() {
        let manager = RewardsManager::new(NetworkType::HanzoMainnet);
        assert!(manager.secondary_client.is_some());

        let manager = RewardsManager::new(NetworkType::ZooMainnet);
        assert!(manager.secondary_client.is_some());
    }

    #[test]
    fn test_rewards_summary_formatting() {
        let summary = RewardsSummary {
            primary_network: NetworkType::HanzoMainnet,
            primary_pending: 1_000_000_000_000_000_000, // 1 token
            primary_balance: 10_000_000_000_000_000_000, // 10 tokens
            secondary_pending: 500_000_000_000_000_000, // 0.5 tokens
            secondary_balance: 5_000_000_000_000_000_000, // 5 tokens
            total_pending: 1_500_000_000_000_000_000,
            total_balance: 15_000_000_000_000_000_000,
            total_claimed: 100_000_000_000_000_000_000,
        };

        assert!((summary.format_primary_pending() - 1.0).abs() < 0.001);
        assert!((summary.format_total_balance() - 15.0).abs() < 0.001);
    }
}
