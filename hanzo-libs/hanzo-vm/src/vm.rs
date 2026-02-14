//! Lux VM engine implementation.
//!
//! [`HanzoVm`] is the primary entry point. It implements [`VmEngine`], which
//! mirrors the Snow VM interface used by Lux subnets:
//!
//! - `initialize` -- load genesis state.
//! - `build_block` -- construct a new block from pending transactions.
//! - `parse_block` / `accept` / `reject` -- consensus lifecycle.
//! - `last_accepted` -- current canonical tip.
//! - `health_check` -- readiness probe.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::block::Block;
use crate::precompiles::PrecompileRegistry;
use crate::state::StateDb;

// ---------------------------------------------------------------------------
// VmEngine trait
// ---------------------------------------------------------------------------

/// Snow-compatible VM interface for a Lux subnet.
///
/// Every method takes `&mut self` where state mutation is required, or
/// `&self` for read-only queries.
pub trait VmEngine {
    /// Load genesis bytes and initialize chain state.
    ///
    /// Called exactly once when the node starts for the first time.
    /// The genesis payload is an opaque blob whose format is defined
    /// by the specific VM implementation.
    fn initialize(&mut self, genesis: &[u8]) -> Result<()>;

    /// Build a new block from the given raw EVM transactions.
    ///
    /// Each element of `txs` is a single RLP-encoded transaction.
    fn build_block(&mut self, txs: Vec<Vec<u8>>) -> Result<Block>;

    /// Deserialize a block received from the network.
    fn parse_block(&self, bytes: &[u8]) -> Result<Block>;

    /// Set the preferred block that this node wants to build on.
    fn set_preference(&mut self, block_id: [u8; 32]) -> Result<()>;

    /// Accept a block into the canonical chain.
    fn accept(&mut self, block_id: [u8; 32]) -> Result<()>;

    /// Reject a block, discarding any state changes.
    fn reject(&mut self, block_id: [u8; 32]) -> Result<()>;

    /// Return the ID of the last accepted (finalized) block.
    fn last_accepted(&self) -> Result<[u8; 32]>;

    /// Return current health status for readiness probes.
    fn health_check(&self) -> Result<HealthStatus>;
}

// ---------------------------------------------------------------------------
// VmConfig
// ---------------------------------------------------------------------------

/// Configuration for the Hanzo VM instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmConfig {
    /// EVM chain ID (e.g., 1313161554 for Hanzo mainnet).
    pub chain_id: u64,

    /// Directory for persistent state (SQLite DB, block store).
    pub data_dir: String,

    /// Port for the JSON-RPC endpoint.
    pub rpc_port: u16,

    /// Maximum gas limit per block.
    pub block_gas_limit: u64,

    /// Target block interval in milliseconds.
    pub target_block_time_ms: u64,
}

impl Default for VmConfig {
    fn default() -> Self {
        Self {
            chain_id: 1313161554,
            data_dir: "/tmp/hanzo-vm".into(),
            rpc_port: 9650,
            block_gas_limit: 30_000_000,
            target_block_time_ms: 2_000,
        }
    }
}

// ---------------------------------------------------------------------------
// HealthStatus
// ---------------------------------------------------------------------------

/// Result of a health-check probe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    /// Whether the node is healthy and ready to serve requests.
    pub healthy: bool,
    /// Human-readable reason when unhealthy.
    pub reason: Option<String>,
    /// Height of the last accepted block.
    pub last_accepted_height: u64,
}

// ---------------------------------------------------------------------------
// HanzoVm
// ---------------------------------------------------------------------------

/// The Hanzo L2 virtual machine.
///
/// Wraps EVM execution behind the Lux Snow consensus interface, using
/// SQLite-backed state for development and custom precompiles for
/// post-quantum crypto and AI operations.
pub struct HanzoVm {
    /// Persistent account/storage state.
    pub state: StateDb,
    /// Custom precompile registry.
    pub precompiles: PrecompileRegistry,
    /// VM configuration.
    pub config: VmConfig,
    /// ID of the currently preferred block.
    preferred: [u8; 32],
    /// ID of the last accepted (finalized) block.
    last_accepted_id: [u8; 32],
    /// Height of the last accepted block.
    last_accepted_height: u64,
    /// Whether genesis has been loaded.
    initialized: bool,
}

impl HanzoVm {
    /// Create a new `HanzoVm` with the given configuration.
    pub fn new(config: VmConfig) -> Self {
        let state = StateDb::new(&config.data_dir);
        let precompiles = PrecompileRegistry::default();
        Self {
            state,
            precompiles,
            config,
            preferred: [0u8; 32],
            last_accepted_id: [0u8; 32],
            last_accepted_height: 0,
            initialized: false,
        }
    }
}

impl VmEngine for HanzoVm {
    fn initialize(&mut self, genesis: &[u8]) -> Result<()> {
        if self.initialized {
            anyhow::bail!("VM already initialized");
        }

        // Parse genesis JSON.
        let genesis_state: GenesisState = serde_json::from_slice(genesis)
            .map_err(|e| anyhow::anyhow!("invalid genesis payload: {e}"))?;

        // Initialize state DB.
        self.state.init()?;

        // Apply genesis allocations.
        for alloc in &genesis_state.alloc {
            let account = crate::state::Account {
                nonce: alloc.nonce,
                balance: alloc.balance,
                code_hash: [0u8; 32],
                storage_root: [0u8; 32],
            };
            self.state.set_account(&alloc.address, &account)?;
        }

        // Build and accept the genesis block.
        let genesis_block = Block::genesis(genesis_state.chain_id, genesis_state.timestamp);
        let block_id = genesis_block.id();
        self.last_accepted_id = block_id;
        self.last_accepted_height = 0;
        self.preferred = block_id;
        self.config.chain_id = genesis_state.chain_id;
        self.initialized = true;

        log::info!(
            "VM initialized: chain_id={}, genesis_block={}",
            genesis_state.chain_id,
            hex::encode(block_id)
        );
        Ok(())
    }

    fn build_block(&mut self, txs: Vec<Vec<u8>>) -> Result<Block> {
        if !self.initialized {
            anyhow::bail!("VM not initialized");
        }

        let transactions: Vec<crate::block::Transaction> = txs
            .into_iter()
            .enumerate()
            .map(|(i, raw)| crate::block::Transaction {
                raw_bytes: raw,
                tx_index: i as u32,
                gas_used: 0,
            })
            .collect();

        let height = self.last_accepted_height + 1;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let block = Block::new(
            self.last_accepted_id,
            height,
            timestamp,
            transactions,
            self.state.root(),
        );

        log::debug!(
            "built block height={height} id={}",
            hex::encode(block.id())
        );
        Ok(block)
    }

    fn parse_block(&self, bytes: &[u8]) -> Result<Block> {
        let block: Block = serde_json::from_slice(bytes)
            .map_err(|e| anyhow::anyhow!("failed to parse block: {e}"))?;
        Ok(block)
    }

    fn set_preference(&mut self, block_id: [u8; 32]) -> Result<()> {
        self.preferred = block_id;
        log::debug!("preference set to {}", hex::encode(block_id));
        Ok(())
    }

    fn accept(&mut self, block_id: [u8; 32]) -> Result<()> {
        if !self.initialized {
            anyhow::bail!("VM not initialized");
        }

        // In a full implementation this would apply block state transitions
        // and persist the accepted block. For now we update bookkeeping.
        self.last_accepted_id = block_id;
        self.last_accepted_height += 1;

        log::info!(
            "accepted block height={} id={}",
            self.last_accepted_height,
            hex::encode(block_id)
        );
        Ok(())
    }

    fn reject(&mut self, block_id: [u8; 32]) -> Result<()> {
        log::info!("rejected block id={}", hex::encode(block_id));
        // Discard any pending state associated with this block.
        // Full implementation would clean up uncommitted state diffs.
        Ok(())
    }

    fn last_accepted(&self) -> Result<[u8; 32]> {
        Ok(self.last_accepted_id)
    }

    fn health_check(&self) -> Result<HealthStatus> {
        if !self.initialized {
            return Ok(HealthStatus {
                healthy: false,
                reason: Some("VM not yet initialized".into()),
                last_accepted_height: 0,
            });
        }

        // Check state DB connectivity.
        if let Err(e) = self.state.ping() {
            return Ok(HealthStatus {
                healthy: false,
                reason: Some(format!("state DB unreachable: {e}")),
                last_accepted_height: self.last_accepted_height,
            });
        }

        Ok(HealthStatus {
            healthy: true,
            reason: None,
            last_accepted_height: self.last_accepted_height,
        })
    }
}

// ---------------------------------------------------------------------------
// Genesis types (internal)
// ---------------------------------------------------------------------------

/// Genesis allocation for a single account.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct GenesisAlloc {
    /// Hex-encoded address (0x-prefixed).
    address: String,
    /// Initial balance in wei.
    balance: u128,
    /// Initial nonce.
    #[serde(default)]
    nonce: u64,
}

/// Full genesis state payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct GenesisState {
    /// Chain identifier.
    chain_id: u64,
    /// Genesis timestamp (unix seconds).
    timestamp: u64,
    /// Initial account allocations.
    #[serde(default)]
    alloc: Vec<GenesisAlloc>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_genesis() -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "chain_id": 31337,
            "timestamp": 1700000000,
            "alloc": [
                {
                    "address": "0x0000000000000000000000000000000000000001",
                    "balance": 1_000_000_000_000_000_000u128,
                    "nonce": 0
                }
            ]
        }))
        .unwrap()
    }

    #[test]
    fn initialize_and_health_check() {
        let dir = tempfile::tempdir().unwrap();
        let config = VmConfig {
            data_dir: dir.path().to_string_lossy().into_owned(),
            ..VmConfig::default()
        };
        let mut vm = HanzoVm::new(config);
        vm.initialize(&test_genesis()).unwrap();

        let status = vm.health_check().unwrap();
        assert!(status.healthy);
        assert_eq!(status.last_accepted_height, 0);
    }

    #[test]
    fn double_initialize_fails() {
        let dir = tempfile::tempdir().unwrap();
        let config = VmConfig {
            data_dir: dir.path().to_string_lossy().into_owned(),
            ..VmConfig::default()
        };
        let mut vm = HanzoVm::new(config);
        vm.initialize(&test_genesis()).unwrap();
        assert!(vm.initialize(&test_genesis()).is_err());
    }

    #[test]
    fn build_and_accept_block() {
        let dir = tempfile::tempdir().unwrap();
        let config = VmConfig {
            data_dir: dir.path().to_string_lossy().into_owned(),
            ..VmConfig::default()
        };
        let mut vm = HanzoVm::new(config);
        vm.initialize(&test_genesis()).unwrap();

        let block = vm.build_block(vec![vec![0xde, 0xad]]).unwrap();
        let block_id = block.id();
        assert_eq!(block.header.height, 1);

        vm.accept(block_id).unwrap();
        assert_eq!(vm.last_accepted().unwrap(), block_id);
        assert_eq!(vm.health_check().unwrap().last_accepted_height, 1);
    }

    #[test]
    fn parse_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let config = VmConfig {
            data_dir: dir.path().to_string_lossy().into_owned(),
            ..VmConfig::default()
        };
        let mut vm = HanzoVm::new(config);
        vm.initialize(&test_genesis()).unwrap();

        let block = vm.build_block(vec![]).unwrap();
        let bytes = serde_json::to_vec(&block).unwrap();
        let parsed = vm.parse_block(&bytes).unwrap();

        assert_eq!(block.id(), parsed.id());
        assert_eq!(block.header.height, parsed.header.height);
    }
}
