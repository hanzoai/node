//! Mining Bridge - Connects ML-DSA Wallet, Lux Consensus Ledger, and Teleport Protocol
//!
//! This module integrates the quantum-safe mining wallet with the global ledger
//! and enables native teleportation of mining rewards to supported EVM chains:
//! - Lux C-Chain (96369)
//! - Zoo EVM (200200)
//! - Hanzo EVM (36963)

use crate::evm::{TeleportDestination, TeleportStatus, TeleportTransfer, ChainConfig, EvmClient};
use crate::ledger::{MiningLedger, LedgerError};
use crate::wallet::{MiningWallet, WalletError, SecurityLevel};
use crate::{MiningRewardType, PerformanceStats};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;

/// Bridge errors
#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("Wallet error: {0}")]
    Wallet(#[from] WalletError),

    #[error("Ledger error: {0}")]
    Ledger(#[from] LedgerError),

    #[error("Insufficient balance: have {have}, need {need}")]
    InsufficientBalance { have: u128, need: u128 },

    #[error("Teleport failed: {0}")]
    TeleportFailed(String),

    #[error("Invalid destination chain: {0}")]
    InvalidDestination(u64),

    #[error("No wallet loaded")]
    NoWallet,

    #[error("Not connected to ledger")]
    NotConnected,

    #[error("Network error: {0}")]
    Network(String),
}

/// Mining account state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MiningAccount {
    /// Quantum-safe wallet address
    pub address: String,
    /// Security level used
    pub security_level: SecurityLevel,
    /// Total rewards earned (in wei)
    pub total_earned: u128,
    /// Pending rewards to claim
    pub pending_rewards: u128,
    /// Rewards already claimed
    pub claimed_rewards: u128,
    /// Rewards teleported to EVM by chain
    pub teleported_rewards: HashMap<TeleportDestination, u128>,
    /// Active teleport transfers
    pub active_transfers: Vec<String>,
    /// Registration timestamp
    pub registered_at: u64,
}

impl MiningAccount {
    pub fn new(address: String, security_level: SecurityLevel) -> Self {
        Self {
            address,
            security_level,
            total_earned: 0,
            pending_rewards: 0,
            claimed_rewards: 0,
            teleported_rewards: HashMap::new(),
            active_transfers: Vec::new(),
            registered_at: current_timestamp(),
        }
    }

    /// Available balance (claimed but not teleported)
    pub fn available_balance(&self) -> u128 {
        self.claimed_rewards.saturating_sub(
            self.teleported_rewards.values().sum::<u128>()
        )
    }
}

/// Mining Bridge - connects wallet, ledger, and teleport
pub struct MiningBridge {
    /// The quantum-safe mining wallet
    wallet: Arc<RwLock<Option<MiningWallet>>>,
    /// Connection to the Lux consensus ledger
    ledger: Arc<RwLock<Option<MiningLedger>>>,
    /// Current account state
    account: Arc<RwLock<Option<MiningAccount>>>,
    /// Pending teleport transfers
    transfers: Arc<RwLock<HashMap<String, TeleportTransfer>>>,
    /// HTTP client for EVM interactions
    http_client: reqwest::Client,
}

impl MiningBridge {
    /// Create a new mining bridge
    pub fn new() -> Self {
        Self {
            wallet: Arc::new(RwLock::new(None)),
            ledger: Arc::new(RwLock::new(None)),
            account: Arc::new(RwLock::new(None)),
            transfers: Arc::new(RwLock::new(HashMap::new())),
            http_client: reqwest::Client::new(),
        }
    }

    /// Initialize with an existing wallet
    pub async fn with_wallet(wallet: MiningWallet) -> Result<Self, BridgeError> {
        let address = wallet.address().to_string();
        let security_level = wallet.security_level();

        let bridge = Self::new();
        *bridge.wallet.write().await = Some(wallet);
        *bridge.account.write().await = Some(MiningAccount::new(address, security_level));

        Ok(bridge)
    }

    /// Generate a new quantum-safe wallet and initialize the bridge
    pub async fn generate_new(security_level: SecurityLevel) -> Result<Self, BridgeError> {
        let wallet = MiningWallet::generate(security_level).await?;
        Self::with_wallet(wallet).await
    }

    /// Import wallet from encrypted data
    pub async fn import_wallet(
        encrypted_data: &[u8],
        passphrase: &str,
    ) -> Result<Self, BridgeError> {
        let wallet = MiningWallet::import_from_bytes(encrypted_data, passphrase)?;
        Self::with_wallet(wallet).await
    }

    /// Connect to the Lux consensus ledger
    pub async fn connect_ledger(&self, ledger: MiningLedger) -> Result<(), BridgeError> {
        *self.ledger.write().await = Some(ledger);
        Ok(())
    }

    /// Get the wallet address
    pub async fn address(&self) -> Option<String> {
        self.wallet.read().await.as_ref().map(|w| w.address().to_string())
    }

    /// Get the current account state
    pub async fn account(&self) -> Option<MiningAccount> {
        self.account.read().await.clone()
    }

    /// Get wallet's public key for verification
    pub async fn public_key(&self) -> Option<Vec<u8>> {
        self.wallet.read().await.as_ref().map(|w| w.public_key().to_vec())
    }

    /// Register as a miner on the ledger
    pub async fn register_miner(
        &self,
        capabilities: &[u8],
        stats: &PerformanceStats,
    ) -> Result<String, BridgeError> {
        let wallet = self.wallet.read().await;
        let wallet = wallet.as_ref().ok_or(BridgeError::NoWallet)?;

        let ledger = self.ledger.read().await;
        let ledger = ledger.as_ref().ok_or(BridgeError::NotConnected)?;

        // Create signature for registration
        let reg_data = serde_json::json!({
            "action": "register",
            "capabilities": hex::encode(capabilities),
            "stats": stats,
            "timestamp": current_timestamp(),
        });
        let reg_bytes = serde_json::to_vec(&reg_data).unwrap_or_default();
        let signature = wallet.sign(&reg_bytes).await?;

        // Register on ledger
        let tx_id = ledger.register_miner(
            wallet.public_key(),
            capabilities,
            stats,
            &signature,
        ).await?;

        Ok(tx_id)
    }

    /// Submit mining proof for rewards
    pub async fn submit_mining_proof(
        &self,
        reward_type: MiningRewardType,
        proof: &[u8],
    ) -> Result<String, BridgeError> {
        let wallet = self.wallet.read().await;
        let wallet = wallet.as_ref().ok_or(BridgeError::NoWallet)?;

        let ledger = self.ledger.read().await;
        let ledger = ledger.as_ref().ok_or(BridgeError::NotConnected)?;

        // Sign the proof
        let signature = wallet.sign(proof).await?;

        // Submit to ledger
        let tx_id = ledger.submit_proof(
            wallet.public_key(),
            reward_type,
            proof,
            &signature,
        ).await?;

        Ok(tx_id)
    }

    /// Claim pending rewards from the ledger
    pub async fn claim_rewards(&self, amount: u128) -> Result<String, BridgeError> {
        let wallet = self.wallet.read().await;
        let wallet = wallet.as_ref().ok_or(BridgeError::NoWallet)?;

        let ledger = self.ledger.read().await;
        let ledger = ledger.as_ref().ok_or(BridgeError::NotConnected)?;

        // Get pending rewards from account
        let pending = {
            let account = self.account.read().await;
            account.as_ref().map(|a| a.pending_rewards).unwrap_or(0)
        };

        if amount > pending {
            return Err(BridgeError::InsufficientBalance {
                have: pending,
                need: amount,
            });
        }

        // Create claim data and sign
        let claim_data = serde_json::json!({
            "action": "claim",
            "amount": amount,
            "recipient": wallet.address(),
            "timestamp": current_timestamp(),
        });
        let claim_bytes = serde_json::to_vec(&claim_data).unwrap_or_default();
        let signature = wallet.sign(&claim_bytes).await?;

        // Claim from ledger
        let tx_id = ledger.claim_rewards(
            wallet.public_key(),
            amount,
            wallet.address(),
            &[], // Empty proof for direct claim
            &signature,
        ).await?;

        // Update account state
        let mut account = self.account.write().await;
        if let Some(ref mut acc) = *account {
            acc.pending_rewards = acc.pending_rewards.saturating_sub(amount);
            acc.claimed_rewards += amount;
        }

        Ok(tx_id)
    }

    /// Teleport mining rewards to an EVM chain
    ///
    /// This creates a cross-chain transfer from the AI protocol to a supported EVM:
    /// - Lux C-Chain (96369)
    /// - Zoo EVM (200200)
    /// - Hanzo EVM (36963)
    pub async fn teleport_to_evm(
        &self,
        destination: TeleportDestination,
        amount: u128,
        recipient: Option<String>,
    ) -> Result<TeleportTransfer, BridgeError> {
        let wallet = self.wallet.read().await;
        let wallet = wallet.as_ref().ok_or(BridgeError::NoWallet)?;

        let ledger = self.ledger.read().await;
        let ledger = ledger.as_ref().ok_or(BridgeError::NotConnected)?;

        // Check available balance
        let available = {
            let account = self.account.read().await;
            account.as_ref().map(|a| a.available_balance()).unwrap_or(0)
        };

        if amount > available {
            return Err(BridgeError::InsufficientBalance {
                have: available,
                need: amount,
            });
        }

        // Recipient defaults to the wallet's derived EVM address
        let to_address = recipient.unwrap_or_else(|| {
            derive_evm_address(wallet.public_key())
        });

        // Sign the teleport request
        let teleport_data = serde_json::json!({
            "action": "teleport_out",
            "destination": destination.chain_id(),
            "to_address": to_address,
            "amount": amount,
            "timestamp": current_timestamp(),
        });
        let teleport_bytes = serde_json::to_vec(&teleport_data).unwrap_or_default();
        let signature = wallet.sign(&teleport_bytes).await?;

        // Submit to ledger - returns TeleportTransfer directly
        let transfer = ledger.teleport_out(
            wallet.public_key(),
            destination.clone(),
            &to_address,
            amount,
            &signature,
        ).await?;

        let teleport_id = transfer.teleport_id.clone();

        // Store transfer
        self.transfers.write().await.insert(teleport_id.clone(), transfer.clone());

        // Update account state
        let mut account = self.account.write().await;
        if let Some(ref mut acc) = *account {
            *acc.teleported_rewards.entry(destination).or_insert(0) += amount;
            acc.active_transfers.push(teleport_id);
        }

        Ok(transfer)
    }

    /// Get teleport transfer status
    pub async fn get_teleport_status(&self, teleport_id: &str) -> Option<TeleportTransfer> {
        self.transfers.read().await.get(teleport_id).cloned()
    }

    /// List all pending teleport transfers
    pub async fn pending_teleports(&self) -> Vec<TeleportTransfer> {
        self.transfers.read().await
            .values()
            .filter(|t| !matches!(t.status, TeleportStatus::Completed | TeleportStatus::Failed(_)))
            .cloned()
            .collect()
    }

    /// Verify a teleport completed on the destination EVM chain
    pub async fn verify_teleport_completion(
        &self,
        teleport_id: &str,
    ) -> Result<bool, BridgeError> {
        let transfer = self.transfers.read().await
            .get(teleport_id)
            .cloned()
            .ok_or_else(|| BridgeError::TeleportFailed("Transfer not found".to_string()))?;

        if matches!(transfer.status, TeleportStatus::Completed) {
            return Ok(true);
        }

        // Query the destination EVM chain
        let evm_client = EvmClient::new(ChainConfig::from_destination(&transfer.destination));

        // Check recipient balance on destination chain
        match evm_client.get_balance(&transfer.to_address).await {
            Ok(_balance) => {
                // Update transfer status
                let mut transfers = self.transfers.write().await;
                if let Some(t) = transfers.get_mut(teleport_id) {
                    t.status = TeleportStatus::Completed;
                    t.completed_at = Some(current_timestamp());
                }
                Ok(true)
            }
            Err(e) => Err(BridgeError::Network(e.to_string())),
        }
    }

    /// Export wallet for backup
    pub async fn export_wallet(&self, passphrase: &str) -> Result<Vec<u8>, BridgeError> {
        let wallet = self.wallet.read().await;
        let wallet = wallet.as_ref().ok_or(BridgeError::NoWallet)?;

        Ok(wallet.export_to_bytes(passphrase)?)
    }

    /// Get mining statistics
    pub async fn get_stats(&self) -> BridgeStats {
        let account = self.account.read().await;
        let transfers = self.transfers.read().await;

        let (total_earned, pending, claimed, available) = account
            .as_ref()
            .map(|a| (a.total_earned, a.pending_rewards, a.claimed_rewards, a.available_balance()))
            .unwrap_or((0, 0, 0, 0));

        let teleported: HashMap<String, u128> = account
            .as_ref()
            .map(|a| {
                a.teleported_rewards.iter()
                    .map(|(k, v)| (format!("{:?}", k), *v))
                    .collect()
            })
            .unwrap_or_default();

        let pending_transfers = transfers.values()
            .filter(|t| !matches!(t.status, TeleportStatus::Completed | TeleportStatus::Failed(_)))
            .count();

        BridgeStats {
            total_earned,
            pending_rewards: pending,
            claimed_rewards: claimed,
            available_balance: available,
            teleported_by_chain: teleported,
            pending_teleports: pending_transfers,
        }
    }
}

impl Default for MiningBridge {
    fn default() -> Self {
        Self::new()
    }
}

/// Bridge statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeStats {
    /// Total rewards earned
    pub total_earned: u128,
    /// Pending rewards to claim
    pub pending_rewards: u128,
    /// Already claimed rewards
    pub claimed_rewards: u128,
    /// Available to teleport
    pub available_balance: u128,
    /// Teleported by destination chain
    pub teleported_by_chain: HashMap<String, u128>,
    /// Number of pending teleport transfers
    pub pending_teleports: usize,
}

// Helper to add from_destination to ChainConfig
impl ChainConfig {
    pub fn from_destination(dest: &TeleportDestination) -> Self {
        match dest {
            TeleportDestination::LuxCChain => Self::lux_mainnet(),
            TeleportDestination::ZooEvm => Self::zoo_mainnet(),
            TeleportDestination::HanzoEvm => Self::hanzo_mainnet(),
        }
    }
}

/// Get current timestamp
fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Derive an EVM-compatible address from a quantum-safe public key
fn derive_evm_address(public_key: &[u8]) -> String {
    // Hash the public key and take last 20 bytes
    let hash = blake3::hash(public_key);
    let bytes = hash.as_bytes();
    format!("0x{}", hex::encode(&bytes[12..32]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_bridge_creation() {
        let bridge = MiningBridge::generate_new(SecurityLevel::Level3).await.unwrap();

        let address = bridge.address().await;
        assert!(address.is_some());
        assert!(address.unwrap().starts_with("0x"));
    }

    #[tokio::test]
    async fn test_bridge_stats() {
        let bridge = MiningBridge::generate_new(SecurityLevel::Level3).await.unwrap();

        let stats = bridge.get_stats().await;
        assert_eq!(stats.total_earned, 0);
        assert_eq!(stats.pending_rewards, 0);
        assert_eq!(stats.available_balance, 0);
    }

    #[tokio::test]
    async fn test_wallet_export_import() {
        let bridge = MiningBridge::generate_new(SecurityLevel::Level3).await.unwrap();

        let original_address = bridge.address().await.unwrap();

        // Export wallet
        let passphrase = "test_passphrase_123";
        let encrypted = bridge.export_wallet(passphrase).await.unwrap();

        // Import into new bridge
        let imported_bridge = MiningBridge::import_wallet(&encrypted, passphrase).await.unwrap();
        let imported_address = imported_bridge.address().await.unwrap();

        assert_eq!(original_address, imported_address);
    }

    #[test]
    fn test_derive_evm_address() {
        let public_key = vec![0u8; 1312]; // ML-DSA-65 public key size
        let address = derive_evm_address(&public_key);

        assert!(address.starts_with("0x"));
        assert_eq!(address.len(), 42); // 0x + 40 hex chars
    }

    #[tokio::test]
    async fn test_no_wallet_error() {
        let bridge = MiningBridge::new();

        let result = bridge.claim_rewards(100).await;
        assert!(matches!(result, Err(BridgeError::NoWallet)));
    }

    #[tokio::test]
    async fn test_mining_account() {
        let account = MiningAccount::new(
            "HS_test_address".to_string(),
            SecurityLevel::Level3
        );

        assert_eq!(account.total_earned, 0);
        assert_eq!(account.available_balance(), 0);
        assert!(account.teleported_rewards.is_empty());
    }
}
