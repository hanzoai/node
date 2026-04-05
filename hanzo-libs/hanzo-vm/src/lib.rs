//! Lux VM interface for the Hanzo L2.
//!
//! This crate implements the VM engine that plugs into a Lux subnet,
//! wrapping EVM execution behind the Snow consensus interface.
//!
//! # Architecture
//!
//! ```text
//! Lux Consensus
//!       |
//!   HanzoVm (this crate)
//!       |
//!   +-----------+----------+-----------+
//!   | EvmBackend | StateDb | Precompiles |
//!   +-----------+----------+-----------+
//!        |
//!   +------+------+--------+
//!   | Revm | Cevm | GoEvm  |
//!   +------+------+--------+
//! ```
//!
//! - [`vm::HanzoVm`] -- main VM struct implementing [`vm::VmEngine`].
//! - [`evm_backend`] -- pluggable EVM execution backends (revm, cevm, go-evm).
//! - [`state::StateDb`] -- account/storage persistence (SQLite for dev).
//! - [`block::Block`] -- block and transaction types.
//! - [`precompiles::PrecompileRegistry`] -- custom precompile contracts
//!   (PQ signatures, quasar queries, AI inference/embeddings).

pub mod block;
pub mod evm_backend;
pub mod precompiles;
pub mod state;
pub mod vm;

// Re-export key types for ergonomic imports.
pub use block::{Block, BlockHeader, Transaction};
pub use evm_backend::{
    BlockResult, CevmExecutor, EvmBackend, EvmExecutor, GoEvmExecutor, RevmExecutor, StateAccess,
};
pub use precompiles::{PrecompileRegistry, PrecompileResult};
pub use state::{Account, StateDb};
pub use vm::{HealthStatus, HanzoVm, VmConfig, VmEngine};
