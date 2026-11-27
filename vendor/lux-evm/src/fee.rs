// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! What a block owes, and what its header must say about it.
//!
//! Go derives four header fields from the parent and from nothing else — the
//! gas limit, the fee window at the front of `extraData`, the base fee and the
//! block gas cost — and after execution holds the block's tips to that cost. A
//! proposer chooses none of them. Each is written here as `luxfi/evm` v1.104.56
//! (the version luxd v1.37.9 links) writes it:
//!
//! - the window: `plugin/evm/upgrade/feewindow/window.go:17-95` and
//!   `plugin/evm/header/dynamic_fee_windower.go:124-156` (`feeWindow`);
//! - the base fee: `dynamic_fee_windower.go:34-118` (`baseFeeFromWindow`);
//! - the block gas cost: `plugin/evm/header/block_gas_cost.go:27-76` and
//!   `plugin/evm/blockgascost/cost.go:19-59`;
//! - the extra: `plugin/evm/header/extra.go:74-135`, and the predicate results
//!   after the window, `predicate/predicate_results.go:148-208`;
//! - the block fee: `consensus/dummy/consensus.go:262-320` (`verifyBlockFee`).
//!
//! WHERE A BLOCK'S COST LIVES. Go keeps it beside the header — a map keyed
//! by the header's hash (`customtypes/header_ext.go:22-48`), persisted under
//! `bgc` (`customrawdb/database_ext.go`) — and in the header only where the
//! header has a place for it. Its builder writes the cost only into that map
//! (`FinalizeAndAssemble`, consensus.go:366-434); its live VM fills a header's
//! absent cost from the parent before verifying (`ensureBlockGasCost`,
//! plugin/evm/block.go:288-319). This crate is that VM: a header that carries
//! a cost carries the derived one, a header that carries none is under the
//! derived one, and a child derives from its parent's cost, carried or filled
//! — none for the genesis, whose sixteen fields have no place for one. What
//! this crate builds carries its cost whenever it is not zero, so Go's import
//! path, which reads an absent cost as zero (consensus.go:169-175), and cevm
//! take it as well.
//!
//! The two Go paths part on one kind of block: one Go built less than
//! `TargetBlockRate` after its parent, owing a cost it did not write. The live
//! VM and this crate take it; the import path and cevm refuse it. None of the
//! 1,082,780 lux-mainnet blocks, nor any Zoo or Lux testnet block, is such a
//! block: every one is at least two seconds after its parent. And a block whose
//! cost Go never persisted — the genesis, a tip written by state sync
//! (`syncervm_client.go:271`, `rawdb.WriteBlock`), a block from a luxd older
//! than the `bgc` key — derives its child from nil, the minimum, after a
//! restart, where a node that verified it derives [`cost`] from its value. The
//! two differ only for a child less than `TargetBlockRate` after that parent;
//! this crate is the second.

use alloy_consensus::{Header, ReceiptEnvelope, Transaction as _, TxEnvelope};
use alloy_primitives::{Address, Bytes, U256};

use crate::rules::Chain;

/// Seconds of gas use the window tracks: `feewindow.WindowLen`.
pub const WINDOW_LEN: usize = 10;

/// Bytes the window encodes to, at the front of every header's extra:
/// `feewindow.WindowSize`.
pub const WINDOW_SIZE: usize = 8 * WINDOW_LEN;

/// What `predicate.Results{}.Bytes()` writes for a block none of whose
/// transactions carries a predicate: version 0, no transactions. After Durango
/// it follows the window in every such block's extra.
pub const NO_PREDICATES: [u8; 6] = [0; 6];

/// Where fees go when a chain names no fee recipient: `constants.BlackholeAddr`.
/// Also the coinbase Go's VM mines with when none is configured
/// (`plugin/evm/vm.go:533-540`).
pub const BLACKHOLE: Address =
    Address::new([1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);

/// A chain's fee schedule: `commontype.FeeConfig`, read from the genesis
/// document's `config.feeConfig`.
///
/// Every field is 64 bits because that is the width Go computes it at: it
/// reads `GasLimit`, `TargetGas` and the three block gas cost fields with
/// `Uint64()` and the first block's base fee with `MinBaseFee.Int64()`. A
/// document with a wider value is one Go truncates, and is refused where it is
/// read rather than truncated here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    pub gas_limit: u64,
    pub target_block_rate: u64,
    pub min_base_fee: u64,
    pub target_gas: u64,
    pub base_fee_change_denominator: u64,
    pub min_block_gas_cost: u64,
    pub max_block_gas_cost: u64,
    pub block_gas_cost_step: u64,
}

impl Config {
    /// `params/extras DefaultFeeConfig`: what a genesis without a `feeConfig`
    /// runs under (`parseGenesis`, plugin/evm/vm.go).
    pub const DEFAULT: Self = Self {
        gas_limit: 8_000_000,
        target_block_rate: 2,
        min_base_fee: 25_000_000_000,
        target_gas: 15_000_000,
        base_fee_change_denominator: 36,
        min_block_gas_cost: 0,
        max_block_gas_cost: 1_000_000,
        block_gas_cost_step: 200_000,
    };

    /// `FeeConfig.Verify` (commontype/fee_config.go): what Go refuses to
    /// start a chain with.
    pub fn verify(&self) -> Result<(), String> {
        let why = match self {
            c if c.gas_limit == 0 => "gasLimit must be above 0",
            c if c.target_block_rate == 0 => "targetBlockRate must be above 0",
            c if c.target_gas == 0 => "targetGas must be above 0",
            c if c.base_fee_change_denominator == 0 => "baseFeeChangeDenominator must be above 0",
            c if c.min_block_gas_cost > c.max_block_gas_cost => {
                "minBlockGasCost cannot be above maxBlockGasCost"
            }
            c if c.min_base_fee > i64::MAX as u64 => {
                "minBaseFee is read as an int64 for the first block, and this one is wider"
            }
            _ => return Ok(()),
        };
        Err(why.into())
    }
}

/// A chain's blob schedule under Cancun: `blobSchedule.cancun`, or
/// `DefaultCancunBlobConfig` where the document gives none.
///
/// No block on a Lux chain carries a blob — a blob transaction is refused, and
/// a header's `blobGasUsed` must be zero — so the excess stays at zero and the
/// price at its floor, one wei. The schedule still names both, because
/// BLOBBASEFEE answers the price and a header carries the excess.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Blob {
    pub target: u64,
    pub max: u64,
    pub fraction: u64,
}

impl Blob {
    /// `params.DefaultCancunBlobConfig`.
    pub const CANCUN: Self = Self {
        target: 3,
        max: 6,
        fraction: 3_338_477,
    };
}

/// Blob gas per blob: `params.BlobTxBlobGasPerBlob`.
pub const BLOB_GAS: u64 = 1 << 17;

/// The excess blob gas a Cancun block on top of `parent` carries:
/// `eip4844.CalcExcessBlobGas` before Osaka (luxfi/geth v1.20.3
/// consensus/misc/eip4844/eip4844.go:131-150). A parent without the field
/// counts as zero.
pub fn excess(blob: &Blob, parent: &Header) -> u64 {
    let (excess, used) = match parent.excess_blob_gas {
        Some(e) => (e, parent.blob_gas_used.unwrap_or_default()),
        None => (0, 0),
    };
    let total = excess.wrapping_add(used);
    let target = blob.target * BLOB_GAS;
    total.saturating_sub(target)
}

/// The gas used in each of the last [`WINDOW_LEN`] seconds, oldest first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Window(pub [u64; WINDOW_LEN]);

impl Window {
    /// The window at the front of `extra`; none when it is shorter than one.
    pub fn parse(extra: &[u8]) -> Option<Self> {
        let front = extra.get(..WINDOW_SIZE)?;
        let mut w = [0u64; WINDOW_LEN];
        for (i, v) in w.iter_mut().enumerate() {
            let mut word = [0u8; 8];
            word.copy_from_slice(&front[8 * i..8 * i + 8]);
            *v = u64::from_be_bytes(word);
        }
        Some(Self(w))
    }

    /// Add `gas` to the current second, saturating as `Window.Add` does.
    pub fn add(&mut self, gas: u64) {
        self.0[WINDOW_LEN - 1] = self.0[WINDOW_LEN - 1].saturating_add(gas);
    }

    /// Drop the oldest `n` seconds and open `n` empty ones.
    pub fn shift(&mut self, n: u64) {
        if n >= WINDOW_LEN as u64 {
            *self = Self::default();
            return;
        }
        let n = n as usize;
        self.0.copy_within(n.., 0);
        self.0[WINDOW_LEN - n..].fill(0);
    }

    /// The gas the window holds, saturating as `Window.Sum` does.
    pub fn sum(&self) -> u64 {
        self.0.iter().fold(0u64, |s, v| s.saturating_add(*v))
    }

    pub fn bytes(&self) -> [u8; WINDOW_SIZE] {
        let mut out = [0u8; WINDOW_SIZE];
        for (i, v) in self.0.iter().enumerate() {
            out[8 * i..8 * i + 8].copy_from_slice(&v.to_be_bytes());
        }
        out
    }
}

/// Why a child's fields cannot be derived from its parent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The parent's extra is shorter than a window (`ErrWindowInsufficientLength`).
    ShortWindow(usize),
    /// The child is dated before its parent (`errInvalidTimestamp`).
    BeforeParent { parent: u64, time: u64 },
    /// The parent has no base fee to derive from.
    NoBaseFee,
    /// A base fee past 64 bits. Go carries one; revm's block environment and
    /// this crate's header do not. No schedule a Lux chain runs reaches it: it
    /// takes hundreds of consecutive blocks at twice the target.
    Wide(U256),
    /// A block before the EVM upgrade, whose rules are not run here.
    PreEvm,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::ShortWindow(n) => {
                write!(f, "the parent's extra is {n} bytes, less than a fee window")
            }
            Error::BeforeParent { parent, time } => {
                write!(f, "timestamp {time} before its parent's {parent}")
            }
            Error::NoBaseFee => write!(f, "the parent has no base fee"),
            Error::Wide(fee) => write!(f, "a base fee of {fee} does not fit 64 bits"),
            Error::PreEvm => write!(f, "a block before the EVM upgrade"),
        }
    }
}

/// The window a block at `time` on top of `parent` carries: `feeWindow`.
///
/// Empty for the first block after the genesis or after the EVM upgrade.
/// Otherwise the parent's window, with the parent's own gas added to its
/// current second, rolled forward by the seconds between them.
pub fn window(chain: &Chain, parent: &Header, time: u64) -> Result<Window, Error> {
    if !chain.at(parent.timestamp).evm || parent.number == 0 {
        return Ok(Window::default());
    }
    let mut w =
        Window::parse(&parent.extra_data).ok_or(Error::ShortWindow(parent.extra_data.len()))?;
    if time < parent.timestamp {
        return Err(Error::BeforeParent {
            parent: parent.timestamp,
            time,
        });
    }
    w.add(parent.gas_used);
    w.shift(time - parent.timestamp);
    Ok(w)
}

/// The base fee a block at `time` on top of `parent` carries:
/// `baseFeeFromWindow`.
///
/// The minimum for the first block; otherwise the parent's, moved by how far
/// the window's gas is from the target — up by at least one wei when over it,
/// down by at least one wei per whole window elapsed when under it — and never
/// below the minimum. A window exactly at the target keeps the parent's fee
/// even outside the bounds, as Go does "for legacy reasons".
pub fn base_fee(chain: &Chain, config: &Config, parent: &Header, time: u64) -> Result<u64, Error> {
    if !chain.at(parent.timestamp).evm || parent.number == 0 {
        return Ok(config.min_base_fee);
    }
    let total = window(chain, parent, time)?.sum();
    let parent_fee = parent.base_fee_per_gas.ok_or(Error::NoBaseFee)?;
    let target = config.target_gas;
    if total == target {
        return Ok(parent_fee);
    }
    let fee = U256::from(parent_fee);
    let delta = |deviation: u64| {
        (U256::from(deviation) * fee
            / U256::from(target)
            / U256::from(config.base_fee_change_denominator))
        .max(U256::from(1u64))
    };
    let next = if total > target {
        fee + delta(total - target)
    } else {
        // `window` has already refused a time before the parent's.
        let windows = (time - parent.timestamp) / WINDOW_LEN as u64;
        let mut down = delta(target - total);
        if windows > 1 {
            down *= U256::from(windows);
        }
        fee.saturating_sub(down)
    };
    let next = next.max(U256::from(config.min_base_fee));
    u64::try_from(next).map_err(|_| Error::Wide(next))
}

/// The block gas cost of a block at `time` on top of a parent at
/// `parent_time` whose own cost is `parent_cost`: `header.BlockGasCost`.
///
/// `parent_cost` is `None` for the genesis, whose sixteen fields have no
/// place for one, and that child's cost is the minimum; any other parent's is
/// its own cost, carried or filled (see the module's note). The parent's cost moves
/// by one step per second the child is faster or slower than the target rate,
/// clamped to the schedule's bounds, with Go's saturation where a step
/// overflows (`blockgascost.BlockGasCost`).
pub fn cost(config: &Config, parent_cost: Option<u64>, parent_time: u64, time: u64) -> u64 {
    let Some(parent) = parent_cost else {
        return config.min_block_gas_cost;
    };
    let elapsed = time.saturating_sub(parent_time);
    let deviation = config.target_block_rate.abs_diff(elapsed);
    let change = config.block_gas_cost_step.saturating_mul(deviation);
    let cost = if elapsed > config.target_block_rate {
        parent
            .checked_sub(change)
            .unwrap_or(config.min_block_gas_cost)
    } else {
        parent
            .checked_add(change)
            .unwrap_or(config.max_block_gas_cost)
    };
    cost.clamp(config.min_block_gas_cost, config.max_block_gas_cost)
}

/// The extra a block carries: its window, and from Durango the predicate
/// results of its transactions after it (`SetPredicateBytesInExtra`) — which
/// for a block this crate can run are always [`NO_PREDICATES`].
pub fn extra(window: &Window, durango: bool) -> Bytes {
    let mut out = window.bytes().to_vec();
    if durango {
        out.extend_from_slice(&NO_PREDICATES);
    }
    out.into()
}

/// Whether the tips in a block cover its block gas cost: `verifyBlockFee`.
///
/// Each transaction's effective tip over the base fee, times the gas it used,
/// summed; divided by the base fee, that is gas the block bought, and it must
/// be at least `cost`. A base fee of zero is refused outright, as Go refuses
/// it, whatever the cost.
pub fn covered(
    base_fee: u64,
    cost: u64,
    transactions: &[TxEnvelope],
    receipts: &[ReceiptEnvelope],
) -> Result<(), String> {
    if base_fee == 0 {
        return Err("a base fee of 0".into());
    }
    let mut paid = U256::ZERO;
    let mut before = 0u64;
    for (tx, receipt) in transactions.iter().zip(receipts) {
        let cumulative = receipt.cumulative_gas_used();
        let used = cumulative - before;
        before = cumulative;
        let tip = tx
            .effective_tip_per_gas(base_fee)
            .ok_or_else(|| format!("a fee cap of {} under the base fee", tx.max_fee_per_gas()))?;
        paid += U256::from(tip) * U256::from(used);
    }
    let bought = paid / U256::from(base_fee);
    if bought < U256::from(cost) {
        return Err(format!(
            "insufficient gas to cover the block cost: expected {cost} but got {bought}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(number: u64, timestamp: u64, gas_used: u64, base_fee: u64, w: Window) -> Header {
        Header {
            number,
            timestamp,
            gas_used,
            base_fee_per_gas: Some(base_fee),
            extra_data: extra(&w, true),
            ..Default::default()
        }
    }

    const MAINNET: Config = Config {
        target_gas: 500_000_000,
        gas_limit: 12_000_000,
        ..Config::DEFAULT
    };

    #[test]
    fn the_window_round_trips_and_rolls() {
        let mut w = Window([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        assert_eq!(Window::parse(&w.bytes()), Some(w));
        assert_eq!(Window::parse(&w.bytes()[..79]), None);
        w.shift(3);
        assert_eq!(w, Window([4, 5, 6, 7, 8, 9, 10, 0, 0, 0]));
        w.add(u64::MAX);
        assert_eq!(w.0[9], u64::MAX, "the current second saturates");
        assert_eq!(w.sum(), u64::MAX, "and so does the sum");
        w.shift(10);
        assert_eq!(w, Window::default());
    }

    #[test]
    fn the_first_block_carries_an_empty_window_and_the_minimum() {
        let chain = Chain::lux();
        let genesis = Header {
            base_fee_per_gas: Some(1),
            ..Default::default()
        };
        assert_eq!(window(&chain, &genesis, 99), Ok(Window::default()));
        assert_eq!(base_fee(&chain, &MAINNET, &genesis, 99), Ok(25_000_000_000));
        assert_eq!(
            cost(&MAINNET, None, 0, 0),
            0,
            "a nil parent cost is the minimum"
        );
    }

    #[test]
    fn the_parents_gas_lands_in_its_second_and_rolls_with_the_time() {
        let chain = Chain::lux();
        let parent = header(
            5,
            100,
            21_000,
            25_000_000_000,
            Window([0, 0, 0, 0, 0, 0, 0, 0, 0, 7]),
        );
        assert_eq!(
            window(&chain, &parent, 100).expect("same second").0[9],
            21_007
        );
        let later = window(&chain, &parent, 102).expect("two seconds on");
        assert_eq!(later.0[7], 21_007);
        assert_eq!(later.0[9], 0);
        assert_eq!(
            window(&chain, &parent, 99),
            Err(Error::BeforeParent {
                parent: 100,
                time: 99
            })
        );
    }

    #[test]
    fn the_base_fee_moves_as_go_moves_it() {
        let chain = Chain::lux();
        let config = Config {
            target_gas: 15_000_000,
            ..Config::DEFAULT
        };
        let fee = 25_000_000_000u64;
        // Over the target: up by (total - target) * fee / target / 36.
        let full = header(
            9,
            100,
            12_000_000,
            fee,
            Window([0, 0, 0, 0, 0, 0, 0, 0, 0, 12_000_000]),
        );
        let over = 24_000_000u64 - 15_000_000;
        let want = fee
            + (U256::from(over) * U256::from(fee) / U256::from(15_000_000u64) / U256::from(36u64))
                .to::<u64>();
        assert_eq!(base_fee(&chain, &config, &full, 100), Ok(want));
        // Exactly the target keeps the parent's, even below the minimum.
        let low = header(9, 100, 15_000_000, 1, Window::default());
        assert_eq!(base_fee(&chain, &config, &low, 100), Ok(1));
        // Under the target, several windows on: the step counts each window,
        // and the result never falls under the minimum.
        let busy = header(9, 100, 0, 30_000_000_000, Window::default());
        let step = 30_000_000_000u64 / 36;
        assert_eq!(
            base_fee(&chain, &config, &busy, 160),
            Ok(30_000_000_000 - 6 * step)
        );
        assert_eq!(
            base_fee(&chain, &config, &busy, 170),
            Ok(25_000_000_000),
            "floored"
        );
        let high = header(9, 100, 0, 90_000_000_000, Window::default());
        let one = 90_000_000_000u64 / 36;
        assert_eq!(
            base_fee(&chain, &config, &high, 105),
            Ok(90_000_000_000 - one)
        );
        assert_eq!(
            base_fee(&chain, &config, &high, 125),
            Ok(90_000_000_000 - 2 * one)
        );
    }

    #[test]
    fn the_block_gas_cost_steps_toward_the_target_rate() {
        let c = MAINNET;
        assert_eq!(cost(&c, Some(0), 100, 100), 400_000, "two seconds fast");
        assert_eq!(cost(&c, Some(0), 100, 101), 200_000, "one second fast");
        assert_eq!(
            cost(&c, Some(300_000), 100, 102),
            300_000,
            "on the target, unchanged"
        );
        assert_eq!(
            cost(&c, Some(300_000), 100, 104),
            0,
            "slow, down to the floor"
        );
        assert_eq!(
            cost(&c, Some(900_000), 100, 100),
            1_000_000,
            "fast, up to the ceiling"
        );
        assert_eq!(
            cost(&c, Some(0), 100, 99),
            400_000,
            "a time before the parent is no time"
        );
        let steep = Config {
            block_gas_cost_step: u64::MAX,
            ..c
        };
        assert_eq!(
            cost(&steep, Some(1), 100, 100),
            1_000_000,
            "an overflowing step saturates"
        );
    }

    #[test]
    fn the_extra_is_the_window_and_after_durango_no_predicates() {
        let w = Window([3; WINDOW_LEN]);
        assert_eq!(extra(&w, false).len(), 80);
        let e = extra(&w, true);
        assert_eq!(e.len(), 86);
        assert_eq!(&e[..80], &w.bytes()[..]);
        assert_eq!(&e[80..], &NO_PREDICATES);
    }

    #[test]
    fn the_excess_blob_gas_falls_to_zero_below_the_target() {
        let b = Blob::CANCUN;
        assert_eq!(excess(&b, &Header::default()), 0, "no field is zero");
        let parent = Header {
            excess_blob_gas: Some(0),
            blob_gas_used: Some(0),
            ..Default::default()
        };
        assert_eq!(excess(&b, &parent), 0);
        let full = Header {
            excess_blob_gas: Some(BLOB_GAS),
            blob_gas_used: Some(6 * BLOB_GAS),
            ..Default::default()
        };
        assert_eq!(excess(&b, &full), 4 * BLOB_GAS);
    }

    #[test]
    fn go_refuses_the_schedules_it_refuses() {
        assert_eq!(Config::DEFAULT.verify(), Ok(()));
        assert!(Config {
            target_gas: 0,
            ..Config::DEFAULT
        }
        .verify()
        .is_err());
        assert!(Config {
            min_block_gas_cost: 2,
            max_block_gas_cost: 1,
            ..Config::DEFAULT
        }
        .verify()
        .is_err());
        assert!(Config {
            min_base_fee: u64::MAX,
            ..Config::DEFAULT
        }
        .verify()
        .is_err());
    }
}
