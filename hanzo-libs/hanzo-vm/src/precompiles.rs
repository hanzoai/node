//! Custom precompile registry for the Hanzo L2.
//!
//! Precompiles are deterministic functions callable from EVM contracts at
//! fixed addresses, avoiding the overhead of interpreted bytecode.
//!
//! # Address space
//!
//! | Range             | Purpose                              |
//! |-------------------|--------------------------------------|
//! | `0x0100..0001`    | PQ signature verification (ML-DSA)   |
//! | `0x0100..0002`    | Quasar committee query               |
//! | `0x0200..0001`    | AI inference call                    |
//! | `0x0200..0002`    | AI embedding computation             |

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Precompile addresses
// ---------------------------------------------------------------------------

/// PQ signature verification via ML-DSA (FIPS 204) from `hanzo-pqc`.
pub const ADDR_PQ_VERIFY: [u8; 20] = addr(0x01, 0x01);

/// Quasar committee membership query.
pub const ADDR_QUASAR_QUERY: [u8; 20] = addr(0x01, 0x02);

/// AI inference call (forward pass through a registered model).
pub const ADDR_AI_INFERENCE: [u8; 20] = addr(0x02, 0x01);

/// AI embedding computation.
pub const ADDR_AI_EMBEDDING: [u8; 20] = addr(0x02, 0x02);

/// Helper to build a 20-byte precompile address from a category and index.
///
/// Layout: `[0x00; 17] ++ [category] ++ [0x00] ++ [index]`
const fn addr(category: u8, index: u8) -> [u8; 20] {
    let mut a = [0u8; 20];
    a[17] = category;
    a[19] = index;
    a
}

// ---------------------------------------------------------------------------
// PrecompileResult
// ---------------------------------------------------------------------------

/// Outcome of a precompile execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrecompileResult {
    /// Successful execution with output bytes.
    Success {
        /// Raw output returned to the EVM caller.
        output: Vec<u8>,
        /// Gas consumed by this precompile call.
        gas_used: u64,
    },
    /// Execution reverted.
    Revert {
        /// Human-readable reason string.
        reason: String,
    },
    /// Execution encountered an unrecoverable error.
    Error {
        /// Human-readable error description.
        message: String,
    },
}

// ---------------------------------------------------------------------------
// PrecompileEntry
// ---------------------------------------------------------------------------

/// A single registered precompile.
#[derive(Clone)]
pub struct PrecompileEntry {
    /// 20-byte EVM address where this precompile lives.
    pub address: [u8; 20],
    /// Human-readable name for logging.
    pub name: String,
    /// Base gas cost (charged before execution).
    pub base_gas: u64,
    /// The execution function.
    ///
    /// Receives raw calldata and returns a [`PrecompileResult`].
    pub execute: fn(input: &[u8]) -> PrecompileResult,
}

impl std::fmt::Debug for PrecompileEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrecompileEntry")
            .field("address", &hex::encode(self.address))
            .field("name", &self.name)
            .field("base_gas", &self.base_gas)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// PrecompileRegistry
// ---------------------------------------------------------------------------

/// Registry of custom precompile contracts.
///
/// Use [`Default::default()`] to get a registry pre-loaded with all Hanzo
/// precompiles, or build one manually with [`new`](Self::new) and
/// [`register`](Self::register).
#[derive(Debug)]
pub struct PrecompileRegistry {
    entries: HashMap<[u8; 20], PrecompileEntry>,
}

impl PrecompileRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Register a precompile. Overwrites any existing entry at the same address.
    pub fn register(&mut self, entry: PrecompileEntry) {
        self.entries.insert(entry.address, entry);
    }

    /// Look up a precompile by its 20-byte EVM address.
    pub fn get(&self, address: &[u8; 20]) -> Option<&PrecompileEntry> {
        self.entries.get(address)
    }

    /// Execute a precompile at `address` with the given `input`.
    ///
    /// Returns `None` if no precompile is registered at that address.
    pub fn call(&self, address: &[u8; 20], input: &[u8]) -> Option<PrecompileResult> {
        self.entries
            .get(address)
            .map(|entry| (entry.execute)(input))
    }

    /// Return the number of registered precompiles.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Return `true` if no precompiles are registered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for PrecompileRegistry {
    /// Build a registry with all built-in Hanzo precompiles.
    fn default() -> Self {
        let mut r = Self::new();

        r.register(PrecompileEntry {
            address: ADDR_PQ_VERIFY,
            name: "pq_verify".into(),
            base_gas: 3_000,
            execute: exec_pq_verify,
        });

        r.register(PrecompileEntry {
            address: ADDR_QUASAR_QUERY,
            name: "quasar_query".into(),
            base_gas: 1_000,
            execute: exec_quasar_query,
        });

        r.register(PrecompileEntry {
            address: ADDR_AI_INFERENCE,
            name: "ai_inference".into(),
            base_gas: 100_000,
            execute: exec_ai_inference,
        });

        r.register(PrecompileEntry {
            address: ADDR_AI_EMBEDDING,
            name: "ai_embedding".into(),
            base_gas: 50_000,
            execute: exec_ai_embedding,
        });

        r
    }
}

// ---------------------------------------------------------------------------
// Built-in precompile implementations
// ---------------------------------------------------------------------------

/// PQ signature verification using ML-DSA (FIPS 204) from `hanzo-pqc`.
///
/// # Calldata layout
///
/// | Offset | Length   | Field             |
/// |--------|---------|-------------------|
/// | 0      | 4       | public key length |
/// | 4      | pk_len  | public key bytes  |
/// | 4+pk   | 4       | signature length  |
/// | 8+pk   | sig_len | signature bytes   |
/// | rest   | ..      | message bytes     |
///
/// Returns `[0x01]` on valid signature, `[0x00]` on invalid.
fn exec_pq_verify(input: &[u8]) -> PrecompileResult {
    // Minimum: 4 (pk_len) + 1 (pk) + 4 (sig_len) + 1 (sig) + 0 (msg)
    if input.len() < 10 {
        return PrecompileResult::Revert {
            reason: "input too short for pq_verify".into(),
        };
    }

    let pk_len = u32::from_be_bytes([input[0], input[1], input[2], input[3]]) as usize;
    if input.len() < 4 + pk_len + 4 {
        return PrecompileResult::Revert {
            reason: "input truncated at public key".into(),
        };
    }

    let _pk_bytes = &input[4..4 + pk_len];

    let sig_offset = 4 + pk_len;
    let sig_len = u32::from_be_bytes([
        input[sig_offset],
        input[sig_offset + 1],
        input[sig_offset + 2],
        input[sig_offset + 3],
    ]) as usize;

    let msg_offset = sig_offset + 4 + sig_len;
    if input.len() < msg_offset {
        return PrecompileResult::Revert {
            reason: "input truncated at signature".into(),
        };
    }

    let _sig_bytes = &input[sig_offset + 4..msg_offset];
    let _msg_bytes = &input[msg_offset..];

    // TODO: wire up hanzo_pqc::signature::Signature::verify once the crate
    // exposes a raw-bytes verification API. For now, return success as a
    // stub so gas accounting and calldata parsing are exercised.
    log::debug!("pq_verify: pk_len={pk_len}, sig_len={sig_len}");

    PrecompileResult::Success {
        output: vec![0x01],
        gas_used: 3_000 + (pk_len as u64 + sig_len as u64) / 16,
    }
}

/// Quasar committee membership query.
///
/// Returns a stub 32-byte committee root and membership flag.
/// Full implementation will query the on-chain Quasar registry.
fn exec_quasar_query(input: &[u8]) -> PrecompileResult {
    if input.len() < 20 {
        return PrecompileResult::Revert {
            reason: "quasar_query requires a 20-byte address".into(),
        };
    }

    // Stub: always return "is member" with a zero committee root.
    let mut output = vec![0u8; 33];
    output[32] = 0x01; // membership flag
    PrecompileResult::Success {
        output,
        gas_used: 1_000,
    }
}

/// AI inference call.
///
/// Accepts an opaque request payload and returns inference output.
/// In production this dispatches to the node's AI runtime or an
/// off-chain oracle with result verification.
fn exec_ai_inference(input: &[u8]) -> PrecompileResult {
    if input.is_empty() {
        return PrecompileResult::Revert {
            reason: "ai_inference requires non-empty input".into(),
        };
    }

    // Stub: echo the input length as a 32-byte big-endian integer.
    let len = input.len() as u64;
    let mut output = vec![0u8; 32];
    output[24..32].copy_from_slice(&len.to_be_bytes());

    PrecompileResult::Success {
        output,
        gas_used: 100_000 + (input.len() as u64) * 8,
    }
}

/// AI embedding computation.
///
/// Accepts text or token IDs and returns a fixed-dimension embedding
/// vector. Stub returns a zero vector of the requested dimension.
fn exec_ai_embedding(input: &[u8]) -> PrecompileResult {
    if input.len() < 4 {
        return PrecompileResult::Revert {
            reason: "ai_embedding requires at least 4 bytes (dimension)".into(),
        };
    }

    let dim = u32::from_be_bytes([input[0], input[1], input[2], input[3]]) as usize;
    if dim == 0 || dim > 4096 {
        return PrecompileResult::Revert {
            reason: format!("invalid embedding dimension: {dim}"),
        };
    }

    // Stub: return a zero vector of `dim * 4` bytes (f32 elements).
    let output = vec![0u8; dim * 4];

    PrecompileResult::Success {
        output,
        gas_used: 50_000 + (dim as u64) * 16,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_registry_has_four_precompiles() {
        let reg = PrecompileRegistry::default();
        assert_eq!(reg.len(), 4);
        assert!(reg.get(&ADDR_PQ_VERIFY).is_some());
        assert!(reg.get(&ADDR_QUASAR_QUERY).is_some());
        assert!(reg.get(&ADDR_AI_INFERENCE).is_some());
        assert!(reg.get(&ADDR_AI_EMBEDDING).is_some());
    }

    #[test]
    fn pq_verify_rejects_short_input() {
        let result = exec_pq_verify(&[0; 5]);
        assert!(matches!(result, PrecompileResult::Revert { .. }));
    }

    #[test]
    fn pq_verify_accepts_valid_structure() {
        // pk_len=1, pk=[0x00], sig_len=1, sig=[0x00], msg=[0x42]
        let mut input = Vec::new();
        input.extend_from_slice(&1u32.to_be_bytes()); // pk_len
        input.push(0x00); // pk
        input.extend_from_slice(&1u32.to_be_bytes()); // sig_len
        input.push(0x00); // sig
        input.push(0x42); // msg

        let result = exec_pq_verify(&input);
        assert!(matches!(result, PrecompileResult::Success { .. }));
    }

    #[test]
    fn quasar_query_needs_20_bytes() {
        let result = exec_quasar_query(&[0; 10]);
        assert!(matches!(result, PrecompileResult::Revert { .. }));

        let result = exec_quasar_query(&[0; 20]);
        assert!(matches!(result, PrecompileResult::Success { .. }));
    }

    #[test]
    fn ai_inference_rejects_empty() {
        let result = exec_ai_inference(&[]);
        assert!(matches!(result, PrecompileResult::Revert { .. }));
    }

    #[test]
    fn ai_inference_returns_length() {
        let input = vec![0xaa; 64];
        let result = exec_ai_inference(&input);
        match result {
            PrecompileResult::Success { output, .. } => {
                let len =
                    u64::from_be_bytes(output[24..32].try_into().unwrap());
                assert_eq!(len, 64);
            }
            other => panic!("expected Success, got {other:?}"),
        }
    }

    #[test]
    fn ai_embedding_validates_dimension() {
        // Too short
        let result = exec_ai_embedding(&[0; 2]);
        assert!(matches!(result, PrecompileResult::Revert { .. }));

        // Dimension = 0
        let result = exec_ai_embedding(&0u32.to_be_bytes());
        assert!(matches!(result, PrecompileResult::Revert { .. }));

        // Dimension = 5000 (over limit)
        let result = exec_ai_embedding(&5000u32.to_be_bytes());
        assert!(matches!(result, PrecompileResult::Revert { .. }));
    }

    #[test]
    fn ai_embedding_returns_correct_size() {
        let dim: u32 = 128;
        let mut input = dim.to_be_bytes().to_vec();
        input.extend_from_slice(b"hello world");

        let result = exec_ai_embedding(&input);
        match result {
            PrecompileResult::Success { output, .. } => {
                assert_eq!(output.len(), 128 * 4);
            }
            other => panic!("expected Success, got {other:?}"),
        }
    }

    #[test]
    fn registry_call_unknown_address_returns_none() {
        let reg = PrecompileRegistry::default();
        let unknown = [0xff; 20];
        assert!(reg.call(&unknown, &[]).is_none());
    }

    #[test]
    fn addr_helper_layout() {
        // ADDR_PQ_VERIFY: category=0x01, index=0x01
        assert_eq!(ADDR_PQ_VERIFY[17], 0x01);
        assert_eq!(ADDR_PQ_VERIFY[19], 0x01);
        assert_eq!(ADDR_PQ_VERIFY[0..17], [0u8; 17]);

        // ADDR_AI_INFERENCE: category=0x02, index=0x01
        assert_eq!(ADDR_AI_INFERENCE[17], 0x02);
        assert_eq!(ADDR_AI_INFERENCE[19], 0x01);
    }
}
