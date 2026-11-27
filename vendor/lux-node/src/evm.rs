// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! The C-chain: an EVM behind the [`Vm`] seam.
//!
//! Everything about *what a block means* is in `lux-evm` — the interpreter, the
//! Lux rules, the tries. What is here is the part that makes a chain out of it:
//! which block is accepted, what is pending, and what the world looks like
//! after each one.
//!
//! THE STATE ROOT IS COMPUTED, NEVER ASSERTED. A block built here carries the
//! root of the world that resulted from running its transactions, and
//! [`Evm::verify`] re-runs a block it did not build and refuses it if the root
//! it arrives at is not the one the header claims. That is the only reason a
//! certificate over a root means anything: both ends did the work.
//!
//! WHAT IS KEPT. Every accepted block, in the chain's directory, written before
//! it is accepted — see [`Evm::open`]. In memory: the accepted world, one world
//! per block verified but not yet accepted, the last [`RETAINED`] accepted
//! blocks, and an index from every height, block id and transaction hash to
//! where it is on disk. Historical WORLDS are not kept: this node answers
//! state questions about the tip, and says so plainly rather than inventing an
//! answer for a state it no longer holds. Historical blocks, transactions,
//! receipts and logs it answers at every accepted height.

mod ledger;
mod pool;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::Path;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use alloy_consensus::transaction::SignerRecoverable as _;
use alloy_consensus::{
    Block as RlpBlock, BlockBody, Header, ReceiptEnvelope, Transaction as _, TxEnvelope,
};
use alloy_eips::eip2718::Decodable2718;
use alloy_eips::Typed2718 as _;
use alloy_primitives::{Address, Bloom, BloomInput, Bytes, Log, B256, U256};
use alloy_rlp::Decodable as _;
use lux_evm::block::{self, Built, Env, Invalid};
use lux_evm::header;
use lux_evm::state::World;
use revm::context::TxEnv;
use revm::primitives::TxKind;
use serde_json::{json, Map, Value};

use crate::vm::{Block, Error, Id, Vm};
use ledger::{Files, Ledger, Span};
use pool::{Pending, Pool};

/// What a chain starts from, as [`Evm::open`] takes it.
pub use lux_evm::Genesis;

/// How many accepted blocks are held in memory.
///
/// Older ones are read back from the ledger when asked for. `BLOCKHASH` reaches
/// 256 blocks, and a peer catching up asks for recent ones, so a margin past
/// that keeps both off the disk.
pub const RETAINED: u64 = 512;

/// The widest block range one `eth_getLogs` call scans.
///
/// Every block in the range may be a read from disk, so an unbounded range is
/// one request that keeps an RPC thread reading the whole chain.
pub const LOGS_RANGE: u64 = 10_000;

/// The most logs one `eth_getLogs` answer carries.
pub const LOGS_MAX: usize = 10_000;

/// The most bytes one `eth_getLogs` call reads.
///
/// A range is bounded in blocks, and a block is up to two mebibytes, so a
/// bound in blocks alone is a bound of gigabytes. A block whose bloom cannot
/// match costs its header; one that can costs its body and its receipts. A
/// query that needs more than this is refused, and a narrower one answered.
pub const LOGS_READ_MAX: u64 = 64 << 20;

/// How many `eth_getLogs` range scans run at once. The rest wait their turn
/// for [`LOGS_WAIT`] and are refused after it, so a flood of scans is a queue
/// at this door — bounded work, bounded threads — and never at the chain's.
pub const LOGS_SCANS: usize = 4;

/// How long a scan waits for its turn.
pub const LOGS_WAIT: Duration = Duration::from_secs(2);

/// How many blocks may be held that are neither accepted nor refused.
///
/// A block is held from `parse` until it is accepted, refused, or its height
/// is decided, and which blocks arrive is a peer's choice. So the set is
/// bounded, and what is let go first is the oldest. Twice a catch-up batch,
/// because a batch is executed whole before any of it is accepted.
pub const UNDECIDED_MAX: usize = 128;

/// How many of those may be unexecuted. A block this node never executed cost
/// the peer that sent it nothing, so these are held to fewer.
pub const UNVERIFIED_MAX: usize = 16;

/// The least gas a transaction costs: its intrinsic gas.
const TX_GAS: u64 = 21_000;

/// Seconds since the epoch, by this node's clock.
fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The fewest signatures one thread recovers for a block; fewer are recovered
/// where they are, since a thread costs more than a few recoveries.
const RECOVER_SHARE: usize = 16;

/// The largest block a record can hold, and so the largest header in one.
const BLOCK_MAX: usize = crate::mesh::BLOCK_MAX as usize;

const _: () = assert!(UNDECIDED_MAX >= 2 * crate::mesh::BATCH_MAX as usize);

/// The version this chain reports.
///
/// Go's C-chain answers `web3_clientVersion` and `VM.Version()` with the same
/// string, so this is one constant and not two.
pub const VERSION: &str = concat!("lux-evm/v", env!("CARGO_PKG_VERSION"));

/// A block of this chain.
#[derive(Clone, Debug)]
pub struct Sealed {
    built: Built,
    id: Id,
    raw: Vec<u8>,
    /// The block gas cost the block is under, once derived: what Go's VM keeps
    /// beside a header that need not carry it (`ensureBlockGasCost`). `None`
    /// for the genesis, which holds none, and for a block not yet verified.
    cost: Option<u64>,
}

impl Sealed {
    fn new(built: Built, cost: Option<u64>) -> Self {
        let id = built.header.seal().0;
        // From Shanghai a block carries its withdrawals — none, on a Lux chain —
        // as Go's `FinalizeAndAssemble` writes it (consensus.go:420-423).
        let withdrawals = built.fields().withdrawals_root.map(|_| Default::default());
        let raw = alloy_rlp::encode(&RlpBlock {
            header: built.header.clone(),
            body: BlockBody {
                transactions: built.transactions.clone(),
                ommers: Vec::new(),
                withdrawals,
            },
        });
        Self { built, id, raw, cost }
    }

    /// The block gas cost a child of this block derives from: `None` for the
    /// genesis. A later block whose cost is not known has not been verified,
    /// and nothing is built or verified on it.
    fn cost_for_child(&self) -> Result<Option<u64>, Error> {
        if self.height() == 0 {
            return Ok(None);
        }
        self.cost.map(Some).ok_or_else(|| Error::Invalid("the parent is not verified".into()))
    }

    /// The header in the shape the chain accepts.
    pub fn sealed(&self) -> &header::Lux {
        &self.built.header
    }

    /// Its Ethereum fields, for everything that does not care about the shape.
    pub fn header(&self) -> &Header {
        self.built.fields()
    }

    pub fn transactions(&self) -> &[TxEnvelope] {
        &self.built.transactions
    }

    pub fn receipts(&self) -> &[ReceiptEnvelope] {
        &self.built.receipts
    }
}

impl Block for Sealed {
    fn id(&self) -> Id {
        self.id
    }
    fn parent(&self) -> Id {
        self.built.fields().parent_hash.0
    }
    fn height(&self) -> u64 {
        self.built.fields().number
    }
    fn timestamp(&self) -> u64 {
        self.built.fields().timestamp
    }
    fn bytes(&self) -> Vec<u8> {
        self.raw.clone()
    }
    fn state_root(&self) -> Id {
        self.built.fields().state_root.0
    }
    fn payload_root(&self) -> Id {
        self.built.fields().transactions_root.0
    }
}

impl Sealed {
    /// A block read back off the wire or the disk, as Go's `DecodeBlock` reads
    /// one: a list of at least three items — header, transactions, uncles —
    /// and whatever follows them taken as raw items and ignored, except that a
    /// fourth turns a 17-field header's hash into its withdrawals root. It
    /// keeps the bytes it arrived as and is named by its header's, as Go names
    /// it. No receipts: those are what execution produces.
    ///
    /// An uncle is refused here, as Go's `ParseBlock` refuses it in
    /// `SyntacticVerify` (plugin/evm/block_verification.go:81-90): a Lux block
    /// has none.
    fn read(raw: Vec<u8>) -> Result<Self, alloy_rlp::Error> {
        let mut rest = &raw[..];
        let list = alloy_rlp::Header::decode(&mut rest)?;
        if !list.list {
            return Err(alloy_rlp::Error::UnexpectedString);
        }
        if rest.len() != list.payload_length {
            return Err(alloy_rlp::Error::Custom("bytes after the block"));
        }
        let mut items = Vec::with_capacity(3);
        while !rest.is_empty() {
            items.push(item(&mut rest)?);
        }
        let [head, txs, uncles, more @ ..] = &items[..] else {
            return Err(alloy_rlp::Error::Custom("a block is at least three items"));
        };
        let mut sealed = header::Lux::read(head)?;
        if !more.is_empty() {
            sealed.as_withdrawals();
        }
        let id = header::seal_bytes(head).0;
        let mut txs = *txs;
        let transactions = Vec::<TxEnvelope>::decode(&mut txs)?;
        let mut uncles = *uncles;
        let ommers = alloy_rlp::Header::decode_bytes(&mut uncles, true)?;
        if !txs.is_empty() || !uncles.is_empty() {
            return Err(alloy_rlp::Error::Custom("bytes after an item"));
        }
        if !ommers.is_empty() {
            return Err(alloy_rlp::Error::Custom("a block with uncles"));
        }
        Ok(Self {
            built: Built { header: sealed, transactions, receipts: Vec::new() },
            id,
            raw,
            cost: None,
        })
    }
}

/// The next item in `rest`, prefix and payload, as Go's `rlp.Stream.Raw`
/// takes one: its length is canonical and within what is left, and its
/// payload is not looked at — a one-byte string that need not have been
/// wrapped is still an item.
fn item<'a>(rest: &mut &'a [u8]) -> Result<&'a [u8], alloy_rlp::Error> {
    let tag = *rest.first().ok_or(alloy_rlp::Error::InputTooShort)?;
    let (prefix, payload) = match tag {
        0x00..=0x7f => (0, 1),
        0x80..=0xb7 => (1, usize::from(tag - 0x80)),
        0xc0..=0xf7 => (1, usize::from(tag - 0xc0)),
        _ => {
            let n = usize::from(if tag >= 0xf8 { tag - 0xf7 } else { tag - 0xb7 });
            let be = rest.get(1..1 + n).ok_or(alloy_rlp::Error::InputTooShort)?;
            if be[0] == 0 {
                return Err(alloy_rlp::Error::LeadingZero);
            }
            let len = be
                .iter()
                .try_fold(0usize, |len, b| len.checked_mul(256).map(|l| l + usize::from(*b)));
            match len {
                Some(len) if len >= 56 => (1 + n, len),
                Some(_) => return Err(alloy_rlp::Error::NonCanonicalSize),
                None => return Err(alloy_rlp::Error::Overflow),
            }
        }
    };
    let whole = prefix.checked_add(payload).filter(|w| *w <= rest.len());
    let whole = whole.ok_or(alloy_rlp::Error::InputTooShort)?;
    let (out, after) = rest.split_at(whole);
    *rest = after;
    Ok(out)
}

/// A block's receipts as the ledger holds them: one RLP list.
fn encode_receipts(receipts: &[ReceiptEnvelope]) -> Vec<u8> {
    let mut out = Vec::new();
    alloy_rlp::encode_list(receipts, &mut out);
    out
}

/// Where an accepted block is.
#[derive(Clone, Copy, Debug)]
struct Link {
    id: Id,
    /// Its block gas cost, as derived when it was verified: what Go serves.
    cost: u64,
    /// Its record in `blocks`. The genesis has none: it is derived from the
    /// genesis document, never written.
    block: Option<Span>,
    /// Its record in `receipts`, when that file could be written.
    receipts: Option<Span>,
}

/// Everything the chain knows, under one lock.
struct Held {
    /// The world as of the last accepted block.
    world: World,
    accepted: Id,
    preferred: Id,
    /// The blocks held in memory: the last [`RETAINED`] accepted, and the
    /// ones above the tip not yet decided — at most [`UNDECIDED_MAX`] of those.
    blocks: BTreeMap<Id, Sealed>,
    /// The world each verified-but-unaccepted block would leave behind.
    verified: BTreeMap<Id, World>,
    pending: Pool,
    /// The accepted chain, one link per height from the genesis.
    chain: Vec<Link>,
    /// The height of every accepted block, by id.
    heights: HashMap<Id, u64>,
    /// Where every accepted transaction is: its block's height, and its index
    /// in it.
    located: HashMap<B256, (u64, u32)>,
    /// The undecided blocks in `blocks`, oldest first.
    undecided: VecDeque<Id>,
    /// The undecided blocks this node built, each with what it took out of the
    /// pool. A block of its own that is dropped puts that back.
    mine: HashMap<Id, Vec<Pending>>,
    ledger: Ledger,
}

impl Held {
    /// Make `sealed` the accepted tip. The world it leaves behind must already
    /// be in place, and its record already on disk: this cannot fail, because
    /// by the time it runs the block IS accepted.
    fn settle(&mut self, sealed: Sealed, link: Link) {
        let height = sealed.height();
        debug_assert_eq!(height, self.chain.len() as u64, "accepted in height order");
        let id = link.id;
        for (i, tx) in sealed.transactions().iter().enumerate() {
            self.located.insert(*tx.tx_hash(), (height, i as u32));
        }
        self.chain.push(link);
        self.heights.insert(id, height);
        self.accepted = id;
        self.preferred = id;
        self.blocks.insert(id, sealed);
        self.undecided.retain(|k| *k != id);
        self.mine.remove(&id);

        // A decided height takes nothing more. What else is held at or below
        // it can never be accepted, and the state it claimed is not reachable
        // from anywhere: dropped — and what this node built there goes back
        // to the pool, less what the accepted block carried.
        let stale: Vec<Id> = self
            .undecided
            .iter()
            .copied()
            .filter(|k| self.blocks.get(k).is_none_or(|b| b.height() <= height))
            .collect();
        for k in stale {
            self.forget(&k);
        }

        // Stop holding accepted blocks too old to be asked about often. Each
        // is still on disk.
        if let Some(floor) = height.checked_sub(RETAINED) {
            let old: Vec<Id> =
                self.blocks.iter().filter(|(_, b)| b.height() < floor).map(|(k, _)| *k).collect();
            for k in old {
                self.blocks.remove(&k);
            }
        }

        // A transaction at a nonce the chain has now used can never run.
        let world = &self.world;
        self.pending.prune(|who| world.nonce(who));
    }

    /// Hold `sealed`, a block above the tip that is not decided, and keep the
    /// undecided within bounds: let go of the oldest block nobody executed —
    /// the kind a peer sends for free — and failing that, the oldest.
    fn hold(&mut self, sealed: Sealed) {
        let id = sealed.id();
        if self.blocks.contains_key(&id) {
            return;
        }
        self.blocks.insert(id, sealed);
        self.undecided.push_back(id);
        while self.undecided.len() > UNDECIDED_MAX || self.unverified() > UNVERIFIED_MAX {
            let oldest = self.undecided.iter().copied().find(|k| !self.verified.contains_key(k));
            match oldest.or(self.undecided.front().copied()) {
                Some(k) => self.forget(&k),
                None => break,
            }
        }
    }

    /// Undecided blocks not executed yet.
    fn unverified(&self) -> usize {
        self.undecided.iter().filter(|k| !self.verified.contains_key(*k)).count()
    }

    /// Stop holding a block that is not accepted — an accepted one is never
    /// let go here. One this node built gives back what it took from the
    /// pool, ahead of the rest so its nonces stay in order, less whatever an
    /// accepted block already carried.
    fn forget(&mut self, id: &Id) {
        if self.heights.contains_key(id) {
            return;
        }
        self.blocks.remove(id);
        self.verified.remove(id);
        self.undecided.retain(|k| k != id);
        if self.preferred == *id {
            self.preferred = self.accepted;
        }
        if let Some(took) = self.mine.remove(id) {
            let back: Vec<Pending> =
                took.into_iter().filter(|p| !self.located.contains_key(p.tx.tx_hash())).collect();
            self.pending.restore(back);
        }
    }

    /// The world the preferred block leaves behind: the one the next block is
    /// built on.
    fn preferred_world(&self) -> &World {
        if self.preferred == self.accepted {
            &self.world
        } else {
            self.verified.get(&self.preferred).unwrap_or(&self.world)
        }
    }

    /// The world `who`'s next transaction is admitted against: the preferred
    /// one, or — where it has taken more of `who`'s nonces — the world after
    /// an undecided block this node executed. That is its own proposal while
    /// the quorum is out, or a peer's block it verified: either way the
    /// sender's transactions are in a block and not the pool, and the sender
    /// goes on from after them. Read at each admission, so an accept or a
    /// reject moves it with nothing to keep in step.
    fn ahead(&self, who: &Address) -> &World {
        self.verified.values().fold(self.preferred_world(), |best, w| {
            if w.nonce(who) > best.nonce(who) {
                w
            } else {
                best
            }
        })
    }
}

/// The C-chain.
pub struct Evm {
    genesis: Genesis,
    env: Env,
    /// Block zero, which no ledger holds.
    first: Sealed,
    held: Mutex<Held>,
    /// Second handles on the chain's files, so an accepted block is READ
    /// without the lock — only found under it. None for a chain that keeps
    /// nothing on disk.
    files: Option<Files>,
    /// Turns at an `eth_getLogs` range scan.
    scans: Turns,
}

impl Evm {
    /// The chain whose directory is `dir`, as it was left.
    ///
    /// This is how a node starts a chain. The directory is created if it is
    /// absent. Every block in its ledger is executed again, in order, and held
    /// to what its header claims — so the chain comes back at the same head,
    /// the same state root, the same receipts and the same transaction index,
    /// recomputed rather than believed. A record cut short by a crash was
    /// never accepted and is dropped — and so is the rest of a run of history
    /// a crash interrupted before its sync, which nothing outside saw.
    ///
    /// From then on [`Vm::accept`] writes each block to the ledger and waits
    /// for the device before accepting it, so a restart finds every block this
    /// chain ever accepted; [`Vm::accept_history`] leaves the wait to
    /// [`Vm::sync`].
    ///
    /// Refused: a directory another process has open, a ledger whose first
    /// block does not follow this genesis, and any recorded block that does not
    /// execute to what its header claims.
    pub fn open(genesis: Genesis, dir: impl AsRef<Path>) -> Result<Self, String> {
        let dir = dir.as_ref();
        let ledger = Ledger::open(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let files = ledger.files().map_err(|e| format!("{}: {e}", dir.display()))?;
        let chain = Self::start(genesis, ledger, files);
        chain.replay().map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(chain)
    }

    /// A chain at its genesis that keeps nothing across a restart.
    ///
    /// For tests and tools. A node that runs this forgets every block it
    /// accepted the moment it stops.
    pub fn new(genesis: Genesis) -> Self {
        Self::start(genesis, Ledger::memory(), None)
    }

    fn start(genesis: Genesis, ledger: Ledger, files: Option<Files>) -> Self {
        let world = genesis.world();
        let first = Sealed::new(
            Built { header: genesis.sealed(), transactions: Vec::new(), receipts: Vec::new() },
            None,
        );
        let id = first.id();
        let env = genesis.env();
        let held = Held {
            world,
            accepted: id,
            preferred: id,
            blocks: BTreeMap::from([(id, first.clone())]),
            verified: BTreeMap::new(),
            // No block's base fee is below the schedule's minimum.
            pending: Pool::new(u128::from(genesis.fee.min_base_fee)),
            chain: vec![Link { id, cost: 0, block: None, receipts: None }],
            heights: HashMap::from([(id, 0)]),
            located: HashMap::new(),
            undecided: VecDeque::new(),
            mine: HashMap::new(),
            ledger,
        };
        Self { genesis, env, first, held: Mutex::new(held), files, scans: Turns::new(LOGS_SCANS) }
    }

    /// Accept every block in the ledger again.
    ///
    /// The world is executed in place: nothing here is provisional, and a
    /// block that fails leaves a chain that is refused as a whole. Each block
    /// is held to the header rule its parent derives — by no clock: it was
    /// accepted — and its state root, transactions root, receipts root, bloom,
    /// gas and block fee are checked as it runs, so a block whose execution
    /// changed is named.
    fn replay(&self) -> Result<(), String> {
        let mut guard = self.lock();
        let held = &mut *guard;
        let mut blocks = held.ledger.blocks.scan().map_err(|e| format!("blocks: {e}"))?;
        let mut receipts = held.ledger.receipts.scan().map_err(|e| format!("receipts: {e}"))?;
        // Where the whole block records end, and where the receipts file is
        // still in step with them.
        let mut end = 0u64;
        let mut agreed = 0u64;
        let mut in_step = true;
        // Past here a crash may have cut records written without waiting.
        let loose = held.ledger.blocks.loose().unwrap_or(u64::MAX);

        while let Some((span, raw)) = blocks.next().map_err(|e| format!("blocks: {e}"))? {
            let height = held.chain.len() as u64;
            let mut sealed = match Sealed::read(raw) {
                Ok(b) => b,
                // Only the last write can be incomplete: each is on the device
                // before the next begins. A last record whose length arrived
                // and whose bytes did not is that write, and was never accepted.
                // Past the start of a run of history nothing was waited for,
                // so the first record that is not whole is where the chain ends.
                Err(_) if blocks.at_end() || span.at >= loose => break,
                Err(e) => return Err(format!("block {height} is not a block: {e}")),
            };
            if sealed.parent() != held.accepted {
                return Err(format!(
                    "block {height} names parent {}, and the chain here is at {} — this ledger is not this genesis's chain",
                    B256::from(sealed.parent()),
                    B256::from(held.accepted),
                ));
            }
            if sealed.height() != height {
                return Err(format!("block {height} says it is block {}", sealed.height()));
            }
            let parent = held.blocks.get(&held.accepted).expect("the tip is held");
            let env =
                self.rule(&sealed, parent, None).map_err(|e| format!("block {height}: {e}"))?;
            let parent = parent.sealed().clone();
            let ran = senders(&sealed, &[])
                .and_then(|txs| {
                    block::execute(&mut held.world, &parent, &env, txs)
                        .map_err(|e| Error::Invalid(e.to_string()))
                })
                .map_err(|e| format!("block {height}: {e}"))?;
            if ran.fields().state_root != sealed.header().state_root {
                return Err(format!(
                    "block {height}: state root {} claimed, {} computed by replaying the chain to it",
                    sealed.header().state_root,
                    ran.fields().state_root
                ));
            }
            check(&sealed, &ran).map_err(|e| format!("block {height}: {e}"))?;
            sealed.built.receipts = ran.receipts;
            sealed.cost = Some(env.block_gas_cost);

            // The receipts file is kept while it says what execution just said,
            // and rewritten from the first record where it does not.
            let encoded = encode_receipts(sealed.receipts());
            let mut kept = None;
            if in_step {
                match receipts.next() {
                    Ok(Some((r, stored))) if stored == encoded => {
                        agreed = r.at + 4 + u64::from(r.len);
                        kept = Some(r);
                    }
                    _ => {
                        in_step = false;
                        receipts.stop();
                        held.ledger.receipts.cut(agreed).map_err(|e| format!("receipts: {e}"))?;
                    }
                }
            }
            if !in_step {
                kept = held.ledger.receipts.append(&encoded, false).ok();
            }

            end = span.at + 4 + u64::from(span.len);
            let id = sealed.id();
            let cost = env.block_gas_cost;
            held.settle(sealed, Link { id, cost, block: Some(span), receipts: kept });
        }
        drop(blocks);
        held.ledger.blocks.cut(end).map_err(|e| format!("blocks: {e}"))?;
        // What is left is whole and on the device: any run of history is over.
        held.ledger.blocks.sync().map_err(|e| format!("blocks: {e}"))?;
        if in_step {
            receipts.stop();
            held.ledger.receipts.cut(agreed).map_err(|e| format!("receipts: {e}"))?;
        }
        Ok(())
    }

    pub fn chain_id(&self) -> u64 {
        self.genesis.chain_id
    }

    /// How many transactions are waiting.
    pub fn pending(&self) -> usize {
        self.lock().pending.len()
    }

    /// The height of the last accepted block.
    pub fn height(&self) -> u64 {
        self.lock().chain.len() as u64 - 1
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Held> {
        // A poisoned lock means a panic left the chain half-written. Refusing
        // to read it is the only safe answer, and there is no recovery that
        // does not invent state.
        self.held.lock().expect("the chain lock is not poisoned")
    }

    /// The environment of the next block on top of `parent`, as a proposer
    /// builds it: dated by the wall clock and never before the parent — equal
    /// is a valid header, several blocks a second — and every other field
    /// derived from the parent (`lux_evm::block::Env::next`).
    fn next(&self, parent: &Sealed) -> Result<Env, Error> {
        let time = unix_now().max(parent.header().timestamp);
        self.env
            .next(parent.sealed(), parent.cost_for_child()?, time)
            .map_err(|e| Error::Invalid(e.to_string()))
    }

    /// Go's header rule (`lux_evm::block::rule`) for `sealed` on top of
    /// `parent`, by this node's clock unless `now` is none — history, which is
    /// not held to a clock again. Returns the environment it runs under. A
    /// block that claims anything its parent does not derive is refused before
    /// a sender is recovered or a transaction run; and every transaction costs
    /// its intrinsic gas, so more of them than the limit holds is refused here
    /// too.
    fn rule(&self, sealed: &Sealed, parent: &Sealed, now: Option<u64>) -> Result<Env, Error> {
        let env =
            block::rule(&self.env, parent.sealed(), parent.cost_for_child()?, sealed.sealed(), now)
                .map_err(|e| Error::Invalid(e.to_string()))?;
        let floor = (sealed.transactions().len() as u64).saturating_mul(TX_GAS);
        if floor > env.gas_limit {
            return Err(Error::Invalid(format!("more gas than the limit of {}", env.gas_limit)));
        }
        Ok(env)
    }

    /// Accept a signed transaction off the wire.
    ///
    /// Returns the hash a caller can look it up by. The signature is recovered
    /// here, which is what makes this the point of admission: a transaction
    /// whose sender cannot be derived never reaches the mempool.
    pub fn submit(&self, raw: &[u8]) -> Result<B256, Error> {
        let tx = TxEnvelope::decode_2718(&mut &raw[..])
            .map_err(|e| Error::Malformed(format!("not a transaction: {e}")))?;
        if let Some(id) = tx.chain_id() {
            if id != self.genesis.chain_id {
                return Err(Error::Invalid(format!(
                    "chain id {id}, this chain is {}",
                    self.genesis.chain_id
                )));
            }
        }
        let sender = tx
            .recover_signer()
            .map_err(|e| Error::Invalid(format!("signature does not recover: {e}")))?;
        let hash = *tx.tx_hash();

        // Held to the world after this sender's transactions already in a
        // block here — `Held::ahead`: a nonce that world has used, or a cost
        // its sender cannot pay, is a transaction no block can carry, and the
        // pool is not a place to keep those. The cost is everything the sender
        // has waiting here and this one — less the one this replaces, if it
        // replaces one — so a sender cannot commit its slots to more than its
        // balance, one affordable transaction at a time.
        let mut held = self.lock();
        if held.pending.get(&hash).is_some() {
            return Ok(hash);
        }
        let world = held.ahead(&sender);
        let nonce = world.nonce(&sender);
        let p = Pending { tx, sender };
        let owed = held.pending.committed(&sender, p.tx.nonce()).saturating_add(p.cost());
        if world.balance(&sender) < owed {
            return Err(Error::Invalid(
                "insufficient funds for gas * price + value, with what this sender has waiting"
                    .into(),
            ));
        }
        held.pending.admit(p, nonce)?;
        Ok(hash)
    }

    /// The accepted block at `height`. With `receipts`, the receipts come too.
    ///
    /// One held in memory is copied out under the lock. One that is only on
    /// disk is FOUND under the lock and read after it: an accepted block's
    /// record never moves and never changes, so reading it needs nothing the
    /// lock guards, and a slow read holds up its caller and not the chain.
    fn load(&self, height: u64, receipts: bool) -> Result<Option<Sealed>, Error> {
        let (link, files) = {
            let held = self.lock();
            let Some(link) = held.chain.get(height as usize).copied() else {
                return Ok(None);
            };
            if let Some(b) = held.blocks.get(&link.id) {
                return Ok(Some(b.clone()));
            }
            if link.block.is_none() {
                return Ok(Some(self.first.clone()));
            }
            let Some(files) = &self.files else {
                // A chain that keeps nothing on disk keeps its records here.
                let read = |s| held.ledger.blocks.read(s);
                let receipts_of = |s| held.ledger.receipts.read(s);
                return rebuild(&link, height, receipts, read, receipts_of).map(|(b, _)| Some(b));
            };
            (link, files)
        };
        rebuild(&link, height, receipts, |s| files.block(s), |s| files.receipts(s))
            .map(|(b, _)| Some(b))
    }

    /// A block by id: one being decided, or any accepted one.
    fn load_id(&self, id: &Id, receipts: bool) -> Result<Option<Sealed>, Error> {
        let height = {
            let held = self.lock();
            if let Some(b) = held.blocks.get(id) {
                return Ok(Some(b.clone()));
            }
            match held.heights.get(id) {
                Some(h) => *h,
                None => return Ok(None),
            }
        };
        self.load(height, receipts)
    }

    /// A block named the way RPC names one: a number, a tag, or a hash.
    fn named(&self, v: Option<&Value>, receipts: bool) -> Result<Option<Sealed>, Error> {
        // A hash is 32 bytes and a height is at most 8, so the length alone
        // says which a string is.
        let hash = match v {
            Some(s @ Value::String(text)) if text.len() == 66 => Some(parse_hash(s)?),
            Some(Value::Object(o)) => o.get("blockHash").map(parse_hash).transpose()?,
            _ => None,
        };
        if let Some(h) = hash {
            return self.load_id(&h.0, receipts);
        }
        let v = match v {
            Some(Value::Object(o)) => o.get("blockNumber"),
            other => other,
        };
        self.load(tag_of(v)?.resolve(self.height()), receipts)
    }

    /// Run `sealed` on the world its parent leaves behind, and hold what that
    /// produced against what the block claims. Returns the block with the
    /// receipts it produced, and the world after it.
    fn execute(
        &self,
        held: &Held,
        sealed: Sealed,
        senders: Vec<(TxEnvelope, Address)>,
        env: &Env,
    ) -> Result<(Sealed, World), Error> {
        let parent_id = sealed.parent();
        let parent = held
            .blocks
            .get(&parent_id)
            .ok_or_else(|| Error::Invalid("parent is not held".into()))?
            .sealed()
            .clone();

        // The world this block claims to start from.
        let mut world = if parent_id == held.accepted {
            held.world.clone()
        } else {
            held.verified
                .get(&parent_id)
                .cloned()
                .ok_or_else(|| Error::Invalid("no state for the parent".into()))?
        };

        let ran = block::execute(&mut world, &parent, env, senders)
            .map_err(|e| Error::Invalid(e.to_string()))?;
        check(&sealed, &ran)?;

        // Keep the receipts execution produced, so the block can be served.
        let mut whole = sealed;
        whole.built.receipts = ran.receipts;
        whole.cost = Some(env.block_gas_cost);
        Ok((whole, world))
    }

    /// Accept `id`: its record in the ledger, then the tip moved to it. With
    /// `wait` the record is on the device first — see [`Vm::accept`]; without,
    /// it is history, and on the device at the next [`Vm::sync`].
    fn commit(&self, id: &Id, wait: bool) -> Result<(), Error> {
        let mut held = self.lock();
        if held.heights.contains_key(id) {
            return Ok(()); // idempotent: catch-up can re-deliver an accepted block
        }
        // Everything that can refuse, refuses BEFORE anything is taken. An
        // accept that fails must leave the chain exactly as it was — a refusal
        // that consumed the block's state would make the retry impossible.
        let sealed = held.blocks.get(id).cloned().ok_or(Error::NotFound)?;
        if sealed.parent() != held.accepted {
            return Err(Error::Invalid("accepting out of order".into()));
        }
        if !held.verified.contains_key(id) {
            return Err(Error::Invalid("accepting a block that was never verified".into()));
        }

        // THE LEDGER FIRST. The record is on the device before anything in
        // memory moves, so there is no instant at which this block is accepted
        // and a restart would not find it. A crash after the write and before
        // the lines below leaves a block on disk that memory never saw
        // accepted; it was certified, and the restart accepts it. History is
        // the one exception: nothing outside this process sees it before the
        // sync that puts it on the device, and the ledger marks the run so a
        // crash before that sync loses it whole.
        let block = held
            .ledger
            .blocks
            .append(&sealed.raw, wait)
            .map_err(|e| Error::Invalid(format!("the block did not reach the disk: {e}")))?;
        // Past this point the block IS accepted, so nothing below may fail.
        // The receipts are derived; a write that does not land costs this
        // block's receipts until the next start rebuilds them, not the block.
        let receipts = held.ledger.receipts.append(&encode_receipts(sealed.receipts()), false).ok();

        held.world = held.verified.remove(id).expect("just checked");
        let cost = sealed.cost.expect("a verified block's cost is derived");
        held.settle(sealed, Link { id: *id, cost, block: Some(block), receipts });
        Ok(())
    }

    /// The nonce a sender's next transaction should carry: the one it is
    /// admitted against, advanced past every transaction of theirs waiting
    /// here in unbroken order. A gap stops the count, because nothing after a
    /// gap can run until it is filled.
    fn next_nonce(held: &Held, who: &Address) -> u64 {
        held.pending.next(who, held.ahead(who).nonce(who))
    }
}

/// Every sender in a block: from `known` where it names one, recovered where
/// it does not. A block whose signatures do not recover is not a block, and
/// this is where that is discovered.
///
/// A recovery is the dearest part of verifying a block — a curve operation per
/// transaction — and one recovery does not depend on another, so what has to
/// be recovered is recovered on several threads at once. The caller holds no
/// lock while this runs.
fn senders(
    sealed: &Sealed,
    known: &[Option<Address>],
) -> Result<Vec<(TxEnvelope, Address)>, Error> {
    let txs = sealed.transactions();
    let recover = |tx: &TxEnvelope| {
        tx.recover_signer().map_err(|e| Error::Invalid(format!("signature does not recover: {e}")))
    };
    let missing: Vec<usize> =
        (0..txs.len()).filter(|i| known.get(*i).copied().flatten().is_none()).collect();
    let run = |chunk: &[usize]| chunk.iter().map(|i| (*i, recover(&txs[*i]))).collect::<Vec<_>>();
    // A handful is not worth a thread: those are recovered here.
    let found = if missing.len() <= RECOVER_SHARE {
        run(&missing)
    } else {
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 8);
        let share = missing.len().div_ceil(threads).max(RECOVER_SHARE);
        std::thread::scope(|s| {
            let work: Vec<_> =
                missing.chunks(share).map(|chunk| s.spawn(move || run(chunk))).collect();
            work.into_iter().flat_map(|w| w.join().expect("a recovery")).collect()
        })
    };
    let mut who: Vec<Option<Address>> =
        (0..txs.len()).map(|i| known.get(i).copied().flatten()).collect();
    for (i, r) in found {
        who[i] = Some(r?);
    }
    Ok(txs
        .iter()
        .cloned()
        .zip(who.into_iter().map(|w| w.expect("every sender, known or recovered")))
        .collect())
}

/// A block's claims, against what running it produced — what Go's
/// `ValidateState` and `ValidateBody` hold it to. Every one of these is a way
/// two implementations can disagree, and each is checked separately so a
/// disagreement says which. Every other header field is the rule's, held to
/// what the parent derives before anything ran (`Evm::rule`).
fn check(sealed: &Sealed, ran: &Built) -> Result<(), Error> {
    let claimed = sealed.header();
    let got = ran.fields();
    if claimed.state_root != got.state_root {
        return Err(Error::Invalid(format!(
            "state root {} claimed, {} computed",
            claimed.state_root, got.state_root
        )));
    }
    if claimed.receipts_root != got.receipts_root {
        return Err(Error::Invalid("receipts root".into()));
    }
    if claimed.transactions_root != got.transactions_root {
        return Err(Error::Invalid("transactions root".into()));
    }
    if claimed.gas_used != got.gas_used {
        return Err(Error::Invalid(format!(
            "gas used {} claimed, {} computed",
            claimed.gas_used, got.gas_used
        )));
    }
    if claimed.logs_bloom != got.logs_bloom {
        return Err(Error::Invalid("logs bloom".into()));
    }
    Ok(())
}

/// An accepted block read back through `blocks` and `receipts_of`, each of
/// which reads one record of its file by span; and how many bytes that took.
fn rebuild(
    link: &Link,
    height: u64,
    receipts: bool,
    blocks: impl Fn(Span) -> std::io::Result<Vec<u8>>,
    receipts_of: impl Fn(Span) -> std::io::Result<Vec<u8>>,
) -> Result<(Sealed, u64), Error> {
    // A read that fails says so without saying why: the reason is this node's
    // disk, and the caller is anyone with a socket.
    let unreadable = || Error::Invalid(format!("block {height} could not be read back"));
    let span = link.block.ok_or_else(unreadable)?;
    let raw = blocks(span).map_err(|_| unreadable())?;
    let mut read = raw.len() as u64;
    let mut sealed = Sealed::read(raw).map_err(|_| unreadable())?;
    if sealed.id != link.id {
        return Err(unreadable());
    }
    sealed.cost = Some(link.cost);
    if receipts && !sealed.transactions().is_empty() {
        let span = link.receipts.ok_or_else(unreadable)?;
        let raw = receipts_of(span).map_err(|_| unreadable())?;
        read += raw.len() as u64;
        sealed.built.receipts =
            Vec::<ReceiptEnvelope>::decode(&mut &raw[..]).map_err(|_| unreadable())?;
    }
    Ok((sealed, read))
}

/// The header of the block recorded at `span`, without the body behind it,
/// and how many bytes reading it took.
///
/// A block is an RLP list whose first item is its header, so the two list
/// prefixes at the front of the record say where the header ends.
fn head(files: &Files, span: Span, height: u64) -> Result<(header::Lux, u64), Error> {
    let unreadable = || Error::Invalid(format!("block {height} could not be read back"));
    // Two prefixes of at most nine bytes each.
    let n = 18.min(span.len as usize);
    let front = files.block_part(span, 0, n).map_err(|_| unreadable())?;
    let (outer, _) = list(&front).ok_or_else(unreadable)?;
    let (inner, body) = list(front.get(outer..).ok_or_else(unreadable)?).ok_or_else(unreadable)?;
    // A length is bytes off the disk. No header is larger than a block, and
    // `block_part` holds the rest to the record and the file, before anything
    // is allocated for it.
    let size = inner.checked_add(body).filter(|s| *s <= BLOCK_MAX).ok_or_else(unreadable)?;
    let raw = files.block_part(span, outer as u64, size).map_err(|_| unreadable())?;
    let lux = header::Lux::decode(&mut &raw[..]).map_err(|_| unreadable())?;
    Ok((lux, (n + size) as u64))
}

/// An RLP list's prefix length and payload length, read off its first bytes;
/// nothing if they are not the front of a list.
fn list(front: &[u8]) -> Option<(usize, usize)> {
    let tag = *front.first()?;
    match tag {
        0xc0..=0xf7 => Some((1, usize::from(tag - 0xc0))),
        0xf8..=0xff => {
            let n = usize::from(tag - 0xf7);
            let mut len = 0usize;
            for byte in front.get(1..1 + n)? {
                len = len.checked_mul(256)?.checked_add(usize::from(*byte))?;
            }
            Some((1 + n, len))
        }
        _ => None,
    }
}

/// Turns at something only a few may do at once.
struct Turns {
    taken: Mutex<usize>,
    freed: Condvar,
    most: usize,
}

/// One turn, given back when it is dropped.
struct Turn<'a>(&'a Turns);

impl Turns {
    fn new(most: usize) -> Self {
        Self { taken: Mutex::new(0), freed: Condvar::new(), most }
    }

    /// A turn, waiting up to `wait` for one to come free; none if none did.
    fn take(&self, wait: Duration) -> Option<Turn<'_>> {
        let taken = self.taken.lock().expect("the turns lock is not poisoned");
        let (mut taken, _) = self
            .freed
            .wait_timeout_while(taken, wait, |t| *t >= self.most)
            .expect("the turns lock is not poisoned");
        if *taken >= self.most {
            return None;
        }
        *taken += 1;
        Some(Turn(self))
    }
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        *self.0.taken.lock().expect("the turns lock is not poisoned") -= 1;
        self.0.freed.notify_one();
    }
}

impl Vm for Evm {
    fn name(&self) -> &'static str {
        "C"
    }

    fn version(&self) -> String {
        VERSION.to_string()
    }

    fn build(&self) -> Result<Box<dyn Block>, Error> {
        let mut held = self.lock();
        // Go refuses an empty block on the build path too: a chain that
        // produces blocks with nothing in them is a chain that produces
        // blocks forever. Nothing to run is nothing: a transaction whose
        // nonce is not yet its sender's next waits, and costs no build.
        let now = held.preferred_world();
        if !held.pending.iter().any(|p| p.tx.nonce() == now.nonce(&p.sender)) {
            return Err(Error::Empty);
        }

        let tip = held.blocks.get(&held.preferred).ok_or(Error::NotFound)?;
        let env = self.next(tip)?;
        let parent = tip.sealed().clone();

        // Build against a copy of the world THIS BLOCK'S PARENT leaves behind —
        // the preferred tip, which is not always the accepted one. Building on
        // the accepted world while preferring a later block produces a block
        // whose transactions were checked against the wrong nonces.
        //
        // A copy, because nothing here is canonical until the block is
        // accepted, and a proposer that mutated the accepted state would be
        // deciding finality by building.
        let mut world = if held.preferred == held.accepted {
            held.world.clone()
        } else {
            held.verified
                .get(&held.preferred)
                .cloned()
                .ok_or_else(|| Error::Invalid("no state for the preferred block".into()))?
        };
        block::begin(&mut world, &env, &parent).map_err(|e| Error::Invalid(e.to_string()))?;
        let mut kept = Vec::new();
        let mut spent = Vec::new();
        let mut receipts = Vec::new();
        let mut gas = 0u64;
        let mut taken = Vec::new();
        let mut dropped = Vec::new();

        for (i, p) in held.pending.iter().enumerate() {
            // Behind a gap: it waits for the nonce below it, and keeps its
            // place.
            if p.tx.nonce() > world.nonce(&p.sender) {
                continue;
            }
            match block::apply(&mut world, &env, gas, &p.tx, p.sender) {
                Ok(done) => {
                    gas += done.gas_used;
                    receipts.push(done.receipt);
                    kept.push(p.tx.clone());
                    spent.push(p.clone());
                    taken.push(i);
                }
                // The permissive composition: a proposer drops what it cannot
                // include and carries on. A verifier does not, which is why
                // this loop is here and not in `lux-evm`.
                Err(Invalid::GasLimit { .. }) => break,
                Err(_) => {
                    taken.push(i);
                    dropped.push(i);
                }
            }
        }
        if kept.is_empty() {
            // Everything that could run was unusable. Drop it rather than
            // retrying it on every build.
            held.pending.take(&taken);
            return Err(Error::Empty);
        }

        // Go builds no block whose tips fall short of its block gas cost
        // (`FinalizeAndAssemble`). What ran stays in the pool — it is valid,
        // and the cost falls as the parent ages — and only what could not run
        // is let go.
        let built = match block::finish(&world, &parent, &env, kept, receipts, gas) {
            Ok(b) => b,
            Err(e) => {
                held.pending.take(&dropped);
                return Err(Error::Invalid(format!("not yet: {e}")));
            }
        };
        let sealed = Sealed::new(built, Some(env.block_gas_cost));
        let id = sealed.id();

        held.pending.take(&taken);
        // Executed before it is held, so it is never the first let go.
        held.verified.insert(id, world);
        held.hold(sealed.clone());
        held.mine.insert(id, spent);
        Ok(Box::new(sealed))
    }

    fn parse(&self, raw: &[u8]) -> Result<Box<dyn Block>, Error> {
        // Receipts are not on the wire — they are what execution produces, and
        // a parsed block has not been executed yet. `verify` fills them in.
        let sealed = Sealed::read(raw.to_vec())
            .map_err(|e| Error::Malformed(format!("not a block: {e}")))?;

        // Hold it, so `verify` and `accept` can find it by id. Go keeps a
        // caching wrapper around the VM for exactly this; without it a parsed
        // block would have to be carried by the caller and handed back, which
        // is the same cache with more places to lose it.
        //
        // A block already held is NOT replaced: one that has been executed
        // carries its receipts, and overwriting it with a freshly parsed copy
        // would throw them away. Only a block above the tip is held at all:
        // one at a decided height can never be accepted, and catch-up
        // re-delivering old blocks must not refill memory. And what is held
        // is bounded — which blocks arrive is a peer's choice.
        let mut held = self.lock();
        if sealed.height() >= held.chain.len() as u64 {
            held.hold(sealed.clone());
        }
        Ok(Box::new(sealed))
    }

    fn get(&self, id: &Id) -> Result<Box<dyn Block>, Error> {
        self.load_id(id, false)?.map(|b| Box::new(b) as Box<dyn Block>).ok_or(Error::NotFound)
    }

    /// Recovering the senders is done with the chain let go: the block and
    /// the senders the pool already knows are read under the lock, the rest
    /// are recovered — on several threads — after it, and only execution
    /// holds the chain.
    fn verify(&self, id: &Id) -> Result<(), Error> {
        // Already executed, or already accepted. Go's engine calls verify more
        // than once for one block, so this has to be idempotent.
        let done = |held: &Held| held.verified.contains_key(id) || held.heights.contains_key(id);
        let (sealed, known, env) = {
            let mut held = self.lock();
            if done(&held) {
                return Ok(());
            }
            let sealed = held.blocks.get(id).cloned().ok_or(Error::NotFound)?;
            // The header first: a block that claims its own limits costs a
            // lookup, not a signature recovered or a transaction run.
            let ruled = held
                .blocks
                .get(&sealed.parent())
                .ok_or_else(|| Error::Invalid("parent is not held".into()))
                .and_then(|parent| self.rule(&sealed, parent, Some(unix_now())));
            let env = match ruled {
                Ok(env) => env,
                Err(e) => {
                    held.forget(id);
                    return Err(e);
                }
            };
            let known: Vec<Option<Address>> =
                sealed.transactions().iter().map(|t| held.pending.sender(t.tx_hash())).collect();
            (sealed, known, env)
        };
        let recovered = senders(&sealed, &known);

        let mut held = self.lock();
        if done(&held) {
            return Ok(());
        }
        // Let go of while the lock was: there is nothing here to verify.
        if !held.blocks.contains_key(id) {
            return Err(Error::NotFound);
        }
        match recovered.and_then(|senders| self.execute(&held, sealed, senders, &env)) {
            Ok((whole, world)) => {
                held.blocks.insert(*id, whole);
                held.verified.insert(*id, world);
                Ok(())
            }
            // Not kept. A block that does not execute to what it claims would
            // otherwise sit in memory until its height is decided, and a peer
            // can send as many as it likes.
            Err(e) => {
                held.forget(id);
                Err(e)
            }
        }
    }

    fn accept(&self, id: &Id) -> Result<(), Error> {
        self.commit(id, true)
    }

    fn accept_history(&self, id: &Id) -> Result<(), Error> {
        self.commit(id, false)
    }

    fn sync(&self) -> Result<(), Error> {
        self.lock()
            .ledger
            .blocks
            .sync()
            .map_err(|e| Error::Invalid(format!("the chain did not reach the disk: {e}")))
    }

    /// An accepted block is final, and is not let go: forgetting it would
    /// take away the parent of whatever is built next. A block this node
    /// built gives its transactions back to the pool — a peer that can make
    /// this node drop its own proposal must not be able to make it drop what
    /// its users sent.
    fn reject(&self, id: &Id) -> Result<(), Error> {
        self.lock().forget(id);
        Ok(())
    }

    fn set_preference(&self, id: &Id) -> Result<(), Error> {
        let mut held = self.lock();
        if !held.blocks.contains_key(id) {
            return Err(Error::NotFound);
        }
        held.preferred = *id;
        Ok(())
    }

    fn last_accepted(&self) -> Id {
        self.lock().accepted
    }

    fn block_id_at(&self, height: u64) -> Result<Id, Error> {
        self.lock().chain.get(height as usize).map(|l| l.id).ok_or(Error::NotFound)
    }

    fn call(&self, method: &str, params: &Value) -> Result<Value, Error> {
        match method {
            // Identity. The chain id is genesis data, and this is where a
            // caller finds out which network it is talking to.
            "eth_chainId" => Ok(quantity(self.genesis.chain_id)),
            "net_version" => Ok(json!(self.genesis.chain_id.to_string())),
            "web3_clientVersion" => Ok(json!(VERSION)),
            // The base fee the next block carries, as Go's own estimate does.
            "eth_gasPrice" => {
                let held = self.lock();
                let tip = held.blocks.get(&held.preferred).ok_or(Error::NotFound)?;
                Ok(quantity(self.next(tip)?.base_fee))
            }
            "eth_syncing" => Ok(json!(false)),
            "net_listening" => Ok(json!(true)),
            "eth_accounts" => Ok(json!([])),

            "eth_blockNumber" => Ok(quantity(self.height())),

            "eth_getBalance" => {
                let who = address_at(params, 0)?;
                self.at_tip(params, 1)?;
                Ok(big(self.lock().world.balance(&who)))
            }
            "eth_getTransactionCount" => {
                let who = address_at(params, 0)?;
                // "pending" is the question a wallet asks before signing the
                // next transaction, and the answer has to count the ones it
                // already sent that are still waiting here — answering with
                // the accepted nonce hands it one it has already used.
                if tag_at(params, 1)? == Tag::Pending {
                    return Ok(quantity(Self::next_nonce(&self.lock(), &who)));
                }
                self.at_tip(params, 1)?;
                Ok(quantity(self.lock().world.nonce(&who)))
            }
            "eth_getCode" => {
                let who = address_at(params, 0)?;
                self.at_tip(params, 1)?;
                Ok(bytes(self.lock().world.code(&who).original_byte_slice()))
            }
            "eth_getStorageAt" => {
                let who = address_at(params, 0)?;
                let slot = u256_at(params, 1)?;
                self.at_tip(params, 2)?;
                let v = self.lock().world.storage(&who, slot);
                Ok(json!(format!("0x{:064x}", v)))
            }

            "eth_sendRawTransaction" => {
                let raw = bytes_at(params, 0)?;
                let hash = self.submit(&raw)?;
                Ok(json!(format!("{hash:#x}")))
            }

            "eth_call" => {
                let out = self.simulate(params)?;
                Ok(bytes(&out.0))
            }
            "eth_estimateGas" => {
                let out = self.simulate(params)?;
                Ok(quantity(out.1))
            }

            "eth_getBlockByNumber" => {
                let at = tag_at(params, 0)?.resolve(self.height());
                Ok(match self.load(at, false)? {
                    Some(b) => block_json(&b, truthy(params, 1)),
                    None => Value::Null,
                })
            }
            "eth_getBlockByHash" => {
                let id = hash_at(params, 0)?;
                Ok(match self.load_id(&id.0, false)? {
                    Some(b) => block_json(&b, truthy(params, 1)),
                    None => Value::Null,
                })
            }
            "eth_getBlockTransactionCountByNumber" | "eth_getBlockTransactionCountByHash" => {
                Ok(match self.named(params.get(0), false)? {
                    Some(b) => quantity(b.transactions().len() as u64),
                    None => Value::Null,
                })
            }
            "eth_getTransactionByBlockNumberAndIndex" | "eth_getTransactionByBlockHashAndIndex" => {
                let i = u256_at(params, 1)?.saturating_to::<usize>();
                Ok(match self.named(params.get(0), false)? {
                    Some(b) => mined_json(&b, i),
                    None => Value::Null,
                })
            }
            "eth_getBlockReceipts" => Ok(match self.named(params.get(0), true)? {
                Some(b) => json!((0..b.transactions().len())
                    .map(|i| receipt_json(&b, i))
                    .collect::<Vec<_>>()),
                None => Value::Null,
            }),
            "eth_getTransactionReceipt" => {
                let hash = hash_at(params, 0)?;
                let Some((height, index)) = self.lock().located.get(&hash).copied() else {
                    return Ok(Value::Null);
                };
                Ok(match self.load(height, true)? {
                    Some(b) => receipt_json(&b, index as usize),
                    None => Value::Null,
                })
            }
            "eth_getTransactionByHash" => {
                let hash = hash_at(params, 0)?;
                // Where it is, or what is waiting, read at one instant; the
                // block it is in is read after.
                let at = {
                    let held = self.lock();
                    match held.located.get(&hash).copied() {
                        Some(at) => at,
                        // Still waiting here: known, and in no block yet.
                        None => {
                            return Ok(match held.pending.get(&hash) {
                                Some(p) => tx_json(&p.tx, Some(p.sender), None),
                                None => Value::Null,
                            })
                        }
                    }
                };
                Ok(match self.load(at.0, false)? {
                    Some(b) => mined_json(&b, at.1 as usize),
                    None => Value::Null,
                })
            }
            "eth_getLogs" => {
                let filter =
                    params.get(0).ok_or_else(|| Error::BadRequest("missing the filter".into()))?;
                self.logs(&Query::read(filter)?)
            }

            other => Err(Error::NoMethod(other.to_string())),
        }
    }
}

impl Evm {
    /// Refuse a question about a state this node does not keep.
    ///
    /// An archive answer would have to be invented, and an invented balance is
    /// worse than no balance.
    fn at_tip(&self, params: &Value, i: usize) -> Result<(), Error> {
        match tag_at(params, i)? {
            Tag::Latest | Tag::Pending => Ok(()),
            Tag::Earliest if self.height() == 0 => Ok(()),
            Tag::Height(n) if n == self.height() => Ok(()),
            _ => Err(Error::BadRequest(
                "this node keeps only the state at its tip; historical state is not retained"
                    .into(),
            )),
        }
    }

    /// `eth_getLogs`.
    fn logs(&self, q: &Query) -> Result<Value, Error> {
        self.logs_within(q, LOGS_READ_MAX)
    }

    /// `eth_getLogs`, reading at most `budget` bytes.
    ///
    /// A range scan takes a turn — [`LOGS_SCANS`] run at once — and finds
    /// where its blocks are under the chain lock ONCE. Every block is then
    /// read without it: an accepted block never changes, so a scan does not
    /// have to stop the chain, or take the lock the chain accepts under ten
    /// thousand times, to read one. A block whose header's bloom cannot match
    /// costs its header and no more.
    fn logs_within(&self, q: &Query, budget: u64) -> Result<Value, Error> {
        let mut out = Vec::new();
        let (from, to) = match q.within {
            Within::Hash(h) => {
                let b = self
                    .load_id(&h.0, true)?
                    .ok_or_else(|| Error::BadRequest("unknown block".into()))?;
                q.collect(&b, &mut out)?;
                return Ok(Value::Array(out));
            }
            Within::Range(from, to) => {
                // Two heights the wrong way round is a malformed question. A
                // tag is resolved first, so "from 9 to latest" on a chain at 4
                // is a range with nothing in it, as it is in Go.
                if let (Tag::Height(f), Tag::Height(t)) = (from, to) {
                    if f > t {
                        return Err(Error::BadRequest("invalid block range params".into()));
                    }
                }
                let head = self.height();
                // A range reaching past the head stops at the head: there is
                // nothing there yet to match.
                (from.resolve(head), to.resolve(head).min(head))
            }
        };
        if from > to {
            return Ok(Value::Array(out));
        }
        if to - from >= LOGS_RANGE {
            return Err(Error::BadRequest(format!(
                "a log query spans at most {LOGS_RANGE} blocks; this one spans {}",
                to - from + 1
            )));
        }
        let _turn = self.scans.take(LOGS_WAIT).ok_or_else(|| {
            Error::BadRequest(format!("{LOGS_SCANS} log queries are running; try again"))
        })?;
        let links: Vec<Link> = {
            let held = self.lock();
            held.chain.iter().skip(from as usize).take((to - from + 1) as usize).copied().collect()
        };

        let mut read = 0u64;
        let mut spend = |n: u64| {
            read += n;
            if read > budget {
                return Err(Error::BadRequest(format!(
                    "a log query reads at most {budget} bytes of blocks; narrow the range"
                )));
            }
            Ok(())
        };
        for (height, link) in (from..).zip(&links) {
            let b = match (&self.files, link.block) {
                (Some(files), Some(span)) => {
                    // The header's bloom says which blocks cannot match, and
                    // those are skipped without reading anything behind it.
                    let (header, cost) = head(files, span, height)?;
                    spend(cost)?;
                    if !q.might(&header.header.logs_bloom) {
                        continue;
                    }
                    match rebuild(link, height, true, |s| files.block(s), |s| files.receipts(s)) {
                        Ok((b, cost)) => {
                            spend(cost)?;
                            b
                        }
                        // Receipts that never reached the file are still in
                        // memory for as long as the block is.
                        Err(_) => match self.load(height, true)? {
                            Some(b) => {
                                spend(b.raw.len() as u64)?;
                                b
                            }
                            None => break,
                        },
                    }
                }
                // The genesis, or a chain that keeps nothing on disk.
                _ => {
                    let Some(b) = self.load(height, true)? else {
                        break;
                    };
                    spend(b.raw.len() as u64)?;
                    if !q.might(&b.header().logs_bloom) {
                        continue;
                    }
                    b
                }
            };
            q.collect(&b, &mut out)?;
        }
        Ok(Value::Array(out))
    }

    /// `eth_call` and `eth_estimateGas`: run against a copy and keep nothing.
    fn simulate(&self, params: &Value) -> Result<(Bytes, u64), Error> {
        let call = params.get(0).ok_or_else(|| Error::BadRequest("no call object".into()))?;
        let from = match call.get("from") {
            Some(v) => parse_address(v)?,
            None => Address::ZERO,
        };
        let to = match call.get("to") {
            Some(v) => TxKind::Call(parse_address(v)?),
            None => TxKind::Create,
        };
        let data = match call.get("data").or_else(|| call.get("input")) {
            Some(v) => Bytes::from(parse_bytes(v)?),
            None => Bytes::new(),
        };
        let value = match call.get("value") {
            Some(v) => parse_u256(v)?,
            None => U256::ZERO,
        };
        let gas = match call.get("gas") {
            Some(v) => parse_u256(v)?.saturating_to::<u64>(),
            None => self.env.gas_limit,
        };

        // Go runs a call in the context of the block it names — the tip —
        // on that block's state (`NewEVMBlockContext` of its header).
        let (world, env) = {
            let held = self.lock();
            let tip = held.blocks.get(&held.accepted).ok_or(Error::NotFound)?;
            (held.world.clone(), self.env.of(tip.sealed()))
        };

        let tx = TxEnv::builder()
            .caller(from)
            .kind(to)
            .data(data)
            .value(value)
            .gas_limit(gas)
            .gas_price(env.base_fee as u128)
            .nonce(world.nonce(&from))
            // A call is not signed, so it carries no EIP-155 binding.
            .chain_id(None)
            .build_fill();

        let out = block::call(world, &env, tx).map_err(|e| Error::Invalid(e.to_string()))?;
        let gas_used = out.gas_used();
        match out {
            revm::context::result::ExecutionResult::Success { output, .. } => {
                Ok((output.into_data(), gas_used))
            }
            revm::context::result::ExecutionResult::Revert { output, .. } => {
                Err(Error::Invalid(format!("execution reverted: 0x{}", hex(&output))))
            }
            revm::context::result::ExecutionResult::Halt { reason, .. } => {
                Err(Error::Invalid(format!("halted: {reason:?}")))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// eth_getLogs, with Go's semantics: an address matches if it is any of the
// listed ones; topics are positional, a null position matches anything, a list
// at a position matches any of its members, and a null INSIDE a list makes the
// whole position match anything. A filter naming more positions than a log
// has topics does not match it, even when the extra positions are null.
// ---------------------------------------------------------------------------

/// Go's own limits on one filter (`eth/filters/filter_system.go`).
const FILTER_ADDRESSES: usize = 1000;
const FILTER_TOPICS: usize = 4;
const FILTER_ALTERNATIVES: usize = 1000;

/// Which blocks a filter reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Within {
    Range(Tag, Tag),
    Hash(B256),
}

/// One `eth_getLogs` filter.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Query {
    within: Within,
    /// Empty matches every address.
    addresses: Vec<Address>,
    /// One entry per position; an empty entry matches anything there.
    topics: Vec<Vec<B256>>,
}

impl Query {
    fn read(v: &Value) -> Result<Self, Error> {
        let o = v.as_object().ok_or_else(|| Error::BadRequest("a filter is an object".into()))?;
        let given = |k: &str| o.get(k).filter(|v| !v.is_null());

        let within = match given("blockHash") {
            Some(h) => {
                if given("fromBlock").is_some() || given("toBlock").is_some() {
                    return Err(Error::BadRequest(
                        "cannot specify both BlockHash and FromBlock/ToBlock, choose one or the other"
                            .into(),
                    ));
                }
                Within::Hash(parse_hash(h)?)
            }
            None => Within::Range(tag_of(given("fromBlock"))?, tag_of(given("toBlock"))?),
        };

        let addresses = match given("address") {
            None => Vec::new(),
            Some(Value::Array(list)) => {
                if list.len() > FILTER_ADDRESSES {
                    return Err(Error::BadRequest("exceed max addresses".into()));
                }
                list.iter().map(parse_address).collect::<Result<_, _>>()?
            }
            Some(one) => vec![parse_address(one)?],
        };

        let topics = match given("topics") {
            None => Vec::new(),
            Some(Value::Array(positions)) => {
                if positions.len() > FILTER_TOPICS {
                    return Err(Error::BadRequest("exceed max topics".into()));
                }
                let mut out = Vec::with_capacity(positions.len());
                for p in positions {
                    out.push(match p {
                        Value::Null => Vec::new(),
                        Value::String(_) => vec![parse_hash(p)?],
                        Value::Array(any) => {
                            if any.len() > FILTER_ALTERNATIVES {
                                return Err(Error::BadRequest("exceed max sub topics".into()));
                            }
                            let mut alternatives = Vec::with_capacity(any.len());
                            for a in any {
                                if a.is_null() {
                                    alternatives.clear();
                                    break;
                                }
                                alternatives.push(parse_hash(a)?);
                            }
                            alternatives
                        }
                        _ => return Err(Error::BadRequest("invalid topic(s)".into())),
                    });
                }
                out
            }
            Some(_) => return Err(Error::BadRequest("invalid topic(s)".into())),
        };

        Ok(Self { within, addresses, topics })
    }

    fn matches(&self, log: &Log) -> bool {
        if !self.addresses.is_empty() && !self.addresses.contains(&log.address) {
            return false;
        }
        let topics = log.topics();
        if self.topics.len() > topics.len() {
            return false;
        }
        self.topics.iter().zip(topics).all(|(any, t)| any.is_empty() || any.contains(t))
    }

    /// Whether a block with this bloom could hold a match.
    fn might(&self, bloom: &Bloom) -> bool {
        let seen = |b: &[u8]| bloom.contains_input(BloomInput::Raw(b));
        (self.addresses.is_empty() || self.addresses.iter().any(|a| seen(a.as_slice())))
            && self
                .topics
                .iter()
                .all(|any| any.is_empty() || any.iter().any(|t| seen(t.as_slice())))
    }

    /// Every matching log in `b`, onto `out`.
    fn collect(&self, b: &Sealed, out: &mut Vec<Value>) -> Result<(), Error> {
        let mut index = 0usize;
        for (i, r) in b.receipts().iter().enumerate() {
            for log in logs_of(r) {
                if self.matches(log) {
                    if out.len() == LOGS_MAX {
                        return Err(Error::BadRequest(format!(
                            "query returned more than {LOGS_MAX} results"
                        )));
                    }
                    out.push(log_json(b, i, index, log));
                }
                index += 1;
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// The JSON-RPC shapes. A quantity is minimal hex; data is byte-width hex.
// Getting these two confused is the classic way to mis-serve a header.
// ---------------------------------------------------------------------------

fn quantity(n: u64) -> Value {
    json!(format!("0x{n:x}"))
}

fn wide(n: u128) -> Value {
    json!(format!("0x{n:x}"))
}

fn big(v: U256) -> Value {
    json!(format!("0x{v:x}"))
}

fn hex(b: &[u8]) -> String {
    alloy_primitives::hex::encode(b)
}

fn bytes(b: &[u8]) -> Value {
    json!(format!("0x{}", hex(b)))
}

/// Which block a question is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tag {
    Latest,
    Earliest,
    Pending,
    Height(u64),
}

impl Tag {
    /// The height this names, on a chain whose head is `head`.
    fn resolve(self, head: u64) -> u64 {
        match self {
            Tag::Latest | Tag::Pending => head,
            Tag::Earliest => 0,
            Tag::Height(n) => n,
        }
    }
}

fn tag_at(params: &Value, i: usize) -> Result<Tag, Error> {
    tag_of(params.get(i))
}

fn tag_of(v: Option<&Value>) -> Result<Tag, Error> {
    let Some(v) = v else {
        return Ok(Tag::Latest);
    };
    match v.as_str() {
        // An accepted block is final here, so the safe and finalized heads are
        // the latest one.
        None | Some("latest") | Some("safe") | Some("finalized") => Ok(Tag::Latest),
        Some("earliest") => Ok(Tag::Earliest),
        Some("pending") => Ok(Tag::Pending),
        Some(s) => {
            let raw = s.strip_prefix("0x").unwrap_or(s);
            u64::from_str_radix(raw, 16)
                .map(Tag::Height)
                .map_err(|_| Error::BadRequest(format!("{s} is not a block")))
        }
    }
}

fn truthy(params: &Value, i: usize) -> bool {
    params.get(i).and_then(|v| v.as_bool()).unwrap_or(false)
}

fn address_at(params: &Value, i: usize) -> Result<Address, Error> {
    parse_address(params.get(i).ok_or_else(|| Error::BadRequest("missing an address".into()))?)
}

fn parse_address(v: &Value) -> Result<Address, Error> {
    let s = v.as_str().ok_or_else(|| Error::BadRequest("an address is a string".into()))?;
    s.parse::<Address>().map_err(|e| Error::BadRequest(format!("{s} is not an address: {e}")))
}

fn hash_at(params: &Value, i: usize) -> Result<B256, Error> {
    parse_hash(params.get(i).ok_or_else(|| Error::BadRequest("missing a hash".into()))?)
}

fn parse_hash(v: &Value) -> Result<B256, Error> {
    let s = v.as_str().ok_or_else(|| Error::BadRequest("a hash is a string".into()))?;
    s.parse::<B256>().map_err(|e| Error::BadRequest(format!("{s} is not a hash: {e}")))
}

fn u256_at(params: &Value, i: usize) -> Result<U256, Error> {
    parse_u256(params.get(i).ok_or_else(|| Error::BadRequest("missing a quantity".into()))?)
}

fn parse_u256(v: &Value) -> Result<U256, Error> {
    let s = v.as_str().ok_or_else(|| Error::BadRequest("a quantity is a string".into()))?;
    let raw = s.strip_prefix("0x").unwrap_or(s);
    U256::from_str_radix(raw, 16).map_err(|e| Error::BadRequest(format!("{s}: {e}")))
}

fn bytes_at(params: &Value, i: usize) -> Result<Vec<u8>, Error> {
    parse_bytes(params.get(i).ok_or_else(|| Error::BadRequest("missing data".into()))?)
}

fn parse_bytes(v: &Value) -> Result<Vec<u8>, Error> {
    let s = v.as_str().ok_or_else(|| Error::BadRequest("data is a string".into()))?;
    alloy_primitives::hex::decode(s).map_err(|e| Error::BadRequest(format!("not hex: {e}")))
}

/// A block in the shape `eth_getBlockByNumber` answers with.
fn block_json(b: &Sealed, full: bool) -> Value {
    let h = b.header();
    let name = b.sealed().seal();
    let txs: Vec<Value> = if full {
        (0..b.transactions().len()).map(|i| mined_json(b, i)).collect()
    } else {
        b.transactions().iter().map(|t| json!(format!("{:#x}", t.tx_hash()))).collect()
    };
    json!({
        "number": quantity(h.number),
        "hash": format!("{:#x}", name),
        "parentHash": format!("{:#x}", h.parent_hash),
        "nonce": format!("{:#x}", h.nonce),
        "sha3Uncles": format!("{:#x}", h.ommers_hash),
        "logsBloom": format!("0x{}", hex(h.logs_bloom.as_slice())),
        "transactionsRoot": format!("{:#x}", h.transactions_root),
        "stateRoot": format!("{:#x}", h.state_root),
        "receiptsRoot": format!("{:#x}", h.receipts_root),
        "miner": format!("{:#x}", h.beneficiary),
        "difficulty": big(h.difficulty),
        "totalDifficulty": "0x0",
        "extraData": format!("0x{}", hex(&h.extra_data)),
        "size": quantity(b.raw.len() as u64),
        "gasLimit": quantity(h.gas_limit),
        "gasUsed": quantity(h.gas_used),
        "timestamp": quantity(h.timestamp),
        "baseFeePerGas": h.base_fee_per_gas.map(quantity),
        "mixHash": format!("{:#x}", h.mix_hash),
        // What Go serves: the cost verification wrote, the header's, zero
        // where it carries none. Block zero was never verified.
        // What Go serves: the cost it derived, which a header built here
        // carries when it is not zero. Block zero has none.
        "blockGasCost": (h.number > 0).then(|| quantity(b.cost.unwrap_or_default())),
        "transactions": txs,
        "uncles": [],
    })
}

/// Transaction `i` of an accepted block.
fn mined_json(b: &Sealed, i: usize) -> Value {
    match b.transactions().get(i) {
        Some(t) => tx_json(t, t.recover_signer().ok(), Some((b, i))),
        None => Value::Null,
    }
}

/// A transaction in the shape `eth_getTransactionByHash` answers with. `at` is
/// the block it landed in and its index there, or `None` while it waits.
///
/// `gasPrice` of a mined fee-market transaction is what it PAID — the block's
/// base fee plus the tip it could afford — and of a waiting one is its cap,
/// which is what Go reports for each.
fn tx_json(t: &TxEnvelope, from: Option<Address>, at: Option<(&Sealed, usize)>) -> Value {
    let paid = match at {
        Some((b, _)) => t.effective_gas_price(b.header().base_fee_per_gas),
        None => t.max_fee_per_gas(),
    };
    let signature = t.signature();
    let mut o = Map::new();
    o.insert("hash".into(), json!(format!("{:#x}", t.tx_hash())));
    o.insert("from".into(), json!(from.map(|a| format!("{a:#x}"))));
    o.insert("to".into(), json!(t.to().map(|a| format!("{a:#x}"))));
    o.insert("value".into(), big(t.value()));
    o.insert("gas".into(), quantity(t.gas_limit()));
    o.insert("gasPrice".into(), wide(paid));
    o.insert("nonce".into(), quantity(t.nonce()));
    o.insert("input".into(), json!(format!("0x{}", hex(t.input()))));
    o.insert("type".into(), quantity(t.ty() as u64));
    o.insert("chainId".into(), json!(t.chain_id().map(quantity)));
    o.insert("r".into(), big(signature.r()));
    o.insert("s".into(), big(signature.s()));
    if t.is_legacy() {
        let v = alloy_consensus::transaction::to_eip155_value(signature.v(), t.chain_id());
        o.insert("v".into(), wide(v));
    } else {
        let parity = quantity(signature.v() as u64);
        o.insert("v".into(), parity.clone());
        o.insert("yParity".into(), parity);
        let list = t.access_list().map(|l| l.0.as_slice()).unwrap_or_default();
        o.insert(
            "accessList".into(),
            json!(list
                .iter()
                .map(|item| json!({
                    "address": format!("{:#x}", item.address),
                    "storageKeys": item.storage_keys.iter().map(|k| format!("{k:#x}")).collect::<Vec<_>>(),
                }))
                .collect::<Vec<_>>()),
        );
    }
    if t.is_dynamic_fee() {
        o.insert("maxFeePerGas".into(), wide(t.max_fee_per_gas()));
        o.insert("maxPriorityFeePerGas".into(), json!(t.max_priority_fee_per_gas().map(wide)));
    }
    match at {
        Some((b, i)) => {
            o.insert("blockHash".into(), json!(format!("{:#x}", b.sealed().seal())));
            o.insert("blockNumber".into(), quantity(b.header().number));
            o.insert("transactionIndex".into(), quantity(i as u64));
        }
        None => {
            o.insert("blockHash".into(), Value::Null);
            o.insert("blockNumber".into(), Value::Null);
            o.insert("transactionIndex".into(), Value::Null);
        }
    }
    Value::Object(o)
}

/// The receipt inside an envelope, whatever its type.
fn inner(r: &ReceiptEnvelope) -> &alloy_consensus::ReceiptWithBloom {
    match r {
        ReceiptEnvelope::Legacy(x)
        | ReceiptEnvelope::Eip2930(x)
        | ReceiptEnvelope::Eip1559(x)
        | ReceiptEnvelope::Eip4844(x)
        | ReceiptEnvelope::Eip7702(x) => x,
    }
}

fn logs_of(r: &ReceiptEnvelope) -> &[Log] {
    &inner(r).receipt.logs
}

/// One log, where it is: its block (and that block's time, as Go reports it),
/// its transaction, and its index in the BLOCK — counted across every receipt
/// before it, which is what a client pages through a block's logs by.
fn log_json(b: &Sealed, tx: usize, index: usize, l: &Log) -> Value {
    json!({
        "address": format!("{:#x}", l.address),
        "topics": l.topics().iter().map(|t| format!("{t:#x}")).collect::<Vec<_>>(),
        "data": format!("0x{}", hex(&l.data.data)),
        "blockNumber": quantity(b.header().number),
        "blockHash": format!("{:#x}", b.sealed().seal()),
        "blockTimestamp": quantity(b.header().timestamp),
        "transactionHash": b.transactions().get(tx).map(|t| format!("{:#x}", t.tx_hash())),
        "transactionIndex": quantity(tx as u64),
        "logIndex": quantity(index as u64),
        "removed": false,
    })
}

fn receipt_json(b: &Sealed, i: usize) -> Value {
    let (Some(t), Some(r)) = (b.transactions().get(i), b.receipts().get(i)) else {
        return Value::Null;
    };
    let h = b.header();
    let name = b.sealed().seal();
    let r = inner(r);
    let cumulative = r.receipt.cumulative_gas_used;
    // A receipt states the running total; this transaction's own share is the
    // difference from the one before it.
    let previous = match i.checked_sub(1).and_then(|p| b.receipts().get(p)) {
        Some(p) => inner(p).receipt.cumulative_gas_used,
        None => 0,
    };
    // Logs are numbered across the block, so this receipt's first log is
    // numbered after every log of every receipt before it.
    let first: usize = b.receipts()[..i].iter().map(|p| logs_of(p).len()).sum();
    let sender = t.recover_signer().ok();
    // A creation's address is its sender's and nonce's, whether or not the
    // creation succeeded — Go reports it either way.
    let created = match (t.kind(), sender) {
        (TxKind::Create, Some(from)) => Some(from.create(t.nonce())),
        _ => None,
    };
    json!({
        "transactionHash": format!("{:#x}", t.tx_hash()),
        "transactionIndex": quantity(i as u64),
        "blockHash": format!("{:#x}", name),
        "blockNumber": quantity(h.number),
        "from": sender.map(|a| format!("{a:#x}")),
        "to": t.to().map(|a| format!("{a:#x}")),
        "cumulativeGasUsed": quantity(cumulative),
        "gasUsed": quantity(cumulative - previous),
        "effectiveGasPrice": wide(t.effective_gas_price(h.base_fee_per_gas)),
        "contractAddress": created.map(|a| format!("{a:#x}")),
        "logs": r.receipt.logs.iter().enumerate()
            .map(|(k, l)| log_json(b, i, first + k, l))
            .collect::<Vec<_>>(),
        "logsBloom": format!("0x{}", hex(r.logs_bloom.as_slice())),
        "status": if r.receipt.status.coerce_status() { "0x1" } else { "0x0" },
        "type": quantity(t.ty() as u64),
    })
}

/// Signing a transaction, for the tests in this crate.
///
/// One signer, used by this module's tests and by [`crate::plugin`]'s — a
/// second copy would be a second thing to keep in step with the encoding.
#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;
    use alloy_consensus::{SignableTransaction, TxEip1559, TxLegacy};
    use alloy_eips::eip2718::Encodable2718 as _;
    use alloy_primitives::{keccak256, Signature};
    use k256::ecdsa::SigningKey;

    /// A schedule under which neither the base fee nor the block gas cost
    /// moves: the localnet genesis's own 12M gas limit, a target no test
    /// reaches, and no step. The chains these tests build take several blocks
    /// a second, and they are about agreement, not fees; the fee tests run
    /// Go's default schedule.
    pub const UNMETERED: lux_evm::fee::Config = lux_evm::fee::Config {
        gas_limit: 12_000_000,
        target_gas: 1_000_000_000_000,
        block_gas_cost_step: 0,
        ..lux_evm::fee::Config::DEFAULT
    };

    pub fn key(b: u8) -> SigningKey {
        SigningKey::from_bytes(&[b; 32].into()).expect("key")
    }

    /// The address a key byte controls.
    pub fn who(b: u8) -> Address {
        let vk = key(b).verifying_key().to_encoded_point(false);
        Address::from_slice(&keccak256(&vk.as_bytes()[1..])[12..])
    }

    /// A signed transfer from `who(7)`, in its 2718 encoding.
    pub fn transfer(nonce: u64, chain_id: u64) -> Vec<u8> {
        raw_transfer(nonce, chain_id, who(9), U256::from(1_000u64))
    }

    pub fn raw_transfer(nonce: u64, chain_id: u64, to: Address, value: U256) -> Vec<u8> {
        sign(TxEip1559 {
            chain_id,
            nonce,
            gas_limit: 100_000,
            max_fee_per_gas: 30_000_000_000,
            max_priority_fee_per_gas: 1_000_000_000,
            to: TxKind::Call(to),
            value,
            ..Default::default()
        })
    }

    /// A signed transfer from `who(from)`, paying `cap` and `tip` per gas.
    pub fn raw_paying(from: u8, nonce: u64, chain_id: u64, cap: u128, tip: u128) -> Vec<u8> {
        let tx = TxEip1559 {
            chain_id,
            nonce,
            gas_limit: 21_000,
            max_fee_per_gas: cap,
            max_priority_fee_per_gas: tip,
            to: TxKind::Call(who(0xee)),
            value: U256::from(1u64),
            ..Default::default()
        };
        let (sig, recid) =
            key(from).sign_prehash_recoverable(tx.signature_hash().as_slice()).expect("sign");
        let signature = Signature::new(
            U256::from_be_slice(&sig.r().to_bytes()),
            U256::from_be_slice(&sig.s().to_bytes()),
            recid.is_y_odd(),
        );
        TxEnvelope::Eip1559(tx.into_signed(signature)).encoded_2718()
    }

    /// A signed deployment from `who(7)` running `code` as its constructor.
    pub fn raw_create(nonce: u64, chain_id: u64, code: Vec<u8>) -> Vec<u8> {
        sign(TxEip1559 {
            chain_id,
            nonce,
            gas_limit: 300_000,
            max_fee_per_gas: 30_000_000_000,
            max_priority_fee_per_gas: 1_000_000_000,
            to: TxKind::Create,
            input: code.into(),
            ..Default::default()
        })
    }

    /// A signed pre-fee-market transfer from `who(7)`, EIP-155 bound.
    pub fn raw_legacy(nonce: u64, chain_id: u64, to: Address, gas_price: u128) -> Vec<u8> {
        let tx = TxLegacy {
            chain_id: Some(chain_id),
            nonce,
            gas_price,
            gas_limit: 21_000,
            to: TxKind::Call(to),
            value: U256::from(1u64),
            ..Default::default()
        };
        let signature = signature(tx.signature_hash());
        TxEnvelope::Legacy(tx.into_signed(signature)).encoded_2718()
    }

    fn sign(tx: TxEip1559) -> Vec<u8> {
        let signature = signature(tx.signature_hash());
        TxEnvelope::Eip1559(tx.into_signed(signature)).encoded_2718()
    }

    fn signature(hash: B256) -> Signature {
        let (sig, recid) = key(7).sign_prehash_recoverable(hash.as_slice()).expect("sign");
        Signature::new(
            U256::from_be_slice(&sig.r().to_bytes()),
            U256::from_be_slice(&sig.s().to_bytes()),
            recid.is_y_odd(),
        )
    }

    /// A fresh, empty directory, one per call, because tests run in parallel.
    pub fn scratch(name: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("lux-evm-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tests_support::{raw_paying, raw_transfer, who};

    /// A chain funding one account.
    fn chain() -> Evm {
        let g = Genesis {
            alloc: vec![(who(7), lux_evm::Alloc::funded(U256::from(10u64).pow(U256::from(21u64))))],
            fee: tests_support::UNMETERED,
            ..lux_evm::genesis::localnet()
        };
        Evm::new(g)
    }

    #[test]
    fn a_fresh_chain_is_at_its_genesis() {
        let c = chain();
        assert_eq!(c.height(), 0);
        assert_eq!(c.last_accepted(), c.block_id_at(0).expect("genesis"));
        assert_eq!(c.call("eth_chainId", &json!([])).expect("id"), json!("0x7a69"));
        assert_eq!(c.call("eth_blockNumber", &json!([])).expect("height"), json!("0x0"));
    }

    #[test]
    fn nothing_pending_is_not_a_block() {
        assert_eq!(chain().build().err(), Some(Error::Empty));
    }

    #[test]
    fn a_transaction_becomes_a_block_and_the_height_advances() {
        let c = chain();
        let to = who(9);
        let raw = raw_transfer(0, c.chain_id(), to, U256::from(1_000u64));
        c.call("eth_sendRawTransaction", &json!([format!("0x{}", hex(&raw))])).expect("accepted");
        assert_eq!(c.pending(), 1);

        let b = c.build().expect("builds");
        let id = b.id();
        assert_eq!(b.height(), 1);
        c.verify(&id).expect("verifies");
        c.accept(&id).expect("accepts");

        assert_eq!(c.height(), 1);
        assert_eq!(c.call("eth_blockNumber", &json!([])).expect("height"), json!("0x1"));
        assert_eq!(
            c.call("eth_getBalance", &json!([format!("{to:#x}"), "latest"])).expect("balance"),
            json!("0x3e8")
        );
        // The block the header names is the block that is served.
        let served = c.call("eth_getBlockByNumber", &json!(["0x1", false])).expect("block");
        assert_eq!(served["number"], json!("0x1"));
        assert_eq!(served["stateRoot"], json!(format!("{:#x}", B256::from(b.state_root()))));
    }

    #[test]
    fn a_block_whose_root_is_a_lie_is_refused() {
        // The claim that makes a certificate worth anything: verify re-runs the
        // block and checks the root it arrives at against the one claimed.
        let c = chain();
        let raw = raw_transfer(0, c.chain_id(), who(9), U256::from(1u64));
        c.submit(&raw).expect("accepted");
        let honest = c.build().expect("builds");

        let mut forged = c.lock().blocks.get(&honest.id()).cloned().expect("held");
        forged.built.header.header.state_root = B256::repeat_byte(0xe5);
        let forged = Sealed::new(forged.built, None);
        let bad = forged.id();
        {
            let mut held = c.lock();
            held.blocks.insert(bad, forged);
            held.verified.remove(&bad);
        }

        let refused = c.verify(&bad).expect_err("a forged root must not verify");
        assert!(
            matches!(&refused, Error::Invalid(why) if why.contains("state root")),
            "{refused:?}"
        );
        assert!(c.get(&bad).is_err(), "and it is not kept");
        assert!(c.get(&honest.id()).is_ok(), "while the honest one is");
    }

    #[test]
    fn this_nodes_blocks_carry_what_go_derives_and_another_node_agrees() {
        // The real schedule: Go's default, a block gas cost of 200,000 a
        // second faster than two.
        let funded = lux_evm::Alloc::funded(U256::from(10u64).pow(U256::from(21u64)));
        let g = Genesis {
            alloc: vec![(who(7), funded.clone()), (who(8), funded)],
            ..lux_evm::genesis::localnet()
        };
        let (a, b) = (Evm::new(g.clone()), Evm::new(g));
        a.submit(&raw_transfer(0, a.chain_id(), who(9), U256::from(1u64))).expect("in");
        let one = a.build().expect("block one");
        a.verify(&one.id()).expect("verifies");
        a.accept(&one.id()).expect("accepts");
        let h1 = a.lock().blocks[&one.id()].header().clone();
        assert_eq!(h1.beneficiary, lux_evm::fee::BLACKHOLE, "no fee recipients on localnet");
        assert_eq!(h1.difficulty, U256::from(1u64));
        assert_eq!(h1.extra_data, Bytes::from(vec![0u8; 86]), "an empty window, no predicates");
        assert_eq!(h1.gas_limit, 8_000_000, "the default schedule's limit, not the genesis's");
        assert_eq!(h1.base_fee_per_gas, Some(25_000_000_000), "the minimum, not the genesis's");

        // A second block at once owes a block gas cost, and a tip that covers
        // the most it can be — two steps — pays it.
        let tip = 500_000_000_000u128;
        a.submit(&raw_paying(8, 0, a.chain_id(), 25_000_000_000 + tip, tip)).expect("in");
        let two = a.build().expect("block two");
        a.verify(&two.id()).expect("verifies");
        a.accept(&two.id()).expect("accepts");
        let held = a.lock().blocks[&two.id()].clone();
        let want = lux_evm::fee::cost(
            &lux_evm::fee::Config::DEFAULT,
            Some(0),
            h1.timestamp,
            held.header().timestamp,
        );
        assert_eq!(
            (held.cost, held.sealed().block_gas_cost),
            (Some(want), (want > 0).then(|| U256::from(want))),
            "carried whenever it is not zero"
        );
        let w = lux_evm::fee::Window::parse(&held.header().extra_data).expect("a window");
        assert_eq!(w.sum(), 21_000, "block one's gas, in the window");

        // Another node holds both to the rule its own parent derives.
        for block in [&one, &two] {
            let p = b.parse(&block.bytes()).expect("parses");
            b.verify(&p.id()).expect("the other node verifies");
            b.accept(&p.id()).expect("and accepts");
        }
        assert_eq!(b.last_accepted(), a.last_accepted());
        let served = b.call("eth_getBlockByNumber", &json!(["0x2", false])).expect("block");
        assert_eq!(served["blockGasCost"], json!(quantity(want)));
        assert_eq!(a.call("eth_getBlockByNumber", &json!(["0x2", false])).expect("block"), served);
    }

    #[test]
    fn a_block_claiming_what_its_parent_does_not_derive_is_refused_before_it_runs() {
        let c = chain();
        c.submit(&raw_transfer(0, c.chain_id(), who(9), U256::from(1u64))).expect("in");
        let honest = c.build().expect("builds");
        let block = c.lock().blocks[&honest.id()].built.clone();
        type Lie = fn(&mut Header);
        let lies: [(&str, Lie); 6] = [
            ("base fee", |h| h.base_fee_per_gas = h.base_fee_per_gas.map(|f| f + 1)),
            ("gas limit", |h| h.gas_limit += 1),
            ("extra", |h| h.extra_data = Bytes::new()),
            ("coinbase", |h| h.beneficiary = Address::ZERO),
            ("difficulty", |h| h.difficulty = U256::ZERO),
            ("mix digest", |h| h.mix_hash = B256::repeat_byte(1)),
        ];
        for (what, lie) in lies {
            let mut forged = block.clone();
            lie(&mut forged.header.header);
            let forged = Sealed::new(forged, None);
            let bad = forged.id();
            c.lock().blocks.insert(bad, forged);
            let refused = c.verify(&bad).expect_err(what);
            assert!(
                matches!(&refused, Error::Invalid(why) if why.starts_with("header:")),
                "{what}: {refused:?}"
            );
            assert!(c.get(&bad).is_err(), "{what}: not kept");
        }
        c.verify(&honest.id()).expect("the honest block still verifies");
    }

    #[test]
    fn a_block_with_an_uncle_is_not_a_block() {
        let c = chain();
        c.submit(&raw_transfer(0, c.chain_id(), who(9), U256::from(1u64))).expect("in");
        let honest = c.build().expect("builds");
        let held = c.lock().blocks[&honest.id()].clone();
        let raw = alloy_rlp::encode(&RlpBlock {
            header: held.sealed().clone(),
            body: BlockBody {
                transactions: held.transactions().to_vec(),
                ommers: vec![c.first.sealed().clone()],
                withdrawals: None,
            },
        });
        let refused = c.parse(&raw).err().expect("refused");
        assert!(matches!(&refused, Error::Malformed(why) if why.contains("uncles")), "{refused:?}");
    }

    /// A block nobody can accept: a header on a parent nobody has, `n` high.
    fn orphan(c: &Evm, n: u64) -> Vec<u8> {
        let mut header = c.first.sealed().clone();
        header.header.number = n;
        header.header.parent_hash = B256::repeat_byte(0x77);
        Sealed::new(Built { header, transactions: Vec::new(), receipts: Vec::new() }, None).bytes()
    }

    #[test]
    fn a_stream_of_blocks_nobody_can_accept_is_held_to_a_bound() {
        // Which blocks arrive is a peer's choice. However many it sends, this
        // node holds the newest few it never executed, and no more.
        let c = chain();
        for n in 0..1000 {
            c.parse(&orphan(&c, 2 + n)).expect("a block");
        }
        let held = c.lock();
        assert_eq!(held.undecided.len(), UNVERIFIED_MAX, "the newest, and no more");
        assert_eq!(held.blocks.len(), 1 + UNVERIFIED_MAX, "the genesis and those");
        let newest = Sealed::read(orphan(&c, 2 + 999)).expect("a block").id();
        assert_eq!(held.undecided.back(), Some(&newest));
    }

    #[test]
    fn a_block_at_a_decided_height_is_not_held() {
        let c = chain();
        c.parse(&orphan(&c, 0)).expect("a block");
        assert_eq!(c.lock().blocks.len(), 1, "height 0 is the genesis's");
    }

    #[test]
    fn a_block_this_node_built_gives_back_what_it_took_when_it_is_dropped() {
        let c = chain();
        let id = c.chain_id();
        c.submit(&raw_transfer(0, id, who(9), U256::from(1u64))).expect("submitted");
        c.submit(&raw_transfer(1, id, who(9), U256::from(1u64))).expect("submitted");
        let b = c.build().expect("builds");
        assert_eq!(c.pending(), 0, "both went into it");

        // Refused — say because a peer served it under a certificate for
        // another block. The block goes; what its users sent does not.
        c.reject(&b.id()).expect("rejected");
        assert!(c.get(&b.id()).is_err(), "the block is gone");
        assert_eq!(c.pending(), 2, "its transactions are back");

        // Ahead of anything newer, so the nonces stay in order.
        c.submit(&raw_transfer(2, id, who(9), U256::from(1u64))).expect("submitted");
        let again = c.build().expect("builds again");
        assert_eq!(c.lock().blocks[&again.id()].transactions().len(), 3, "all three, in order");
    }

    #[test]
    fn a_decided_height_lets_go_of_what_else_was_built_there() {
        let id = chain().chain_id();
        let x = raw_transfer(0, id, who(9), U256::from(1u64));

        // This node built on X; the network accepted another block at that
        // height, carrying X and one more.
        let c = chain();
        c.submit(&x).expect("submitted");
        let mine = c.build().expect("builds");
        let other = chain();
        other.submit(&x).expect("submitted");
        other.submit(&raw_transfer(1, id, who(8), U256::from(1u64))).expect("submitted");
        let theirs = other.build().expect("builds");
        assert_ne!(mine.id(), theirs.id());
        c.parse(&theirs.bytes()).expect("parses");
        c.verify(&theirs.id()).expect("verifies");
        c.accept(&theirs.id()).expect("accepts");
        assert!(c.get(&mine.id()).is_err(), "its height is decided, and it lost");
        assert_eq!(c.pending(), 0, "X is in the accepted block, so it does not come back");

        // The accepted block carried another transaction at X's nonce, and
        // not the one after it: X can never run, and the one after comes back.
        let c = chain();
        c.submit(&x).expect("submitted");
        let after = raw_transfer(1, id, who(9), U256::from(1u64));
        c.submit(&after).expect("submitted");
        let mine = c.build().expect("builds");
        let other = chain();
        other.submit(&raw_transfer(0, id, who(8), U256::from(2u64))).expect("submitted");
        let theirs = other.build().expect("builds");
        c.parse(&theirs.bytes()).expect("parses");
        c.verify(&theirs.id()).expect("verifies");
        c.accept(&theirs.id()).expect("accepts");
        assert!(c.get(&mine.id()).is_err());
        assert_eq!(c.pending(), 1, "the transaction after X is back in the pool");
        let hash = *TxEnvelope::decode_2718(&mut &after[..]).expect("tx").tx_hash();
        assert!(c.lock().pending.get(&hash).is_some());
    }

    #[test]
    fn a_flood_of_transactions_stays_in_the_pool_and_the_chain_goes_on() {
        const CAP: u128 = 100_000_000_000;
        const GWEI: u128 = 1_000_000_000;
        let (most, each, gap) = (32, 8, 4);
        let funded = lux_evm::Alloc::funded(U256::from(10u64).pow(U256::from(21u64)));
        let c = Evm::new(Genesis {
            alloc: (1..=61u8).map(|b| (who(b), funded.clone())).collect(),
            fee: tests_support::UNMETERED,
            ..lux_evm::genesis::localnet()
        });
        let id = c.chain_id();
        c.lock().pending = Pool::bounded(most, each, gap, u128::from(c.env.base_fee));
        let send =
            |raw: &[u8]| c.call("eth_sendRawTransaction", &json!([format!("0x{}", hex(raw))]));

        // One sender, as fast as it can: held to its share.
        for nonce in 0..200 {
            let _ = send(&raw_paying(1, nonce, id, CAP, GWEI));
        }
        assert_eq!(c.pending(), each, "one sender's share, and no more");
        // Nor can it run ahead of what it can run.
        let far = send(&raw_paying(2, gap + 1, id, CAP, GWEI)).expect_err("past the gap");
        assert!(matches!(&far, Error::Invalid(why) if why.contains("past the next")), "{far:?}");

        // Many senders, each outbidding the last: never past the bound.
        for (i, b) in (2..=60u8).enumerate() {
            send(&raw_paying(b, 0, id, CAP, GWEI + 1 + i as u128)).expect("outbids");
            assert!(c.pending() <= most, "{} waiting", c.pending());
        }
        assert_eq!(c.pending(), most);
        let cheap = send(&raw_paying(61, 0, id, CAP, GWEI)).expect_err("does not outbid");
        assert!(matches!(&cheap, Error::Invalid(why) if why.contains("cheapest")), "{cheap:?}");

        // And the chain goes on: every block takes what can run.
        let mut blocks = 0;
        while let Ok(b) = c.build() {
            c.verify(&b.id()).expect("verifies");
            c.accept(&b.id()).expect("accepts");
            blocks += 1;
        }
        assert!(blocks > 0);
        assert_eq!(c.pending(), 0, "everything that waited ran");
        assert_eq!(c.height(), blocks);
    }

    #[test]
    fn a_sender_behind_a_gap_keeps_its_place_and_runs_when_the_gap_fills() {
        let c = chain();
        let id = c.chain_id();
        let pay = |n: u64| raw_transfer(n, id, who(9), U256::from(1u64));
        for n in [0, 2, 3] {
            c.submit(&pay(n)).expect("admitted");
        }
        let b = c.build().expect("nonce 0 runs");
        assert_eq!(c.lock().blocks[&b.id()].transactions().len(), 1, "and nothing past the gap");
        c.verify(&b.id()).expect("verifies");
        c.accept(&b.id()).expect("accepts");
        assert_eq!(c.pending(), 2, "2 and 3 wait for 1, and are not dropped");
        assert_eq!(c.build().err(), Some(Error::Empty), "nothing can run, so nothing is built");

        c.submit(&pay(1)).expect("the gap");
        let b = c.build().expect("1, 2 and 3 run");
        assert_eq!(c.lock().blocks[&b.id()].transactions().len(), 3);
    }

    /// A block of `n` transfers from `who(7)`, and its transactions as they
    /// arrived — without recovering a signature to make them.
    fn full(n: u64) -> (Vec<u8>, Vec<Pending>) {
        let source = chain();
        let id = source.chain_id();
        let base = u128::from(source.env.base_fee);
        source.lock().pending = Pool::bounded(1000, 1000, 1000, base);
        let mut arrived = Vec::new();
        for nonce in 0..n {
            let raw = raw_paying(7, nonce, id, 100_000_000_000, 1_000_000_000);
            let tx = TxEnvelope::decode_2718(&mut &raw[..]).expect("a transaction");
            arrived.push(Pending { tx, sender: who(7) });
        }
        for p in &arrived {
            source.lock().pending.admit(p.clone(), 0).expect("admitted");
        }
        let block = source.build().expect("builds");
        assert_eq!(source.lock().blocks[&block.id()].transactions().len() as u64, n);
        (block.bytes(), arrived)
    }

    /// The longest `c`'s lock was held while `work` ran.
    fn held_while(c: &Evm, work: impl FnOnce() + Send) -> Duration {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::Instant;
        let done = AtomicBool::new(false);
        std::thread::scope(|s| {
            let probe = s.spawn(|| {
                let mut longest = Duration::ZERO;
                while !done.load(Ordering::Relaxed) {
                    let t = Instant::now();
                    drop(c.lock());
                    longest = longest.max(t.elapsed());
                    std::thread::sleep(Duration::from_micros(100));
                }
                longest
            });
            work();
            done.store(true, Ordering::Relaxed);
            probe.join().expect("the probe")
        })
    }

    #[test]
    fn a_block_is_verified_without_holding_the_chain_while_its_signatures_recover() {
        use alloy_consensus::transaction::SignerRecoverable as _;
        const TXS: u64 = 500;
        let (raw, arrived) = full(TXS);

        // What recovering every sender costs, one after another.
        let t = std::time::Instant::now();
        for p in &arrived {
            assert_eq!(p.tx.recover_signer().expect("recovers"), who(7));
        }
        let serial = t.elapsed();

        // A node that never saw the transactions recovers them all — and the
        // chain is not held while it does.
        let c = chain();
        let id = c.parse(&raw).expect("parses").id();
        let held = held_while(&c, || c.verify(&id).expect("verifies"));
        assert!(held < serial / 4, "held {held:?}; recovering alone takes {serial:?}");
        assert!(c.verify(&id).is_ok());

        // A node that did see them recovers none: the pool kept each sender.
        let c = chain();
        c.lock().pending = Pool::bounded(1000, 1000, 1000, u128::from(c.env.base_fee));
        for p in &arrived {
            c.lock().pending.admit(p.clone(), 0).expect("admitted");
        }
        let id = c.parse(&raw).expect("parses").id();
        let t = std::time::Instant::now();
        c.verify(&id).expect("verifies");
        let known = t.elapsed();
        assert!(known < serial / 2, "{known:?} with every sender known, {serial:?} to recover");
    }

    #[test]
    fn a_header_whose_lengths_lie_is_refused_before_it_is_read() {
        let dir = tests_support::scratch("head");
        let lengths = |header: &[u8]| {
            let mut record = vec![0xf9, 0x01, 0x00];
            record.extend_from_slice(header);
            record.resize(0x103, 0);
            record
        };
        // A header claiming 2^56 bytes, one claiming more than a block, one
        // claiming more than its record, and one whose length is honest and
        // which is not a header.
        let records = [
            lengths(&[0xff, 0x01, 0, 0, 0, 0, 0, 0, 0]),
            lengths(&[0xfa, 0x40, 0x00, 0x00]),
            lengths(&[0xf9, 0x10, 0x00]),
            lengths(&[0xc0]),
        ];
        let mut raw = Vec::new();
        let mut spans = Vec::new();
        for r in &records {
            spans.push(Span { at: raw.len() as u64, len: r.len() as u32 });
            raw.extend_from_slice(&(r.len() as u32).to_be_bytes());
            raw.extend_from_slice(r);
        }
        std::fs::write(dir.join("blocks"), &raw).expect("blocks");
        let l = Ledger::open(&dir).expect("open");
        let files = l.files().expect("handles").expect("on disk");
        for span in &spans {
            assert!(head(&files, *span, 1).is_err(), "{span:?}");
        }
        // A span that runs past the file is refused before it is allocated.
        let beyond = Span { at: raw.len() as u64, len: u32::MAX };
        assert!(files.block(beyond).is_err());
        assert!(files.block_part(beyond, 0, 18).is_err());
        assert!(files.block_part(spans[3], u64::MAX, 2).is_err(), "no overflow");
    }

    #[test]
    fn a_sender_cannot_commit_more_than_its_balance() {
        const CAP: u128 = 100_000_000_000;
        const TIP: u128 = 1_000_000_000;
        // `raw_paying` sends 1 wei on 21,000 gas.
        let cost = U256::from(21_000u64 * CAP as u64) + U256::from(1u64);
        let c = Evm::new(Genesis {
            alloc: vec![(
                who(1),
                lux_evm::Alloc::funded(cost * U256::from(7u64) / U256::from(2u64)),
            )],
            fee: tests_support::UNMETERED,
            ..lux_evm::genesis::localnet()
        });
        let id = c.chain_id();
        for nonce in 0..3 {
            c.submit(&raw_paying(1, nonce, id, CAP, TIP)).expect("within the balance");
        }
        // Each one alone is affordable; all four together are not.
        let why = c.submit(&raw_paying(1, 3, id, CAP, TIP)).expect_err("past the balance");
        assert!(matches!(&why, Error::Invalid(w) if w.contains("insufficient funds")), "{why:?}");
        assert_eq!(c.pending(), 3);

        // A replacement is counted in place of what it replaces: a ten
        // percent bump fits in what is left, doubling the cap does not.
        let bump = raw_paying(1, 2, id, CAP * 11 / 10, TIP * 11 / 10);
        c.submit(&bump).expect("the bump fits");
        let greedy = c.submit(&raw_paying(1, 2, id, CAP * 2, TIP * 2)).expect_err("does not");
        assert!(matches!(&greedy, Error::Invalid(w) if w.contains("insufficient funds")));
        assert_eq!(c.pending(), 3);
        let hash = *TxEnvelope::decode_2718(&mut &bump[..]).expect("tx").tx_hash();
        assert!(c.lock().pending.get(&hash).is_some(), "the bump stands");

        // And what it committed, the chain carries.
        let b = c.build().expect("builds");
        assert_eq!(c.lock().blocks[&b.id()].transactions().len(), 3);
    }

    #[test]
    fn a_sender_goes_on_from_after_this_nodes_own_undecided_block() {
        let c = chain();
        let id = c.chain_id();
        let pay = |n: u64| raw_transfer(n, id, who(9), U256::from(1u64));
        let me = format!("{:#x}", who(7));
        let next = |c: &Evm| c.call("eth_getTransactionCount", &json!([me, "pending"])).expect("n");
        let order = |c: &Evm| c.lock().pending.iter().map(|p| p.tx.nonce()).collect::<Vec<_>>();
        let carried = |c: &Evm, b: &dyn Block| c.lock().blocks[&b.id()].transactions().len();

        // Nonces 0 to 5 in this node's own block, waiting on a quorum.
        for n in 0..6 {
            c.submit(&pay(n)).expect("admitted");
        }
        let b = c.build().expect("builds");
        assert_eq!(c.pending(), 0);
        // The sender goes on after them: 6 to 20 are neither a gap nor a
        // repeat, and a repeat of what the block carries is.
        for n in 6..=20 {
            c.submit(&pay(n)).expect("the next nonces");
        }
        assert_eq!(c.pending(), 15);
        assert_eq!(next(&c), json!("0x15"));
        assert!(c.submit(&pay(5)).is_err(), "5 is in the block");

        // Rejected: its six come back in front, and the sender is in order.
        c.reject(&b.id()).expect("rejected");
        assert_eq!(order(&c), (0..=20).collect::<Vec<_>>());
        let b = c.build().expect("all of them");
        assert_eq!(carried(&c, b.as_ref()), 21);

        // Accepted: what followed it is what the next block carries.
        let c = chain();
        for n in 0..6 {
            c.submit(&pay(n)).expect("admitted");
        }
        let b = c.build().expect("builds");
        for n in 6..=20 {
            c.submit(&pay(n)).expect("the next nonces");
        }
        c.verify(&b.id()).expect("verifies");
        c.accept(&b.id()).expect("accepts");
        assert_eq!(order(&c), (6..=20).collect::<Vec<_>>());
        let b = c.build().expect("the rest");
        assert_eq!(carried(&c, b.as_ref()), 15);
    }

    #[test]
    fn turns_are_few_and_come_back() {
        let t = Turns::new(2);
        let one = t.take(Duration::ZERO).expect("one");
        let _two = t.take(Duration::ZERO).expect("two");
        assert!(t.take(Duration::from_millis(20)).is_none(), "no third");
        drop(one);
        assert!(t.take(Duration::ZERO).is_some(), "one came back");
    }

    #[test]
    fn a_transaction_for_another_chain_is_not_admitted() {
        let c = chain();
        let raw = raw_transfer(0, c.chain_id() + 1, who(9), U256::from(1u64));
        assert!(matches!(c.submit(&raw), Err(Error::Invalid(_))));
        assert_eq!(c.pending(), 0);
    }

    #[test]
    fn garbage_is_not_a_transaction() {
        let c = chain();
        assert!(matches!(c.submit(&[0xff, 0x00, 0x13]), Err(Error::Malformed(_))));
        assert_eq!(c.pending(), 0);
    }

    #[test]
    fn every_block_is_in_the_shape_go_encodes_for_its_fork() {
        // A chain in Cancun from a genesis that keeps its post-merge fields:
        // block zero and every block after it carry the withdrawals root, the
        // blob fields and the parent beacon root — Ethereum's twenty fields —
        // and a block carries its (empty) withdrawals.
        let c = chain();
        let genesis = c.get(&c.last_accepted()).expect("genesis");
        assert_eq!(c.lock().blocks[&genesis.id()].sealed().layout.name, "20 eth");
        c.submit(&raw_transfer(0, c.chain_id(), who(9), U256::from(1u64))).expect("accepted");
        let built = c.build().expect("builds");
        let held = c.lock().blocks[&built.id()].clone();
        assert_eq!(held.sealed().layout.name, "20 eth");
        let h = held.header();
        assert_eq!(
            (h.withdrawals_root, h.blob_gas_used, h.excess_blob_gas, h.parent_beacon_block_root),
            (Some(lux_evm::root::EMPTY), Some(0), Some(0), Some(B256::ZERO))
        );
        assert_eq!(built.id(), held.sealed().seal().0);
        assert!(Sealed::read(built.bytes()).is_ok(), "and reads back, withdrawals and all");

        // Before Shanghai, the Lux shape: `[baseFee, ""]`, the seventeenth an
        // absent ExtDataHash; and block zero is sixteen fields.
        let old = Evm::new(Genesis {
            alloc: vec![(who(7), lux_evm::Alloc::funded(U256::from(10u64).pow(U256::from(21u64))))],
            chain: lux_evm::Chain {
                durango: None,
                shanghai: None,
                cancun: None,
                ..lux_evm::Chain::lux()
            },
            fee: tests_support::UNMETERED,
            ..lux_evm::genesis::localnet()
        });
        let zero = old.get(&old.last_accepted()).expect("genesis");
        assert_eq!(old.lock().blocks[&zero.id()].sealed().fields(), 16);
        old.submit(&raw_transfer(0, old.chain_id(), who(9), U256::from(1u64))).expect("in");
        let built = old.build().expect("builds");
        let held = old.lock().blocks[&built.id()].clone();
        assert_eq!(held.sealed().layout.name, "17", "an ordinary block is the Lux shape");
        assert_eq!(held.header().extra_data.len(), 80, "no predicate results before Durango");
        assert_ne!(
            built.id(),
            lux_evm::header::seal(held.header()).0,
            "hashing the plain header would name the block something else"
        );
    }

    /// A header as Go's oracle reports one: its name, its field count, and
    /// every field `DecodeHeader` may leave nil, as `hexutil` writes it.
    fn answer(h: &header::Lux, name: B256) -> Value {
        let big = |b: &[u8]| {
            let s = hex(b);
            let s = s.trim_start_matches('0');
            format!("0x{}", if s.is_empty() { "0" } else { s })
        };
        let hash = |v: Option<B256>| v.map(|v| format!("{v:#x}"));
        let u = &h.header;
        json!({
            "hash": format!("{name:#x}"),
            "fields": h.fields(),
            "baseFeePerGas": u.base_fee_per_gas.map(|v| format!("{v:#x}")),
            "blockGasCost": h.block_gas_cost.map(|v| format!("{v:#x}")),
            "extDataHash": hash(h.ext_data_hash),
            "extDataGasUsed": h.ext_data_gas_used.as_ref().map(|b| big(b)),
            "blobGasUsed": u.blob_gas_used.map(|v| format!("{v:#x}")),
            "excessBlobGas": u.excess_blob_gas.map(|v| format!("{v:#x}")),
            "parentBeaconRoot": hash(u.parent_beacon_block_root),
            "withdrawalsHash": hash(u.withdrawals_root),
            "requestsHash": hash(u.requests_hash),
        })
    }

    #[test]
    fn every_header_shape_go_reads_is_read_as_go_reads_it() {
        // luxfi/conformance `evm_header_layouts.json`: what luxfi/geth's
        // `DecodeHeader` — or `DecodeBlock`, for a case wrapped in a block of
        // `block_items` items — makes of every header shape, from Go's own
        // oracle. The fields it reads and the name it gives, or its refusal.
        let vectors = std::env::var_os("LUX_CONFORMANCE").expect(
            "LUX_CONFORMANCE (luxfi/conformance vectors/) must be set; this test compares against \
             Go's answers and has nothing to compare without them",
        );
        let path = std::path::Path::new(&vectors).join("evm_header_layouts.json");
        let doc: Value = serde_json::from_str(
            &std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
        )
        .expect("the layouts JSON");
        let unhex =
            |v: &Value| alloy_primitives::hex::decode(v.as_str().expect("hex")).expect("hex");
        let mut n = 0;
        for c in doc["cases"].as_array().expect("cases") {
            n += 1;
            let name = c["name"].as_str().expect("a name");
            let raw = unhex(&c["header"]);
            let read = match c["block_items"].as_u64() {
                None => header::Lux::read(&raw).map(|h| {
                    let named = h.seal();
                    (h, named)
                }),
                Some(items) => {
                    let mut payload = raw.clone();
                    payload.extend([0xc0, 0xc0]);
                    if items == 4 {
                        payload.extend(c.get("fourth").map_or(vec![0xc0], unhex));
                    }
                    let mut block = Vec::new();
                    alloy_rlp::Header { list: true, payload_length: payload.len() }
                        .encode(&mut block);
                    block.extend(payload);
                    Sealed::read(block).map(|b| (b.built.header, B256::from(b.id)))
                }
            };
            let go = &c["go"];
            if let Some(why) = go.get("error") {
                assert!(read.is_err(), "{name}: Go refuses it ({why}), and this read it");
                continue;
            }
            // Go reads a base fee of any width; revm's block environment holds
            // 64 bits, so a wider one is refused here while reading. Go refuses
            // one past 256 bits in SyntacticVerify, and no fee schedule these
            // chains run reaches 2^64.
            if go["baseFeePerGas"].as_str().is_some_and(|f| f.len() > 18) {
                assert!(read.is_err(), "{name}: a base fee past 64 bits is refused");
                continue;
            }
            let (h, named) =
                read.unwrap_or_else(|e| panic!("{name}: Go reads it, and this refused it: {e}"));
            assert_eq!(answer(&h, named), *go, "{name} ({})", h.layout.name);
            assert_eq!(named, h.seal(), "{name}: named by the bytes it arrived as");
        }
        assert_eq!(n, 40, "the corpus holds forty shapes");
    }

    #[test]
    fn a_block_round_trips_through_its_encoding() {
        let c = chain();
        c.submit(&raw_transfer(0, c.chain_id(), who(9), U256::from(1u64))).expect("accepted");
        let built = c.build().expect("builds");
        let parsed = c.parse(&built.bytes()).expect("parses");
        assert_eq!(
            parsed.id(),
            built.id(),
            "a block that does not survive its own encoding has two names"
        );
        assert_eq!(parsed.state_root(), built.state_root());
    }

    #[test]
    fn an_unknown_method_is_refused_rather_than_answered() {
        let c = chain();
        assert!(matches!(c.call("eth_coinbase", &json!([])), Err(Error::NoMethod(_))));
    }

    #[test]
    fn historical_state_is_refused_rather_than_invented() {
        let c = chain();
        c.submit(&raw_transfer(0, c.chain_id(), who(9), U256::from(1u64))).expect("accepted");
        let b = c.build().expect("builds");
        c.verify(&b.id()).expect("verifies");
        c.accept(&b.id()).expect("accepts");

        let asked = c.call("eth_getBalance", &json!([format!("{:#x}", who(7)), "0x0"]));
        assert!(
            matches!(&asked, Err(Error::BadRequest(why)) if why.contains("historical")),
            "{asked:?}"
        );
    }

    #[test]
    fn the_sha256_precompile_answers_through_eth_call() {
        // Real bytecode execution over the RPC surface, checked against a
        // published constant.
        let c = chain();
        let out = c
            .call(
                "eth_call",
                &json!([{
                    "to": "0x0000000000000000000000000000000000000002",
                    "data": "0x616263",
                }, "latest"]),
            )
            .expect("the call runs");
        assert_eq!(
            out,
            json!("0xba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
    }

    #[test]
    fn a_receipt_is_served_for_a_transaction_that_landed() {
        let c = chain();
        let raw = raw_transfer(0, c.chain_id(), who(9), U256::from(1u64));
        let hash = c.submit(&raw).expect("accepted");
        let b = c.build().expect("builds");
        c.verify(&b.id()).expect("verifies");
        c.accept(&b.id()).expect("accepts");

        let r =
            c.call("eth_getTransactionReceipt", &json!([format!("{hash:#x}")])).expect("receipt");
        assert_eq!(r["status"], json!("0x1"));
        assert_eq!(r["gasUsed"], json!("0x5208"), "21000");
        assert_eq!(r["blockNumber"], json!("0x1"));
    }

    #[test]
    fn accepting_out_of_order_is_refused() {
        let c = chain();
        c.submit(&raw_transfer(0, c.chain_id(), who(9), U256::from(1u64))).expect("accepted");
        let first = c.build().expect("builds");
        c.verify(&first.id()).expect("verifies");

        // A second block on top of the first, accepted before it.
        c.set_preference(&first.id()).expect("prefers");
        c.submit(&raw_transfer(1, c.chain_id(), who(9), U256::from(1u64))).expect("accepted");
        let second = c.build().expect("builds");
        c.verify(&second.id()).expect("verifies");

        assert!(matches!(c.accept(&second.id()), Err(Error::Invalid(_))));
        c.accept(&first.id()).expect("the parent accepts");
        c.accept(&second.id()).expect("then the child does");
        assert_eq!(c.height(), 2);
    }
}

/// The chain across a restart, and everything it serves about its past.
#[cfg(test)]
mod durable {
    use super::*;
    use std::fs::OpenOptions;
    use std::io::Write as _;
    use tests_support::{raw_create, raw_legacy, raw_transfer, scratch, who};

    fn genesis() -> Genesis {
        Genesis {
            alloc: vec![(who(7), lux_evm::Alloc::funded(U256::from(10u64).pow(U256::from(21u64))))],
            fee: tests_support::UNMETERED,
            ..lux_evm::genesis::localnet()
        }
    }

    /// Put `raws` in one block and accept it.
    fn land(c: &Evm, raws: &[Vec<u8>]) -> Id {
        for raw in raws {
            c.submit(raw).expect("admitted");
        }
        let b = c.build().expect("builds");
        let id = b.id();
        assert_eq!(c.lock().blocks[&id].transactions().len(), raws.len(), "every one fits");
        c.verify(&id).expect("verifies");
        c.accept(&id).expect("accepts");
        id
    }

    /// Constructor code that logs `topics` over one 32-byte word holding
    /// `word`, and deploys nothing.
    fn logging(word: u8, topics: &[B256]) -> Vec<u8> {
        let mut code = vec![0x60, word, 0x60, 0x00, 0x52]; // MSTORE(0, word)
                                                           // LOGn takes offset, size, then topic 0 first — so the topics go on
                                                           // the stack last-first.
        for t in topics.iter().rev() {
            code.push(0x7f);
            code.extend_from_slice(t.as_slice());
        }
        code.extend_from_slice(&[0x60, 0x20, 0x60, 0x00, 0xa0 + topics.len() as u8, 0x00]);
        code
    }

    const A: B256 = B256::repeat_byte(0xaa);
    const B: B256 = B256::repeat_byte(0xbb);
    const C: B256 = B256::repeat_byte(0xcc);

    /// Four blocks: logs in three of them, two creations in one, a legacy
    /// transfer, and a block with no logs at all. Returns the transaction
    /// hashes in order.
    fn history(c: &Evm) -> Vec<B256> {
        let id = c.chain_id();
        let raws = [
            vec![
                raw_create(0, id, logging(1, &[A])),
                raw_transfer(1, id, who(9), U256::from(5u64)),
            ],
            vec![raw_create(2, id, logging(2, &[A, B])), raw_create(3, id, logging(3, &[B]))],
            vec![raw_legacy(4, id, who(9), 30_000_000_000)],
            vec![raw_create(5, id, logging(4, &[C, A]))],
        ];
        let mut hashes = Vec::new();
        for block in &raws {
            for raw in block {
                hashes.push(*TxEnvelope::decode_2718(&mut &raw[..]).expect("tx").tx_hash());
            }
            land(c, block);
        }
        hashes
    }

    /// Everything the chain says about its past and its tip, as a client
    /// would read it.
    fn picture(c: &Evm, hashes: &[B256]) -> Vec<Value> {
        let mut out = Vec::new();
        for h in 0..=c.height() {
            let n = format!("0x{h:x}");
            out.push(c.call("eth_getBlockByNumber", &json!([n, true])).expect("block"));
            out.push(c.call("eth_getBlockReceipts", &json!([n])).expect("receipts"));
        }
        for t in hashes {
            let t = format!("{t:#x}");
            out.push(c.call("eth_getTransactionByHash", &json!([t])).expect("tx"));
            out.push(c.call("eth_getTransactionReceipt", &json!([t])).expect("receipt"));
        }
        out.push(c.call("eth_getLogs", &json!([{"fromBlock": "earliest"}])).expect("logs"));
        for a in [who(7), who(9)] {
            let a = format!("{a:#x}");
            out.push(c.call("eth_getBalance", &json!([a, "latest"])).expect("balance"));
            out.push(c.call("eth_getTransactionCount", &json!([a, "latest"])).expect("nonce"));
        }
        out
    }

    /// Drop every accepted block but the tip from memory, so every answer
    /// about the past has to come off the disk.
    fn forget(c: &Evm) {
        let mut held = c.lock();
        let tip = held.accepted;
        held.blocks.retain(|id, _| *id == tip);
    }

    #[test]
    fn a_reopened_chain_is_the_chain_that_was_closed() {
        let dir = scratch("reopen");
        let c = Evm::open(genesis(), &dir).expect("open");
        let hashes = history(&c);
        let head = c.last_accepted();
        let root = c.lock().world.root();
        let before = picture(&c, &hashes);
        assert!(before.iter().all(|v| !v.is_null()), "every question has an answer");
        drop(c);

        let c = Evm::open(genesis(), &dir).expect("reopen");
        assert_eq!(c.last_accepted(), head, "the same head");
        assert_eq!(c.height(), 4);
        assert_eq!(c.lock().world.root(), root, "the same world");
        assert_eq!(B256::from(c.get(&head).expect("tip").state_root()), root);
        for h in 0..=4 {
            assert!(c.block_id_at(h).is_ok(), "height {h} is named");
        }
        assert_eq!(picture(&c, &hashes), before, "the same answers, from a replay");

        forget(&c);
        assert_eq!(picture(&c, &hashes), before, "and the same answers, read off the disk");

        // It goes on from where it was: the next nonce is the one after the
        // last accepted, and the new block lands on the old head.
        let next = land(&c, &[raw_transfer(6, c.chain_id(), who(9), U256::from(1u64))]);
        assert_eq!(c.get(&next).expect("held").parent(), head);
        drop(c);
        let c = Evm::open(genesis(), &dir).expect("a third open");
        assert_eq!(c.last_accepted(), next);
    }

    #[test]
    fn a_chain_is_opened_by_one_process_at_a_time() {
        let dir = scratch("once");
        let c = Evm::open(genesis(), &dir).expect("open");
        let twice = Evm::open(genesis(), &dir).err().expect("a second writer is refused");
        assert!(twice.contains("another process"), "{twice}");
        drop(c);
        Evm::open(genesis(), &dir).expect("and allowed once the first is gone");
    }

    #[test]
    fn a_torn_last_record_is_dropped_and_the_chain_goes_on() {
        let dir = scratch("torn");
        let c = Evm::open(genesis(), &dir).expect("open");
        for n in 0..3 {
            land(&c, &[raw_transfer(n, c.chain_id(), who(9), U256::from(1u64))]);
        }
        let head = c.last_accepted();
        drop(c);
        let whole = std::fs::metadata(dir.join("blocks")).expect("blocks").len();

        // A crash mid-write: a length promising more bytes than follow it.
        let mut f = OpenOptions::new().append(true).open(dir.join("blocks")).expect("raw");
        f.write_all(&[0, 0, 0, 90, 0xf9, 0x02]).expect("tear");
        drop(f);

        let c = Evm::open(genesis(), &dir).expect("reopen");
        assert_eq!(c.last_accepted(), head, "the torn record was never accepted");
        assert_eq!(std::fs::metadata(dir.join("blocks")).expect("blocks").len(), whole, "cut");
        let next = land(&c, &[raw_transfer(3, c.chain_id(), who(9), U256::from(1u64))]);
        drop(c);

        let c = Evm::open(genesis(), &dir).expect("reopen after writing past the tear");
        assert_eq!(c.last_accepted(), next);
        assert_eq!(c.height(), 4);
    }

    #[test]
    fn a_last_record_whose_bytes_never_arrived_is_a_torn_write() {
        // The length reached the device and the bytes did not: a file extended
        // with zeros. It is the last write, so it is the one that was never
        // accepted.
        let dir = scratch("zeros");
        let c = Evm::open(genesis(), &dir).expect("open");
        let head = land(&c, &[raw_transfer(0, c.chain_id(), who(9), U256::from(1u64))]);
        drop(c);
        let mut f = OpenOptions::new().append(true).open(dir.join("blocks")).expect("raw");
        f.write_all(&[0, 0, 0, 4, 0, 0, 0, 0]).expect("zeros");
        drop(f);

        let c = Evm::open(genesis(), &dir).expect("reopen");
        assert_eq!(c.last_accepted(), head);
    }

    /// Land a block as history: accepted, and on the device only at a sync.
    fn imported(c: &Evm, raw: Vec<u8>) -> Id {
        c.submit(&raw).expect("admitted");
        let id = c.build().expect("builds").id();
        c.verify(&id).expect("verifies");
        c.accept_history(&id).expect("accepts");
        id
    }

    #[test]
    fn a_run_of_history_a_crash_cut_short_ends_where_it_is_whole() {
        // Two blocks accepted and waited for, then three of history nobody
        // synced. A crash that wrote the first and zeroed the rest — what an
        // unsynced tail can look like — reopens at the first, and goes on.
        let dir = scratch("history");
        let blocks = dir.join("blocks");
        let c = Evm::open(genesis(), &dir).expect("open");
        for n in 0..2 {
            land(&c, &[raw_transfer(n, c.chain_id(), who(9), U256::from(1u64))]);
        }
        let third = imported(&c, raw_transfer(2, c.chain_id(), who(9), U256::from(1u64)));
        let whole = std::fs::metadata(&blocks).expect("blocks").len();
        for n in 3..5 {
            imported(&c, raw_transfer(n, c.chain_id(), who(9), U256::from(1u64)));
        }
        assert!(dir.join("blocks.loose").exists(), "the run is marked");
        drop(c);
        let len = std::fs::metadata(&blocks).expect("blocks").len();
        let f = OpenOptions::new().write(true).open(&blocks).expect("raw");
        let zeros = vec![0u8; (len - whole) as usize];
        std::os::unix::fs::FileExt::write_all_at(&f, &zeros, whole).expect("a zeroed tail");
        drop(f);

        let c = Evm::open(genesis(), &dir).expect("reopen");
        assert_eq!((c.last_accepted(), c.height()), (third, 3), "the whole records, and no more");
        assert_eq!(std::fs::metadata(&blocks).expect("blocks").len(), whole, "cut");
        assert!(!dir.join("blocks.loose").exists(), "and the run is over");
        let next = land(&c, &[raw_transfer(3, c.chain_id(), who(9), U256::from(1u64))]);
        drop(c);
        let c = Evm::open(genesis(), &dir).expect("reopen after writing past it");
        assert_eq!(c.last_accepted(), next);
    }

    #[test]
    fn a_synced_run_of_history_is_the_chain() {
        let dir = scratch("synced");
        let c = Evm::open(genesis(), &dir).expect("open");
        let mut tip = c.last_accepted();
        for n in 0..3 {
            tip = imported(&c, raw_transfer(n, c.chain_id(), who(9), U256::from(1u64)));
        }
        c.sync().expect("sync");
        assert!(!dir.join("blocks.loose").exists());
        drop(c);
        // Damage before the end is damage again once the run is synced.
        let f = OpenOptions::new().write(true).open(dir.join("blocks")).expect("raw");
        let c = Evm::open(genesis(), &dir).expect("reopen");
        assert_eq!((c.last_accepted(), c.height()), (tip, 3));
        drop(c);
        std::os::unix::fs::FileExt::write_all_at(&f, &[0x00], 4).expect("damage block 1");
        let refused = Evm::open(genesis(), &dir).err().expect("refused");
        assert!(refused.contains("block 1 is not a block"), "{refused}");
    }

    #[test]
    fn a_damaged_record_before_the_last_refuses_the_chain() {
        // Not a crash: every record after it was written after it was on the
        // device. Guessing past it would serve a chain with a hole in it.
        let dir = scratch("damaged");
        let c = Evm::open(genesis(), &dir).expect("open");
        for n in 0..2 {
            land(&c, &[raw_transfer(n, c.chain_id(), who(9), U256::from(1u64))]);
        }
        drop(c);
        let f = OpenOptions::new().write(true).open(dir.join("blocks")).expect("raw");
        std::os::unix::fs::FileExt::write_all_at(&f, &[0x00], 4).expect("damage block 1");
        drop(f);

        let refused = Evm::open(genesis(), &dir).err().expect("refused");
        assert!(refused.contains("block 1 is not a block"), "{refused}");
    }

    #[test]
    fn a_recorded_block_that_does_not_execute_to_its_claim_refuses_the_chain() {
        let honest = Evm::new(genesis());
        honest.submit(&raw_transfer(0, honest.chain_id(), who(9), U256::from(1u64))).expect("in");
        let built = honest.build().expect("builds");
        let block = honest.lock().blocks[&built.id()].built.clone();

        let forge = |lie: fn(&mut Header)| {
            let mut b = block.clone();
            lie(&mut b.header.header);
            Sealed::new(b, None).bytes()
        };
        for (name, lie, says) in [
            (
                "root",
                (|h: &mut Header| h.state_root = B256::repeat_byte(0xe5)) as fn(&mut Header),
                "state root",
            ),
            (
                "receipts",
                |h: &mut Header| h.receipts_root = B256::repeat_byte(0xe5),
                "receipts root",
            ),
            ("gas", |h: &mut Header| h.gas_used += 1, "gas used"),
        ] {
            let dir = scratch(name);
            write_hanzod(&dir, &[forge(lie)]);
            let refused = Evm::open(genesis(), &dir).err().expect("refused");
            assert!(refused.contains(says), "{name}: {refused}");
        }
    }

    #[test]
    fn a_ledger_of_another_genesis_is_refused() {
        let dir = scratch("other");
        let c = Evm::open(genesis(), &dir).expect("open");
        land(&c, &[raw_transfer(0, c.chain_id(), who(9), U256::from(1u64))]);
        drop(c);

        let other = Genesis {
            alloc: vec![(who(7), lux_evm::Alloc::funded(U256::from(1u64)))],
            ..genesis()
        };
        let refused = Evm::open(other, &dir).err().expect("refused");
        assert!(refused.contains("not this genesis's chain"), "{refused}");
    }

    /// A block log exactly as hanzod 0.2.2 writes one: four big-endian bytes
    /// of length, then the block.
    fn write_hanzod(dir: &std::path::Path, blocks: &[Vec<u8>]) {
        let mut f = std::fs::File::create(dir.join("blocks")).expect("create");
        for raw in blocks {
            f.write_all(&(raw.len() as u32).to_be_bytes()).expect("length");
            f.write_all(raw).expect("block");
        }
    }

    #[test]
    fn a_block_log_hanzod_wrote_opens_and_its_receipts_are_rebuilt() {
        let lived = Evm::new(genesis());
        let hashes = history(&lived);
        let raws: Vec<Vec<u8>> = (1..=4)
            .map(|h| lived.get(&lived.block_id_at(h).expect("id")).expect("b").bytes())
            .collect();
        let dir = scratch("hanzod");
        write_hanzod(&dir, &raws);
        assert!(!dir.join("receipts").exists(), "hanzod writes no receipts");

        let c = Evm::open(genesis(), &dir).expect("open");
        assert_eq!(c.last_accepted(), lived.last_accepted());
        forget(&c);
        assert_eq!(picture(&c, &hashes), picture(&lived, &hashes));
        let written = std::fs::read(dir.join("receipts")).expect("rebuilt");
        drop(c);

        // And the file it wrote is the one it keeps: a second open agrees with
        // every record and rewrites nothing.
        let c = Evm::open(genesis(), &dir).expect("reopen");
        assert_eq!(std::fs::read(dir.join("receipts")).expect("receipts"), written);
        forget(&c);
        assert_eq!(picture(&c, &hashes), picture(&lived, &hashes));
    }

    #[test]
    fn receipts_that_disagree_with_execution_are_rewritten() {
        let dir = scratch("receipts");
        let c = Evm::open(genesis(), &dir).expect("open");
        let hashes = history(&c);
        let before = picture(&c, &hashes);
        drop(c);
        let good = std::fs::read(dir.join("receipts")).expect("receipts");

        // Keep the first record, then garbage in the shape of a record.
        let first = 4 + u32::from_be_bytes(good[..4].try_into().expect("four")) as usize;
        let mut bad = good[..first].to_vec();
        bad.extend_from_slice(&[0, 0, 0, 3, 0xc2, 0xc0, 0xc0]);
        std::fs::write(dir.join("receipts"), &bad).expect("damage");

        let c = Evm::open(genesis(), &dir).expect("reopen");
        forget(&c);
        assert_eq!(picture(&c, &hashes), before);
        assert_eq!(std::fs::read(dir.join("receipts")).expect("receipts"), good, "rewritten");
    }

    #[test]
    fn every_accepted_height_answers_after_it_leaves_memory() {
        // The window used to be the whole history: a block RETAINED behind the
        // tip was dropped while the transaction index still pointed at it.
        let c = Evm::new(genesis());
        let first = raw_transfer(0, c.chain_id(), who(9), U256::from(1u64));
        let hash = *TxEnvelope::decode_2718(&mut &first[..]).expect("tx").tx_hash();
        land(&c, &[first]);
        for n in 1..=RETAINED + 2 {
            land(&c, &[raw_transfer(n, c.chain_id(), who(9), U256::from(1u64))]);
        }
        let one = c.block_id_at(1).expect("id");
        assert!(!c.lock().blocks.contains_key(&one), "out of memory");

        let b = c.call("eth_getBlockByNumber", &json!(["0x1", false])).expect("block");
        assert_eq!(b["number"], json!("0x1"));
        let t = c.call("eth_getTransactionByHash", &json!([format!("{hash:#x}")])).expect("tx");
        assert_eq!(t["blockNumber"], json!("0x1"));
        let r = c.call("eth_getTransactionReceipt", &json!([format!("{hash:#x}")])).expect("r");
        assert_eq!(r["status"], json!("0x1"));
        assert_eq!(c.get(&c.block_id_at(0).expect("genesis")).expect("genesis").height(), 0);
    }

    // -- eth_getLogs ---------------------------------------------------------

    fn logs(c: &Evm, filter: Value) -> Vec<Value> {
        match c.call("eth_getLogs", &json!([filter])).expect("answered") {
            Value::Array(v) => v,
            other => panic!("not a list: {other}"),
        }
    }

    fn words(found: &[Value]) -> Vec<u8> {
        found
            .iter()
            .map(|l| {
                let data = l["data"].as_str().expect("data");
                u8::from_str_radix(&data[data.len() - 2..], 16).expect("word")
            })
            .collect()
    }

    #[test]
    fn get_logs_answers_with_gos_filter_semantics() {
        let c = Evm::new(genesis());
        let hashes = history(&c);
        let made = |nonce: u64| format!("{:#x}", who(7).create(nonce));
        let a = format!("{A:#x}");
        let b = format!("{B:#x}");
        let cc = format!("{C:#x}");

        let all = logs(&c, json!({"fromBlock": "0x0"}));
        assert_eq!(words(&all), vec![1, 2, 3, 4], "every log, in chain order");

        // Numbered across the BLOCK: block 2's two logs come from two
        // transactions and are 0 and 1.
        assert_eq!(all[1]["logIndex"], json!("0x0"));
        assert_eq!(all[2]["logIndex"], json!("0x1"));
        assert_eq!(all[2]["transactionIndex"], json!("0x1"));
        assert_eq!(all[2]["transactionHash"], json!(format!("{:#x}", hashes[3])));
        assert_eq!(all[2]["blockNumber"], json!("0x2"));
        assert_eq!(all[2]["address"], json!(made(3)));
        assert_eq!(all[2]["removed"], json!(false));
        let block2 = c.call("eth_getBlockByNumber", &json!(["0x2", false])).expect("block");
        assert_eq!(all[2]["blockHash"], block2["hash"]);
        // The same objects a receipt carries.
        let r = c
            .call("eth_getTransactionReceipt", &json!([format!("{:#x}", hashes[3])]))
            .expect("receipt");
        assert_eq!(r["logs"][0], all[2]);

        let by = |f: Value| words(&logs(&c, f));
        assert_eq!(by(json!({"blockHash": block2["hash"]})), vec![2, 3]);
        assert_eq!(by(json!({"fromBlock": "0x2", "toBlock": "0x3"})), vec![2, 3]);
        assert_eq!(by(json!({"fromBlock": "0x3", "toBlock": "0x3"})), Vec::<u8>::new());
        assert_eq!(by(json!({"fromBlock": "0x4", "toBlock": "0x400"})), vec![4], "clamped");
        assert_eq!(by(json!({"fromBlock": "0x9"})), Vec::<u8>::new(), "past the head");
        assert_eq!(by(json!({})), vec![4], "latest to latest");
        assert_eq!(by(json!({"fromBlock": "earliest", "toBlock": "pending"})), vec![1, 2, 3, 4]);

        assert_eq!(by(json!({"fromBlock": "0x0", "address": made(3)})), vec![3]);
        assert_eq!(by(json!({"fromBlock": "0x0", "address": [made(0), made(5)]})), vec![1, 4]);
        assert_eq!(by(json!({"fromBlock": "0x0", "address": []})), vec![1, 2, 3, 4]);
        assert_eq!(
            by(json!({"fromBlock": "0x0", "address": format!("{:#x}", who(9))})),
            Vec::<u8>::new()
        );

        let t = |topics: Value| by(json!({"fromBlock": "0x0", "topics": topics}));
        assert_eq!(t(json!([a])), vec![1, 2], "topic 0 is A");
        assert_eq!(t(json!([null, a])), vec![4], "topic 1 is A");
        assert_eq!(t(json!([[a, cc]])), vec![1, 2, 4], "topic 0 is A or C");
        assert_eq!(t(json!([[a, null]])), vec![1, 2, 3, 4], "a null in a list is anything");
        assert_eq!(t(json!([null, null])), vec![2, 4], "a position is a topic that must exist");
        assert_eq!(t(json!([a, b])), vec![2]);
        assert_eq!(t(json!([[], b])), vec![2], "an empty list is anything");
        assert_eq!(t(json!([b])), vec![3]);
        assert_eq!(t(json!([format!("{:#x}", B256::repeat_byte(1))])), Vec::<u8>::new());
        assert_eq!(t(json!([])), vec![1, 2, 3, 4]);
        assert_eq!(
            by(json!({"fromBlock": "0x0", "address": [made(2), made(5)], "topics": [null, a]})),
            vec![4],
            "address and topics together"
        );
    }

    #[test]
    fn get_logs_refuses_what_go_refuses() {
        let c = Evm::new(genesis());
        history(&c);
        let h = format!("{:#x}", B256::repeat_byte(7));
        let a = format!("{A:#x}");
        let refused = |f: Value| match c.call("eth_getLogs", &json!([f])) {
            Err(Error::BadRequest(why)) => why,
            other => panic!("{f} was answered: {other:?}"),
        };
        assert_eq!(
            refused(json!({"fromBlock": "0x3", "toBlock": "0x2"})),
            "invalid block range params"
        );
        assert!(refused(json!({"blockHash": h, "fromBlock": "0x1"})).contains("choose one"));
        assert_eq!(refused(json!({"blockHash": h})), "unknown block");
        assert_eq!(refused(json!({"topics": [a, a, a, a, a]})), "exceed max topics");
        assert!(refused(json!({"topics": [7]})).contains("invalid topic"));
        assert!(refused(json!({"address": "0x12"})).contains("not an address"));
        let wide = json!({"fromBlock": "0x0", "toBlock": format!("0x{:x}", LOGS_RANGE)});
        // The chain is shorter than the limit, so a wide range is clamped to it
        // first and answered.
        assert!(c.call("eth_getLogs", &json!([wide])).is_ok());
    }

    #[test]
    fn a_log_query_reads_no_more_than_its_budget() {
        let dir = scratch("budget");
        let c = Evm::open(genesis(), &dir).expect("open");
        history(&c);
        let every = Query::read(&json!({"fromBlock": "0x0"})).expect("a filter");
        let refused = c.logs_within(&every, 64).expect_err("more than 64 bytes");
        assert!(
            matches!(&refused, Error::BadRequest(why) if why.contains("narrow the range")),
            "{refused:?}"
        );
        let all = c.logs_within(&every, LOGS_READ_MAX).expect("answered");
        assert_eq!(words(all.as_array().expect("a list")), vec![1, 2, 3, 4]);

        // A block whose bloom cannot match costs its header: a filter nothing
        // matches is answered within what the headers take, less than the
        // blocks do.
        let files = c.files.as_ref().expect("on disk");
        let links = c.lock().chain.clone();
        let mut headers = c.first.raw.len() as u64;
        let mut blocks = headers;
        for (h, l) in links.iter().enumerate().skip(1) {
            let span = l.block.expect("recorded");
            headers += head(files, span, h as u64).expect("a header").1;
            blocks += u64::from(span.len);
        }
        assert!(headers < blocks);
        let t = format!("{:#x}", B256::repeat_byte(1));
        let nothing = Query::read(&json!({"fromBlock": "0x0", "topics": [t]})).expect("a filter");
        assert_eq!(c.logs_within(&nothing, headers).expect("answered"), json!([]));
    }

    #[test]
    fn a_flood_of_wide_log_queries_does_not_hold_up_the_chain() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::time::Instant;

        // Past what memory holds, so most of every scan is the disk.
        const BLOCKS: u64 = RETAINED + 88;
        let lived = Evm::new(genesis());
        let id = lived.chain_id();
        for k in 0..BLOCKS {
            land(&lived, &[raw_create(k, id, logging(1, &[A]))]);
        }
        let raws: Vec<Vec<u8>> = (1..=BLOCKS)
            .map(|h| lived.get(&lived.block_id_at(h).expect("id")).expect("b").bytes())
            .collect();
        drop(lived);
        let dir = scratch("flood");
        write_hanzod(&dir, &raws);
        let c = Arc::new(Evm::open(genesis(), &dir).expect("open"));

        let stop = Arc::new(AtomicBool::new(false));
        let floods: Vec<_> = (0..32)
            .map(|_| {
                let (c, stop) = (c.clone(), stop.clone());
                std::thread::spawn(move || {
                    let mut answered = 0u32;
                    let wide = json!([{"fromBlock": "0x0", "toBlock": "latest"}]);
                    while !stop.load(Ordering::Relaxed) {
                        match c.call("eth_getLogs", &wide) {
                            Ok(Value::Array(v)) => {
                                assert!(v.len() as u64 >= BLOCKS, "a log a block");
                                answered += 1;
                            }
                            Err(Error::BadRequest(why)) if why.contains("try again") => {}
                            other => panic!("{other:?}"),
                        }
                    }
                    answered
                })
            })
            .collect();

        // The chain goes on under it: each block built, verified, accepted
        // and on the device within the bound.
        let mut slowest = Duration::ZERO;
        for k in BLOCKS..BLOCKS + 20 {
            let t = Instant::now();
            land(&c, &[raw_create(k, id, logging(1, &[A]))]);
            slowest = slowest.max(t.elapsed());
        }
        stop.store(true, Ordering::Relaxed);
        let answered: u32 = floods.into_iter().map(|f| f.join().expect("a flood")).sum();
        assert!(answered > 0, "the scans ran");
        // Wall time, measured beside the rest of the suite, so the bound is
        // loose: what it catches is a block waiting out whole scans.
        assert!(slowest < Duration::from_secs(2), "a block took {slowest:?} under the flood");
        assert_eq!(c.height(), BLOCKS + 20);
    }

    #[test]
    fn get_logs_reads_blocks_that_left_memory() {
        let dir = scratch("logs");
        let c = Evm::open(genesis(), &dir).expect("open");
        history(&c);
        let all = logs(&c, json!({"fromBlock": "0x0"}));
        forget(&c);
        assert_eq!(logs(&c, json!({"fromBlock": "0x0"})), all);
        drop(c);
        let c = Evm::open(genesis(), &dir).expect("reopen");
        forget(&c);
        assert_eq!(logs(&c, json!({"fromBlock": "0x0"})), all);
    }

    // -- receipts ------------------------------------------------------------

    #[test]
    fn a_creation_receipt_names_the_contract_it_made() {
        let c = Evm::new(genesis());
        let hashes = history(&c);
        let r = c
            .call("eth_getTransactionReceipt", &json!([format!("{:#x}", hashes[0])]))
            .expect("receipt");
        assert_eq!(r["status"], json!("0x1"));
        assert_eq!(r["contractAddress"], json!(format!("{:#x}", who(7).create(0))));
        assert_eq!(r["to"], Value::Null);
        assert_eq!(r["logs"][0]["address"], r["contractAddress"], "the constructor logged");

        let transfer = c
            .call("eth_getTransactionReceipt", &json!([format!("{:#x}", hashes[1])]))
            .expect("receipt");
        assert_eq!(transfer["contractAddress"], Value::Null, "a call creates nothing");
    }

    #[test]
    fn a_fee_market_receipt_states_what_was_paid_not_the_cap() {
        let c = Evm::new(genesis());
        let hashes = history(&c);
        let one = c.block_id_at(1).expect("id");
        let base = c.lock().blocks[&one].header().base_fee_per_gas;
        let base = u128::from(base.expect("a base fee"));
        let (cap, tip) = (30_000_000_000u128, 1_000_000_000u128);
        assert!(base < cap, "the test needs a cap above the base fee");
        let paid = base + tip.min(cap - base);

        let r = c
            .call("eth_getTransactionReceipt", &json!([format!("{:#x}", hashes[1])]))
            .expect("receipt");
        assert_eq!(r["effectiveGasPrice"], json!(format!("0x{paid:x}")));
        assert_eq!(r["type"], json!("0x2"));
        let t =
            c.call("eth_getTransactionByHash", &json!([format!("{:#x}", hashes[1])])).expect("tx");
        assert_eq!(t["gasPrice"], json!(format!("0x{paid:x}")));
        assert_eq!(t["maxFeePerGas"], json!(format!("0x{cap:x}")));
        assert_eq!(t["maxPriorityFeePerGas"], json!(format!("0x{tip:x}")));
        assert_eq!(t["from"], json!(format!("{:#x}", who(7))));
        assert!(t["yParity"] == json!("0x0") || t["yParity"] == json!("0x1"));

        // A legacy transaction pays its price, whatever the base fee.
        let legacy = c
            .call("eth_getTransactionReceipt", &json!([format!("{:#x}", hashes[4])]))
            .expect("receipt");
        assert_eq!(legacy["effectiveGasPrice"], json!(format!("0x{:x}", 30_000_000_000u64)));
        assert_eq!(legacy["type"], json!("0x0"));
        let t =
            c.call("eth_getTransactionByHash", &json!([format!("{:#x}", hashes[4])])).expect("tx");
        let v = u64::from_str_radix(t["v"].as_str().expect("v").trim_start_matches("0x"), 16)
            .expect("v");
        let chain = c.chain_id();
        assert!(v == 35 + 2 * chain || v == 36 + 2 * chain, "EIP-155 v, got {v}");
    }

    #[test]
    fn the_pending_nonce_counts_what_is_waiting() {
        let c = Evm::new(genesis());
        let me = format!("{:#x}", who(7));
        let nonce = |tag: &str| c.call("eth_getTransactionCount", &json!([me, tag])).expect("n");
        let id = c.chain_id();
        let first = raw_transfer(0, id, who(9), U256::from(1u64));
        c.submit(&first).expect("0");
        c.submit(&raw_transfer(1, id, who(9), U256::from(1u64))).expect("1");
        c.submit(&raw_transfer(3, id, who(9), U256::from(1u64))).expect("3, after a gap");
        assert_eq!(nonce("latest"), json!("0x0"));
        assert_eq!(nonce("pending"), json!("0x2"), "0 and 1 wait; 3 cannot run before 2");

        let hash = *TxEnvelope::decode_2718(&mut &first[..]).expect("tx").tx_hash();
        let waiting =
            c.call("eth_getTransactionByHash", &json!([format!("{hash:#x}")])).expect("waiting");
        assert_eq!(waiting["blockHash"], Value::Null, "known, and in no block yet");

        // Built and preferred, not yet accepted: the transactions left the
        // pool, and the answer still counts them.
        let b = c.build().expect("builds");
        c.verify(&b.id()).expect("verifies");
        c.set_preference(&b.id()).expect("prefers");
        assert_eq!(nonce("latest"), json!("0x0"));
        assert_eq!(nonce("pending"), json!("0x2"));
        c.accept(&b.id()).expect("accepts");
        assert_eq!(nonce("latest"), json!("0x2"));
    }

    // -- the cost of a restart ----------------------------------------------

    /// Bytes this process's allocator has handed out and not had back.
    fn heap() -> usize {
        // SAFETY: mallinfo2 reads the allocator's own counters and takes no
        // arguments.
        let m = unsafe { libc::mallinfo2() };
        m.uordblks + m.hblkhd
    }

    /// Build `n` one-transfer blocks in memory, write them as a ledger, and
    /// time opening it twice: the first open also writes the receipts file,
    /// the second only checks it.
    fn replay(n: u64, recipients: u64) {
        let lived = Evm::new(genesis());
        let id = lived.chain_id();
        let t = std::time::Instant::now();
        for k in 0..n {
            let to = Address::with_last_byte(1).create(k % recipients);
            land(&lived, &[raw_transfer(k, id, to, U256::from(1u64))]);
        }
        let built = t.elapsed();
        let raws: Vec<Vec<u8>> = (1..=n)
            .map(|h| lived.get(&lived.block_id_at(h).expect("id")).expect("block").bytes())
            .collect();
        let accounts = lived.lock().world.len();
        drop(lived);

        let dir = scratch("replay");
        write_hanzod(&dir, &raws);
        let size = std::fs::metadata(dir.join("blocks")).expect("blocks").len();
        for pass in ["first open", "second open"] {
            let before = heap();
            let t = std::time::Instant::now();
            let c = Evm::open(genesis(), &dir).expect("open");
            let took = t.elapsed();
            assert_eq!(c.height(), n);
            let held = heap().saturating_sub(before) / 1024;
            let index = {
                let h = c.lock();
                h.chain.capacity() * std::mem::size_of::<Link>()
                    + h.heights.capacity() * (32 + 8 + 1)
                    + h.located.capacity() * (32 + 12 + 1)
            };
            println!(
                "{n} blocks, {accounts} accounts, {size} B of blocks: {pass} {took:.2?} ({:.1} us/block); holds {held} kB, of it the index {} kB ({} B/block)",
                took.as_secs_f64() * 1e6 / n as f64,
                index / 1024,
                index as u64 / n,
            );
            drop(c);
        }
        println!("  (building them took {built:.2?})");
    }

    #[test]
    #[ignore = "a measurement: cargo test --release -- --ignored --nocapture replay_"]
    fn replay_ten_thousand_blocks_over_a_small_world() {
        replay(10_000, 16);
    }

    #[test]
    #[ignore = "a measurement: cargo test --release -- --ignored --nocapture replay_"]
    fn replay_ten_thousand_blocks_over_a_growing_world() {
        replay(10_000, 10_000);
    }
}
