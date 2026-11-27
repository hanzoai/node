// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! Which precompile answers at which address, and at what price.
//!
//! Two sets meet here. revm's [`EthPrecompiles`] is Ethereum's, and the C-chain
//! serves it unchanged. On top of it the chain registers its own [`Module`]s at
//! addresses of its own choosing, and where the two land on the same address
//! the module SHADOWS the Ethereum precompile — `LuxPrecompileOverrider` is
//! consulted before the standard table in `luxfi/evm core/precompile_overrider.
//! go`, so the standard table never answers there. That order is not a detail:
//! at 0x0c the standard table computes a multi-scalar multiplication where the
//! chain computes a single scalar multiplication, and reading the wrong one
//! returns bytes no Lux contract asked for.
//!
//! A module states its price and its work as two things, because Go does and
//! because the difference is observable. `contract.DeductGas` runs at the top
//! of every module's `Run`, so a call that reads its input and then refuses it
//! has already paid; revm computes a precompile's price inside the function
//! that does the work, and its error carries no charge at all. [`Module`] keeps
//! them apart so the charge survives a refusal.
//!
//! The last thing here is the classical-precompile gate. A post-quantum chain
//! refuses the precompiles whose security rests on a problem a quantum computer
//! solves. Which families it refuses is a per-chain value, not a global: one
//! process hosts several chains and each carries its own [`Profile`] on its
//! config, exactly as Go carries `ChainConfig.PQ`.
//!
//! Two things about the Go gate matter, and both are easy to get wrong:
//!
//! 1. **Gas is charged before the refusal, not instead of it.** `(*EVM).
//!    runPrecompile` in `luxfi/geth@v1.20.2 core/vm/evm.go` deducts
//!    `RequiredGas(input)`, and only then calls `RefuseUnder`. A refusal that
//!    charged nothing would be visible in gas, and a chain that reads its own
//!    posture out of gas timing is a chain that leaks it.
//! 2. **The classification is on identity, not address.** Go type-switches on
//!    the precompile implementation so that remapping a precompile to another
//!    address does not change which family it belongs to. revm carries the same
//!    identity as [`PrecompileId`], so this maps that, not the address.
//!
//! ACTIVATION. A stateful precompile a chain turns on — by a genesis config
//! key, or as a protocol system precompile at a fixed time — is an
//! [`Activation`]. The block whose transition crosses its time writes the
//! marker Go writes (`ApplyPrecompileActivations`, luxfi/evm
//! core/state_processor_ext.go:29-119: nonce 1 and one code byte), at the
//! genesis when the time is not after it. From then on Go answers calls there
//! with the precompile; this crate answers the BLS12-381 [`MODULES`] and
//! REFUSES every other one: a transaction that reaches it cannot be in a
//! block here, where running the marker byte as code would answer something
//! Go does not.
//!
//! `identity` (0x04) and `modexp` (0x05) are deliberately ungated: Go's
//! `opForPrecompile` has no case for them, so they fall to [`Op::Unknown`],
//! which every profile admits. A [`Module`] falls there too — see [`Op::of`].

use revm::bytecode::Bytecode;
use revm::context::{Cfg, ContextTr, JournalTr, LocalContextTr};
use revm::handler::{EthPrecompiles, PrecompileProvider};
use revm::interpreter::{CallInput, CallInputs, Gas, InstructionResult, InterpreterResult};
use revm::precompile::bls12_381_const::{
    DISCOUNT_TABLE_G1_MSM, DISCOUNT_TABLE_G2_MSM, G1_ADD_BASE_GAS_FEE, G1_MSM_BASE_GAS_FEE,
    G1_MSM_INPUT_LENGTH, G2_ADD_BASE_GAS_FEE, G2_MSM_BASE_GAS_FEE, G2_MSM_INPUT_LENGTH,
    PADDED_G1_LENGTH, PADDED_G2_LENGTH, PAIRING_INPUT_LENGTH, PAIRING_MULTIPLIER_BASE,
    PAIRING_OFFSET_BASE,
};
use revm::precompile::bls12_381_utils::msm_required_gas;
use revm::precompile::{PrecompileError, PrecompileId};
use revm::primitives::{address, hardfork::SpecId, Address, Bytes, U256};

use crate::state::{Account, World};

// revm's EIP-2537 operations, which sit at the addresses the final assignment
// gives them. The modules below reuse this arithmetic and re-key and re-price
// it; the curve is the same curve, and a second implementation of it here would
// be a second thing to keep in step with Go.
use revm::precompile::bls12_381::g1_add::g1_add as eth_g1_add;
use revm::precompile::bls12_381::g1_msm::g1_msm as eth_g1_msm;
use revm::precompile::bls12_381::g2_add::g2_add as eth_g2_add;
use revm::precompile::bls12_381::g2_msm::g2_msm as eth_g2_msm;
use revm::precompile::bls12_381::pairing::pairing as eth_pairing;

/// A stateful precompile a chain turns on, and the time it turns on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Activation {
    pub address: Address,
    pub at: u64,
}

impl Activation {
    /// Whether the transition from `parent` (none for the genesis) to `time`
    /// crosses it: `IsForkTransition` and `applyAlwaysOnActivations`.
    pub const fn crossed(&self, parent: Option<u64>, time: u64) -> bool {
        match parent {
            None => self.at <= time,
            Some(p) => p < self.at && self.at <= time,
        }
    }
}

/// The protocol's system precompiles, which no config names: the DEX
/// settlement precompile, `AlwaysOn` in luxfi/precompile v0.19.22
/// (dex/settle_module.go:116) and turned on at `DexSettleActivationTime`
/// (luxfi/evm precompile/registry/bridge.go:53).
pub const ALWAYS_ON: [Activation; 1] = [Activation {
    address: address!("0x0000000000000000000000000000000000009999"),
    at: 1_766_704_800,
}];

/// The one code byte an activated precompile's account carries.
pub const MARKER: [u8; 1] = [0x01];

/// Write the activation marker on `who`: `SetNonce(1)` and `SetCode({0x01})`,
/// keeping its balance and storage, which neither touches.
pub fn mark(world: &mut World, who: Address) {
    let code = Bytecode::new_raw(Bytes::from_static(&MARKER));
    let held = world.get(&who);
    let account = Account {
        nonce: 1,
        balance: held.map_or(U256::ZERO, |a| a.balance),
        code_hash: code.hash_slow(),
        storage: held.map(|a| a.storage.clone()).unwrap_or_default(),
    };
    world.insert(who, account, Some(code));
}

/// The precompile families a profile can refuse, mirroring `luxfi/pq`'s `Op`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Ecrecover,
    P256Verify,
    Sha256,
    Ripemd160,
    Blake2f,
    Bn256,
    Bls12381,
    Kzg,
    /// Not gated by any profile — admitted unconditionally.
    Unknown,
}

impl Op {
    /// The family a precompile belongs to.
    ///
    /// One arm per case in Go's `opForPrecompile`. Go separates bn256 into add,
    /// mul and pairing, but `Profile` carries a single `ForbidBn256` flag for
    /// all three, so the distinction has no observable effect and is not
    /// reproduced here.
    pub fn of(id: &PrecompileId) -> Self {
        match id {
            PrecompileId::EcRec => Op::Ecrecover,
            PrecompileId::P256Verify => Op::P256Verify,
            PrecompileId::Sha256 => Op::Sha256,
            PrecompileId::Ripemd160 => Op::Ripemd160,
            PrecompileId::Blake2F => Op::Blake2f,
            PrecompileId::Bn254Add | PrecompileId::Bn254Mul | PrecompileId::Bn254Pairing => {
                Op::Bn256
            }
            PrecompileId::Bls12G1Add
            | PrecompileId::Bls12G1Msm
            | PrecompileId::Bls12G2Add
            | PrecompileId::Bls12G2Msm
            | PrecompileId::Bls12Pairing
            | PrecompileId::Bls12MapFpToGp1
            | PrecompileId::Bls12MapFp2ToGp2 => Op::Bls12381,
            PrecompileId::KzgPointEvaluation => Op::Kzg,
            PrecompileId::Identity | PrecompileId::ModExp | PrecompileId::Custom(_) => Op::Unknown,
        }
    }
}

/// Which classical families a chain refuses.
///
/// The default admits everything, which is Go's nil `*pq.Profile` and is what
/// every Lux chain serving traffic today installs — the post-quantum block is
/// not active on the C-chain.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    pub ecrecover: bool,
    pub p256: bool,
    pub sha256: bool,
    pub ripemd160: bool,
    pub blake2f: bool,
    pub bn256: bool,
    pub bls12381: bool,
    pub kzg: bool,
}

impl Profile {
    /// Refuses every classical family. `pq.Strict()`.
    pub const fn strict() -> Self {
        Self {
            ecrecover: true,
            p256: true,
            sha256: true,
            ripemd160: true,
            blake2f: true,
            bn256: true,
            bls12381: true,
            kzg: true,
        }
    }

    /// What a Lux chain installs: strict, less bn256.
    ///
    /// The alt_bn128 precompiles at 0x06–0x08 stay available so Ethereum
    /// contracts keep working; Lux's own pairing use lives in its own
    /// precompiles and is gated separately. See
    /// `luxfi/geth core/vm/pq_lux_profile_test.go`.
    pub const fn lux() -> Self {
        Self {
            bn256: false,
            ..Self::strict()
        }
    }

    /// Whether this profile refuses `op`.
    pub const fn forbids(&self, op: Op) -> bool {
        match op {
            Op::Ecrecover => self.ecrecover,
            Op::P256Verify => self.p256,
            Op::Sha256 => self.sha256,
            Op::Ripemd160 => self.ripemd160,
            Op::Blake2f => self.blake2f,
            Op::Bn256 => self.bn256,
            Op::Bls12381 => self.bls12381,
            Op::Kzg => self.kzg,
            Op::Unknown => false,
        }
    }
}

/// A precompile the chain registers at an address of its own choosing.
///
/// Go's `modules.Module`: a config key, an address, and a contract. The
/// contract states its price and its work as two things — `RequiredGas` runs,
/// `contract.DeductGas` takes its answer, and only then does the body see the
/// input — so a refusal is charged for. [`gas`](Self::gas) and
/// [`run`](Self::run) are those two, kept apart for the same reason.
#[derive(Clone, Copy, Debug)]
pub struct Module {
    /// The `Key()` of the Go module this reproduces, which is also its name
    /// under `precompileUpgrades` in a chain's `upgrade.json`. It travels into
    /// the refusal message, so a refusal names the precompile that refused
    /// rather than the one revm keeps at the address.
    pub key: &'static str,
    /// Where the chain registers it.
    pub address: Address,
    /// `RequiredGas`: the price, read off the input alone.
    pub gas: fn(&[u8]) -> u64,
    /// The answer, once the price is paid.
    pub run: fn(&[u8]) -> Result<Bytes, PrecompileError>,
}

/// The modules the C-chain serves, from `luxfi/precompile@v0.19.21`.
///
/// These seven BLS12-381 keys sit on the **pre-final** EIP-2537 assignment
/// (`bls12381/contract.go`): G1Add, G1Mul, G1MSM, G2Add, G2Mul, G2MSM, Pairing
/// at 0x0b through 0x11. revm's set is the final one, where 0x0c is the G1 MSM
/// and 0x0d is the G2 addition, so from 0x0c onwards the same address names a
/// different operation in each — the largest divergence between the two, and
/// the reason this table exists. The C-chain is at Cancun, whose stock table
/// holds nothing at 0x0b–0x11, so these are the only thing that has ever
/// answered there.
///
/// Which modules a chain serves is `precompileUpgrades` in its `upgrade.json`,
/// and every Lux network turns these seven on at one timestamp — mainnet's is
/// 1766708400. This crate does not read that document, so the table is
/// unconditional: right from that second onward, and wrong for a replay of the
/// window before it, where the addresses held nothing.
///
/// Two things registered in Go are deliberately not here. `aiInferenceConfig`
/// at 0x0300…0003 runs an integer transformer over a frozen fixture model — a
/// subject of its own, not an arithmetic primitive. `secp256r1Config` at 0x0100
/// is registered in Go and enabled by no network: it appears in no
/// `precompileUpgrades` and in no genesis config, so serving it would answer
/// where every Lux chain answers nothing.
pub const MODULES: &[Module] = &[
    Module {
        key: "bls12381G1AddConfig",
        address: at(0x0b),
        gas: price::g1_add,
        run: g1_add,
    },
    Module {
        key: "bls12381G1MulConfig",
        address: at(0x0c),
        gas: price::g1_mul,
        run: g1_mul,
    },
    Module {
        key: "bls12381G1MSMConfig",
        address: at(0x0d),
        gas: price::g1_msm,
        run: g1_msm,
    },
    Module {
        key: "bls12381G2AddConfig",
        address: at(0x0e),
        gas: price::g2_add,
        run: g2_add,
    },
    Module {
        key: "bls12381G2MulConfig",
        address: at(0x0f),
        gas: price::g2_mul,
        run: g2_mul,
    },
    Module {
        key: "bls12381G2MSMConfig",
        address: at(0x10),
        gas: price::g2_msm,
        run: g2_msm,
    },
    Module {
        key: "bls12381PairingConfig",
        address: at(0x11),
        gas: price::pairing,
        run: pairing,
    },
];

/// The address `n` sits at: twenty bytes, big-endian, left-padded with zeros.
const fn at(n: u16) -> Address {
    let mut a = [0u8; 20];
    a[18] = (n >> 8) as u8;
    a[19] = n as u8;
    Address::new(a)
}

/// What a module charges. Every price below is EIP-2537's, and revm already
/// writes each number down; Go's `bls12381/contract.go` names the same values,
/// so these read revm's rather than start a second table that could drift from
/// it.
///
/// That reasoning holds here because both sides price EIP-2537 alike. It does
/// not generalise, and 0x0100 is where it breaks: Go's `secp256r1/contract.go`
/// charges 3450 for RIP-7212 where revm's Osaka table charges EIP-7951's 6900.
/// Neither number reaches a chain — see [`MODULES`] — so no price for it is
/// written down anywhere, here least of all.
mod price {
    use super::*;

    /// `GasG1Add`.
    pub fn g1_add(_: &[u8]) -> u64 {
        G1_ADD_BASE_GAS_FEE
    }

    /// `GasG1Mul`. EIP-2537 prices a multiplication as the one-pair
    /// multi-scalar multiplication, whose k = 1 discount is 1000/1000, so this
    /// is the MSM's per-pair cost with no discount applied.
    pub fn g1_mul(_: &[u8]) -> u64 {
        G1_MSM_BASE_GAS_FEE
    }

    /// `G1MSMGas`. The pair count is a floor division, so a length that is not
    /// a whole number of pairs is still priced; [`super::g1_msm`] is what
    /// refuses it, after the charge.
    pub fn g1_msm(input: &[u8]) -> u64 {
        msm_required_gas(
            input.len() / G1_MSM_INPUT_LENGTH,
            &DISCOUNT_TABLE_G1_MSM,
            G1_MSM_BASE_GAS_FEE,
        )
    }

    /// `GasG2Add`.
    pub fn g2_add(_: &[u8]) -> u64 {
        G2_ADD_BASE_GAS_FEE
    }

    /// `GasG2Mul`.
    pub fn g2_mul(_: &[u8]) -> u64 {
        G2_MSM_BASE_GAS_FEE
    }

    /// `G2MSMGas`.
    pub fn g2_msm(input: &[u8]) -> u64 {
        msm_required_gas(
            input.len() / G2_MSM_INPUT_LENGTH,
            &DISCOUNT_TABLE_G2_MSM,
            G2_MSM_BASE_GAS_FEE,
        )
    }

    /// `PairingGas`: a base plus a per-pair cost, over a floor-divided count.
    pub fn pairing(input: &[u8]) -> u64 {
        PAIRING_OFFSET_BASE + PAIRING_MULTIPLIER_BASE * (input.len() / PAIRING_INPUT_LENGTH) as u64
    }
}

/// The gas limit handed to revm's precompile once a module has charged the
/// price. revm asks for one because it prices its own work; by the time these
/// run the price question is settled, so this is the limit that cannot bind
/// rather than a second budget.
const PAID: u64 = u64::MAX;

/// Go's `contract.ErrInvalidInput`, for a length a module will not read.
const INVALID: PrecompileError = PrecompileError::other_static("invalid input");

/// An input that must be exactly two points wide, split.
fn two(input: &[u8], point: usize) -> Result<(&[u8], &[u8]), PrecompileError> {
    if input.len() != 2 * point {
        return Err(INVALID);
    }
    Ok(input.split_at(point))
}

/// The G1 subgroup check.
///
/// Go's `decodeG1` is one decoder shared by all seven operations and it checks
/// the subgroup every time, including on the additions; the final EIP-2537
/// dropped that check from the additions and revm's `g1_add` follows the final
/// text. So an on-curve point outside the r-order subgroup is refused by the
/// chain and accepted by revm, and this is what closes that.
///
/// The one place revm's public set performs a G1 subgroup check is the MSM,
/// where a zero scalar makes it check the point and then compute nothing:
/// `p1_msm_bytes` reads the point through `read_g1` before it looks at the
/// scalar, and drops the pair after.
fn subgroup_g1(point: &[u8]) -> Result<(), PrecompileError> {
    let mut pair = [0u8; G1_MSM_INPUT_LENGTH];
    pair[..PADDED_G1_LENGTH].copy_from_slice(point);
    eth_g1_msm(&pair, PAID).map(|_| ())
}

/// The G2 subgroup check. See [`subgroup_g1`].
fn subgroup_g2(point: &[u8]) -> Result<(), PrecompileError> {
    let mut pair = [0u8; G2_MSM_INPUT_LENGTH];
    pair[..PADDED_G2_LENGTH].copy_from_slice(point);
    eth_g2_msm(&pair, PAID).map(|_| ())
}

/// 0x0b: two G1 points, added.
fn g1_add(input: &[u8]) -> Result<Bytes, PrecompileError> {
    let (a, b) = two(input, PADDED_G1_LENGTH)?;
    subgroup_g1(a)?;
    subgroup_g1(b)?;
    Ok(eth_g1_add(input, PAID)?.bytes)
}

/// 0x0c: one G1 point by one scalar.
///
/// The chain takes exactly one pair here and refuses anything else; the MSM it
/// shares a body with lives one address up. Both compute the same product for
/// one pair, so this is that MSM behind the length the chain demands.
fn g1_mul(input: &[u8]) -> Result<Bytes, PrecompileError> {
    if input.len() != G1_MSM_INPUT_LENGTH {
        return Err(INVALID);
    }
    Ok(eth_g1_msm(input, PAID)?.bytes)
}

/// 0x0d: a G1 multi-scalar multiplication.
fn g1_msm(input: &[u8]) -> Result<Bytes, PrecompileError> {
    Ok(eth_g1_msm(input, PAID)?.bytes)
}

/// 0x0e: two G2 points, added.
fn g2_add(input: &[u8]) -> Result<Bytes, PrecompileError> {
    let (a, b) = two(input, PADDED_G2_LENGTH)?;
    subgroup_g2(a)?;
    subgroup_g2(b)?;
    Ok(eth_g2_add(input, PAID)?.bytes)
}

/// 0x0f: one G2 point by one scalar. See [`g1_mul`].
fn g2_mul(input: &[u8]) -> Result<Bytes, PrecompileError> {
    if input.len() != G2_MSM_INPUT_LENGTH {
        return Err(INVALID);
    }
    Ok(eth_g2_msm(input, PAID)?.bytes)
}

/// 0x10: a G2 multi-scalar multiplication.
fn g2_msm(input: &[u8]) -> Result<Bytes, PrecompileError> {
    Ok(eth_g2_msm(input, PAID)?.bytes)
}

/// 0x11: a pairing check over k pairs.
fn pairing(input: &[u8]) -> Result<Bytes, PrecompileError> {
    Ok(eth_pairing(input, PAID)?.bytes)
}

/// The precompiles a Lux chain serves: [`MODULES`] first, Ethereum's set under
/// them, and the [`Profile`] over what is left.
///
/// A call the profile refuses keeps the gas the charge already took and loses
/// its output, which is what Go produces: `runPrecompile` returns
/// `(nil, suppliedGas, err)` after the deduction. Running the precompile and
/// then discarding its answer gives the same observable result as never running
/// it, because a precompile touches nothing but its own input — and it keeps the
/// gas schedule in one place, revm's, rather than in a second table here that
/// could drift.
#[derive(Clone, Debug)]
pub struct Precompiles {
    inner: EthPrecompiles,
    /// The modules that answer: every one, or those [`Precompiles::serving`]
    /// keeps — the ones the chain has turned on.
    modules: Vec<&'static Module>,
    profile: Profile,
    /// Active precompiles this crate does not run: a call that reaches one
    /// makes its transaction one no block here can carry.
    refused: Vec<Address>,
}

impl Precompiles {
    /// The precompiles for `spec` under `profile`: Ethereum's set, with the
    /// chain's [`MODULES`] shadowing it where the two land on one address.
    pub fn new(spec: SpecId, profile: Profile) -> Self {
        Self {
            inner: EthPrecompiles::new(spec),
            modules: MODULES.iter().collect(),
            profile,
            refused: Vec::new(),
        }
    }

    /// The same set, with only the modules at `active` answering. Go serves a
    /// module only once its precompile is enabled (`IsPrecompileEnabled`);
    /// before that its address is an account like any other.
    pub fn serving(mut self, active: &[Address]) -> Self {
        self.modules.retain(|m| active.contains(&m.address));
        self
    }

    /// The same set, refusing a call to any of `refused`.
    pub fn refusing(mut self, refused: Vec<Address>) -> Self {
        self.refused = refused;
        self
    }

    /// The module registered at `address`, if one is.
    ///
    /// Asked before the Ethereum set on every path here, because that is the
    /// order `(*EVM).precompile` asks in.
    pub fn module(&self, address: &Address) -> Option<&'static Module> {
        self.modules.iter().copied().find(|m| &m.address == address)
    }

    /// The family the precompile at `address` belongs to, if one lives there.
    pub fn op(&self, address: &Address) -> Option<Op> {
        if self.module(address).is_some() {
            // Go's `opForPrecompile` type-switches on the implementation and
            // has no case for the `precompileAdapter` an overrider installs, so
            // a module falls to the arm every profile admits. The consequence
            // is worth stating plainly: a strict-PQ chain still serves
            // BLS12-381 at 0x0b–0x11, because the modules there are outside the
            // gate. That is Go's behaviour, and this reproduces it.
            return Some(Op::Unknown);
        }
        self.inner.precompiles.get(address).map(|p| Op::of(p.id()))
    }

    /// Whether anything answers at `address`.
    pub fn contains(&self, address: &Address) -> bool {
        self.module(address).is_some() || self.inner.contains(address)
    }

    /// The profile this set is gated by.
    pub const fn profile(&self) -> Profile {
        self.profile
    }
}

/// One call to a module: the price, and then the work.
///
/// The second half of the pair is the message to park for a refusal, which the
/// caller can only write once it has stopped borrowing the input.
fn serve(m: &Module, input: &[u8], limit: u64) -> (InterpreterResult, Option<String>) {
    let mut gas = Gas::new(limit);
    if !gas.record_cost((m.gas)(input)) {
        // The offer was below the price, so nothing was charged and nothing
        // ran — Go's `DeductGas` returning `(0, ErrOutOfGas)`. A refused
        // `record_cost` takes nothing, so the meter still reads untouched.
        return (
            InterpreterResult {
                result: InstructionResult::PrecompileOOG,
                gas,
                output: Bytes::new(),
            },
            None,
        );
    }
    match (m.run)(input) {
        Ok(output) => (
            InterpreterResult {
                result: InstructionResult::Return,
                gas,
                output,
            },
            None,
        ),
        // The charge stands: the module read the input before it refused it.
        Err(e) => (
            InterpreterResult {
                result: InstructionResult::PrecompileError,
                gas,
                output: Bytes::new(),
            },
            Some(format!("{}: {e}", m.key)),
        ),
    }
}

impl<CTX: ContextTr> PrecompileProvider<CTX> for Precompiles {
    type Output = InterpreterResult;

    fn set_spec(&mut self, spec: <CTX::Cfg as Cfg>::Spec) -> bool {
        <EthPrecompiles as PrecompileProvider<CTX>>::set_spec(&mut self.inner, spec)
    }

    fn run(
        &mut self,
        context: &mut CTX,
        inputs: &CallInputs,
    ) -> Result<Option<InterpreterResult>, String> {
        if self.refused.contains(&inputs.bytecode_address) {
            // An error here is not a revert: it ends the transaction, which
            // `block::apply` then refuses.
            return Err(format!(
                "a call to the stateful precompile at {}, which this node does not run",
                inputs.bytecode_address
            ));
        }
        if let Some(m) = self.module(&inputs.bytecode_address) {
            // A module answers alone. The gate is not consulted because Go's
            // does not classify it — see `Precompiles::op`.
            let (result, why) = {
                let held;
                let bytes = match &inputs.input {
                    CallInput::SharedBuffer(range) => {
                        match context.local().shared_memory_buffer_slice(range.clone()) {
                            Some(slice) => {
                                held = slice;
                                held.as_ref()
                            }
                            None => &[],
                        }
                    }
                    CallInput::Bytes(b) => b.as_ref(),
                };
                serve(m, bytes, inputs.gas_limit)
            };
            // Where revm parks a refusal's message, and on the same terms: the
            // top frame only, so a nested call cannot overwrite what the
            // caller is about to read.
            if let Some(why) = why {
                if context.journal().depth() == 1 {
                    context.local_mut().set_precompile_error_context(why);
                }
            }
            return Ok(Some(result));
        }

        // Read the family before running: the borrow on the set ends with the
        // call, and an address alone does not say which implementation answered.
        let op = self.op(&inputs.bytecode_address);
        let mut run = self.inner.run(context, inputs)?;

        if let (Some(op), Some(out)) = (op, run.as_mut()) {
            // A call that could not pay never reached the gate in Go either:
            // the gas check comes first and returns out-of-gas.
            if out.result != InstructionResult::PrecompileOOG && self.profile.forbids(op) {
                out.result = InstructionResult::PrecompileError;
                out.output = Bytes::new();
            }
        }
        Ok(run)
    }

    /// Ethereum's set for the fork, and nothing else: Go warms
    /// `vm.ActivePrecompiles(rules)` (core/state_transition.go:524), which no
    /// Lux module is in, so a module is cold until a transaction touches it.
    fn warm_addresses(&self) -> Box<impl Iterator<Item = Address>> {
        Box::new(self.inner.precompiles.addresses().cloned())
    }

    fn contains(&self, address: &Address) -> bool {
        Self::contains(self, address)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: SpecId = SpecId::SHANGHAI;

    #[test]
    fn every_unshadowed_address_classifies_the_way_go_does() {
        let p = Precompiles::new(SpecId::PRAGUE, Profile::default());
        for (byte, want) in [
            (1u16, Op::Ecrecover),
            (2, Op::Sha256),
            (3, Op::Ripemd160),
            (4, Op::Unknown),
            (5, Op::Unknown),
            (6, Op::Bn256),
            (7, Op::Bn256),
            (8, Op::Bn256),
            (9, Op::Blake2f),
            (0x0a, Op::Kzg),
        ] {
            assert_eq!(
                p.op(&at(byte)),
                Some(want),
                "the precompile at 0x{byte:02x} is classified wrongly"
            );
        }
    }

    #[test]
    fn a_module_is_outside_the_gate() {
        let p = Precompiles::new(SpecId::PRAGUE, Profile::strict());
        for m in MODULES {
            assert_eq!(
                p.op(&m.address),
                Some(Op::Unknown),
                "{} is reached through the overrider, which Go's opForPrecompile \
                 does not classify",
                m.key
            );
            assert!(!Profile::strict().forbids(Op::Unknown));
        }
    }

    #[test]
    fn identity_and_modexp_are_ungated() {
        let p = Precompiles::new(SPEC, Profile::strict());
        for byte in [4u16, 5] {
            let op = p.op(&at(byte)).unwrap();
            assert_eq!(op, Op::Unknown);
            assert!(
                !Profile::strict().forbids(op),
                "Go's opForPrecompile has no case for 0x{byte:02x}, so no profile refuses it"
            );
        }
    }

    #[test]
    fn the_lux_profile_is_strict_less_bn256() {
        let lux = Profile::lux();
        assert!(!lux.forbids(Op::Bn256), "the Ethereum-compat carve-out");
        for op in [
            Op::Ecrecover,
            Op::P256Verify,
            Op::Sha256,
            Op::Ripemd160,
            Op::Blake2f,
            Op::Bls12381,
            Op::Kzg,
        ] {
            assert!(lux.forbids(op), "{op:?} must still be refused");
        }
    }

    #[test]
    fn no_profile_refuses_anything_by_default() {
        let d = Profile::default();
        for op in [
            Op::Ecrecover,
            Op::P256Verify,
            Op::Sha256,
            Op::Ripemd160,
            Op::Blake2f,
            Op::Bn256,
            Op::Bls12381,
            Op::Kzg,
            Op::Unknown,
        ] {
            assert!(!d.forbids(op), "a nil profile is classical semantics");
        }
    }

    #[test]
    fn an_address_with_no_precompile_has_no_family() {
        let p = Precompiles::new(SPEC, Profile::default());
        assert_eq!(p.op(&at(0xff)), None);
        assert!(!p.contains(&at(0xff)));
    }

    #[test]
    fn the_modules_sit_where_the_chain_registers_them() {
        // `luxfi/precompile@v0.19.21 bls12381/contract.go`, the pre-final
        // EIP-2537 assignment. Every network's `upgrade.json` names these seven
        // keys and no other module this crate implements.
        let want = [
            (0x0bu16, "bls12381G1AddConfig"),
            (0x0c, "bls12381G1MulConfig"),
            (0x0d, "bls12381G1MSMConfig"),
            (0x0e, "bls12381G2AddConfig"),
            (0x0f, "bls12381G2MulConfig"),
            (0x10, "bls12381G2MSMConfig"),
            (0x11, "bls12381PairingConfig"),
        ];
        assert_eq!(MODULES.len(), want.len());
        let p = Precompiles::new(SPEC, Profile::default());
        for (n, key) in want {
            assert_eq!(p.module(&at(n)).map(|m| m.key), Some(key));
            assert!(p.contains(&at(n)));
        }
    }

    #[test]
    fn a_module_shadows_the_ethereum_precompile_at_its_address() {
        let p = Precompiles::new(SpecId::OSAKA, Profile::default());
        // The stock table at the latest revision keeps a BLS12-381 operation at
        // every one of 0x0b–0x11, and a module answers at each instead.
        for m in MODULES {
            assert!(
                p.inner.precompiles.get(&m.address).is_some(),
                "{} would not shadow anything",
                m.key
            );
            assert_eq!(p.op(&m.address), Some(Op::Unknown));
        }
    }

    #[test]
    fn the_prices_are_the_ones_go_charges() {
        // The `bls12381/contract.go` gas constants, read against a one-pair
        // input where the price varies with the input and against nothing where
        // it does not.
        let one_g1 = [0u8; G1_MSM_INPUT_LENGTH];
        let one_g2 = [0u8; G2_MSM_INPUT_LENGTH];
        let one_pair = [0u8; PAIRING_INPUT_LENGTH];
        for (key, input, want) in [
            ("bls12381G1AddConfig", &[][..], 375u64),
            ("bls12381G1MulConfig", &[][..], 12_000),
            ("bls12381G1MSMConfig", &one_g1[..], 12_000),
            ("bls12381G2AddConfig", &[][..], 600),
            ("bls12381G2MulConfig", &[][..], 22_500),
            ("bls12381G2MSMConfig", &one_g2[..], 22_500),
            ("bls12381PairingConfig", &one_pair[..], 37_700 + 32_600),
        ] {
            let m = MODULES.iter().find(|m| m.key == key).unwrap();
            assert_eq!((m.gas)(input), want, "{key} charges the wrong price");
        }
    }

    #[test]
    fn an_msm_over_no_pairs_is_free_and_then_refused() {
        // Go prices an MSM from a floor division and never rejects on price,
        // so an empty input costs nothing and the routine is what refuses it.
        let g1 = MODULES.iter().find(|m| m.key == "bls12381G1MSMConfig").unwrap();
        let g2 = MODULES.iter().find(|m| m.key == "bls12381G2MSMConfig").unwrap();
        for m in [g1, g2] {
            assert_eq!((m.gas)(&[]), 0);
            assert!((m.run)(&[]).is_err());
        }
    }

    #[test]
    fn a_multiplication_takes_one_pair_and_no_more() {
        // The MSM at the next address up reads any whole number of pairs; the
        // multiplication reads exactly one. Two pairs of zeros is a valid MSM
        // input and not a valid multiplication input.
        let two_g1 = [0u8; 2 * G1_MSM_INPUT_LENGTH];
        let two_g2 = [0u8; 2 * G2_MSM_INPUT_LENGTH];
        assert!(g1_msm(&two_g1).is_ok());
        assert!(g1_mul(&two_g1).is_err());
        assert!(g2_msm(&two_g2).is_ok());
        assert!(g2_mul(&two_g2).is_err());
    }

    /// A G1 point on the curve and outside the r-order subgroup, from
    /// `bls_g1mul_g1_not_in_correct_subgroup` in `luxfi/precompile@v0.19.21
    /// bls12381/testdata/eip2537_fail.json`.
    const STRAY_G1: [u8; PADDED_G1_LENGTH] = revm::primitives::hex!(
        "000000000000000000000000000000000123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        "00000000000000000000000000000000193fb7cedb32b2c3adc06ec11a96bc0d661869316f5e4a577a9f7c179593987beb4fb2ee424dbb2f5dd891e228b46c4a"
    );

    /// The same for G2, from `bls_g2mul_g2_not_in_correct_subgroup`.
    const STRAY_G2: [u8; PADDED_G2_LENGTH] = revm::primitives::hex!(
        "00000000000000000000000000000000197bfd0342bbc8bee2beced2f173e1a87be576379b343e93232d6cef98d84b1d696e5612ff283ce2cfdccb2cfb65fa0c"
        "00000000000000000000000000000000184e811f55e6f9d84d77d2f79102fd7ea7422f4759df5bf7f6331d550245e3f1bcf6a30e3b29110d85e0ca16f9f6ae7a"
        "000000000000000000000000000000000f10e1eb3c1e53d2ad9cf2d398b2dc22c5842fab0a74b174f691a7e914975da3564d835cd7d2982815b8ac57f507348f"
        "000000000000000000000000000000000767d1c453890f1b9110fda82f5815c27281aba3f026ee868e4176a0654feea41a96575e0c4d58a14dbfbcc05b5010b1"
    );

    #[test]
    fn an_addition_refuses_a_point_outside_the_subgroup() {
        // Go's shared decoder checks the subgroup everywhere, including in the
        // additions, where the final EIP-2537 does not — so revm's `g1_add`
        // accepts this and the chain refuses it. Added to the point at
        // infinity, which every implementation accepts, so the subgroup is the
        // only thing left that can refuse it.
        let mut g1 = [0u8; 2 * PADDED_G1_LENGTH];
        g1[..PADDED_G1_LENGTH].copy_from_slice(&STRAY_G1);
        assert!(eth_g1_add(&g1, PAID).is_ok(), "the final EIP-2537 admits it");
        assert!(g1_add(&g1).is_err(), "the chain does not");

        let mut g2 = [0u8; 2 * PADDED_G2_LENGTH];
        g2[..PADDED_G2_LENGTH].copy_from_slice(&STRAY_G2);
        assert!(eth_g2_add(&g2, PAID).is_ok(), "the final EIP-2537 admits it");
        assert!(g2_add(&g2).is_err(), "the chain does not");
    }

    #[test]
    fn a_stray_point_is_refused_wherever_it_appears() {
        // The same point through every module that reads one, so the check
        // cannot be present in the addition and missing elsewhere.
        let mut mul_g1 = [0u8; G1_MSM_INPUT_LENGTH];
        mul_g1[..PADDED_G1_LENGTH].copy_from_slice(&STRAY_G1);
        assert!(g1_mul(&mul_g1).is_err());
        assert!(g1_msm(&mul_g1).is_err());

        let mut mul_g2 = [0u8; G2_MSM_INPUT_LENGTH];
        mul_g2[..PADDED_G2_LENGTH].copy_from_slice(&STRAY_G2);
        assert!(g2_mul(&mul_g2).is_err());
        assert!(g2_msm(&mul_g2).is_err());

        let mut pair = [0u8; PAIRING_INPUT_LENGTH];
        pair[..PADDED_G1_LENGTH].copy_from_slice(&STRAY_G1);
        assert!(pairing(&pair).is_err());
    }

    /// A top-level call to `address` with `input`, and nothing else.
    fn call(address: Address, input: &[u8], gas_limit: u64) -> CallInputs {
        use revm::interpreter::{CallScheme, CallValue};
        use revm::primitives::U256;
        CallInputs {
            input: CallInput::Bytes(Bytes::copy_from_slice(input)),
            return_memory_offset: 0..0,
            gas_limit,
            bytecode_address: address,
            known_bytecode: None,
            target_address: address,
            caller: Address::ZERO,
            value: CallValue::Transfer(U256::ZERO),
            scheme: CallScheme::Call,
            is_static: false,
        }
    }

    #[test]
    fn the_provider_reaches_the_module_and_not_the_ethereum_precompile() {
        // The seam the whole thing turns on, driven the way a node drives it.
        // Two G1 pairs of zeros is a whole number of pairs, so revm's MSM —
        // which the final EIP-2537 puts at 0x0c — reads it happily. The chain
        // puts a single multiplication there, which refuses two pairs, and
        // puts the MSM one address up.
        use revm::{Context, MainContext};

        let mut p = Precompiles::new(SpecId::OSAKA, Profile::default());
        let mut ctx = Context::mainnet();
        ctx.journal_mut().checkpoint();

        let two = [0u8; 2 * G1_MSM_INPUT_LENGTH];
        assert!(eth_g1_msm(&two, PAID).is_ok(), "revm reads it as an MSM");

        let mul = PrecompileProvider::run(&mut p, &mut ctx, &call(at(0x0c), &two, 1_000_000))
            .unwrap()
            .expect("0x0c is served");
        assert_eq!(mul.result, InstructionResult::PrecompileError);
        assert_eq!(mul.gas.spent(), 12_000, "and the refusal is paid for");
        assert!(ctx
            .local_mut()
            .take_precompile_error_context()
            .unwrap()
            .starts_with("bls12381G1MulConfig: "));

        let msm = PrecompileProvider::run(&mut p, &mut ctx, &call(at(0x0d), &two, 1_000_000))
            .unwrap()
            .expect("0x0d is served");
        assert_eq!(msm.result, InstructionResult::Return);
        assert_eq!(msm.gas.spent(), 2 * 12_000 * 949 / 1_000, "the k=2 discount");
        assert_eq!(msm.output.len(), PADDED_G1_LENGTH);
    }

    #[test]
    fn a_refusal_is_charged_for_and_an_unaffordable_call_is_not() {
        // The whole reason a module states price and work apart. `serve` on an
        // input the module reads and refuses: the charge stands. The same
        // input with a limit below the price: nothing is charged.
        let m = MODULES
            .iter()
            .find(|m| m.key == "bls12381G1AddConfig")
            .unwrap();
        let (refused, why) = serve(m, &[0u8; 7], 1_000_000);
        assert_eq!(refused.result, InstructionResult::PrecompileError);
        assert_eq!(refused.gas.spent(), 375);
        assert!(refused.output.is_empty());
        assert!(why.unwrap().starts_with("bls12381G1AddConfig: "));

        let (poor, why) = serve(m, &[0u8; 7], 374);
        assert_eq!(poor.result, InstructionResult::PrecompileOOG);
        assert_eq!(poor.gas.spent(), 0);
        assert!(why.is_none());
    }
}
