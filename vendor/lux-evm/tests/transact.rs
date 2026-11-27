// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! The rules, driven through a real transaction.
//!
//! A policy that is only unit-tested is a policy that agrees with itself. These
//! read the committed balances back, so what is checked is what a validator
//! would see.

mod common;

use common::{run, Run, BASEFEE, PRICE};
use lux_evm::Chain;
use revm::primitives::U256;

/// `PUSH1 0 PUSH1 0 SSTORE STOP` — clears a set slot, which is the only way to
/// earn a refund counter.
const CLEAR_SLOT: &[u8] = &[0x60, 0x00, 0x60, 0x00, 0x55, 0x00];

fn clearing(chain: Chain) -> common::Outcome {
    run(Run {
        chain,
        code: CLEAR_SLOT,
        slot: Some((U256::ZERO, U256::from(1u64))),
        ..Default::default()
    })
}

#[test]
fn lux_pays_the_beneficiary_the_whole_fee_and_ethereum_burns_the_base() {
    let lux = run(Run::default());
    let eth = run(Run {
        chain: Chain::ethereum(),
        ..Default::default()
    });

    assert_eq!(lux.gas_used, 21_000, "a bare call is intrinsic gas only");
    assert_eq!(eth.gas_used, 21_000);

    assert_eq!(
        lux.coinbase,
        U256::from(21_000u128 * PRICE),
        "Lux credits gasUsed * gasPrice with nothing burned"
    );
    assert_eq!(
        eth.coinbase,
        U256::from(21_000u128 * (PRICE - BASEFEE as u128)),
        "Ethereum credits the tip and burns the base fee"
    );
    assert!(
        lux.coinbase > eth.coinbase,
        "the difference is the whole point of the rule"
    );
}

#[test]
fn the_fee_split_halves_what_the_beneficiary_receives() {
    let whole = run(Run::default());
    let half = run(Run {
        chain: Chain {
            split: Some(0),
            ..Chain::lux()
        },
        ..Default::default()
    });
    assert_eq!(half.coinbase, whole.coinbase / U256::from(2u64));
    assert_eq!(
        half.gas_used, whole.gas_used,
        "the split moves money, not gas"
    );
}

#[test]
fn a_rule_that_has_not_activated_yet_does_not_apply() {
    let split_later = Chain {
        split: Some(10),
        ..Chain::lux()
    };
    assert_eq!(
        run(Run {
            chain: split_later,
            time: 9,
            ..Default::default()
        })
        .coinbase,
        run(Run::default()).coinbase,
        "the block before the upgrade pays the whole fee"
    );
    assert_eq!(
        run(Run {
            chain: split_later,
            time: 10,
            ..Default::default()
        })
        .coinbase,
        run(Run::default()).coinbase / U256::from(2u64),
        "and the block it activates in pays half"
    );
}

#[test]
fn lux_grants_no_refund_for_clearing_storage() {
    let lux = clearing(Chain::lux());
    let eth = clearing(Chain::ethereum());

    assert!(
        lux.gas_used > eth.gas_used,
        "Ethereum refunds the storage clear ({} used) and Lux does not ({} used)",
        eth.gas_used,
        lux.gas_used
    );
    // Clearing a set slot is worth 4800 after EIP-3529, under the cap of a
    // fifth of the gas spent. That whole 4800 is what Lux discards.
    const CLEAR_REFUND: u64 = 4_800;
    assert!(
        CLEAR_REFUND < lux.gas_used / 5,
        "the cap is not what binds here"
    );
    assert_eq!(
        eth.gas_used,
        lux.gas_used - CLEAR_REFUND,
        "the whole difference is the refund Lux discards"
    );
}

#[test]
fn the_sender_is_charged_for_the_refund_it_does_not_get() {
    // The refund rule has two halves and one switch: the sender is not handed
    // the gas back, and the block reports it as used. Reading the sender's
    // balance is the half a gas-only test would miss.
    let lux = clearing(Chain::lux());
    let eth = clearing(Chain::ethereum());

    assert_eq!(
        lux.spent,
        U256::from(lux.gas_used as u128 * PRICE),
        "everything the sender paid is gas at the price it offered"
    );
    assert_eq!(eth.spent, U256::from(eth.gas_used as u128 * PRICE));
    assert!(
        lux.spent > eth.spent,
        "the sender keeps {} wei more on Ethereum",
        lux.spent - eth.spent
    );
}

#[test]
fn the_beneficiary_is_paid_on_the_gas_the_rules_leave_behind() {
    // The two rules compose: under Lux the beneficiary is paid for gas that
    // Ethereum would have handed back to the sender.
    let lux = clearing(Chain::lux());
    assert_eq!(lux.coinbase, U256::from(lux.gas_used as u128 * PRICE));
}
