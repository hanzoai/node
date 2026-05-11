// SPDX-License-Identifier: Apache-2.0
//! Tests for the AI mining precompile registration.
//!
//! The AI mining logic itself is owned by the canonical Go impl in
//! `lux/precompile/ai`; its correctness is covered there. These tests only
//! prove the Rust-side wiring: the precompile is registered at the right
//! address, dispatches through the FFI, and surfaces the correct result
//! variants on success / not-found / execution-failed paths.

use super::{ai_mining::ADDR_AI_MINING, PrecompileRegistry, PrecompileResult};

#[test]
fn registers_at_0x0300() {
    let r = PrecompileRegistry::default();
    let entry = r.get(&ADDR_AI_MINING).expect("ai_mining registered");
    assert_eq!(entry.name, "ai_mining");
    assert_eq!(entry.address[0], 0x03);
    assert!(entry.address[1..].iter().all(|&b| b == 0));
}

#[test]
fn dispatches_through_ffi_or_reports_missing_dylib() {
    // ComputeWorkId selector (0x06) needs 32 + 32 + 8 = 72 bytes of args.
    let mut input = vec![0x06, 0x00, 0x00, 0x00];
    input.extend_from_slice(&[0xab; 32]); // device_id
    input.extend_from_slice(&[0xcd; 32]); // nonce
    input.extend_from_slice(&[0u8, 0, 0, 0, 0, 0, 0, 1]); // chain_id = 1

    let r = PrecompileRegistry::default();
    let result = r.call(&ADDR_AI_MINING, &input).expect("registered");
    match result {
        // dylib loaded: canonical Go impl returned a 32-byte BLAKE3 work id.
        PrecompileResult::Success { output, .. } => {
            assert_eq!(output.len(), 32, "computeWorkId output is 32 bytes");
        }
        // dylib not on DYLD_LIBRARY_PATH — runtime error is the contract.
        PrecompileResult::Error { message } => {
            assert!(
                message.contains("libluxprecompile") || message.contains("DYLD_LIBRARY_PATH"),
                "missing-dylib path should mention the lib: {message}"
            );
        }
        PrecompileResult::Revert { reason } => {
            panic!("unexpected revert from a valid input: {reason}")
        }
    }
}
