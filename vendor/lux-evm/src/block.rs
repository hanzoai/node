// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! Executing a block.
//!
//! One primitive and two compositions, because building a block and verifying
//! one are the same work read in opposite directions:
//!
//! - [`apply`] runs ONE transaction against the world and commits it. It is the
//!   only thing here that touches the interpreter.
//! - [`execute`] runs a list of them strictly — the first invalid transaction
//!   ends the block, because a block containing one is not a block.
//! - a proposer composes [`apply`] itself, dropping what it cannot include.
//!
//! Writing the strict path as a flag on the permissive one would mean a
//! verifier and a proposer that disagree are still one function; keeping them
//! as two compositions of one primitive means the thing they share is the
//! thing that matters — the execution — and the thing they differ on is stated
//! where it happens.
//!
//! WHAT A BLOCK COMMITS TO. Every root in the header produced here is computed
//! from what actually ran: the state root is the world's trie after the last
//! transaction, the receipts root is over receipts carrying the real cumulative
//! gas, and the transactions root is over the same 2718 bytes the network
//! gossiped. None of them is carried in from the caller.

use alloy_consensus::{
    Eip658Value, Header, Receipt, ReceiptEnvelope, Transaction as _, TxEnvelope, TxType,
    EMPTY_OMMER_ROOT_HASH,
};
use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::{Address, Bloom, Bytes, Log, B256, B64, U256};
use revm::context::result::{EVMError, ExecutionResult};
use revm::context::{Context, ContextSetters, ContextTr, Evm, TxEnv};
use revm::context_interface::JournalTr;
use revm::handler::instructions::EthInstructions;
use revm::handler::Handler as _;
use revm::{DatabaseCommit as _, MainContext};

use crate::fee;
use crate::header;
use crate::precompile::{self, Activation};
use crate::root;
use crate::rules::{Chain, WARP};
use crate::state::World;
use crate::{Handler, Precompiles, Profile};

/// EIP-4788's beacon-roots contract, which a Cancun header's parent beacon
/// root is handed to before the first transaction.
pub const BEACON_ROOTS: Address =
    alloy_primitives::address!("0x000F3df6D732807Ef1319fB7B8bB8522d0Beac02");

/// What a block is executed under: the chain's configuration, and the values
/// its header carries — every one of which Go derives from the parent.
///
/// A chain's own is [`Genesis::env`](crate::Genesis::env); [`Env::next`] is the
/// one way to the environment of a block after it, and [`rule`] holds a block
/// someone else built to it.
#[derive(Clone, Debug)]
pub struct Env {
    /// When each Lux rule turns on.
    pub chain: Chain,
    /// EIP-155's chain id. Genesis data, not a constant — the C-chain of one
    /// Lux network is not the C-chain of another, and a node that hard-codes
    /// this replays every transaction onto whichever network it was compiled
    /// for.
    pub chain_id: u64,
    /// Which precompiles are admitted.
    pub profile: Profile,
    /// The fee schedule every header field below is derived under.
    pub fee: fee::Config,
    /// The blob schedule under Cancun.
    pub blob: fee::Blob,
    /// The stateful precompiles the chain turns on, and when.
    pub activations: Vec<Activation>,
    /// `allowFeeRecipients`: a builder may name its own coinbase.
    pub allow_fee_recipients: bool,
    /// The coinbase. A builder's is its fee recipient where the chain allows
    /// one and [`fee::BLACKHOLE`] otherwise — also Go's when none is set.
    pub beneficiary: Address,
    pub number: u64,
    pub timestamp: u64,
    pub gas_limit: u64,
    pub base_fee: u64,
    /// 1 after the genesis: Go's `Prepare`, and the only value its VM accepts.
    pub difficulty: U256,
    /// The fee window, then — from Durango — the predicate results.
    pub extra_data: Bytes,
    pub mix_hash: B256,
    /// The block gas cost the block is under, derived from its parent's
    /// ([`fee::cost`]). The header carries it when it is not zero.
    pub block_gas_cost: u64,
    /// From Shanghai, the (empty) withdrawals root.
    pub withdrawals_root: Option<B256>,
    /// From Cancun, the blob gas used (zero), the excess blob gas and the
    /// parent beacon root.
    pub blob_gas_used: Option<u64>,
    pub excess_blob_gas: Option<u64>,
    pub parent_beacon_root: Option<B256>,
}

impl Env {
    /// The environment of a block at `time` on top of `parent`, whose own
    /// block gas cost is `parent_cost` — the one it was derived under, which
    /// its header need not carry; `None` for the genesis, whose sixteen fields
    /// have no place for one.
    ///
    /// Every field Go derives, derived: the gas limit (`header.GasLimit`), the
    /// base fee, the block gas cost and the extra (see [`fee`]). And the ones
    /// its builder fixes: difficulty 1, a zero mix digest, the blackhole for a
    /// coinbase unless the chain lets a builder name its fee recipient
    /// (miner/worker.go:135-205 `commitNewWork`); from Shanghai an empty
    /// withdrawals root (consensus.go:420-423), and from Cancun zero blob gas
    /// used, the excess blob gas `CalcExcessBlobGas` gives, and a zero parent
    /// beacon root — the worker's `beaconRoot` (miner/worker.go:121, 174-185).
    /// Refused before the EVM upgrade, whose rules this crate does not run.
    pub fn next(
        &self,
        parent: &header::Lux,
        parent_cost: Option<u64>,
        time: u64,
    ) -> Result<Self, fee::Error> {
        let rules = self.chain.at(time);
        if !rules.evm {
            return Err(fee::Error::PreEvm);
        }
        let head = &parent.header;
        let window = fee::window(&self.chain, head, time)?;
        Ok(Self {
            number: head.number + 1,
            withdrawals_root: rules.shanghai.then_some(root::EMPTY),
            blob_gas_used: rules.cancun.then_some(0),
            excess_blob_gas: rules.cancun.then(|| fee::excess(&self.blob, head)),
            parent_beacon_root: rules.cancun.then_some(B256::ZERO),
            timestamp: time,
            gas_limit: self.fee.gas_limit,
            base_fee: fee::base_fee(&self.chain, &self.fee, head, time)?,
            block_gas_cost: fee::cost(&self.fee, parent_cost, head.timestamp, time),
            extra_data: fee::extra(&window, self.chain.at(time).durango),
            difficulty: U256::from(1u64),
            mix_hash: B256::ZERO,
            beneficiary: if self.allow_fee_recipients {
                self.beneficiary
            } else {
                fee::BLACKHOLE
            },
            ..self.clone()
        })
    }

    /// The environment `block` itself ran under, read off its header: what
    /// Go's `eth_call` runs a call in (`NewEVMBlockContext` of the header it
    /// names).
    pub fn of(&self, block: &header::Lux) -> Self {
        let h = &block.header;
        Self {
            number: h.number,
            timestamp: h.timestamp,
            gas_limit: h.gas_limit,
            base_fee: h.base_fee_per_gas.unwrap_or_default(),
            beneficiary: h.beneficiary,
            difficulty: h.difficulty,
            extra_data: h.extra_data.clone(),
            mix_hash: h.mix_hash,
            block_gas_cost: block.block_gas_cost.unwrap_or_default().saturating_to(),
            withdrawals_root: h.withdrawals_root,
            blob_gas_used: h.blob_gas_used,
            excess_blob_gas: h.excess_blob_gas,
            parent_beacon_root: h.parent_beacon_block_root,
            ..self.clone()
        }
    }

    /// Whether the precompile at `address` is active in this block.
    pub fn active(&self, address: &Address) -> bool {
        self.activations
            .iter()
            .any(|a| &a.address == address && a.at <= self.timestamp)
    }

    /// Every precompile active in this block.
    pub fn actives(&self) -> Vec<Address> {
        self.activations
            .iter()
            .filter(|a| a.at <= self.timestamp)
            .map(|a| a.address)
            .collect()
    }

    /// The active precompiles this crate does not run.
    pub fn refused(&self) -> Vec<Address> {
        self.actives()
            .into_iter()
            .filter(|a| !precompile::MODULES.iter().any(|m| &m.address == a))
            .collect()
    }

    /// The precompiles a block here runs under: Ethereum's for its fork, the
    /// modules its chain has turned on, and a refusal at every other active
    /// one.
    fn precompiles(&self) -> Precompiles {
        Precompiles::new(self.chain.spec(self.timestamp), self.profile)
            .serving(&self.actives())
            .refusing(self.refused())
    }
}

/// How far past a node's clock a block may be dated: `allowedFutureBlockTime`
/// (consensus/dummy/consensus.go:28) and the VM's `maxFutureBlockTime`
/// (plugin/evm/vm.go:114).
pub const FUTURE: u64 = 10;

/// Go's rule for a header: whether `claimed` may follow `parent` — whose own
/// block gas cost is `parent_cost` (see [`Env::next`]) — by the clock `now`.
/// Returns the environment the block runs under, its block gas cost the
/// derived one.
///
/// Every clause is one Go applies, cited where it is: the dummy engine's
/// `verifyHeader` and `verifyHeaderGasFields` (consensus/dummy/consensus.go:
/// 129-226), the VM's `SyntacticVerify` (plugin/evm/block_verification.go:
/// 32-170) and `verifyPredicates` (plugin/evm/block.go:429-460), and the body
/// check's uncle root (core/block_validator.go:65-76). What is left — the
/// block fee, and the roots and gas that only running the block gives — is
/// [`finish`]'s, on both the proposer's side and the verifier's.
///
/// `now` is `None` for history: a block a node already accepted is not held to
/// that node's clock a second time, as Go does not re-verify what it wrote.
pub fn rule(
    env: &Env,
    parent: &header::Lux,
    parent_cost: Option<u64>,
    claimed: &header::Lux,
    now: Option<u64>,
) -> Result<Env, Invalid> {
    let h = &claimed.header;
    let (head, parent) = (parent, &parent.header);
    let no = |why: String| Err(Invalid::Header(why));
    // consensus.go:213-216: the parent's number plus one.
    if h.number != parent.number + 1 {
        return no(format!(
            "number {} on a parent at {}",
            h.number, parent.number
        ));
    }
    // consensus.go:204-212: not before the parent — equal is several blocks a
    // second — and not more than `FUTURE` past the clock.
    if h.timestamp < parent.timestamp {
        return no(format!(
            "timestamp {} before its parent's {}",
            h.timestamp, parent.timestamp
        ));
    }
    if now.is_some_and(|now| h.timestamp > now.saturating_add(FUTURE)) {
        return no(format!("timestamp {} in the future", h.timestamp));
    }
    // block_verification.go:49-62.
    if h.difficulty != U256::from(1u64) {
        return no(format!("difficulty {}", h.difficulty));
    }
    if h.nonce != B64::ZERO {
        return no(format!("nonce {}", h.nonce));
    }
    if h.mix_hash != B256::ZERO {
        return no(format!("mix digest {}", h.mix_hash));
    }
    // consensus.go:182-186, 250-255 and block_validator.go:72-75: no uncles,
    // so the uncle root is the empty list's. The list itself is the reader's.
    if h.ommers_hash != EMPTY_OMMER_ROOT_HASH {
        return no(format!("uncle hash {}", h.ommers_hash));
    }
    // consensus.go:217-224: Lux runs no blobs.
    if h.blob_gas_used.is_some_and(|g| g != 0) {
        return no("blob gas used".into());
    }

    let mut next = env
        .next(head, parent_cost, h.timestamp)
        .map_err(|e| Invalid::Header(e.to_string()))?;
    let durango = env.chain.at(h.timestamp).durango;

    // header/gas_limit.go:68-83 and 46-65: the schedule's limit, exactly, and
    // no more gas than it.
    if h.gas_limit != next.gas_limit {
        return no(format!("gas limit {}, not {}", h.gas_limit, next.gas_limit));
    }
    if h.gas_used > next.gas_limit {
        return no(format!(
            "gas used {} over the limit {}",
            h.gas_used, next.gas_limit
        ));
    }
    // header/extra.go:74-106: at least a window from Durango, exactly one
    // before it.
    let extra = &h.extra_data;
    if extra.len() < fee::WINDOW_SIZE || (!durango && extra.len() != fee::WINDOW_SIZE) {
        return no(format!("{} bytes of extra", extra.len()));
    }
    // header/extra.go:47-68: the window derived from the parent.
    if extra[..fee::WINDOW_SIZE] != next.extra_data[..fee::WINDOW_SIZE] {
        return no("an extra that does not begin with the fee window".into());
    }
    // block.go:429-460: the predicate results the block's transactions give.
    // For a block this crate can run that is none — a transaction carrying a
    // predicate is refused in [`apply`] — so the bytes are `NO_PREDICATES`.
    if extra[fee::WINDOW_SIZE..] != next.extra_data[fee::WINDOW_SIZE..] {
        return no("predicate results this block's transactions do not give".into());
    }
    // consensus.go:149-159: the base fee as derived.
    if h.base_fee_per_gas != Some(next.base_fee) {
        return no(format!(
            "base fee {:?}, not {}",
            h.base_fee_per_gas, next.base_fee
        ));
    }
    // consensus.go:161-177 and 336-350: the block gas cost as derived. A
    // header that carries none is under the derived one, as Go's VM reads it:
    // `ensureBlockGasCost` fills an absent cost from the parent before the
    // check (plugin/evm/block.go:288-319). See [`fee`] on where a cost lives.
    if let Some(cost) = claimed.block_gas_cost {
        if cost != U256::from(next.block_gas_cost) {
            return no(format!(
                "block gas cost {cost}, not {}",
                next.block_gas_cost
            ));
        }
    }
    // The post-merge fields are read, not derived: Go checks only that no
    // blob gas is used (above), and runs the block under the excess blob gas
    // and the parent beacon root its header carries (core/evm.go:71-79,
    // core/state_processor.go:112-114).
    next.withdrawals_root = h.withdrawals_root;
    next.blob_gas_used = h.blob_gas_used;
    next.excess_blob_gas = h.excess_blob_gas;
    next.parent_beacon_root = h.parent_beacon_block_root;
    // consensus.go:106-127 and core/blockchain_reader.go:418-431: any coinbase
    // where fee recipients are allowed, the blackhole where they are not.
    if env.allow_fee_recipients {
        next.beneficiary = h.beneficiary;
    } else if h.beneficiary != fee::BLACKHOLE {
        return no(format!(
            "coinbase {}, not {}",
            h.beneficiary,
            fee::BLACKHOLE
        ));
    }
    Ok(next)
}

/// Why a transaction cannot be in a block, or a block cannot be one.
///
/// A transaction that runs and reverts is NOT here: it consumed gas, it has a
/// receipt, and it belongs in the block. This is only the transactions that
/// cannot be executed at all, and the blocks Go's rule refuses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Invalid {
    /// The signature does not recover to an address.
    Signature,
    /// Signed for a different chain.
    ChainId { want: u64, got: u64 },
    /// The block has no room left.
    GasLimit { used: u64, limit: u64 },
    /// revm refused it: wrong nonce, too poor, fee below base.
    Rejected(String),
    /// Its access list names the warp precompile while warp is on, which makes
    /// it a predicate Go verifies and prices, and this crate does not.
    Predicate,
    /// A blob transaction. No Lux block carries a blob: its header's blob gas
    /// used must be zero (consensus.go:217-224, block_validator.go:84-101).
    Blob,
    /// The header is not the one Go derives from its parent ([`rule`]).
    Header(String),
    /// The block's tips do not cover its block gas cost ([`fee::covered`]).
    Fee(String),
}

impl core::fmt::Display for Invalid {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Invalid::Signature => write!(f, "signature does not recover"),
            Invalid::ChainId { want, got } => write!(f, "chain id {got}, expected {want}"),
            Invalid::GasLimit { used, limit } => {
                write!(f, "gas {used} over the block limit {limit}")
            }
            Invalid::Rejected(why) => write!(f, "{why}"),
            Invalid::Predicate => write!(f, "a warp predicate, which this node does not verify"),
            Invalid::Blob => write!(f, "a blob transaction, which no Lux block carries"),
            Invalid::Header(why) => write!(f, "header: {why}"),
            Invalid::Fee(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for Invalid {}

/// What one transaction did.
#[derive(Clone, Debug)]
pub struct Applied {
    pub receipt: ReceiptEnvelope,
    /// Gas this transaction alone used.
    pub gas_used: u64,
    pub success: bool,
    pub output: Bytes,
    /// The address a deployment created.
    pub created: Option<Address>,
}

/// Run one transaction against `world` and commit what it did.
///
/// `cumulative` is the gas the block has already used, because a receipt states
/// the running total rather than its own transaction's share.
///
/// The world is only written if the transaction was executable: a rejected
/// transaction leaves no trace, which is what lets a proposer try one and move
/// on.
pub fn apply(
    world: &mut World,
    env: &Env,
    cumulative: u64,
    tx: &TxEnvelope,
    sender: Address,
) -> Result<Applied, Invalid> {
    if let Some(id) = tx.chain_id() {
        if id != env.chain_id {
            return Err(Invalid::ChainId {
                want: env.chain_id,
                got: id,
            });
        }
    }
    if tx.is_eip4844() {
        return Err(Invalid::Blob);
    }
    if env.active(&WARP)
        && tx
            .access_list()
            .is_some_and(|l| l.iter().any(|item| item.address == WARP))
    {
        return Err(Invalid::Predicate);
    }

    let (result, state) = run(world, env, to_revm(tx, sender), Checks::All)?;
    let gas_used = result.gas_used();
    if cumulative.saturating_add(gas_used) > env.gas_limit {
        // Refused after running, because whether it fits is only known once the
        // gas is known. Nothing was committed, so the world is untouched.
        return Err(Invalid::GasLimit {
            used: cumulative + gas_used,
            limit: env.gas_limit,
        });
    }

    world.commit(state);

    let created = match &result {
        ExecutionResult::Success { output, .. } => output.address().copied(),
        _ => None,
    };
    let success = result.is_success();
    let logs: Vec<Log> = result.logs().to_vec();
    let output = result.clone().into_output().unwrap_or_default();

    let receipt = Receipt {
        status: Eip658Value::Eip658(success),
        cumulative_gas_used: cumulative + gas_used,
        logs,
    };
    Ok(Applied {
        receipt: envelope(tx.tx_type(), receipt),
        gas_used,
        success,
        output,
        created,
    })
}

/// Which rules a run is subject to.
///
/// Not a flag: the two are different questions. Consensus asks "may this
/// transaction be in a block", where paying for gas is the point. A simulation
/// asks "what would this do", where the caller is not paying and a balance is
/// not part of the question — `eth_call` from an address holding nothing is the
/// normal case, not an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Checks {
    /// Every rule. What a block is verified under.
    All,
    /// The rules about being able to pay are set aside, and nothing else is.
    /// Only ever reachable through [`call`], which cannot commit.
    Simulated,
}

/// Drive the interpreter once. The ONE place execution happens.
///
/// Returns what the transaction did and the state it would leave behind —
/// separately, because whether that state is kept is the caller's decision and
/// not the interpreter's. [`apply`] keeps it; [`call`] throws it away.
pub fn run(
    world: &mut World,
    env: &Env,
    tx: TxEnv,
    checks: Checks,
) -> Result<(ExecutionResult, revm::state::EvmState), Invalid> {
    let spec = env.chain.spec(env.timestamp);
    let relaxed = checks == Checks::Simulated;
    // The context borrows the world for exactly this transaction. Access lists
    // and warm slots are per-transaction state and are meant to be dropped
    // here; what carries between transactions is committed state, which lives
    // in the world.
    let ctx = Context::mainnet()
        .with_db(&mut *world)
        .modify_cfg_chained(|c| {
            c.spec = spec;
            c.chain_id = env.chain_id;
            // A simulation is not paying, so the fee gates are not its
            // question. Every other rule — gas metering, the interpreter, the
            // precompile set — is untouched.
            c.disable_balance_check = relaxed;
            c.disable_base_fee = relaxed;
            c.disable_nonce_check = relaxed;
            c.disable_eip3607 = relaxed;
        })
        .modify_block_chained(|b| {
            b.number = U256::from(env.number);
            b.beneficiary = env.beneficiary;
            b.timestamp = U256::from(env.timestamp);
            b.basefee = env.base_fee;
            b.gas_limit = env.gas_limit;
            b.difficulty = env.difficulty;
            // Go's block context sets Random to the header's difficulty from
            // Shanghai on (core/evm.go:82-89), so PREVRANDAO answers 1 on
            // every Lux block, not the mix digest.
            b.prevrandao = Some(B256::from(env.difficulty));
            // BLOBBASEFEE answers `CalcBlobFee` of the header's excess blob
            // gas (core/evm.go:71-79). A Cancun header without one leaves Go
            // with none, and its opcode dereferences nil; revm needs a value
            // under Cancun, and zero excess is the only one a Lux chain has.
            b.set_blob_excess_gas_and_price(
                env.excess_blob_gas.unwrap_or_default(),
                env.blob.fraction,
            );
        });
    let mut evm = Evm::new(
        ctx,
        EthInstructions::new_mainnet_with_spec(spec),
        env.precompiles(),
    );
    evm.ctx.set_tx(tx);

    let mut handler: Handler<_, EVMError<core::convert::Infallible>, _> = Handler::new(env.chain);
    let result: ExecutionResult = handler
        .run(&mut evm)
        .map_err(|e| Invalid::Rejected(format!("{e:?}")))?;
    let state = evm.ctx.journal_mut().finalize();
    Ok((result, state))
}

/// Run a transaction and keep nothing — `eth_call`.
///
/// The world is taken by value so the discarding is structural: there is no
/// path from here that could commit, because the caller handed over a copy.
pub fn call(mut world: World, env: &Env, tx: TxEnv) -> Result<ExecutionResult, Invalid> {
    run(&mut world, env, tx, Checks::Simulated).map(|(result, _)| result)
}

/// A block, and everything needed to serve it.
#[derive(Clone, Debug)]
pub struct Built {
    /// The header in the shape the chain accepts: sixteen fields at genesis,
    /// seventeen after. See [`crate::header`] for why a shape is a layout and
    /// not one list with optional fields.
    pub header: header::Lux,
    pub transactions: Vec<TxEnvelope>,
    pub receipts: Vec<ReceiptEnvelope>,
}

impl Built {
    /// The block's hash: keccak of its header's RLP, and nothing else.
    pub fn hash(&self) -> B256 {
        self.header.seal()
    }

    /// The Ethereum fields, for everything that does not care about the shape.
    pub fn fields(&self) -> &Header {
        &self.header.header
    }
}

/// What a block does before its first transaction, in the order Go's
/// `Process` does it (core/state_processor.go:84-114): the activation marker of
/// every precompile whose time the transition from `parent` crosses
/// ([`precompile::Activation`]); then, where the header carries a parent
/// beacon root, EIP-4788's system call handing it to the beacon-roots
/// contract (`ProcessBeaconBlockRoot`, :290-306) — nothing where no account
/// is there, and an account there that is empty is deleted, as Go's `Call`
/// touches it. The parent's hash is also remembered for `BLOCKHASH`.
///
/// Both the proposer and the verifier call this, before [`apply`].
pub fn begin(world: &mut World, env: &Env, parent: &header::Lux) -> Result<(), Invalid> {
    world.remember(parent.header.number, parent.seal());
    for a in &env.activations {
        if a.crossed(Some(parent.header.timestamp), env.timestamp) {
            precompile::mark(world, a.address);
        }
    }
    let Some(root) = env.parent_beacon_root else {
        return Ok(());
    };
    // Go's `Call` to an account that does not exist returns before touching
    // anything; to one that exists, it runs — and a touched empty account is
    // then deleted (EIP-161), code or none.
    if world.get(&BEACON_ROOTS).is_none() {
        return Ok(());
    }
    use revm::handler::SystemCallEvm as _;
    let spec = env.chain.spec(env.timestamp);
    let ctx = Context::mainnet()
        .with_db(&mut *world)
        .modify_cfg_chained(|c| {
            c.spec = spec;
            c.chain_id = env.chain_id;
        })
        .modify_block_chained(|b| {
            b.number = U256::from(env.number);
            b.beneficiary = env.beneficiary;
            b.timestamp = U256::from(env.timestamp);
            b.basefee = env.base_fee;
            b.gas_limit = env.gas_limit;
            b.difficulty = env.difficulty;
            b.prevrandao = Some(B256::from(env.difficulty));
            b.set_blob_excess_gas_and_price(
                env.excess_blob_gas.unwrap_or_default(),
                env.blob.fraction,
            );
        });
    let mut evm = Evm::new(
        ctx,
        EthInstructions::new_mainnet_with_spec(spec),
        env.precompiles(),
    );
    let done = evm
        .system_call(BEACON_ROOTS, Bytes::copy_from_slice(root.as_slice()))
        .map_err(|e| Invalid::Rejected(format!("the beacon-root system call: {e:?}")))?;
    world.commit(done.state);
    Ok(())
}

/// Execute a whole block strictly: every transaction must be executable.
///
/// This is the verifier's composition. A proposer wants the permissive one and
/// writes it out of [`apply`] directly, because "skip what does not fit" is a
/// policy about a mempool and not a property of execution.
pub fn execute(
    world: &mut World,
    parent: &header::Lux,
    env: &Env,
    txs: Vec<(TxEnvelope, Address)>,
) -> Result<Built, Invalid> {
    begin(world, env, parent)?;
    let mut receipts = Vec::with_capacity(txs.len());
    let mut kept = Vec::with_capacity(txs.len());
    let mut cumulative = 0u64;

    for (tx, sender) in txs {
        let done = apply(world, env, cumulative, &tx, sender)?;
        cumulative += done.gas_used;
        receipts.push(done.receipt);
        kept.push(tx);
    }

    finish(world, parent, env, kept, receipts, cumulative)
}

/// Seal what ran into a header, once its tips are known to cover the block gas
/// cost (consensus.go:322-364 `Finalize`, 366-434 `FinalizeAndAssemble`).
///
/// Separate from [`execute`] because a proposer reaches this point by a
/// different route and must arrive at the same header — one place that decides
/// what a block commits to means a proposer and a verifier cannot drift.
pub fn finish(
    world: &World,
    parent: &header::Lux,
    env: &Env,
    transactions: Vec<TxEnvelope>,
    receipts: Vec<ReceiptEnvelope>,
    gas_used: u64,
) -> Result<Built, Invalid> {
    if env.chain.at(env.timestamp).evm {
        fee::covered(env.base_fee, env.block_gas_cost, &transactions, &receipts)
            .map_err(Invalid::Fee)?;
    }
    let encoded: Vec<Vec<u8>> = transactions.iter().map(|t| t.encoded_2718()).collect();
    let receipts_encoded: Vec<Vec<u8>> = receipts.iter().map(|r| r.encoded_2718()).collect();
    let logs_bloom = receipts
        .iter()
        .fold(Bloom::ZERO, |acc, r| acc | bloom_of(r));

    let fields = Header {
        parent_hash: parent.seal(),
        ommers_hash: EMPTY_OMMER_ROOT_HASH,
        beneficiary: env.beneficiary,
        state_root: world.root(),
        transactions_root: root::transactions(&encoded),
        receipts_root: root::receipts(&receipts_encoded),
        logs_bloom,
        difficulty: env.difficulty,
        number: parent.header.number + 1,
        gas_limit: env.gas_limit,
        gas_used,
        timestamp: env.timestamp,
        extra_data: env.extra_data.clone(),
        mix_hash: env.mix_hash,
        nonce: B64::ZERO,
        base_fee_per_gas: Some(env.base_fee),
        withdrawals_root: env.withdrawals_root,
        blob_gas_used: env.blob_gas_used,
        excess_blob_gas: env.excess_blob_gas,
        parent_beacon_block_root: env.parent_beacon_root,
        ..Default::default()
    };
    Ok(Built {
        // The genesis is built by `genesis::Genesis`, never here.
        header: header::Lux::block(fields, U256::from(env.block_gas_cost)),
        transactions,
        receipts,
    })
}

/// The bloom a receipt carries.
fn bloom_of(r: &ReceiptEnvelope) -> Bloom {
    match r {
        ReceiptEnvelope::Legacy(r)
        | ReceiptEnvelope::Eip2930(r)
        | ReceiptEnvelope::Eip1559(r)
        | ReceiptEnvelope::Eip4844(r)
        | ReceiptEnvelope::Eip7702(r) => r.logs_bloom,
    }
}

/// Wrap a receipt in the envelope its transaction type calls for. The type flag
/// is part of what the receipts trie commits to, so a receipt filed under the
/// wrong one changes the root.
fn envelope(ty: TxType, receipt: Receipt) -> ReceiptEnvelope {
    let with_bloom = receipt.with_bloom();
    match ty {
        TxType::Legacy => ReceiptEnvelope::Legacy(with_bloom),
        TxType::Eip2930 => ReceiptEnvelope::Eip2930(with_bloom),
        TxType::Eip1559 => ReceiptEnvelope::Eip1559(with_bloom),
        TxType::Eip4844 => ReceiptEnvelope::Eip4844(with_bloom),
        TxType::Eip7702 => ReceiptEnvelope::Eip7702(with_bloom),
    }
}

/// A signed transaction as revm wants it stated.
fn to_revm(tx: &TxEnvelope, sender: Address) -> TxEnv {
    let mut builder = TxEnv::builder()
        .tx_type(Some(tx.tx_type() as u8))
        .caller(sender)
        .gas_limit(tx.gas_limit())
        .kind(tx.kind())
        .value(tx.value())
        .data(tx.input().clone())
        .nonce(tx.nonce())
        .chain_id(tx.chain_id())
        .gas_price(tx.max_fee_per_gas())
        .gas_priority_fee(tx.max_priority_fee_per_gas());
    if let Some(list) = tx.access_list() {
        builder = builder.access_list(list.clone());
    }
    builder.build_fill()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genesis;
    use alloy_consensus::{SignableTransaction, TxEip1559, TxLegacy};
    use alloy_primitives::{address, keccak256, Signature, TxKind};
    use k256::ecdsa::SigningKey;

    const TO: Address = address!("00000000000000000000000000000000000000ff");

    /// The test account's key. Signing here rather than reaching for a signer
    /// crate: secp256k1 is already in the tree under alloy's `k256` feature,
    /// and a second one would be a second curve implementation.
    fn key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32].into()).expect("key")
    }

    /// The address a key controls: keccak of the uncompressed public key
    /// without its tag byte, last twenty bytes.
    fn who() -> Address {
        let vk = key().verifying_key().to_encoded_point(false);
        Address::from_slice(&keccak256(&vk.as_bytes()[1..])[12..])
    }

    /// Sign a transaction's signature hash.
    fn sign(hash: B256) -> Signature {
        let (sig, recid) = key()
            .sign_prehash_recoverable(hash.as_slice())
            .expect("sign");
        Signature::new(
            U256::from_be_slice(&sig.r().to_bytes()),
            U256::from_be_slice(&sig.s().to_bytes()),
            recid.is_y_odd(),
        )
    }

    /// Mainnet's configuration, with the test key funded.
    fn funded() -> genesis::Genesis {
        genesis::Genesis {
            alloc: vec![(
                who(),
                genesis::Alloc::funded(U256::from(10u64).pow(U256::from(21u64))),
            )],
            ..genesis::mainnet()
        }
    }

    /// A funded world, its genesis, and the environment of block one a minute
    /// after it — derived, as Go derives it.
    fn chain() -> (World, header::Lux, Env, Address) {
        let g = funded();
        let parent = g.sealed();
        let env = g
            .env()
            .next(&parent, None, parent.header.timestamp + 60)
            .expect("derives");
        (g.world(), parent, env, who())
    }

    fn send(nonce: u64, chain_id: u64, value: U256) -> (TxEnvelope, Address) {
        paying(nonce, chain_id, value, 1_000_000_000)
    }

    /// A transfer whose tip over the base fee is `tip`.
    fn paying(nonce: u64, chain_id: u64, value: U256, tip: u128) -> (TxEnvelope, Address) {
        let tx = TxEip1559 {
            chain_id,
            nonce,
            gas_limit: 100_000,
            max_fee_per_gas: 25_000_000_000 + tip,
            max_priority_fee_per_gas: tip,
            to: TxKind::Call(TO),
            value,
            ..Default::default()
        };
        let sig = sign(tx.signature_hash());
        (TxEnvelope::Eip1559(tx.into_signed(sig)), who())
    }

    /// The clock the tests verify by: well after every block they build.
    const NOW: Option<u64> = Some(u64::MAX / 2);

    #[test]
    fn a_block_moves_value_and_the_state_root_with_it() {
        let (mut world, parent, env, who) = chain();
        let before = world.root();
        let value = U256::from(1_000u64);

        let built = execute(
            &mut world,
            &parent,
            &env,
            vec![send(0, env.chain_id, value)],
        )
        .expect("the block executes");

        assert_eq!(built.fields().number, 1);
        assert_eq!(built.fields().parent_hash, parent.seal());
        assert_eq!(world.balance(&TO), value, "the transfer landed");
        assert_eq!(world.nonce(&who), 1, "the sender's nonce advanced");
        assert_ne!(built.fields().state_root, before, "the state moved");
        assert_eq!(
            built.fields().state_root,
            world.root(),
            "the header commits to the world that actually resulted"
        );
        assert_eq!(built.fields().gas_used, 21_000);
    }

    #[test]
    fn the_roots_are_over_what_ran_not_over_nothing() {
        let (mut world, parent, env, _) = chain();
        let built = execute(
            &mut world,
            &parent,
            &env,
            vec![send(0, env.chain_id, U256::from(1u64))],
        )
        .expect("executes");
        assert_ne!(built.fields().transactions_root, root::EMPTY);
        assert_ne!(built.fields().receipts_root, root::EMPTY);
        assert_eq!(built.receipts.len(), 1);
    }

    #[test]
    fn an_empty_block_still_advances_and_keeps_the_empty_roots() {
        let (mut world, parent, env, _) = chain();
        let built = execute(&mut world, &parent, &env, vec![]).expect("executes");
        assert_eq!(built.fields().number, 1);
        assert_eq!(built.fields().gas_used, 0);
        assert_eq!(built.fields().transactions_root, root::EMPTY);
        assert_eq!(built.fields().receipts_root, root::EMPTY);
        assert_eq!(
            built.fields().state_root,
            parent.header.state_root,
            "nothing ran, so nothing changed"
        );
    }

    #[test]
    fn a_transaction_for_another_chain_is_not_in_this_one() {
        let (mut world, parent, env, _) = chain();
        let wrong = env.chain_id + 1;
        let refused = execute(&mut world, &parent, &env, vec![send(0, wrong, U256::ZERO)]);
        assert_eq!(
            refused.err(),
            Some(Invalid::ChainId {
                want: env.chain_id,
                got: wrong
            })
        );
    }

    #[test]
    fn a_repeated_nonce_is_refused_and_leaves_no_trace() {
        let (mut world, _parent, env, _) = chain();
        let one = send(0, env.chain_id, U256::from(1u64));
        let done = apply(&mut world, &env, 0, &one.0, one.1).expect("first");
        assert!(done.success);
        let after = world.root();

        let replay = apply(&mut world, &env, done.gas_used, &one.0, one.1);
        assert!(matches!(replay, Err(Invalid::Rejected(_))), "{replay:?}");
        assert_eq!(world.root(), after, "a refused transaction changed nothing");
    }

    #[test]
    fn two_blocks_chain_onto_each_other() {
        let (mut world, genesis, env, _) = chain();
        let first = execute(
            &mut world,
            &genesis,
            &env,
            vec![send(0, env.chain_id, U256::from(1u64))],
        )
        .expect("first");
        let next = env
            .next(&first.header, Some(env.block_gas_cost), env.timestamp + 2)
            .expect("derives");
        let second = execute(
            &mut world,
            &first.header,
            &next,
            vec![send(1, env.chain_id, U256::from(1u64))],
        )
        .expect("second");

        assert_eq!(second.fields().number, 2);
        assert_eq!(second.fields().parent_hash, first.hash());
        assert_ne!(second.fields().state_root, first.fields().state_root);
        assert_eq!(world.balance(&TO), U256::from(2u64));
    }

    #[test]
    fn the_beneficiary_is_paid_the_whole_fee_under_lux_rules() {
        let (mut world, parent, env, _) = chain();
        let built = execute(
            &mut world,
            &parent,
            &env,
            vec![send(0, env.chain_id, U256::ZERO)],
        )
        .expect("executes");
        // Lux credits gasUsed * effectiveGasPrice with nothing burned. The
        // effective price under a 25 gwei base fee and a 1 gwei tip is 26 gwei.
        let paid = world.balance(&env.beneficiary);
        assert_eq!(
            env.beneficiary,
            fee::BLACKHOLE,
            "mainnet names no fee recipient"
        );
        assert_eq!(
            paid,
            U256::from(built.fields().gas_used as u128 * 26_000_000_000u128)
        );
    }

    #[test]
    fn a_built_block_is_one_the_rule_derives() {
        let g = funded();
        let (mut world, parent, env, _) = chain();
        let built = execute(
            &mut world,
            &parent,
            &env,
            vec![send(0, env.chain_id, U256::from(1u64))],
        )
        .expect("builds");
        let h = built.fields();
        assert_eq!(h.extra_data.len(), 86, "the window and no predicates");
        assert_eq!(
            &h.extra_data[..80],
            &[0u8; 80][..],
            "the first block's window is empty"
        );
        assert_eq!(h.beneficiary, fee::BLACKHOLE);
        assert_eq!(h.difficulty, U256::from(1u64));
        assert_eq!(h.gas_limit, 12_000_000, "the schedule's limit");
        assert_eq!(
            h.base_fee_per_gas,
            Some(25_000_000_000),
            "the minimum, after the genesis"
        );
        let ruled = rule(&g.env(), &parent, None, &built.header, NOW).expect("passes");
        assert_eq!(ruled.base_fee, env.base_fee);
        assert_eq!(ruled.extra_data, env.extra_data);
        assert_eq!(ruled.block_gas_cost, 0);

        // The second, in the same second: its window holds the first's gas,
        // and it owes a block gas cost of two steps.
        let next = env
            .next(&built.header, Some(env.block_gas_cost), env.timestamp)
            .expect("derives");
        assert_eq!(next.block_gas_cost, 400_000);
        let w = fee::Window::parse(&next.extra_data).expect("a window");
        assert_eq!(w.0[9], 21_000, "the parent's gas, in its second");
        let second = execute(
            &mut world,
            &built.header,
            &next,
            vec![paying(1, env.chain_id, U256::from(1u64), 500_000_000_000)],
        )
        .expect("the tip covers the cost");
        assert_eq!(
            second.header.layout.name, "23",
            "a cost is carried, after Cancun"
        );
        assert_eq!(second.header.block_gas_cost, Some(U256::from(400_000u64)));
        rule(&g.env(), &built.header, Some(0), &second.header, NOW).expect("passes");
    }

    #[test]
    fn a_fast_block_pays_its_block_gas_cost_or_is_not_a_block() {
        let (mut world, parent, env, _) = chain();
        let first = execute(
            &mut world,
            &parent,
            &env,
            vec![send(0, env.chain_id, U256::ZERO)],
        )
        .expect("first");
        let next = env
            .next(&first.header, Some(env.block_gas_cost), env.timestamp)
            .expect("derives");
        // 400,000 gas at 25 gwei is 1e16 wei of tips; 21,000 gas at 1 gwei is
        // far short of it.
        let refused = execute(
            &mut world.clone(),
            &first.header,
            &next,
            vec![send(1, env.chain_id, U256::ZERO)],
        );
        assert!(matches!(refused, Err(Invalid::Fee(_))), "{refused:?}");
        // 476,190,476,191 wei a gas is the least tip that covers it.
        let short = paying(1, env.chain_id, U256::ZERO, 476_190_476_190);
        let enough = paying(1, env.chain_id, U256::ZERO, 476_190_476_191);
        assert!(matches!(
            execute(&mut world.clone(), &first.header, &next, vec![short]),
            Err(Invalid::Fee(_))
        ));
        execute(&mut world, &first.header, &next, vec![enough]).expect("exactly covered");

        // Two seconds on, the target rate, and nothing is owed.
        let slow = env
            .next(&first.header, Some(env.block_gas_cost), env.timestamp + 2)
            .expect("derives");
        assert_eq!(slow.block_gas_cost, 0);
    }

    #[test]
    fn every_derived_field_is_held_to_what_the_parent_gives() {
        let g = funded();
        let (mut world, parent, env, _) = chain();
        let built = execute(
            &mut world,
            &parent,
            &env,
            vec![send(0, env.chain_id, U256::ZERO)],
        )
        .expect("builds");
        let ok = |h: &header::Lux| rule(&g.env(), &parent, None, h, NOW);
        ok(&built.header).expect("the block as built");
        type Edit = fn(&mut header::Lux);
        let edits: [(&str, Edit); 15] = [
            ("number", |h| h.header.number += 1),
            ("before its parent", |h| h.header.timestamp = 0),
            ("gas limit", |h| h.header.gas_limit += 1),
            ("base fee", |h| {
                h.header.base_fee_per_gas = Some(25_000_000_001)
            }),
            ("window", |h| {
                let mut e = h.header.extra_data.to_vec();
                e[79] = 1;
                h.header.extra_data = e.into();
            }),
            ("predicate results", |h| {
                let mut e = h.header.extra_data.to_vec();
                e[85] = 1;
                h.header.extra_data = e.into();
            }),
            ("no predicate results", |h| {
                h.header.extra_data = h.header.extra_data.slice(..80)
            }),
            ("short extra", |h| {
                h.header.extra_data = h.header.extra_data.slice(..79)
            }),
            ("coinbase", |h| h.header.beneficiary = Address::ZERO),
            ("difficulty", |h| h.header.difficulty = U256::ZERO),
            ("nonce", |h| h.header.nonce = B64::from(1u64)),
            ("mix digest", |h| h.header.mix_hash = B256::repeat_byte(1)),
            ("uncle hash", |h| h.header.ommers_hash = B256::ZERO),
            ("block gas cost", |h| {
                h.layout = header::named("23");
                h.block_gas_cost = Some(U256::from(1u64));
                h.ext_data_hash = Some(B256::ZERO);
                h.ext_data_gas_used = Some(Bytes::new());
            }),
            ("blob gas", |h| {
                h.layout = header::named("20 lux");
                h.block_gas_cost = Some(U256::ZERO);
                h.ext_data_gas_used = Some(Bytes::new());
                h.header.blob_gas_used = Some(1);
            }),
        ];
        for (what, edit) in edits {
            let mut h = built.header.clone();
            edit(&mut h);
            assert!(
                matches!(ok(&h), Err(Invalid::Header(_))),
                "{what}: {:?}",
                ok(&h)
            );
        }
        let future = rule(
            &g.env(),
            &parent,
            None,
            &built.header,
            Some(env.timestamp - FUTURE - 1),
        );
        assert!(
            matches!(future, Err(Invalid::Header(_))),
            "a block past the clock"
        );
        rule(
            &g.env(),
            &parent,
            None,
            &built.header,
            Some(env.timestamp - FUTURE),
        )
        .expect("ten seconds ahead is allowed");
        rule(&g.env(), &parent, None, &built.header, None).expect("history is not clocked");
    }

    #[test]
    fn a_header_that_omits_its_cost_is_under_the_derived_one() {
        // Go's builder never writes the cost into a header, and its VM fills
        // an absent one from the parent before checking it; what this crate
        // builds always carries it. A header that omits one is held to the
        // derived cost — and the block fee is still owed.
        let g = funded();
        let (mut world, parent, env, _) = chain();
        let first = execute(
            &mut world,
            &parent,
            &env,
            vec![send(0, env.chain_id, U256::ZERO)],
        )
        .expect("first");
        let next = env
            .next(&first.header, Some(0), env.timestamp)
            .expect("derives");
        let second = execute(
            &mut world,
            &first.header,
            &next,
            vec![paying(1, env.chain_id, U256::ZERO, 500_000_000_000)],
        )
        .expect("second");
        assert_eq!(second.header.block_gas_cost, Some(U256::from(400_000u64)));
        let bare = header::Lux::block(second.fields().clone(), U256::ZERO);
        assert_eq!(bare.layout.name, "20 eth");
        let ruled = rule(&g.env(), &first.header, Some(0), &bare, NOW).expect("passes");
        assert_eq!(
            ruled.block_gas_cost, 400_000,
            "and it is under the derived cost"
        );
        // Its child derives from that cost, not from the absent field.
        let child = ruled
            .next(&bare, Some(ruled.block_gas_cost), ruled.timestamp)
            .expect("derives");
        assert_eq!(child.block_gas_cost, 800_000);
    }

    #[test]
    fn a_coinbase_is_the_builders_only_where_the_chain_allows_it() {
        let g = genesis::Genesis {
            allow_fee_recipients: true,
            ..funded()
        };
        let parent = g.sealed();
        let mine = address!("00000000000000000000000000000000000000c0");
        let env = Env {
            beneficiary: mine,
            ..g.env()
        }
        .next(&parent, None, parent.header.timestamp + 60)
        .expect("derives");
        assert_eq!(env.beneficiary, mine);
        let mut world = g.world();
        let built = execute(
            &mut world,
            &parent,
            &env,
            vec![send(0, env.chain_id, U256::ZERO)],
        )
        .expect("builds");
        let ruled = rule(&g.env(), &parent, None, &built.header, NOW).expect("any coinbase");
        assert_eq!(
            ruled.beneficiary, mine,
            "the block runs under the one it names"
        );
    }

    #[test]
    fn a_warp_predicate_is_refused_once_warp_is_on() {
        let (mut world, _parent, env, _) = chain();
        let tx = TxEip1559 {
            chain_id: env.chain_id,
            nonce: 0,
            gas_limit: 100_000,
            max_fee_per_gas: 30_000_000_000,
            max_priority_fee_per_gas: 1_000_000_000,
            to: TxKind::Call(TO),
            access_list: alloy_eips::eip2930::AccessList(vec![
                alloy_eips::eip2930::AccessListItem {
                    address: WARP,
                    storage_keys: vec![B256::ZERO],
                },
            ]),
            ..Default::default()
        };
        let sig = sign(tx.signature_hash());
        let tx = TxEnvelope::Eip1559(tx.into_signed(sig));
        let at = |t: u64| Env {
            activations: vec![Activation {
                address: WARP,
                at: t,
            }],
            ..env.clone()
        };
        assert_eq!(
            apply(&mut world.clone(), &at(env.timestamp), 0, &tx, who()).err(),
            Some(Invalid::Predicate)
        );
        apply(&mut world, &at(env.timestamp + 1), 0, &tx, who())
            .expect("an access list, not yet a predicate");
    }

    /// A call to `to` from the test key, at `nonce`.
    fn calling(nonce: u64, env: &Env, to: Address, input: Bytes) -> TxEnvelope {
        let tx = TxEip1559 {
            chain_id: env.chain_id,
            nonce,
            gas_limit: 200_000,
            max_fee_per_gas: 30_000_000_000,
            max_priority_fee_per_gas: 1_000_000_000,
            to: TxKind::Call(to),
            input,
            ..Default::default()
        };
        let sig = sign(tx.signature_hash());
        TxEnvelope::Eip1559(tx.into_signed(sig))
    }

    #[test]
    fn a_call_to_an_active_precompile_this_crate_does_not_run_is_refused() {
        let (mut world, _parent, env, _) = chain();
        let dex = precompile::ALWAYS_ON[0].address;
        let at = |t: u64| Env {
            activations: vec![Activation {
                address: dex,
                at: t,
            }],
            ..env.clone()
        };
        let tx = calling(0, &env, dex, Bytes::new());
        let refused = apply(&mut world.clone(), &at(env.timestamp), 0, &tx, who());
        assert!(
            matches!(&refused, Err(Invalid::Rejected(why)) if why.contains("does not run")),
            "{refused:?}"
        );
        apply(&mut world, &at(env.timestamp + 1), 0, &tx, who()).expect("not yet active");
    }

    #[test]
    fn an_activation_writes_its_marker_on_the_block_that_crosses_it() {
        let (mut world, parent, env, _) = chain();
        let dex = precompile::ALWAYS_ON[0].address;
        let at = |t: u64| Env {
            activations: vec![Activation {
                address: dex,
                at: t,
            }],
            ..env.clone()
        };
        let mut before = world.clone();
        begin(&mut before, &at(env.timestamp + 1), &parent).expect("begins");
        assert!(before.get(&dex).is_none(), "not crossed yet");
        begin(&mut world, &at(env.timestamp), &parent).expect("begins");
        let a = world.get(&dex).expect("marked");
        assert_eq!(
            (a.nonce, world.code(&dex).original_byte_slice()),
            (1, &precompile::MARKER[..])
        );
    }

    #[test]
    fn a_blob_transaction_is_not_in_a_block() {
        let (mut world, _parent, env, _) = chain();
        let tx = alloy_consensus::TxEip4844 {
            chain_id: env.chain_id,
            nonce: 0,
            gas_limit: 100_000,
            max_fee_per_gas: 30_000_000_000,
            max_priority_fee_per_gas: 1_000_000_000,
            to: TO,
            max_fee_per_blob_gas: 1,
            blob_versioned_hashes: vec![B256::repeat_byte(1)],
            ..Default::default()
        };
        let sig = sign(tx.signature_hash());
        let tx: TxEnvelope = tx.into_signed(sig).into();
        assert_eq!(
            apply(&mut world, &env, 0, &tx, who()).err(),
            Some(Invalid::Blob)
        );
    }

    #[test]
    fn the_fork_is_the_one_the_block_time_selects() {
        use revm::primitives::hardfork::SpecId;
        let late = funded().timestamp + 100;
        let g = genesis::Genesis {
            chain: Chain {
                cancun: Some(late),
                ..Chain::lux()
            },
            ..funded()
        };
        let parent = g.sealed();
        let shanghai = g.env().next(&parent, None, late - 1).expect("derives");
        assert_eq!(g.chain.spec(late - 1), SpecId::SHANGHAI);
        let mut world = g.world();
        let built = execute(
            &mut world,
            &parent,
            &shanghai,
            vec![send(0, g.chain_id, U256::ZERO)],
        )
        .expect("a Shanghai block");
        assert_eq!(built.header.layout.name, "17, withdrawals");
        assert_eq!(built.fields().withdrawals_root, Some(root::EMPTY));
        assert_eq!(built.fields().excess_blob_gas, None);
        let cancun = g
            .env()
            .next(&built.header, Some(shanghai.block_gas_cost), late)
            .expect("derives");
        assert_eq!(g.chain.spec(late), SpecId::CANCUN);
        let tip = 500_000_000_000;
        let next = execute(
            &mut world,
            &built.header,
            &cancun,
            vec![paying(1, g.chain_id, U256::ZERO, tip)],
        )
        .expect("a Cancun block");
        assert_eq!(next.header.layout.name, "23", "Cancun, owing a cost");
        assert_eq!(next.fields().blob_gas_used, Some(0));
        assert_eq!(next.fields().excess_blob_gas, Some(0));
        assert_eq!(next.fields().parent_beacon_block_root, Some(B256::ZERO));
        rule(
            &g.env(),
            &built.header,
            Some(shanghai.block_gas_cost),
            &next.header,
            NOW,
        )
        .expect("passes");
    }

    #[test]
    fn cancun_answers_as_go_does() {
        // TSTORE 42 at 7, TLOAD 7 to memory 0; MCOPY it to 32; BLOBBASEFEE to
        // 64, BLOBHASH(0) to 96, NUMBER to 128; return all five words.
        let code = Bytes::from_static(&[
            0x60, 0x2a, 0x60, 0x07, 0x5d, // tstore(7, 42)
            0x60, 0x07, 0x5c, 0x5f, 0x52, // mstore(0, tload(7))
            0x60, 0x20, 0x5f, 0x60, 0x20, 0x5e, // mcopy(32, 0, 32)
            0x4a, 0x60, 0x40, 0x52, // mstore(64, blobbasefee)
            0x5f, 0x49, 0x60, 0x60, 0x52, // mstore(96, blobhash(0))
            0x43, 0x60, 0x80, 0x52, // mstore(128, number)
            0x60, 0xa0, 0x5f, 0xf3, // return(0, 160)
        ]);
        let (mut world, _parent, env, _) = chain();
        let at = address!("00000000000000000000000000000000000000aa");
        let bytecode = revm::bytecode::Bytecode::new_raw(code);
        world.insert(
            at,
            crate::state::Account {
                code_hash: bytecode.hash_slow(),
                ..Default::default()
            },
            Some(bytecode),
        );
        let out = call(
            world,
            &env,
            TxEnv::builder()
                .caller(who())
                .kind(TxKind::Call(at))
                .gas_limit(100_000)
                .nonce(0)
                .chain_id(None)
                .build_fill(),
        )
        .expect("runs");
        assert!(out.is_success(), "{out:?}");
        let words: Vec<U256> = out
            .into_output()
            .unwrap_or_default()
            .chunks(32)
            .map(U256::from_be_slice)
            .collect();
        assert_eq!(
            words,
            vec![
                U256::from(42u64),
                U256::from(42u64),
                U256::from(1u64),
                U256::ZERO,
                U256::from(env.number)
            ]
        );
    }

    #[test]
    fn an_empty_beacon_roots_account_is_touched_away() {
        // Go's `Call` to an existing account touches it, and EIP-161 deletes a
        // touched empty account: an empty one at the beacon-roots address does
        // not survive the first Cancun block.
        let (mut world, parent, env, _) = chain();
        world.insert(BEACON_ROOTS, crate::state::Account::default(), None);
        assert!(world.get(&BEACON_ROOTS).is_some());
        begin(&mut world, &env, &parent).expect("begins");
        assert!(world.get(&BEACON_ROOTS).is_none());
    }

    #[test]
    fn a_parent_beacon_root_reaches_the_beacon_roots_contract() {
        // EIP-4788's contract, as deployed on Ethereum.
        let code = Bytes::from(alloy_primitives::hex::decode(
            "3373fffffffffffffffffffffffffffffffffffffffe14604d57602036146024575f5ffd5b5f35801560495762001fff810690815414603c575f5ffd5b62001fff01545f5260205ff35b5f5ffd5b62001fff42064281555f359062001fff015500",
        ).expect("hex"));
        let (mut world, parent, env, _) = chain();
        let bytecode = revm::bytecode::Bytecode::new_raw(code);
        world.insert(
            BEACON_ROOTS,
            crate::state::Account {
                nonce: 1,
                code_hash: bytecode.hash_slow(),
                ..Default::default()
            },
            Some(bytecode),
        );
        let root = B256::repeat_byte(0x42);
        let with = Env {
            parent_beacon_root: Some(root),
            ..env.clone()
        };
        begin(&mut world, &with, &parent).expect("begins");
        let slot = U256::from(env.timestamp % 8191);
        assert_eq!(
            world.storage(&BEACON_ROOTS, slot),
            U256::from(env.timestamp)
        );
        assert_eq!(
            world.storage(&BEACON_ROOTS, slot + U256::from(8191u64)),
            U256::from_be_bytes(root.0)
        );
    }

    #[test]
    fn the_sha256_precompile_returns_the_digest_anyone_else_computes() {
        // Real bytecode execution, checked against a value that has nothing to
        // do with this codebase: SHA-256("abc") is a published constant.
        let (world, env, _) = {
            let (w, _p, e, who) = chain();
            (w, e, who)
        };
        let out = call(
            world,
            &env,
            TxEnv::builder()
                .caller(who())
                .kind(TxKind::Call(Address::from_slice(&{
                    let mut a = [0u8; 20];
                    a[19] = 2;
                    a
                })))
                .gas_limit(100_000)
                .gas_price(env.base_fee as u128)
                .data(Bytes::from_static(b"abc"))
                .nonce(0)
                // A call is not signed, so it carries no EIP-155 binding.
                .chain_id(None)
                .build_fill(),
        )
        .expect("the call runs");
        assert!(out.is_success(), "{out:?}");
        assert_eq!(
            alloy_primitives::hex::encode(out.into_output().unwrap_or_default()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "SHA-256(\"abc\")"
        );
    }

    #[test]
    fn prevrandao_is_the_headers_difficulty_as_go_reads_it() {
        // PREVRANDAO; PUSH0 MSTORE; RETURN(0, 32): the word the opcode answers.
        let code = Bytes::from_static(&[0x44, 0x5f, 0x52, 0x60, 0x20, 0x5f, 0xf3]);
        let (mut world, _parent, env, _) = chain();
        let at = address!("00000000000000000000000000000000000000aa");
        world.insert(
            at,
            crate::state::Account {
                code_hash: revm::bytecode::Bytecode::new_raw(code.clone()).hash_slow(),
                ..Default::default()
            },
            Some(revm::bytecode::Bytecode::new_raw(code)),
        );
        assert_eq!(env.difficulty, U256::from(1u64));
        let out = call(
            world,
            &env,
            TxEnv::builder()
                .caller(who())
                .kind(TxKind::Call(at))
                .gas_limit(100_000)
                .nonce(0)
                .chain_id(None)
                .build_fill(),
        )
        .expect("runs");
        assert_eq!(
            out.into_output().unwrap_or_default(),
            Bytes::from(B256::from(U256::from(1u64)).to_vec()),
            "the difficulty, not the zero mix digest"
        );
    }

    #[test]
    fn consensus_still_refuses_a_sender_that_cannot_pay() {
        // The guard on the simulation relaxation: `Checks::All` is what a block
        // is verified under, and enabling revm's optional_* features must not
        // have loosened it. An account with nothing cannot send a transaction.
        let (mut world, _parent, env, _) = chain();
        let broke = address!("00000000000000000000000000000000000000bb");
        let refused = run(
            &mut world,
            &env,
            TxEnv::builder()
                .caller(broke)
                .kind(TxKind::Call(TO))
                .gas_limit(100_000)
                .gas_price(30_000_000_000u128)
                .nonce(0)
                .chain_id(Some(env.chain_id))
                .build_fill(),
            Checks::All,
        );
        assert!(
            matches!(&refused, Err(Invalid::Rejected(why)) if why.contains("LackOfFundForMaxFee")),
            "{refused:?}"
        );
        // And the same transaction simulates fine, which is the whole point of
        // there being two answers.
        assert!(call(
            world,
            &env,
            TxEnv::builder()
                .caller(broke)
                .kind(TxKind::Call(TO))
                .gas_limit(100_000)
                .nonce(0)
                .chain_id(None)
                .build_fill()
        )
        .is_ok());
    }

    #[test]
    fn a_call_keeps_nothing() {
        let (world, _parent, env, _) = chain();
        let before = world.root();
        let _ = call(
            world.clone(),
            &env,
            TxEnv::builder()
                .caller(who())
                .kind(TxKind::Call(TO))
                .gas_limit(100_000)
                .gas_price(env.base_fee as u128)
                .value(U256::from(1u64))
                .nonce(0)
                .chain_id(None)
                .build_fill(),
        )
        .expect("runs");
        assert_eq!(world.root(), before, "a call is not a transaction");
        assert_eq!(world.balance(&TO), U256::ZERO);
    }

    /// One receipt, so each field can be moved on its own.
    fn one_receipt(ty: TxType, success: bool, cumulative: u64, logs: Vec<Log>) -> ReceiptEnvelope {
        envelope(
            ty,
            Receipt {
                status: Eip658Value::Eip658(success),
                cumulative_gas_used: cumulative,
                logs,
            },
        )
    }

    fn receipts_root(rs: &[ReceiptEnvelope]) -> B256 {
        let encoded: Vec<Vec<u8>> = rs.iter().map(|r| r.encoded_2718()).collect();
        root::receipts(&encoded)
    }

    #[test]
    fn every_field_a_receipt_commits_to_moves_the_receipts_root() {
        // The root is a commitment, and a commitment that ignores a field is a
        // field two chains can disagree about while agreeing on the block. Each
        // of these was a mutation the tests here used to sleep through.
        let log = Log::new_unchecked(
            TO,
            vec![B256::repeat_byte(0x11)],
            Bytes::from_static(b"data"),
        );
        let base = one_receipt(TxType::Eip1559, true, 21_000, vec![log.clone()]);
        let root_of = |r: &ReceiptEnvelope| receipts_root(std::slice::from_ref(r));
        let want = root_of(&base);

        assert_ne!(
            want,
            root_of(&one_receipt(
                TxType::Legacy,
                true,
                21_000,
                vec![log.clone()]
            )),
            "the type flag is part of the 2718 envelope the trie commits to"
        );
        assert_ne!(
            want,
            root_of(&one_receipt(
                TxType::Eip1559,
                false,
                21_000,
                vec![log.clone()]
            )),
            "success and failure are different receipts"
        );
        assert_ne!(
            want,
            root_of(&one_receipt(
                TxType::Eip1559,
                true,
                21_001,
                vec![log.clone()]
            )),
            "cumulative gas is committed to"
        );
        assert_ne!(
            want,
            root_of(&one_receipt(TxType::Eip1559, true, 21_000, vec![])),
            "dropping the logs is a different receipt"
        );

        // And the bloom is not decoration: it is inside the encoding, so a
        // receipt whose bloom was zeroed does not hash to the same value.
        let mut blanked = match base.clone() {
            ReceiptEnvelope::Eip1559(mut r) => {
                r.logs_bloom = Bloom::ZERO;
                ReceiptEnvelope::Eip1559(r)
            }
            other => other,
        };
        assert_ne!(want, root_of(&blanked), "the logs bloom is committed to");
        blanked = base.clone();
        assert_eq!(
            want,
            root_of(&blanked),
            "and the unchanged receipt still matches"
        );
    }

    #[test]
    fn a_receipt_in_another_position_is_another_root() {
        let a = one_receipt(TxType::Eip1559, true, 21_000, vec![]);
        let b = one_receipt(TxType::Eip1559, true, 42_000, vec![]);
        assert_ne!(
            receipts_root(&[a.clone(), b.clone()]),
            receipts_root(&[b, a]),
            "position is part of the commitment"
        );
    }

    #[test]
    fn the_block_bloom_is_the_or_of_its_receipts() {
        // The header's bloom has to be derivable from the receipts, or a node
        // could serve a block whose bloom answers filters the receipts do not.
        let log = Log::new_unchecked(TO, vec![B256::repeat_byte(0x22)], Bytes::new());
        let r = one_receipt(TxType::Eip1559, true, 21_000, vec![log]);
        let folded = [r.clone()]
            .iter()
            .fold(Bloom::ZERO, |acc, x| acc | bloom_of(x));
        assert_eq!(folded, bloom_of(&r));
        assert_ne!(folded, Bloom::ZERO, "a receipt with a log has a bloom");
    }

    #[test]
    fn a_legacy_transaction_is_a_legacy_receipt() {
        let (mut world, _parent, env, _) = chain();
        let tx = TxLegacy {
            chain_id: Some(env.chain_id),
            nonce: 0,
            gas_price: 30_000_000_000,
            gas_limit: 100_000,
            to: TxKind::Call(TO),
            value: U256::from(5u64),
            ..Default::default()
        };
        let sig = sign(tx.signature_hash());
        let env_tx = TxEnvelope::Legacy(tx.into_signed(sig));
        let done = apply(&mut world, &env, 0, &env_tx, who()).expect("applies");
        assert!(matches!(done.receipt, ReceiptEnvelope::Legacy(_)));
        assert_eq!(world.balance(&TO), U256::from(5u64));
    }

    // Red's review probes (review7), held to Go's answers.

    fn install(world: &mut World, at: Address, code: Vec<u8>) {
        let bytecode = revm::bytecode::Bytecode::new_raw(Bytes::from(code));
        world.insert(
            at,
            crate::state::Account {
                code_hash: bytecode.hash_slow(),
                nonce: 1,
                ..Default::default()
            },
            Some(bytecode),
        );
    }

    fn run_at(world: World, env: &Env, at: Address) -> ExecutionResult {
        call(
            world,
            env,
            TxEnv::builder()
                .caller(who())
                .kind(TxKind::Call(at))
                .gas_limit(1_000_000)
                .nonce(0)
                .chain_id(None)
                .build_fill(),
        )
        .expect("runs")
    }

    fn word(out: ExecutionResult) -> U256 {
        assert!(out.is_success(), "{out:?}");
        U256::from_be_slice(&out.into_output().unwrap_or_default())
    }

    const PROBE: Address = address!("00000000000000000000000000000000000000aa");

    #[test]
    fn an_address_is_warm_only_if_go_warms_it() {
        // GAS; PUSH20 a; BALANCE; POP; GAS; SWAP1; SUB; return it: 7 gas of
        // opcodes around BALANCE's access cost. Go warms Ethereum's set for the
        // fork (0x01–0x0a under Cancun) and nothing else.
        let (world, _parent, env, _) = chain();
        for (target, want) in [
            (address!("0000000000000000000000000000000000000009"), 107u64),
            (address!("000000000000000000000000000000000000000a"), 107),
            (address!("000000000000000000000000000000000000000b"), 2_607),
            (address!("0000000000000000000000000000000000000011"), 2_607),
            (address!("0000000000000000000000000000000000000012"), 2_607),
            (WARP, 2_607),
            (precompile::ALWAYS_ON[0].address, 2_607),
        ] {
            let mut code = vec![0x5a, 0x73];
            code.extend_from_slice(target.as_slice());
            code.extend_from_slice(&[
                0x31, 0x50, 0x5a, 0x90, 0x03, 0x5f, 0x52, 0x60, 0x20, 0x5f, 0xf3,
            ]);
            let mut w = world.clone();
            install(&mut w, PROBE, code);
            assert_eq!(word(run_at(w, &env, PROBE)), U256::from(want), "{target}");
        }
    }

    #[test]
    fn every_kind_of_call_to_an_active_unrun_precompile_refuses_the_block() {
        let (world, parent, env, _) = chain();
        let dex = precompile::ALWAYS_ON[0].address;
        let on = Env {
            activations: vec![Activation {
                address: dex,
                at: env.timestamp,
            }],
            ..env.clone()
        };
        let kinds: [(&str, &[u8]); 4] = [
            (
                "CALL",
                &[
                    0x5f, 0x5f, 0x5f, 0x5f, 0x5f, 0x61, 0x99, 0x99, 0x5a, 0xf1, 0x50, 0x00,
                ],
            ),
            (
                "CALLCODE",
                &[
                    0x5f, 0x5f, 0x5f, 0x5f, 0x5f, 0x61, 0x99, 0x99, 0x5a, 0xf2, 0x50, 0x00,
                ],
            ),
            (
                "DELEGATECALL",
                &[
                    0x5f, 0x5f, 0x5f, 0x5f, 0x61, 0x99, 0x99, 0x5a, 0xf4, 0x50, 0x00,
                ],
            ),
            (
                "STATICCALL",
                &[
                    0x5f, 0x5f, 0x5f, 0x5f, 0x61, 0x99, 0x99, 0x5a, 0xfa, 0x50, 0x00,
                ],
            ),
        ];
        for (name, code) in kinds {
            let mut w = world.clone();
            install(&mut w, PROBE, code.to_vec());
            let block = execute(
                &mut w,
                &parent,
                &on,
                vec![(calling(0, &on, PROBE, Bytes::new()), who())],
            );
            assert!(block.is_err(), "{name}");
        }
        let mut w = world.clone();
        let plain = execute(
            &mut w,
            &parent,
            &on,
            vec![(calling(0, &on, dex, Bytes::new()), who())],
        );
        assert!(plain.is_err(), "a plain transaction to it");
    }

    #[test]
    fn bls_answers_only_once_turned_on() {
        // STATICCALL(gas, 0x0b, 0, 256, 0, 0); return RETURNDATASIZE.
        let code = vec![
            0x5f, 0x5f, 0x61, 0x01, 0x00, 0x5f, 0x60, 0x0b, 0x5a, 0xfa, 0x50, 0x3d, 0x5f, 0x52,
            0x60, 0x20, 0x5f, 0xf3,
        ];
        let (mut world, _parent, env, _) = chain();
        install(&mut world, PROBE, code);
        assert_eq!(
            word(run_at(world.clone(), &env, PROBE)),
            U256::ZERO,
            "an account, not a precompile"
        );
        let g1_add = address!("000000000000000000000000000000000000000b");
        let on = Env {
            activations: vec![Activation {
                address: g1_add,
                at: 0,
            }],
            ..env.clone()
        };
        assert_eq!(
            word(run_at(world, &on, PROBE)),
            U256::from(128u64),
            "the G1 sum of two infinities"
        );
    }

    #[test]
    fn a_shanghai_only_block_owing_a_cost_reads_back_as_itself() {
        let g = genesis::Genesis {
            chain: Chain {
                cancun: None,
                ..Chain::lux()
            },
            ..funded()
        };
        let parent = g.sealed();
        let mut world = g.world();
        let env = g
            .env()
            .next(&parent, None, parent.header.timestamp + 60)
            .expect("derives");
        let first = execute(
            &mut world,
            &parent,
            &env,
            vec![send(0, g.chain_id, U256::ZERO)],
        )
        .expect("first");
        let next = env
            .next(&first.header, Some(env.block_gas_cost), env.timestamp)
            .expect("derives");
        let second = execute(
            &mut world,
            &first.header,
            &next,
            vec![paying(1, g.chain_id, U256::ZERO, 500_000_000_000)],
        )
        .expect("second");
        assert_eq!(second.header.layout.name, "23");
        let read = header::Lux::read(&second.header.encoded()).expect("reads back");
        assert_eq!(
            (read.layout.name, read.seal()),
            ("23", second.header.seal())
        );
        let ruled = rule(
            &g.env(),
            &first.header,
            Some(env.block_gas_cost),
            &read,
            NOW,
        )
        .expect("passes");
        assert_eq!(ruled.block_gas_cost, 400_000);
    }

    #[test]
    fn blockhash_answers_the_last_256_blocks() {
        // [BLOCKHASH(n-1), BLOCKHASH(n), BLOCKHASH(n-256), BLOCKHASH(n-257),
        //  BLOCKHASH(0), NUMBER]
        let code = vec![
            0x60, 0x01, 0x43, 0x03, 0x40, 0x5f, 0x52, 0x43, 0x40, 0x60, 0x20, 0x52, 0x61, 0x01,
            0x00, 0x43, 0x03, 0x40, 0x60, 0x40, 0x52, 0x61, 0x01, 0x01, 0x43, 0x03, 0x40, 0x60,
            0x60, 0x52, 0x5f, 0x40, 0x60, 0x80, 0x52, 0x43, 0x60, 0xa0, 0x52, 0x60, 0xc0, 0x5f,
            0xf3,
        ];
        let g = funded();
        let mut parent = g.sealed();
        let mut world = g.world();
        install(&mut world, PROBE, code);
        let mut seals = vec![parent.seal()];
        let mut env = g.env();
        let mut cost = None;
        for n in 1..=300u64 {
            env = env
                .next(&parent, cost, parent.header.timestamp + 60)
                .expect("derives");
            if n == 2 || n == 300 {
                let mut w = world.clone();
                begin(&mut w, &env, &parent).expect("begins");
                let words: Vec<U256> = run_at(w, &env, PROBE)
                    .into_output()
                    .unwrap_or_default()
                    .chunks(32)
                    .map(U256::from_be_slice)
                    .collect();
                let hash = |k: u64| U256::from_be_bytes(seals[k as usize].0);
                assert_eq!(words[0], hash(n - 1), "the parent");
                assert_eq!(words[1], U256::ZERO, "not this block");
                assert_eq!(words[5], U256::from(n), "NUMBER");
                if n == 300 {
                    assert_eq!(words[2], hash(n - 256), "256 back");
                    assert_eq!(words[3], U256::ZERO, "257 back");
                    assert_eq!(words[4], U256::ZERO, "the genesis, out of reach");
                } else {
                    assert_eq!(words[4], hash(0), "the genesis, in reach");
                }
            }
            let built = execute(&mut world, &parent, &env, vec![]).expect("an empty block");
            seals.push(built.header.seal());
            cost = Some(env.block_gas_cost);
            parent = built.header;
        }
    }
}
