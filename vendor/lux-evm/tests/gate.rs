// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! The classical-precompile gate, driven through real bytecode.
//!
//! The gate is only worth anything at the dispatch chokepoint a contract
//! actually reaches, so these `CALL` a precompile from the interpreter and read
//! the success flag the contract saw.

mod common;

use common::{run, Run, LIMIT};
use lux_evm::Profile;
use revm::primitives::U256;

/// Calls the precompile at `address` with 32 zero bytes and returns the `CALL`
/// success flag as a 32-byte word.
///
/// ```text
/// PUSH1 20  retLength     PUSH1 00  retOffset
/// PUSH1 20  argsLength    PUSH1 00  argsOffset
/// PUSH1 00  value         PUSH1 xx  address     GAS  CALL
/// PUSH1 00  MSTORE        PUSH1 20  PUSH1 00    RETURN
/// ```
macro_rules! call_precompile {
    ($address:expr) => {
        &[
            0x60, 0x20, 0x60, 0x00, 0x60, 0x20, 0x60, 0x00, 0x60, 0x00, 0x60, $address, 0x5a, 0xf1,
            0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xf3,
        ]
    };
}

const CALL_SHA256: &[u8] = call_precompile!(0x02);
const CALL_BN256_ADD: &[u8] = call_precompile!(0x06);
const CALL_IDENTITY: &[u8] = call_precompile!(0x04);

fn flag(out: &[u8]) -> U256 {
    U256::from_be_slice(out)
}

fn call(code: &'static [u8], profile: Profile) -> common::Outcome {
    run(Run {
        code,
        profile,
        ..Default::default()
    })
}

#[test]
fn a_permissive_chain_runs_the_classical_precompiles() {
    for code in [CALL_SHA256, CALL_BN256_ADD, CALL_IDENTITY] {
        let out = call(code, Profile::default());
        assert_eq!(
            flag(&out.output),
            U256::from(1u64),
            "a nil profile admits every family"
        );
    }
}

#[test]
fn a_strict_chain_refuses_sha256() {
    let out = call(CALL_SHA256, Profile::strict());
    assert_eq!(
        flag(&out.output),
        U256::ZERO,
        "sha256 is forbidden under strict-PQ, so the CALL reports failure"
    );
}

#[test]
fn the_refusal_costs_the_caller_the_gas_it_forwarded() {
    // Go charges RequiredGas and only then refuses, and the refusal is an error
    // rather than a revert — so the call frame keeps nothing. A gate that made
    // refusal cheap would let a contract read the chain's posture off its own
    // gas meter.
    let allowed = call(CALL_SHA256, Profile::default());
    let refused = call(CALL_SHA256, Profile::strict());
    assert!(
        refused.gas_used > allowed.gas_used,
        "refused {} gas, allowed {}",
        refused.gas_used,
        allowed.gas_used
    );
    // Everything the sub-call was given is gone; what is left is the
    // sixty-fourth EIP-150 keeps back from the caller, and nothing more.
    let unused = LIMIT - refused.gas_used;
    assert!(
        unused <= LIMIT / 64,
        "{unused} gas survived a refused call; only the retained 1/64 should"
    );
}

#[test]
fn the_lux_profile_keeps_bn256_for_ethereum_contracts() {
    assert_eq!(
        flag(&call(CALL_BN256_ADD, Profile::lux()).output),
        U256::from(1u64),
        "0x06 is the deliberate carve-out: adding the point at infinity to \
         itself is valid and must still answer"
    );
    assert_eq!(
        flag(&call(CALL_SHA256, Profile::lux()).output),
        U256::ZERO,
        "and the carve-out is only bn256"
    );
    assert_eq!(
        flag(&call(CALL_BN256_ADD, Profile::strict()).output),
        U256::ZERO,
        "the maximal profile refuses bn256 too"
    );
}

#[test]
fn identity_and_modexp_answer_under_every_profile() {
    // Go's opForPrecompile has no case for them, so they fall to the unknown
    // family, which no profile refuses.
    assert_eq!(
        flag(&call(CALL_IDENTITY, Profile::strict()).output),
        U256::from(1u64)
    );
}
