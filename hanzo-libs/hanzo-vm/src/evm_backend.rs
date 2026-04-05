//! Pluggable EVM execution backend.
//!
//! The Hanzo VM supports multiple EVM implementations behind a common trait.
//! This allows swapping between the built-in Rust revm, the high-performance
//! C++ EVM (luxcpp/evm via FFI), or the Go EVM (luxfi/evm via subprocess)
//! without changing any consensus or state logic.
//!
//! # Backends
//!
//! | Backend | Speed    | GPU | Notes                              |
//! |---------|----------|-----|------------------------------------|
//! | Revm    | Good     | No  | Built-in, no external deps         |
//! | Cevm    | Best     | Yes | FFI to libevm + libevm-gpu         |
//! | GoEvm   | Moderate | No  | Subprocess, maximum compatibility  |

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::block::Transaction;
use crate::state::StateDb;

// ---------------------------------------------------------------------------
// EvmBackend enum
// ---------------------------------------------------------------------------

/// Which EVM execution backend to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EvmBackend {
    /// Built-in Rust revm (default).
    Revm,
    /// C++ EVM via FFI (luxcpp/evm, highest performance).
    Cevm,
    /// Go EVM via subprocess (luxfi/evm, maximum compatibility).
    GoEvm,
}

impl std::fmt::Display for EvmBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EvmBackend::Revm => write!(f, "revm"),
            EvmBackend::Cevm => write!(f, "cevm"),
            EvmBackend::GoEvm => write!(f, "go-evm"),
        }
    }
}

// ---------------------------------------------------------------------------
// BlockResult
// ---------------------------------------------------------------------------

/// Result of executing a block of transactions.
#[derive(Debug, Clone)]
pub struct BlockResult {
    /// Gas used per transaction (indexed by position in the input slice).
    pub gas_used: Vec<u64>,
    /// Total gas consumed by the block.
    pub total_gas: u64,
    /// Execution wall-clock time in milliseconds.
    pub exec_time_ms: f64,
    /// Number of state conflicts detected (Block-STM parallel execution).
    pub conflicts: u32,
    /// Number of transaction re-executions due to conflicts.
    pub re_executions: u32,
}

// ---------------------------------------------------------------------------
// StateAccess trait
// ---------------------------------------------------------------------------

/// Minimal state interface that backends use to read/write account state.
///
/// This decouples the executor from the concrete storage implementation
/// (SQLite for dev, MPT for production).
pub trait StateAccess: Send {
    /// Get the balance of an account by hex address.
    fn get_balance(&self, address: &str) -> Result<u128>;
    /// Get the nonce of an account by hex address.
    fn get_nonce(&self, address: &str) -> Result<u64>;
    /// Read a storage slot.
    fn get_storage(&self, address: &str, slot: &str) -> Result<Vec<u8>>;
    /// Write a storage slot.
    fn set_storage(&mut self, address: &str, slot: &str, value: &[u8]) -> Result<()>;
    /// Set the balance of an account.
    fn set_balance(&mut self, address: &str, balance: u128) -> Result<()>;
    /// Set the nonce of an account.
    fn set_nonce(&mut self, address: &str, nonce: u64) -> Result<()>;
}

impl StateAccess for StateDb {
    fn get_balance(&self, address: &str) -> Result<u128> {
        Ok(self.get_account(address)?.balance)
    }

    fn get_nonce(&self, address: &str) -> Result<u64> {
        Ok(self.get_account(address)?.nonce)
    }

    fn get_storage(&self, address: &str, slot: &str) -> Result<Vec<u8>> {
        StateDb::get_storage(self, address, slot)
    }

    fn set_storage(&mut self, address: &str, slot: &str, value: &[u8]) -> Result<()> {
        StateDb::set_storage(self, address, slot, value)
    }

    fn set_balance(&mut self, address: &str, balance: u128) -> Result<()> {
        let mut account = self.get_account(address)?;
        account.balance = balance;
        self.set_account(address, &account)
    }

    fn set_nonce(&mut self, address: &str, nonce: u64) -> Result<()> {
        let mut account = self.get_account(address)?;
        account.nonce = nonce;
        self.set_account(address, &account)
    }
}

// ---------------------------------------------------------------------------
// EvmExecutor trait
// ---------------------------------------------------------------------------

/// Pluggable EVM execution engine.
///
/// Implementors process a batch of transactions against a state backend
/// and return aggregated results. The trait is object-safe so backends
/// can be swapped at runtime via `Box<dyn EvmExecutor>`.
pub trait EvmExecutor: Send + Sync {
    /// Execute a block of transactions against the given state.
    fn execute_block(
        &self,
        txs: &[Transaction],
        state: &mut dyn StateAccess,
    ) -> Result<BlockResult>;

    /// Human-readable name of this backend.
    fn name(&self) -> &str;

    /// Whether this backend supports GPU acceleration.
    fn gpu_capable(&self) -> bool;
}

// ---------------------------------------------------------------------------
// RevmExecutor
// ---------------------------------------------------------------------------

/// Built-in Rust EVM backend (revm).
///
/// Default backend with no external dependencies. Executes transactions
/// sequentially. Good baseline performance, no GPU support.
pub struct RevmExecutor;

impl EvmExecutor for RevmExecutor {
    fn execute_block(
        &self,
        txs: &[Transaction],
        _state: &mut dyn StateAccess,
    ) -> Result<BlockResult> {
        let start = std::time::Instant::now();

        // Per-transaction gas accounting. A full implementation would decode
        // each RLP transaction and run it through revm. For now we charge
        // the intrinsic 21000 gas per transaction as a baseline.
        let gas_used: Vec<u64> = txs.iter().map(|tx| {
            if tx.raw_bytes.is_empty() { 0 } else { 21_000 }
        }).collect();

        let total_gas = gas_used.iter().sum();
        let elapsed = start.elapsed();

        Ok(BlockResult {
            gas_used,
            total_gas,
            exec_time_ms: elapsed.as_secs_f64() * 1000.0,
            conflicts: 0,
            re_executions: 0,
        })
    }

    fn name(&self) -> &str {
        "revm"
    }

    fn gpu_capable(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// CevmExecutor
// ---------------------------------------------------------------------------

/// C++ EVM backend via FFI to luxcpp/evm (libevm + libevm-gpu).
///
/// Highest performance backend. Supports:
/// - Block-STM parallel execution
/// - GPU Keccak-256 state hashing (Metal/CUDA)
/// - GPU batch ecrecover (Metal/CUDA)
/// - GPU EVM opcode interpreter (Metal/CUDA)
///
/// Requires `libevm` and `libevm-gpu` to be built and available on the
/// library path. See `luxcpp/evm/` for build instructions.
pub struct CevmExecutor {
    /// 0=CPU sequential, 1=CPU parallel, 2=GPU Metal, 3=GPU CUDA.
    backend_mode: u8,
}

impl CevmExecutor {
    /// Create a new C++ EVM executor with the specified GPU backend mode.
    ///
    /// Modes: 0=CPU sequential, 1=CPU parallel, 2=GPU Metal, 3=GPU CUDA.
    pub fn new(backend_mode: u8) -> Self {
        Self { backend_mode }
    }

    /// Create with auto-detected best available backend.
    pub fn auto() -> Self {
        // In a linked build, this would call gpu_auto_detect_backend() via FFI.
        // Default to CPU parallel (1) when not linked.
        Self { backend_mode: 1 }
    }
}

impl EvmExecutor for CevmExecutor {
    fn execute_block(
        &self,
        txs: &[Transaction],
        _state: &mut dyn StateAccess,
    ) -> Result<BlockResult> {
        let start = std::time::Instant::now();

        // A full implementation calls gpu_execute_block() from go_bridge.h
        // via Rust FFI (bindgen or manual extern "C"). The C function
        // signature is:
        //
        //   CGpuBlockResult gpu_execute_block(
        //       const CGpuTx* txs,
        //       uint32_t      num_txs,
        //       uint8_t       backend
        //   );
        //
        // For now, mirror the gas accounting from RevmExecutor so the
        // interface is exercised.
        let gas_used: Vec<u64> = txs.iter().map(|tx| {
            if tx.raw_bytes.is_empty() { 0 } else { 21_000 }
        }).collect();

        let total_gas = gas_used.iter().sum();
        let elapsed = start.elapsed();

        log::debug!(
            "cevm: executed {} txs (backend_mode={}) in {:.2}ms",
            txs.len(),
            self.backend_mode,
            elapsed.as_secs_f64() * 1000.0,
        );

        Ok(BlockResult {
            gas_used,
            total_gas,
            exec_time_ms: elapsed.as_secs_f64() * 1000.0,
            conflicts: 0,
            re_executions: 0,
        })
    }

    fn name(&self) -> &str {
        "cevm"
    }

    fn gpu_capable(&self) -> bool {
        self.backend_mode >= 2
    }
}

// ---------------------------------------------------------------------------
// GoEvmExecutor
// ---------------------------------------------------------------------------

/// Go EVM backend via subprocess (luxfi/evm or luxfi/cevm Go wrapper).
///
/// Maximum compatibility with the Go EVM codebase. Communicates with a
/// co-process over stdin/stdout using length-prefixed JSON messages.
/// Slower than native backends but useful for cross-validation and
/// testing.
pub struct GoEvmExecutor {
    /// Path to the Go EVM binary.
    binary_path: String,
}

impl GoEvmExecutor {
    /// Create a new Go EVM executor pointing to the given binary.
    pub fn new(binary_path: String) -> Self {
        Self { binary_path }
    }
}

impl EvmExecutor for GoEvmExecutor {
    fn execute_block(
        &self,
        txs: &[Transaction],
        _state: &mut dyn StateAccess,
    ) -> Result<BlockResult> {
        let start = std::time::Instant::now();

        // A full implementation would spawn the Go binary and pipe
        // transactions over stdin, reading results from stdout.
        // Verify the binary exists.
        if !std::path::Path::new(&self.binary_path).exists() {
            anyhow::bail!(
                "go-evm binary not found at {}: build with `go build` in luxfi/cevm",
                self.binary_path,
            );
        }

        let gas_used: Vec<u64> = txs.iter().map(|tx| {
            if tx.raw_bytes.is_empty() { 0 } else { 21_000 }
        }).collect();

        let total_gas = gas_used.iter().sum();
        let elapsed = start.elapsed();

        log::debug!(
            "go-evm: executed {} txs via {} in {:.2}ms",
            txs.len(),
            self.binary_path,
            elapsed.as_secs_f64() * 1000.0,
        );

        Ok(BlockResult {
            gas_used,
            total_gas,
            exec_time_ms: elapsed.as_secs_f64() * 1000.0,
            conflicts: 0,
            re_executions: 0,
        })
    }

    fn name(&self) -> &str {
        "go-evm"
    }

    fn gpu_capable(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Auto-detection
// ---------------------------------------------------------------------------

/// Auto-detect the best available EVM backend.
///
/// Priority: cevm (fastest) > revm (built-in fallback).
/// GoEvm is not auto-selected -- it must be explicitly configured.
pub fn auto_detect() -> Box<dyn EvmExecutor> {
    // Check if the cevm shared library is loadable.
    // In a production build this would use dlopen or link-time detection.
    // For now, check if the binary exists as a proxy.
    let cevm_path = format!(
        "{}/work/luxcpp/evm/build/bin/cevm",
        std::env::var("HOME").unwrap_or_default()
    );

    if std::path::Path::new(&cevm_path).exists() {
        log::info!("auto_detect: using cevm backend");
        Box::new(CevmExecutor::auto())
    } else {
        log::info!("auto_detect: using revm backend (cevm not found)");
        Box::new(RevmExecutor)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Transaction;

    fn sample_txs() -> Vec<Transaction> {
        vec![
            Transaction { raw_bytes: vec![0xde, 0xad], tx_index: 0, gas_used: 0 },
            Transaction { raw_bytes: vec![0xbe, 0xef], tx_index: 1, gas_used: 0 },
            Transaction { raw_bytes: vec![], tx_index: 2, gas_used: 0 },
        ]
    }

    /// Dummy state for testing -- does not persist anything.
    struct NullState;

    impl StateAccess for NullState {
        fn get_balance(&self, _: &str) -> Result<u128> { Ok(0) }
        fn get_nonce(&self, _: &str) -> Result<u64> { Ok(0) }
        fn get_storage(&self, _: &str, _: &str) -> Result<Vec<u8>> { Ok(vec![]) }
        fn set_storage(&mut self, _: &str, _: &str, _: &[u8]) -> Result<()> { Ok(()) }
        fn set_balance(&mut self, _: &str, _: u128) -> Result<()> { Ok(()) }
        fn set_nonce(&mut self, _: &str, _: u64) -> Result<()> { Ok(()) }
    }

    #[test]
    fn revm_executor_name() {
        let exec = RevmExecutor;
        assert_eq!(exec.name(), "revm");
        assert!(!exec.gpu_capable());
    }

    #[test]
    fn revm_execute_block() {
        let exec = RevmExecutor;
        let txs = sample_txs();
        let mut state = NullState;
        let result = exec.execute_block(&txs, &mut state).unwrap();

        assert_eq!(result.gas_used.len(), 3);
        assert_eq!(result.gas_used[0], 21_000);
        assert_eq!(result.gas_used[1], 21_000);
        assert_eq!(result.gas_used[2], 0); // empty tx
        assert_eq!(result.total_gas, 42_000);
        assert_eq!(result.conflicts, 0);
    }

    #[test]
    fn cevm_executor_name() {
        let exec = CevmExecutor::new(0);
        assert_eq!(exec.name(), "cevm");
        assert!(!exec.gpu_capable()); // mode 0 = CPU sequential
    }

    #[test]
    fn cevm_gpu_capable_modes() {
        assert!(!CevmExecutor::new(0).gpu_capable());
        assert!(!CevmExecutor::new(1).gpu_capable());
        assert!(CevmExecutor::new(2).gpu_capable());  // Metal
        assert!(CevmExecutor::new(3).gpu_capable());  // CUDA
    }

    #[test]
    fn cevm_execute_block() {
        let exec = CevmExecutor::auto();
        let txs = sample_txs();
        let mut state = NullState;
        let result = exec.execute_block(&txs, &mut state).unwrap();
        assert_eq!(result.gas_used.len(), 3);
        assert_eq!(result.total_gas, 42_000);
    }

    #[test]
    fn go_evm_fails_on_missing_binary() {
        let exec = GoEvmExecutor::new("/nonexistent/path/cevm".into());
        let txs = sample_txs();
        let mut state = NullState;
        let result = exec.execute_block(&txs, &mut state);
        assert!(result.is_err());
    }

    #[test]
    fn auto_detect_returns_executor() {
        let exec = auto_detect();
        // Should always succeed -- falls back to revm.
        assert!(!exec.name().is_empty());
    }

    #[test]
    fn evm_backend_display() {
        assert_eq!(format!("{}", EvmBackend::Revm), "revm");
        assert_eq!(format!("{}", EvmBackend::Cevm), "cevm");
        assert_eq!(format!("{}", EvmBackend::GoEvm), "go-evm");
    }

    #[test]
    fn block_result_empty_block() {
        let exec = RevmExecutor;
        let mut state = NullState;
        let result = exec.execute_block(&[], &mut state).unwrap();
        assert!(result.gas_used.is_empty());
        assert_eq!(result.total_gas, 0);
    }
}
