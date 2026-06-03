//! Custom precompile registry for the Hanzo L2.
//!
//! Precompiles are deterministic functions callable from EVM contracts at
//! fixed addresses, avoiding the overhead of interpreted bytecode.
//!
//! # Address space
//!
//! | Hanzo address  | Routes to                                | Purpose                  |
//! |----------------|------------------------------------------|--------------------------|
//! | `0x0101..0001` | [`hanzo_pqc::signature::MlDsa`]          | PQ signature verify      |
//! | `0x0102..0002` | `libluxprecompile` Quasar (0x0300..0020) | Quasar committee query   |
//! | `0x0201..0001` | [`hanzo_engine::infer`]             | AI inference forward pass|
//! | `0x0202..0002` | [`hanzo_engine::embed`]             | AI embedding             |
//!
//! All four entry points dispatch into the canonical Hanzo or Lux
//! implementation — there are no in-tree fakes. When a downstream impl
//! is not available at runtime (e.g. no LLM engine registered) the
//! precompile reverts with a descriptive reason rather than returning
//! synthetic bytes.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use hanzo_engine::{self as llm_engine, EngineError};
use hanzo_pqc::signature::MlDsa;

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

/// Canonical Lux Quasar (Verkle witness) precompile, addressed inside the
/// `libluxprecompile` Go-backed dispatcher. The Hanzo Quasar precompile
/// is a thin shim that forwards to this address.
pub const LUX_QUASAR_ADDR: &str = "0x0300000000000000000000000000000000000020";

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
            base_gas: 3_000,
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

/// PQ signature verification using ML-DSA (FIPS 204) via `hanzo-pqc`.
///
/// # Calldata layout
///
/// | Offset    | Length  | Field             |
/// |-----------|---------|-------------------|
/// | 0         | 4       | public key length |
/// | 4         | pk_len  | public key bytes  |
/// | 4+pk      | 4       | signature length  |
/// | 8+pk      | sig_len | signature bytes   |
/// | rest      | ..      | message bytes     |
///
/// All lengths are big-endian `u32`. The public-key length determines the
/// FIPS 204 parameter set (ML-DSA-44 / 65 / 87 for 1312 / 1952 / 2592 bytes).
///
/// Returns the 32-byte big-endian word `0x..01` on a valid signature and
/// `0x..00` on an invalid one, matching the canonical Lux ML-DSA precompile
/// output shape.
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

    let pk_bytes = &input[4..4 + pk_len];

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

    let sig_bytes = &input[sig_offset + 4..msg_offset];
    let msg_bytes = &input[msg_offset..];

    // Gas cost: base + per-byte cost over the verified payload.
    let gas_used = 3_000u64.saturating_add((pk_len as u64 + sig_len as u64) / 16);

    let valid = match MlDsa::verify_raw(pk_bytes, msg_bytes, sig_bytes) {
        Ok(b) => b,
        Err(err) => {
            return PrecompileResult::Revert {
                reason: format!("ml-dsa verify error: {err}"),
            };
        }
    };

    // 32-byte word, right-aligned (mirrors the canonical Lux precompile).
    let mut output = vec![0u8; 32];
    if valid {
        output[31] = 1;
    }
    PrecompileResult::Success { output, gas_used }
}

/// Quasar committee membership query, routed through the canonical Lux
/// Quasar precompile (Verkle witness verifier) in `libluxprecompile`.
///
/// # Calldata layout
///
/// | Offset  | Length | Field                                  |
/// |---------|--------|----------------------------------------|
/// | 0       | 20     | validator address (the query)          |
/// | 20      | 32     | Verkle commitment (committee root)     |
/// | 52      | 32     | Verkle membership proof                |
/// | 84      | 1      | threshold-met flag (0 = unmet, !0 = met)|
///
/// Total: 85 bytes. The 20-byte validator address is the membership
/// query, the next 65 bytes are a Verkle witness over the committee
/// state; they are forwarded to the canonical Lux precompile at
/// [`LUX_QUASAR_ADDR`] in `[commitment(32)][proof(32)][thresholdMet(1)]`
/// order.
///
/// Output layout (53 bytes):
///
/// | Offset  | Length | Field                      |
/// |---------|--------|----------------------------|
/// | 0       | 20     | validator address          |
/// | 20      | 32     | committee root (commitment)|
/// | 52      | 1      | is-member flag (0/1)       |
fn exec_quasar_query(input: &[u8]) -> PrecompileResult {
    const REQUIRED_LEN: usize = 20 + 32 + 32 + 1;
    if input.len() < REQUIRED_LEN {
        return PrecompileResult::Revert {
            reason: format!(
                "quasar_query requires {} bytes (addr ++ commitment ++ proof ++ flag)",
                REQUIRED_LEN
            ),
        };
    }

    let validator = &input[0..20];
    let commitment = &input[20..52];
    let proof = &input[52..84];
    let threshold_met_byte = input[84];

    // Build the canonical Verkle witness input expected by the Lux Quasar
    // precompile (see lux/precompile/quasar/contract.go).
    let mut witness = Vec::with_capacity(65);
    witness.extend_from_slice(commitment);
    witness.extend_from_slice(proof);
    witness.push(threshold_met_byte);

    // The committee membership decision is derived from the canonical
    // Verkle verification result. Failure to dispatch is a hard error —
    // not a silent zero — so the EVM caller learns the actual reason.
    let res = match luxprecompile_sys::run(LUX_QUASAR_ADDR, &witness, 1_000_000) {
        Ok(r) => r,
        Err(err) => {
            return PrecompileResult::Revert {
                reason: format!("luxprecompile dispatch failed: {err}"),
            };
        }
    };

    // The canonical precompile returns a single byte (0 or 1). Anything
    // else is a protocol break.
    let is_member = match res.output.as_slice() {
        [b] => *b != 0,
        _ => {
            return PrecompileResult::Revert {
                reason: format!(
                    "unexpected quasar output length: {} bytes",
                    res.output.len()
                ),
            };
        }
    };

    let mut output = Vec::with_capacity(20 + 32 + 1);
    output.extend_from_slice(validator);
    output.extend_from_slice(commitment); // committee root == commitment
    output.push(if is_member { 0x01 } else { 0x00 });

    // Gas: 1_000_000 supplied to the canonical impl; charge what it
    // consumed plus a 1k routing fee.
    let gas_used = 1_000_000u64
        .saturating_sub(res.remaining_gas)
        .saturating_add(1_000);

    PrecompileResult::Success { output, gas_used }
}

/// AI inference forward pass.
///
/// # Calldata layout
///
/// | Offset  | Length | Field                                     |
/// |---------|--------|-------------------------------------------|
/// | 0       | 4      | selector (callers may pass 0 — reserved)  |
/// | 4       | 32     | model id (`blake3(model_name)` or hash)   |
/// | 36      | ..     | prompt bytes                              |
///
/// Dispatches to [`hanzo_engine::infer`]. When no engine has been
/// registered on this node the call reverts with the engine name. There is
/// no synthetic fallback. Production code installs a real engine at startup
/// via [`hanzo_engine::register_inference_engine`].
fn exec_ai_inference(input: &[u8]) -> PrecompileResult {
    const HEADER_LEN: usize = 4 + 32;
    if input.len() < HEADER_LEN {
        return PrecompileResult::Revert {
            reason: format!(
                "ai_inference requires at least {} bytes (selector + model id)",
                HEADER_LEN
            ),
        };
    }

    let mut model_id = [0u8; 32];
    model_id.copy_from_slice(&input[4..36]);
    let prompt = &input[HEADER_LEN..];

    if prompt.is_empty() {
        return PrecompileResult::Revert {
            reason: "ai_inference requires a non-empty prompt".into(),
        };
    }

    match llm_engine::infer(&model_id, prompt) {
        Ok(output) => {
            let gas_used = 100_000u64.saturating_add(output.len() as u64 * 8);
            PrecompileResult::Success { output, gas_used }
        }
        Err(EngineError::EngineNotRegistered(kind)) => PrecompileResult::Revert {
            reason: format!("no {kind} engine registered on this node"),
        },
        Err(EngineError::InvalidInput(msg)) => PrecompileResult::Revert {
            reason: format!("invalid ai_inference input: {msg}"),
        },
        Err(EngineError::EngineFailed(msg)) => PrecompileResult::Revert {
            reason: format!("ai_inference engine failure: {msg}"),
        },
        Err(EngineError::ModelNotFound(id)) => PrecompileResult::Revert {
            reason: format!("ai_inference model not found: {id}"),
        },
        Err(EngineError::Other(msg)) => PrecompileResult::Revert {
            reason: format!("ai_inference engine error: {msg}"),
        },
    }
}

/// AI embedding generation.
///
/// # Calldata layout
///
/// | Offset  | Length | Field                                   |
/// |---------|--------|-----------------------------------------|
/// | 0       | 4      | selector (callers may pass 0 — reserved)|
/// | 4       | 4      | embedding dimension `dim` (big-endian)  |
/// | 8       | ..     | text bytes                              |
///
/// Output: `dim * 4` bytes, each four-byte group an IEEE-754 little-endian
/// `f32` of the embedding vector — matching the byte layout that the
/// canonical Lux embedding tensor produces.
fn exec_ai_embedding(input: &[u8]) -> PrecompileResult {
    const HEADER_LEN: usize = 4 + 4;
    if input.len() < HEADER_LEN {
        return PrecompileResult::Revert {
            reason: format!(
                "ai_embedding requires at least {} bytes (selector + dim)",
                HEADER_LEN
            ),
        };
    }

    let dim = u32::from_be_bytes([input[4], input[5], input[6], input[7]]) as usize;
    if dim == 0 || dim > 4096 {
        return PrecompileResult::Revert {
            reason: format!("invalid embedding dimension: {dim}"),
        };
    }

    let text = &input[HEADER_LEN..];
    if text.is_empty() {
        return PrecompileResult::Revert {
            reason: "ai_embedding requires non-empty text".into(),
        };
    }

    match llm_engine::embed(dim, text) {
        Ok(vec) => {
            let mut output = Vec::with_capacity(dim * 4);
            for v in vec {
                output.extend_from_slice(&v.to_le_bytes());
            }
            let gas_used = 50_000u64.saturating_add(dim as u64 * 16);
            PrecompileResult::Success { output, gas_used }
        }
        Err(EngineError::EngineNotRegistered(kind)) => PrecompileResult::Revert {
            reason: format!("no {kind} engine registered on this node"),
        },
        Err(EngineError::InvalidInput(msg)) => PrecompileResult::Revert {
            reason: format!("invalid ai_embedding input: {msg}"),
        },
        Err(EngineError::EngineFailed(msg)) => PrecompileResult::Revert {
            reason: format!("ai_embedding engine failure: {msg}"),
        },
        Err(EngineError::ModelNotFound(id)) => PrecompileResult::Revert {
            reason: format!("ai_embedding model not found: {id}"),
        },
        Err(EngineError::Other(msg)) => PrecompileResult::Revert {
            reason: format!("ai_embedding engine error: {msg}"),
        },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Registry-level tests
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // pq_verify
    // -----------------------------------------------------------------------

    #[test]
    fn pq_verify_rejects_short_input() {
        let result = exec_pq_verify(&[0; 5]);
        assert!(matches!(result, PrecompileResult::Revert { .. }));
    }

    #[test]
    fn pq_verify_rejects_unsupported_pubkey_length() {
        // pk_len=1, pk=[0x00], sig_len=1, sig=[0x00], msg=[0x42] — pk length
        // 1 does not match any ML-DSA parameter set, so the verifier returns
        // Ok(false) → the precompile returns a 32-byte zero word.
        let mut input = Vec::new();
        input.extend_from_slice(&1u32.to_be_bytes());
        input.push(0x00);
        input.extend_from_slice(&1u32.to_be_bytes());
        input.push(0x00);
        input.push(0x42);

        let result = exec_pq_verify(&input);
        match result {
            PrecompileResult::Success { output, .. } => {
                assert_eq!(output.len(), 32);
                assert!(output.iter().all(|&b| b == 0));
            }
            other => panic!("expected Success(zero word), got {other:?}"),
        }
    }

    /// Sign a message with ML-DSA-65 via hanzo-pqc, then verify it through
    /// the precompile. Exercises the full sign → wire-format → verify path.
    #[test]
    fn exec_pq_verify_real_mldsa65_roundtrip() {
        use hanzo_pqc::signature::SignatureAlgorithm;

        let (vk, sk) = MlDsa::generate_keypair_sync(SignatureAlgorithm::MlDsa65)
            .expect("keypair generation");

        let message = b"hanzo-vm pq_verify integration message";
        let sig = MlDsa::sign_sync(&sk, message).expect("signing");

        // Assemble the calldata: [pk_len][pk][sig_len][sig][msg]
        let mut input = Vec::new();
        input.extend_from_slice(&(vk.key_bytes.len() as u32).to_be_bytes());
        input.extend_from_slice(&vk.key_bytes);
        input.extend_from_slice(&(sig.signature_bytes.len() as u32).to_be_bytes());
        input.extend_from_slice(&sig.signature_bytes);
        input.extend_from_slice(message);

        let result = exec_pq_verify(&input);
        match result {
            PrecompileResult::Success { output, gas_used } => {
                assert_eq!(output.len(), 32, "output should be a 32-byte word");
                assert_eq!(output[31], 1, "signature should verify");
                assert!(gas_used >= 3_000, "gas should include base cost");
            }
            other => panic!("expected Success, got {other:?}"),
        }

        // Flip a single byte of the message — verification must fail.
        let mut bad_input = input.clone();
        let msg_offset = 4 + vk.key_bytes.len() + 4 + sig.signature_bytes.len();
        bad_input[msg_offset] ^= 0x01;
        match exec_pq_verify(&bad_input) {
            PrecompileResult::Success { output, .. } => {
                assert_eq!(output[31], 0, "tampered message should not verify");
            }
            other => panic!("expected Success(0), got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // quasar_query
    // -----------------------------------------------------------------------

    #[test]
    fn quasar_query_rejects_short_input() {
        let result = exec_quasar_query(&[0; 10]);
        assert!(matches!(result, PrecompileResult::Revert { .. }));
    }

    /// Wire the precompile through `libluxprecompile` and confirm the
    /// canonical Verkle Quasar at `0x0300..0020` is the actual dispatch
    /// target. The proof is a no-op (commitment == proof) which the
    /// canonical impl accepts when the threshold flag is set; the test
    /// then flips the flag to verify the non-member path.
    #[test]
    fn exec_quasar_query_routes_through_libluxprecompile() {
        // Confirm the address is registered in the live dylib.
        let registry = luxprecompile_sys::list().expect("list precompiles");
        let found = registry
            .iter()
            .any(|p| p.address.eq_ignore_ascii_case(LUX_QUASAR_ADDR));
        assert!(
            found,
            "expected {} in libluxprecompile registry; got {:?}",
            LUX_QUASAR_ADDR, registry
        );

        let validator = [0x42u8; 20];
        let commitment = [0xAAu8; 32];
        let proof = commitment; // matches → verkle light verifier returns true

        // threshold met: caller is a member.
        let mut input = Vec::with_capacity(85);
        input.extend_from_slice(&validator);
        input.extend_from_slice(&commitment);
        input.extend_from_slice(&proof);
        input.push(0x01);

        match exec_quasar_query(&input) {
            PrecompileResult::Success { output, gas_used } => {
                assert_eq!(output.len(), 53);
                assert_eq!(&output[0..20], &validator[..]);
                assert_eq!(&output[20..52], &commitment[..]);
                assert_eq!(output[52], 0x01, "should report member");
                assert!(gas_used > 0);
            }
            other => panic!("expected Success(member), got {other:?}"),
        }

        // threshold unmet: non-member.
        let mut input = Vec::with_capacity(85);
        input.extend_from_slice(&validator);
        input.extend_from_slice(&commitment);
        input.extend_from_slice(&proof);
        input.push(0x00);
        match exec_quasar_query(&input) {
            PrecompileResult::Success { output, .. } => {
                assert_eq!(output[52], 0x00, "should report non-member");
            }
            other => panic!("expected Success(non-member), got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // ai_inference
    // -----------------------------------------------------------------------

    #[test]
    fn ai_inference_rejects_empty() {
        let result = exec_ai_inference(&[]);
        assert!(matches!(result, PrecompileResult::Revert { .. }));
    }

    #[test]
    fn ai_inference_rejects_missing_prompt() {
        let mut input = vec![0u8; 4 + 32];
        // header only, no prompt
        input.extend_from_slice(&[]);
        let result = exec_ai_inference(&input);
        match result {
            PrecompileResult::Revert { reason } => {
                assert!(reason.contains("non-empty"), "got reason: {reason}");
            }
            other => panic!("expected Revert, got {other:?}"),
        }
    }

    /// Default builds run without a registered LLM engine, so the
    /// precompile must revert with `no inference engine registered`. The
    /// runtime impl in [`exec_ai_inference`] always calls
    /// [`hanzo_engine::infer`] — there is no in-tree fallback path —
    /// so this test verifies the dispatch contract.
    ///
    /// With the `real-llm` feature a real engine would be installed in a
    /// startup hook and the call would return real bytes; that path is
    /// exercised by integration tests in the runtime crate, not here.
    #[cfg(not(feature = "real-llm"))]
    #[test]
    fn exec_ai_inference_real_model() {
        let mut input = Vec::new();
        input.extend_from_slice(&[0u8; 4]); // selector
        input.extend_from_slice(&[0xABu8; 32]); // model id
        input.extend_from_slice(b"summarize: this is a tiny prompt");

        let res = exec_ai_inference(&input);
        match res {
            PrecompileResult::Revert { reason } => {
                assert!(
                    reason.contains("inference") && reason.contains("engine"),
                    "expected 'no inference engine registered'-like reason, got: {reason}"
                );
            }
            PrecompileResult::Success { output, .. } => {
                // If another test in this binary has installed a real
                // engine via `register_inference_engine`, we expect the
                // engine's actual output bytes.
                assert!(!output.is_empty(), "engine must return real output");
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[cfg(feature = "real-llm")]
    #[test]
    fn exec_ai_inference_real_model() {
        // With the real-llm feature, production startup code installs an
        // engine. This test only enforces the dispatch contract — the
        // engine itself is exercised by upstream integration tests.
        assert!(
            hanzo_engine::inference_engine_registered(),
            "real-llm build must register an inference engine before tests run"
        );

        let mut input = Vec::new();
        input.extend_from_slice(&[0u8; 4]);
        input.extend_from_slice(&[0xABu8; 32]);
        input.extend_from_slice(b"summarize: this is a tiny prompt");

        match exec_ai_inference(&input) {
            PrecompileResult::Success { output, .. } => {
                assert!(!output.is_empty(), "engine must return real output");
            }
            other => panic!("expected Success, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // ai_embedding
    // -----------------------------------------------------------------------

    #[test]
    fn ai_embedding_validates_dimension() {
        // Too short
        let result = exec_ai_embedding(&[0; 2]);
        assert!(matches!(result, PrecompileResult::Revert { .. }));

        // Dimension = 0 (header complete but dim invalid)
        let mut input = vec![0u8; 4];
        input.extend_from_slice(&0u32.to_be_bytes());
        input.extend_from_slice(b"text");
        let result = exec_ai_embedding(&input);
        assert!(matches!(result, PrecompileResult::Revert { .. }));

        // Dimension = 5000 (over limit)
        let mut input = vec![0u8; 4];
        input.extend_from_slice(&5000u32.to_be_bytes());
        input.extend_from_slice(b"text");
        let result = exec_ai_embedding(&input);
        assert!(matches!(result, PrecompileResult::Revert { .. }));
    }

    /// Like [`exec_ai_inference_real_model`], this checks the dispatch
    /// contract: with no engine the precompile reverts with the engine
    /// name; with an engine it returns real bytes.
    #[cfg(not(feature = "real-llm"))]
    #[test]
    fn exec_ai_embedding_real_model() {
        let dim: u32 = 128;
        let mut input = Vec::new();
        input.extend_from_slice(&[0u8; 4]); // selector
        input.extend_from_slice(&dim.to_be_bytes());
        input.extend_from_slice(b"hello world");

        let res = exec_ai_embedding(&input);
        match res {
            PrecompileResult::Revert { reason } => {
                assert!(
                    reason.contains("embedding") && reason.contains("engine"),
                    "expected 'no embedding engine registered'-like reason, got: {reason}"
                );
            }
            PrecompileResult::Success { output, .. } => {
                // Engine installed elsewhere: must match dim * 4 bytes.
                assert_eq!(output.len(), (dim as usize) * 4);
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[cfg(feature = "real-llm")]
    #[test]
    fn exec_ai_embedding_real_model() {
        assert!(
            hanzo_engine::embedding_engine_registered(),
            "real-llm build must register an embedding engine before tests run"
        );

        let dim: u32 = 128;
        let mut input = Vec::new();
        input.extend_from_slice(&[0u8; 4]);
        input.extend_from_slice(&dim.to_be_bytes());
        input.extend_from_slice(b"hello world");

        match exec_ai_embedding(&input) {
            PrecompileResult::Success { output, .. } => {
                assert_eq!(output.len(), (dim as usize) * 4);
            }
            other => panic!("expected Success, got {other:?}"),
        }
    }
}
