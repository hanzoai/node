// SPDX-License-Identifier: Apache-2.0
//! AI Mining precompile (0x0300) — thin wrapper around the canonical
//! `libluxprecompile.dylib` shipped from `~/work/lux/precompile`.
//!
//! There is exactly one source of truth for every Lux precompile: the Go
//! implementation in `lux/precompile/<name>`, exposed as a C ABI shared
//! library at `lux/precompile/dist/libluxprecompile.{a,dylib}`. This module
//! routes the Hanzo Rust EVM's `0x0300` calls through that library via the
//! `luxprecompile-sys` crate, so a proof produced by zoo/node (Go) verifies
//! identically here, and vice versa.
//!
//! Build-time wiring: the dylib must be discoverable via dyld/ld at runtime.
//! Set `DYLD_LIBRARY_PATH=/Users/z/work/lux/precompile/dist` (macOS) or
//! `LD_LIBRARY_PATH=...` (Linux), or `cmake --install` it into a system path.

use super::{PrecompileEntry, PrecompileRegistry, PrecompileResult};

/// Canonical EVM address for AI mining — same on every brand chain so the
/// shared A-Chain accepts proofs without per-chain remapping.
pub const ADDR_AI_MINING: [u8; 20] = [
    0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00,
];

/// Hex form used for the FFI (`luxprecompile_sys::run` takes a hex address).
const ADDR_HEX: &str = "0x0300000000000000000000000000000000000000";

/// Register the AI mining precompile in `r`. Idempotent; safe to call from
/// `PrecompileRegistry::default()` even when the dylib isn't installed —
/// calls then revert at runtime with a clear message instead of failing
/// registry construction.
pub fn register_ai_mining(r: &mut PrecompileRegistry) {
    r.register(PrecompileEntry {
        address: ADDR_AI_MINING,
        name: "ai_mining".into(),
        // Base gas is informational here; the dylib is authoritative for the
        // actual per-call cost (returned via `lux_precompile_required_gas`).
        base_gas: 1_000,
        execute: exec_ai_mining,
    });
}

fn exec_ai_mining(input: &[u8]) -> PrecompileResult {
    // 1 billion gas — the C ABI returns the real consumed amount; we never
    // pass through user gas limits at this layer because the registry's
    // PrecompileResult shape does not surface them. EVM-side metering is
    // enforced by the calling executor.
    const GAS_CEILING: u64 = 1_000_000_000;
    match luxprecompile_sys::run(ADDR_HEX, input, GAS_CEILING) {
        Ok(rr) => PrecompileResult::Success {
            output: rr.output,
            gas_used: GAS_CEILING.saturating_sub(rr.remaining_gas),
        },
        Err(luxprecompile_sys::PrecompileError::ExecutionFailed(_)) => PrecompileResult::Revert {
            reason: "ai_mining: execution failed (check input layout / selector)".into(),
        },
        Err(luxprecompile_sys::PrecompileError::NotFound(_)) => PrecompileResult::Error {
            message: "ai_mining: libluxprecompile not loaded — set DYLD_LIBRARY_PATH or install"
                .into(),
        },
        Err(e) => PrecompileResult::Error {
            message: format!("ai_mining: {e}"),
        },
    }
}
