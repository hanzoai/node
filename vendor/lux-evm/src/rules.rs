// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! The rules that make Lux not Ethereum.
//!
//! There are four, and in Go they are four `if` branches spread across a
//! seven-hundred-line state transition. Here they are one value, because that
//! is what they are: a chain says when each rule turns on, a block time selects
//! them, and the selected rules answer what the sender pays, what the sender
//! gets back, and what the beneficiary receives.
//!
//! Their sources, so a reader can check rather than trust:
//!
//! - **no refunds.** `luxfi/evm@v1.104.30 core/state_transition.go refundGas`:
//!   the refund counter is applied only `if !isEVM`. `luxfi/geth@v1.20.2
//!   core/state_transition.go calcRefund` says the same as `return 0`.
//! - **the whole fee, not the tip.** `luxfi/evm` computes
//!   `fee = gasUsed * gasPrice` with no base-fee subtraction, where Ethereum
//!   pays the beneficiary `gasUsed * (gasPrice - baseFee)` and burns the rest.
//! - **the split.** `luxfi/evm@v1.104.30 core/fee_split.go creditTxFee`: once
//!   `IsFeeSplit(blockTime)`, the beneficiary receives `floor(fee/2)` and the
//!   other half is burned by never being credited to anyone.
//! - **initcode is metered from Durango, not from Shanghai.**
//!   `luxfi/evm@v1.104.30 core/state_transition.go IntrinsicGas` charges the
//!   EIP-3860 word cost under `IsDurango`, a Lux upgrade, where Ethereum
//!   charges it under Shanghai. On a chain with Durango live from genesis the
//!   two agree; on one without, a Rust node overcharges every deployment.
//!
//! - **the Ethereum fork is a Lux upgrade.** `params.SetEthUpgrades`
//!   (luxfi/evm params/config_extra.go:96-150) sets Shanghai to Durango and
//!   Cancun to Quasar wherever those are scheduled; otherwise the document's
//!   `shanghaiTime`/`cancunTime` stand, and `parseGenesis` sets both to 0 when
//!   the document names neither. [`Chain::spec`] turns that into revm's fork.
//!
//! The field names are Go's — `evm` is `IsEVM`, `split` is `IsFeeSplit`,
//! `durango` is `IsDurango`, `shanghai` and `cancun` are the geth config's
//! times — so that one rule has one name in both languages.

use alloy_primitives::Address;
use revm::primitives::hardfork::SpecId;

/// The warp precompile: the address whose access-list entries are predicates
/// once it is active.
pub const WARP: Address = Address::new([
    0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x05,
]);

/// When a chain turns each rule on. `None` is never; `Some(t)` is every block
/// at or after `t`.
///
/// This is `isTimestampForked` from `luxfi/geth params/config.go`: a fork whose
/// timestamp is unset is not active, and one that is set is active from that
/// second onward.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Chain {
    /// `ChainConfig.EVMTimestamp` — Lux gas accounting.
    pub evm: Option<u64>,
    /// The fee-split upgrade.
    pub split: Option<u64>,
    /// `NetworkUpgrades.DurangoTimestamp` — among other things, when initcode
    /// starts being metered and when the predicate results follow the fee
    /// window in a header's extra.
    pub durango: Option<u64>,
    /// `ShanghaiTime`: PUSH0, initcode limits, and a withdrawals root in
    /// every header after the genesis.
    pub shanghai: Option<u64>,
    /// `CancunTime`: transient storage, MCOPY, EIP-6780 SELFDESTRUCT, the blob
    /// opcodes, and the blob and beacon-root header fields.
    pub cancun: Option<u64>,
}

impl Chain {
    /// The C-chain: Lux gas accounting from genesis, no split.
    ///
    /// `LuxMainnetChainConfig` sets `EVMTimestamp: 0`, so every block a Lux
    /// chain has ever produced is under these rules. A Rust node that defaults
    /// to Ethereum's rules forks at block one.
    pub const fn lux() -> Self {
        Self {
            evm: Some(0),
            split: None,
            durango: Some(0),
            shanghai: Some(0),
            cancun: Some(0),
        }
    }

    /// Ethereum's rules: no Lux upgrade ever turns on.
    ///
    /// `durango` is left unset for symmetry with the others, and callers that
    /// want Ethereum's own initcode metering should say so with a spec at or
    /// past Shanghai and `durango: Some(0)`. Nothing on a Lux network runs
    /// this configuration; it exists so the difference each rule makes can be
    /// measured against the chain that does not have it.
    pub const fn ethereum() -> Self {
        Self {
            evm: None,
            split: None,
            durango: None,
            shanghai: None,
            cancun: None,
        }
    }

    /// The rules in force at `time`.
    pub const fn at(&self, time: u64) -> Rules {
        Rules {
            evm: forked(self.evm, time),
            split: forked(self.split, time),
            durango: forked(self.durango, time),
            shanghai: forked(self.shanghai, time),
            cancun: forked(self.cancun, time),
        }
    }

    /// The Ethereum fork a block at `time` runs: `ChainConfig.Rules` with
    /// `IsMergeTODO` (luxfi/evm params/config_extra.go:27), so before Shanghai
    /// a Lux block is Paris. Every block number fork is at 0 on a chain this
    /// crate runs, and Prague is never scheduled on one (the node refuses it).
    pub const fn spec(&self, time: u64) -> SpecId {
        let r = self.at(time);
        if r.cancun {
            SpecId::CANCUN
        } else if r.shanghai {
            SpecId::SHANGHAI
        } else {
            SpecId::MERGE
        }
    }
}

const fn forked(at: Option<u64>, time: u64) -> bool {
    match at {
        Some(t) => t <= time,
        None => false,
    }
}

/// The rules in force in one block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rules {
    /// Lux gas accounting: no refunds, and the beneficiary is paid the whole
    /// gas price rather than the tip.
    pub evm: bool,
    /// The beneficiary receives half the fee; the other half is burned.
    pub split: bool,
    /// Deploying meters its initcode by the word, and a header's extra carries
    /// predicate results after its fee window.
    pub durango: bool,
    /// Ethereum's Shanghai.
    pub shanghai: bool,
    /// Ethereum's Cancun.
    pub cancun: bool,
}

impl Rules {
    /// Whether the refund counter survives to the end of the transaction.
    ///
    /// Under Lux it does not, so `gasUsed == gasSpent` for every transaction —
    /// an `SSTORE` that clears a slot buys nothing back. Get this wrong and
    /// every block containing a storage clear reports a different `gasUsed`
    /// and hashes to a different receipt root.
    pub const fn refunds(&self) -> bool {
        !self.evm
    }

    /// The price per gas the beneficiary is credited at.
    ///
    /// Ethereum burns the base fee — the beneficiary is paid the tip. Lux burns
    /// nothing here and pays the whole price. Both branches are written for a
    /// chain at or past London, which every Lux chain is from genesis.
    pub const fn price(&self, price: u128, basefee: u128) -> u128 {
        if self.evm {
            price
        } else {
            price.saturating_sub(basefee)
        }
    }

    /// What the beneficiary receives for `used` gas.
    ///
    /// The halving is a floor and is applied to the fee, not to the price, so
    /// it rounds once at the end exactly as `creditTxFee` does with
    /// `Rsh(fee, 1)`. Halving the price first would round per gas unit and
    /// diverge by up to `used/2` wei.
    pub const fn reward(&self, used: u64, price: u128, basefee: u128) -> u128 {
        let fee = self.price(price, basefee).saturating_mul(used as u128);
        if self.split {
            fee >> 1
        } else {
            fee
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fork_is_active_from_its_second_onward() {
        let c = Chain {
            evm: Some(100),
            ..Chain::ethereum()
        };
        assert!(!c.at(99).evm);
        assert!(
            c.at(100).evm,
            "isTimestampForked is `s <= head`, not `s < head`"
        );
        assert!(c.at(101).evm);
    }

    #[test]
    fn an_unset_fork_never_activates() {
        let c = Chain::ethereum();
        assert!(!c.at(0).evm);
        assert!(!c.at(u64::MAX).evm);
        assert!(!c.at(u64::MAX).split);
    }

    #[test]
    fn the_c_chain_is_under_lux_rules_at_genesis() {
        let r = Chain::lux().at(0);
        assert!(r.evm, "EVMTimestamp is 0 on every Lux chain config");
        assert!(!r.refunds());
        assert!(!r.split);
    }

    #[test]
    fn lux_pays_the_whole_price_and_ethereum_pays_the_tip() {
        let (price, basefee, used) = (25_000_000_000u128, 20_000_000_000u128, 21_000u64);
        assert_eq!(
            Chain::lux().at(0).reward(used, price, basefee),
            used as u128 * price
        );
        assert_eq!(
            Chain::ethereum().at(0).reward(used, price, basefee),
            used as u128 * (price - basefee)
        );
    }

    #[test]
    fn the_split_floors_the_fee_once_not_the_price() {
        // An odd fee is the only way to see the difference: halving the price
        // per gas unit and halving the total disagree by up to used/2.
        let r = Rules {
            evm: true,
            split: true,
            durango: true,
            ..Rules::default()
        };
        assert_eq!(r.reward(3, 3, 0), 4, "floor(9/2), not 3*floor(3/2)");
    }

    #[test]
    fn a_zero_price_block_rewards_nothing() {
        assert_eq!(Chain::lux().at(0).reward(21_000, 0, 0), 0);
    }
}
