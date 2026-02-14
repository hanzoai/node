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
//!   +---------+-----------+
//!   | StateDb | Precompiles |
//!   +---------+-----------+
//! ```
//!
//! - [`vm::HanzoVm`] -- main VM struct implementing [`vm::VmEngine`].
//! - [`state::StateDb`] -- account/storage persistence (SQLite for dev).
//! - [`block::Block`] -- block and transaction types.
//! - [`precompiles::PrecompileRegistry`] -- custom precompile contracts
//!   (PQ signatures, quasar queries, AI inference/embeddings).

pub mod block;
pub mod precompiles;
pub mod state;
pub mod vm;

// Re-export key types for ergonomic imports.
pub use block::{Block, BlockHeader, Transaction};
pub use precompiles::{PrecompileRegistry, PrecompileResult};
pub use state::{Account, StateDb};
pub use vm::{HealthStatus, HanzoVm, VmConfig, VmEngine};
