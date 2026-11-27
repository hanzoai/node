// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! What a transaction costs before a single opcode runs.
//!
//! Intrinsic gas decides whether a transaction fits in a block at all, so an
//! implementation that computes it differently disagrees about the contents of
//! the block rather than about the result of one call. Every term is Ethereum's
//! and revm already has them; the one Lux difference is *when* the initcode
//! term applies.
//!
//! `luxfi/evm@v1.104.30 core/state_transition.go IntrinsicGas` charges the
//! EIP-3860 word cost under `IsDurango` — a Lux upgrade — where Ethereum
//! charges it under Shanghai. Every Lux network has Durango live from genesis
//! (`InitiallyActiveTime` is the Unix epoch), so the two agree in production
//! and differ only on a chain configured without it.

use revm::interpreter::gas::{calculate_initial_tx_gas, INITCODE_WORD_COST};
use revm::primitives::hardfork::SpecId;

use crate::rules::Rules;

/// The gas a transaction owes before execution begins.
///
/// `accounts` and `slots` are the access-list counts. They are counts rather
/// than the list itself because `luxfi/evm` lets a chain register a *predicate*
/// for an access-list address and price that entry with its own function; a
/// chain that has done so has to price those entries itself, and passing counts
/// makes that the caller's decision rather than a silent wrong answer here.
pub fn intrinsic(
    spec: SpecId,
    rules: Rules,
    input: &[u8],
    create: bool,
    accounts: u64,
    slots: u64,
) -> u64 {
    let gas = calculate_initial_tx_gas(spec, input, create, accounts, slots, 0).initial_gas;
    if create && !rules.durango && spec.is_enabled_in(SpecId::SHANGHAI) {
        // revm charged the initcode words on Shanghai's schedule. Lux does not
        // charge them until Durango, so take them back off.
        return gas - words(input.len()) * INITCODE_WORD_COST;
    }
    gas
}

/// Initcode length in 32-byte words, rounded up — geth's `toWordSize`.
const fn words(len: usize) -> u64 {
    (len as u64).div_ceil(32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Chain;

    const SHANGHAI: SpecId = SpecId::SHANGHAI;

    fn lux() -> Rules {
        Chain::lux().at(0)
    }

    #[test]
    fn a_plain_transfer_is_the_bare_stipend() {
        assert_eq!(intrinsic(SHANGHAI, lux(), &[], false, 0, 0), 21_000);
    }

    #[test]
    fn calldata_is_priced_by_byte_and_zeros_cost_less() {
        assert_eq!(
            intrinsic(SHANGHAI, lux(), &[0xde, 0xad, 0xbe, 0xef], false, 0, 0),
            21_000 + 4 * 16
        );
        assert_eq!(
            intrinsic(SHANGHAI, lux(), &[0, 0, 0, 0], false, 0, 0),
            21_000 + 4 * 4
        );
    }

    #[test]
    fn deploying_pays_the_creation_stipend_and_its_initcode() {
        assert_eq!(
            intrinsic(SHANGHAI, lux(), &[0xde, 0xad, 0xbe, 0xef], true, 0, 0),
            53_000 + 4 * 16 + 2,
            "one word of initcode at 2 gas, on top of the 32000 creation stipend"
        );
    }

    #[test]
    fn before_durango_the_initcode_words_are_free() {
        let before = Chain {
            durango: Some(100),
            ..Chain::lux()
        };
        let code = [0u8; 64];
        assert_eq!(
            intrinsic(SHANGHAI, before.at(99), &code, true, 0, 0)
                + words(code.len()) * INITCODE_WORD_COST,
            intrinsic(SHANGHAI, before.at(100), &code, true, 0, 0),
            "the whole difference is two words at 2 gas"
        );
    }

    #[test]
    fn the_initcode_term_only_applies_to_creation() {
        let code = [0u8; 64];
        let before = Chain {
            durango: Some(100),
            ..Chain::lux()
        };
        assert_eq!(
            intrinsic(SHANGHAI, before.at(99), &code, false, 0, 0),
            intrinsic(SHANGHAI, before.at(100), &code, false, 0, 0),
            "a call carrying data is unaffected by the initcode rule"
        );
    }

    #[test]
    fn an_access_list_entry_costs_its_address_and_its_slots() {
        assert_eq!(
            intrinsic(SHANGHAI, lux(), &[], false, 1, 3),
            21_000 + 2_400 + 3 * 1_900
        );
    }

    #[test]
    fn a_word_is_rounded_up() {
        assert_eq!(words(0), 0);
        assert_eq!(words(1), 1);
        assert_eq!(words(32), 1);
        assert_eq!(words(33), 2);
    }
}
