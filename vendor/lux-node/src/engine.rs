// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! Turning a block into an accepted block.
//!
//! This is the join. [`crate::evm`] produces a block and the state root that
//! running it actually left behind; [`crate::host`] carries proposals and
//! votes; `lux-consensus` decides when there are enough of them. The engine is
//! what puts the first into the rest — and the ONE line that matters is
//! [`Engine::position`], where `execution_state_root` is filled in from the
//! block the interpreter produced.
//!
//! A certificate over a root nobody computed certifies nothing. A node that
//! votes on a constant has a working consensus and no chain. So the root a
//! validator signs here is the one revm arrived at, and a peer's block is
//! re-executed before this node will sign for it — [`Vm::verify`] refuses a
//! header whose claimed root is not the one it computes.
//!
//! THE RULE IS GO'S (`luxfi/consensus engine/chain`, B1: the α-of-K certificate
//! is the only decider, `b1_cert_no_viewchange_test.go`), one clause at a time:
//!
//! - ONE PROPOSER A WINDOW. [`crate::proposer::Windower`] names who may build at
//!   each slot of a height; a proposal is signed and refused from anyone
//!   else's slot (proposervm `verifyPostDurangoBlockDelay`, block.go:474).
//!   A validator builds only in its own window and only while no block is
//!   live or signed at the height — the next window's proposer builds when the
//!   one before it went silent. Every block a slot's proposer signed that
//!   executes is live, two from one proposer included: Go counts every live
//!   sibling. Bounded per proposer and per height, the lowest ids are kept.
//! - ONE SIGNATURE PER HEIGHT. The journal is the height-only non-equivocation
//!   guard (`reserveSlotForSign`, engine.go:2489; `SlotKey{Height}`,
//!   vote_guard.go:60): a validator signs one block at a height, before or
//!   after a restart, and never a second.
//! - THE CONVERGED WINNER. A validator does not bind its one signature to the
//!   block it built or saw first. Once no new block has arrived at the height
//!   for a settle window, it signs the lowest block id among ALL the live blocks
//!   it holds there (`snapshotVotableSlotsLocked`, engine.go:2922;
//!   `convergedWinnerAtHeightLocked`, engine.go:2829; `emitConvergedVote`,
//!   integration.go:1002). Every honest validator holding the same blocks signs
//!   the same one, and α of K signatures make one certificate. One clause is
//!   this node's and not Go's: a block the verified votes already shut out of
//!   α — its votes plus every validator not yet heard from fall short — is
//!   passed over. Go's winner is vote-blind, so a seat returning to a height
//!   three others signed can sign its own block there and halt it.
//! - CLONE RECOVERY. A validator that restarts with a signature already bound
//!   at the height says that SAME statement again — never a fresh winner
//!   (integration.go:1018) — and offers the block again: the block a validator
//!   signs is on the device before its signature is ([`crate::held`]).
//! - THE RETRY. On a poll timeout, doubling from twice the settle window to
//!   sixteen seconds, a validator says its standing vote again and a proposer
//!   pushes its block again, so one validator that missed them cannot hold the
//!   height; after eight tries it asks its peers for the certified block
//!   instead (`rePollAllPending`, engine.go:1784).
//!
//! WHY IT IS SAFE: two certificates at one height would need α signers each,
//! so 2α − n > f signers in both — an honest validator signing twice at one
//! height, which the journal refuses (vote_guard.go:60). The winner rule and
//! the retry only decide which block an honest validator signs, never whether
//! it signs twice.
//!
//! WHAT IT CANNOT DO, AND GO CANNOT EITHER: a height whose honest validators
//! are already bound to different blocks, none of them able to reach α, stays
//! uncertified — the price of the one-signature guarantee. Go names it: a
//! split vote "under one-signature-per-height, is unrecoverable"
//! (`convergenceSettleWindow`, engine.go:1716, luxfi/consensus 5ba68171); its
//! note on deleting the round layer (d13b4559) moves the answer to Nova, the
//! majority-accept tier (a664c825), which this node does not run — it accepts
//! on the Quasar certificate alone. The settle window is what keeps honest
//! validators out of it; a proposer that signs different blocks to different
//! halves of the committee, past what relaying evens out, can still put a
//! height there. So can a height v0.2 left bound to blocks it kept only in
//! memory: nobody holds them after the restart, and a signature bound to a
//! block nobody holds can never be moved.
//!
//! A NODE THAT FELL BEHIND IS NOT VOTED BACK IN. Nobody re-votes a decided
//! height, so a validator that was down while the others moved on is handed
//! each block it missed together with the certificate the network already
//! formed for it, and accepts on that — [`Engine::catch_up`]. The certificate is
//! checked by the same rule a live one is, against this node's own committee,
//! before the chain is shown a byte of the block; a block without one is never
//! accepted, and never parsed.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use blst::min_pk::SecretKey;
use lux_consensus::cert::{NodeId, QuorumCert, ValidatorSet};
use lux_consensus::finality::{canonical_vote_message, Finality, Position, EMPTY};
use lux_consensus::vote::{SignedVote, VoteTransport as _};
use lux_consensus::Tally;
use sha2::{Digest, Sha256};

use crate::gate::Gate;
use crate::held::Held;
use crate::host::Host;
use crate::journal::Journal;
use crate::ledger::{self, Ledger};
use crate::mesh::{Proposal, Record, BATCH_MAX};
use crate::pq;
use crate::proposer::{self, Windower};
use crate::vm::{Block, Error as VmError, Id, Vm};

/// The longest a catch-up waits on one round of questions, all peers asked at
/// once.
const ASK_WAIT: Duration = Duration::from_secs(2);

/// A proposer's window when [`Engine::with_window`] does not say.
pub const WINDOW: Duration = Duration::from_secs(1);

/// The settle window's bounds: Go's clamp (`convergenceSettleWindow`,
/// engine.go:1699) — long enough for sibling blocks to gossip in, short
/// enough not to stall a height.
pub const SETTLE_MIN: Duration = Duration::from_millis(150);
pub const SETTLE_MAX: Duration = Duration::from_secs(2);

/// Go's `maxRePollBackoff` and `maxRePollAttempts` (engine.go:1574-1588).
pub const RETRY_MAX: Duration = Duration::from_secs(16);
pub const RETRY_ATTEMPTS: u32 = 8;

/// Slots past its own a validator takes a proposal for: one, for the spread
/// between validators' clocks. A proposal from further ahead is not yet in its
/// window; its proposer pushes it again.
pub const EARLY: u64 = 1;

/// The most blocks held live at one height, and from one proposer. One is
/// the ordinary case: a proposer builds once a height. Past either bound the
/// HIGHEST id goes, never the lowest, so no validator drops the block the
/// others converge on — and one proposer's blocks never crowd out another's.
pub const SIBLINGS_MAX: usize = 64;
pub const EACH: usize = 16;

/// Refused frames — not a block on the tip, or one that did not run here —
/// taken from one proposer on one link in a settle window. Past it, that
/// proposer is not looked at on that link until the window turns; what honest
/// peers relay of it still is. The block is refused, never the proposer: a
/// failure can be this node's own (a chain link timing out).
pub const JUNK_EACH: u32 = 16;

/// Refused frames remembered at one height. Past it a refused frame is not
/// remembered, and costs one of [`JUNK_EACH`] each time it comes.
pub const REFUSED_MAX: usize = 256;

/// Windows a proposal's slot may run ahead of this validator's own wall-clock
/// slot and still be taken. Go's windows are five seconds with seconds of
/// skew between them; this node's are one, so the skew is counted in windows.
pub const SKEW: u64 = 2;

/// Signature checks a link is given, each settle window, on votes for later
/// heights. A peer that raises the height on every vote buys this many.
pub const AHEAD_EACH: u32 = 4;

/// The most arrivals kept from one link for the height after this one.
pub const LATER_EACH: usize = 16;

/// Why a block did not become an accepted block.
#[derive(Debug)]
pub enum Failed {
    /// The chain refused it.
    Chain(VmError),
    /// The votes are in and they do not make a certificate.
    NoCert(String),
    /// A record did not reach the device. Before a block: its certificate,
    /// so the block was not accepted — a block this node could never prove is
    /// not one it takes. After one: the lifting of the import gate, so the
    /// gate still stands until it is lifted again.
    NotDurable(std::io::Error),
}

impl core::fmt::Display for Failed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Failed::Chain(e) => write!(f, "the chain refused the block: {e}"),
            Failed::NoCert(why) => write!(f, "no certificate: {why}"),
            Failed::NotDurable(e) => write!(f, "a record did not reach the device: {e}"),
        }
    }
}

impl std::error::Error for Failed {}

impl From<VmError> for Failed {
    fn from(e: VmError) -> Self {
        Failed::Chain(e)
    }
}

/// What a catch-up did.
#[derive(Debug, Default)]
pub struct Caught {
    /// Blocks accepted, each on a certificate this node verified.
    pub accepted: u64,
    /// The highest height a peer said it had accepted. A claim, not a proof.
    pub reported: u64,
    /// The peers not believed, and why. What a peer served before the record
    /// it lied in stood on its own certificates and stays accepted.
    pub refused: Vec<(NodeId, String)>,
}

/// What one [`Engine::step`] did that is worth saying.
#[derive(Debug)]
pub enum Event {
    /// The block at `height` was accepted on a certificate. `slot` is the
    /// window it was proposed in, when this validator saw the proposal;
    /// `built` when this validator built it.
    Decided { height: u64, slot: Option<u64>, cert: Box<QuorumCert>, built: bool },
    /// A retry fell due with the height still open: `most` votes on one block
    /// of the `need`, over `blocks` live blocks.
    Waiting { height: u64, slot: u64, most: usize, need: usize, blocks: usize },
    /// This node asked its peers for what it missed.
    Caught(Caught),
    /// Something was refused, and why.
    Refused(String),
}

/// One pass of [`Engine::step`].
#[derive(Debug, Default)]
pub struct Step {
    pub events: Vec<Event>,
    /// Nothing arrived from a peer.
    pub quiet: bool,
}

/// Something a peer sent about some height.
#[derive(Clone, Debug)]
enum Arrival {
    Proposal(Proposal),
    Vote(Box<SignedVote>),
}

/// A block live at the height: executed, and what it was proposed as.
struct Sibling {
    proposal: Proposal,
    /// The frame's digest: a relayed copy of it is not looked at again.
    frame: [u8; 32],
    /// The validator whose window it was proposed in.
    proposer: NodeId,
    own: bool,
}

/// The height being decided.
struct Instance {
    height: u64,
    /// The accepted block this height builds on.
    tip: Id,
    /// When this validator's slots at the height count from: when it took the
    /// height up, or earlier, once a proposal that ran showed the network
    /// further on — never past this validator's own wall-clock slot.
    began: Instant,
    /// The tip's timestamp, seconds since the epoch: the wall-clock slot is
    /// counted from it, as Go counts a proposer's slot from its parent's.
    opened: u64,
    /// The live blocks at the height, by id — so the first is the lowest.
    siblings: BTreeMap<Id, Sibling>,
    /// When a block last arrived here, kept or not: the settle window runs
    /// from it (`snapshotVotableSlotsLocked` settles from the latest sibling,
    /// and Go tracks every one).
    offered: Option<Instant>,
    /// Until when an arriving block restarts the settle window: [`SETTLE_MAX`]
    /// after the first arrived, one deadline for the height and every
    /// proposer in it. Honest siblings gossip in well inside it; past it,
    /// blocks arriving are a proposer streaming, and the vote is not held.
    closes: Option<Instant>,
    /// Refused frames left to each (proposer, link) this settle window.
    junk: BTreeMap<(NodeId, NodeId), (u32, Instant)>,
    /// Frames refused here, by digest, and until when: for the rest of the
    /// height when the refusal is the block's own; when the chain could not be
    /// asked, until a wait that doubles from a settle window to [`RETRY_MAX`].
    refused: BTreeMap<[u8; 32], (Option<Instant>, Duration)>,
    /// Each validator's signature at this height: the first stands.
    votes: BTreeMap<NodeId, SignedVote>,
    /// This validator's own, once it has signed here.
    bound: Option<SignedVote>,
    /// A block this validator built here.
    built: Option<Id>,
    /// When a build last found nothing to put in a block.
    empty: Option<Instant>,
    /// When a statement last did not reach the device.
    stalled: Option<Instant>,
    /// When the retry was last brought forward, and whether a push is owed
    /// ahead of the schedule — one that counts as no attempt.
    hurried: Option<Instant>,
    push: bool,
    /// The retry: when it is next due, the window after that, and how many.
    retry: Instant,
    backoff: Duration,
    attempts: u32,
}

/// A validator driving one chain.
///
/// Every member of a committee weighs one ([`Committee::read`]), so counting
/// signers is counting the stake a certificate is checked against.
pub struct Engine {
    node: NodeId,
    key: SecretKey,
    set: ValidatorSet,
    members: usize,
    /// Each member's ML-DSA-65 key: what a proposal is checked against.
    identities: BTreeMap<NodeId, Vec<u8>>,
    epoch: u64,
    /// The 32-byte identity of the chain being driven.
    chain: Id,
    /// A commitment to the validator set that produced a vote.
    set_root: Id,
    /// Who proposes at each height and slot.
    windower: Windower,
    /// A slot's length on this validator's clock.
    window: Duration,
    /// The heights this node accepted without a certificate, when there are
    /// any. Closed by an import and lifted by a peer's certificate; while it
    /// stands this validator builds nothing and signs nothing.
    gate: Arc<Gate>,
    /// Where the certificate of every block this node accepts is kept, so a
    /// peer that fell behind can be served it.
    ledger: Option<Arc<Ledger>>,
    /// The block this validator signed, kept until its height is decided.
    held: Held,
    /// The height in progress.
    at: Option<Instance>,
    /// What arrived for the height after it, by the link it came in on — a
    /// peer fills its own share and nobody else's.
    later: BTreeMap<NodeId, Vec<Arrival>>,
    /// The highest height past this one each validator signed a vote at, from
    /// votes whose signature checked.
    heights: BTreeMap<NodeId, u64>,
    /// Signature checks left to each link this settle window on votes for
    /// later heights, and when the window began.
    checks: BTreeMap<NodeId, (u32, Instant)>,
    /// Links the mesh has taken so far: a new one is a peer that may have
    /// missed everything said at this height.
    joined: usize,
    /// When this node last asked its peers for what it missed.
    asked: Option<Instant>,
    /// When the import gate was last looked at.
    looked: Option<Instant>,
}

impl Engine {
    pub fn new(
        node: NodeId,
        key: SecretKey,
        committee: &Committee,
        epoch: u64,
        chain: Id,
    ) -> Result<Self, String> {
        let weights: Vec<(NodeId, u64)> =
            committee.members().iter().map(|m| (m.node, m.weight)).collect();
        Ok(Self {
            node,
            key,
            set: committee.set()?,
            members: committee.len(),
            identities: committee.members().iter().map(|m| (m.node, m.identity.clone())).collect(),
            epoch,
            chain,
            set_root: committee.root(),
            windower: Windower::new(&chain, &weights)?,
            window: WINDOW,
            gate: Arc::new(Gate::memory()),
            ledger: None,
            held: Held::memory(),
            at: None,
            later: BTreeMap::new(),
            heights: BTreeMap::new(),
            checks: BTreeMap::new(),
            joined: 0,
            asked: None,
            looked: None,
        })
    }

    /// Keep the certificate of every block this node accepts in `ledger`,
    /// written before the block is accepted.
    pub fn with_ledger(mut self, ledger: Arc<Ledger>) -> Self {
        self.ledger = Some(ledger);
        self
    }

    /// Keep the block this validator signs in `held` — `<data>/held` — before
    /// it signs, so a restart can offer it again.
    pub fn with_held(mut self, held: Held) -> Self {
        self.held = held;
        self
    }

    /// Hold this validator to `gate`, the record of what it imported.
    ///
    /// An RLP export carries blocks and no certificates, so a node that read
    /// one has an accepted chain nothing agreed to. Go's proposervm reaches the
    /// same conclusion from the shape of its own database — an inner tip with
    /// no outer envelope beneath it — and gates block production off until the
    /// missing state "is rebuilt from certified peer state". This is that gate,
    /// stated where it can be seen rather than inferred, and kept in the data
    /// directory, so a restart without the import flag is still gated.
    pub fn with_gate(mut self, gate: Arc<Gate>) -> Self {
        self.gate = gate;
        self
    }

    /// Let a proposer's window last `window`. Every validator of a network
    /// runs the same one. The settle window is a fifth of it, within Go's
    /// clamp, and the retry starts at twice the settle window — Go's round
    /// budget, of which its settle window is half.
    pub fn with_window(mut self, window: Duration) -> Self {
        self.window = window.max(Duration::from_millis(1));
        self
    }

    fn settle(&self) -> Duration {
        (self.window / 5).clamp(SETTLE_MIN, SETTLE_MAX)
    }

    /// How often a proposer with nothing to build looks again: transactions
    /// arrive in bursts, and one that looked only at the start of its window
    /// would hold everything that came after.
    fn poll(&self) -> Duration {
        self.window / 100
    }

    /// Whether this node may act as a caught-up validator.
    ///
    /// False after an import and until a peer's certificate anchors it. Nothing
    /// else moves it: a node cannot decide on its own that the history it read
    /// off a file is the history the network agreed to.
    pub fn caught_up(&self) -> bool {
        self.gate.held().is_none()
    }

    pub fn set(&self) -> &ValidatorSet {
        &self.set
    }

    pub fn node(&self) -> NodeId {
        self.node
    }

    /// How many distinct signatures a Quasar certificate needs from this set.
    pub fn quorum(&self) -> usize {
        lux_consensus::finality::signer_floor(Finality::Quasar, self.set.len() as i64).max(0)
            as usize
    }

    /// The validator whose window is `slot` at `height`.
    pub fn proposer(&self, height: u64, slot: u64) -> NodeId {
        self.windower.expected(height, slot)
    }

    /// The height in progress and this validator's slot in it, once
    /// [`Engine::step`] has begun one.
    pub fn slot(&self) -> Option<(u64, u64)> {
        self.at.as_ref().map(|i| (i.height, self.slot_of(i)))
    }

    fn slot_of(&self, at: &Instance) -> u64 {
        let n = at.began.elapsed().as_nanos() / self.window.as_nanos();
        u64::try_from(n).unwrap_or(u64::MAX)
    }

    /// The statement a validator makes about `block`.
    ///
    /// `execution_state_root` is the block's own — the root the interpreter
    /// arrived at after running the block's transactions. That single field is
    /// the difference between a consensus harness and a chain: everything else
    /// here is identity and position, and this is the only part that says what
    /// the world looks like.
    pub fn position(&self, block: &dyn Block) -> Position {
        Position {
            chain_id: self.chain,
            height: block.height(),
            // One statement per height (vote_guard.go:60), so there is no
            // round for it to name. Round 0 is what every statement says.
            round: 0,
            block_id: block.id(),
            parent_id: block.parent(),
            // The block IS its own canonical identity here. Go's proposervm
            // wraps an inner block and the canonical id is the inner one's, so
            // two wrappers of one block collapse rather than fork; with no
            // wrapper the two are the same value.
            canonical_id: block.id(),
            parent_canonical_id: block.parent(),
            execution_state_root: block.state_root(),
            payload_root: block.payload_root(),
            validator_set_root: self.set_root,
        }
    }

    /// One pass: take what arrived, propose if this validator's window is
    /// open and nothing is live, sign the converged winner once the height has
    /// settled, accept a block once a certificate stands for it, and retry
    /// what is due.
    ///
    /// Never waits on the network: a pass is as long as what it does, so one
    /// thread drives it in a loop and nothing a peer sends — or does not send
    /// — holds it.
    pub fn step(&mut self, vm: &dyn Vm, host: &Host, votes: &mut Journal) -> Step {
        let mut out = Step::default();
        let tip = vm.last_accepted();
        let height = match vm.get(&tip) {
            Ok(b) => b.height() + 1,
            Err(e) => {
                out.events.push(Event::Refused(format!("the chain has no tip: {e}")));
                return out;
            }
        };
        let mut arrivals: Vec<(NodeId, Arrival)> = Vec::new();
        let mut at = match self.at.take() {
            Some(i) if i.tip == tip => i,
            _ => {
                for (link, kept) in std::mem::take(&mut self.later) {
                    arrivals.extend(kept.into_iter().map(|a| (link, a)));
                }
                self.heights.retain(|_, h| *h > height);
                self.enter(height, tip, vm, host, votes, &mut out)
            }
        };
        let mesh = host.mesh();
        arrivals.extend(mesh.proposals().into_iter().map(|p| (p.from, Arrival::Proposal(p))));
        arrivals.extend(mesh.drain().into_iter().map(|(l, v)| (l, Arrival::Vote(Box::new(v)))));
        out.quiet = arrivals.is_empty();
        for (link, a) in arrivals {
            self.arrive(&mut at, link, a, vm, host, &mut out);
        }
        // A peer came onto the mesh: what this validator said and holds here
        // goes out now, not a backed-off retry later.
        let joined = mesh.joined();
        if joined != self.joined {
            self.joined = joined;
            self.hurry(&mut at);
        }

        // A statement that did not reach the device is not tried again at
        // once: a fifth of a window passes first. And an imported chain is not
        // an agreed chain: while the gate stands this node builds nothing and
        // signs nothing. It still executes, so it can accept what the others
        // certify — which is what lifts the gate.
        let calm = at.stalled.is_none_or(|t| t.elapsed() >= self.window / 5);
        if self.caught_up() && calm {
            self.offer(&mut at, vm, host, &mut out);
            self.vote(&mut at, vm, host, votes, &mut out);
        }
        let lost = calm && self.decide(&mut at, vm, &mut out);
        let exhausted = self.retry(&mut at, host, &mut out);
        let settled = at.began.elapsed() >= self.window / 2;
        self.at = Some(at);
        self.behind(vm, host, height, lost || exhausted, settled, &mut out);
        if !self.caught_up() && self.looked.is_none_or(|t| t.elapsed() >= self.window) {
            self.looked = Some(Instant::now());
            if let Err(e) = self.anchored(vm) {
                out.events.push(Event::Refused(format!("lifting the import gate: {e}")));
            }
        }
        out
    }

    /// Take `height` up on `tip`. A signature this validator already made here
    /// — in an earlier run, or by v0.2.x — is bound, and said again as it was:
    /// Go's clone recovery re-signs the durable binding, never a fresh winner.
    /// The block it signed is offered again too, when this node kept it.
    fn enter(
        &mut self,
        height: u64,
        tip: Id,
        vm: &dyn Vm,
        host: &Host,
        votes: &mut Journal,
        out: &mut Step,
    ) -> Instance {
        if let Err(e) = self.held.decided(height) {
            out.events.push(Event::Refused(format!("letting go of a decided block: {e}")));
        }
        let slot = lux_consensus::vote::Slot { chain: self.chain, height, round: 0 };
        let bound = votes
            .standing(&slot)
            .map(<[u8]>::to_vec)
            .and_then(|m| host.resend(&m, &self.key, votes).ok());
        let mut own = BTreeMap::new();
        if let Some(v) = &bound {
            own.insert(self.node, v.clone());
        }
        let base = self.settle() * 2;
        let mut at = Instance {
            height,
            tip,
            began: Instant::now(),
            opened: vm.get(&tip).map_or(0, |b| b.timestamp()),
            siblings: BTreeMap::new(),
            offered: None,
            closes: None,
            junk: BTreeMap::new(),
            refused: BTreeMap::new(),
            votes: own,
            bound,
            built: None,
            empty: None,
            stalled: None,
            hurried: None,
            push: false,
            retry: Instant::now() + base,
            backoff: base,
            attempts: 0,
        };
        let Some(id) = at.bound.as_ref().map(|v| v.position.signed_identity()) else {
            return at;
        };
        let kept = self
            .held
            .get(height)
            .map(|p| Proposal { from: self.node, ..p })
            .filter(|p| vm.parse(&p.block).is_ok_and(|b| b.id() == id));
        match kept {
            Some(p) => {
                let who = self.proposer(height, p.slot);
                if vm.verify(&id).is_ok() {
                    host.mesh().propose(&p);
                    let frame = digest(&p);
                    self.live(
                        &mut at,
                        id,
                        Sibling { proposal: p, frame, proposer: who, own: false },
                    );
                }
            }
            None => out.events.push(Event::Refused(format!(
                "height {height}: this validator signed block {} and kept no copy of it; \
                 the height waits on a validator that did",
                short(&id)
            ))),
        }
        at
    }

    /// Take one arrival: into this height, held for the next one, or noted as
    /// a sign this node is behind. `link` is the peer it came in on — proven
    /// by the handshake — and nothing an arrival says about who made it is
    /// believed before its signature checks.
    fn arrive(
        &mut self,
        at: &mut Instance,
        link: NodeId,
        a: Arrival,
        vm: &dyn Vm,
        host: &Host,
        out: &mut Step,
    ) {
        if !self.set.contains(&link) {
            return;
        }
        match a {
            Arrival::Proposal(p) if p.height == at.height => self.proposed(at, p, vm, host, out),
            Arrival::Proposal(p) if p.height == at.height + 1 => {
                self.later_from(link, Arrival::Proposal(p))
            }
            Arrival::Vote(v) if v.position.height == at.height => self.voted(at, *v),
            Arrival::Vote(v) if v.position.height > at.height => {
                let height = v.position.height;
                let next = height == at.height + 1;
                // A signer already heard this far on says nothing new — unless
                // it is a vote to keep for the next height.
                if !next && self.heights.get(&v.node).is_some_and(|h| *h >= height) {
                    return;
                }
                let settle = self.settle();
                if !spend(&mut self.checks, link, AHEAD_EACH, settle) || !self.signed(&v) {
                    return;
                }
                let h = self.heights.entry(v.node).or_insert(0);
                *h = (*h).max(height);
                if next {
                    self.later_from(link, Arrival::Vote(v));
                }
            }
            _ => {}
        }
    }

    /// Keep an arrival for the next height, in `link`'s own share.
    fn later_from(&mut self, link: NodeId, a: Arrival) {
        let kept = self.later.entry(link).or_default();
        if kept.len() < LATER_EACH {
            kept.push(a);
        }
    }

    /// Whether `v` is an accepting vote on this chain that its signer signed.
    fn signed(&self, v: &SignedVote) -> bool {
        v.accept
            && v.position.chain_id == self.chain
            && v.position.round == 0
            && v.verify(&self.set, self.epoch)
    }

    /// A proposal: signed by its slot's proposer, each live frame looked at
    /// once. Every one that is signed and executes is live — two a proposer
    /// signed are two siblings, and the lower is the winner on every validator
    /// holding both (`convergedWinnerAtHeightLocked` counts every live
    /// sibling, engine.go:2829) — within [`EACH`] a proposer and
    /// [`SIBLINGS_MAX`] a height, the lowest kept. A block taken up is relayed,
    /// so a peer its proposer never reached, or sent another one, holds it too.
    ///
    /// A slot ahead of this validator's clock is taken when its own wall clock
    /// since the tip's timestamp says the slot can have opened, within
    /// [`SKEW`] windows — Go reads a proposer's slot off the block's time
    /// against its parent's, never off a local start. Once such a block has
    /// run, the clock moves up to it, and never past its own wall-clock slot:
    /// a validator restarted into a quiet height counts from the tip, like the
    /// others, and nothing a peer sends puts it ahead of its own clock.
    fn proposed(&self, at: &mut Instance, p: Proposal, vm: &dyn Vm, host: &Host, out: &mut Step) {
        let who = self.proposer(at.height, p.slot);
        let link = p.from;
        let settle = self.settle();
        if at.junk.get(&(who, link)).is_some_and(|(left, t)| *left == 0 && t.elapsed() < settle) {
            return;
        }
        let frame = digest(&p);
        let now = Instant::now();
        if at.siblings.values().any(|s| s.frame == frame)
            || at.refused.get(&frame).is_some_and(|(until, _)| until.is_none_or(|u| now < u))
        {
            return;
        }
        let wall = self.wall(at);
        let ahead = p.slot > self.slot_of(at).saturating_add(EARLY);
        if ahead && p.slot.saturating_sub(EARLY) > wall.saturating_add(SKEW) {
            return;
        }
        let Some(identity) = self.identities.get(&who) else { return };
        let header = proposer::header(
            &self.chain,
            &self.set_root,
            &at.tip,
            at.height,
            p.slot,
            identity,
            &p.block,
        );
        // Not that slot's proposer's: not believed, not parsed, not executed,
        // and not logged — a peer that could make every forgery a log line
        // could fill the disk with them.
        if pq::verify(identity, &header, proposer::CONTEXT, &p.signature).is_err() {
            return;
        }
        let id = match vm.parse(&p.block) {
            Ok(b) if b.height() == at.height && b.parent() == at.tip => b.id(),
            Ok(_) => {
                return self.refuse(
                    at,
                    who,
                    link,
                    frame,
                    &VmError::Invalid("not on the tip".into()),
                    out,
                )
            }
            Err(e) => return self.refuse(at, who, link, frame, &e, out),
        };
        if at.siblings.contains_key(&id) {
            return;
        }
        // No room: let go — a block the votes come to name is taken when it is
        // pushed again. It still arrived, and the settle window restarts.
        let Some(displaced) = self.room(at, who, id) else {
            let _ = vm.reject(&id);
            return self.stir(at);
        };
        match vm.verify(&id) {
            Ok(()) => {}
            // Let go by the chain before it ran: looked at again when pushed.
            Err(VmError::NotFound) => return,
            Err(e) => return self.refuse(at, who, link, frame, &e, out),
        }
        if let Some(old) = displaced {
            at.siblings.remove(&old);
            let _ = vm.reject(&old);
        }
        if ahead {
            let to = u32::try_from(p.slot.saturating_sub(EARLY).min(wall)).unwrap_or(u32::MAX);
            let behind = self.window.checked_mul(to);
            if let Some(then) = behind.and_then(|d| Instant::now().checked_sub(d)) {
                at.began = at.began.min(then);
            }
        }
        host.mesh().propose(&p);
        self.live(at, id, Sibling { proposal: p, frame, proposer: who, own: false });
    }

    /// This validator's own wall-clock slot at the height: windows since the
    /// tip's timestamp.
    fn wall(&self, at: &Instance) -> u64 {
        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        let since = now.saturating_sub(Duration::from_secs(at.opened));
        u64::try_from(since.as_nanos() / self.window.as_nanos()).unwrap_or(u64::MAX)
    }

    /// Whether block `id` from `who` has room at this height, and which block
    /// it displaces: past [`EACH`] of that proposer's, or [`SIBLINGS_MAX`] in
    /// all, the highest id that nothing holds here — not this validator's own
    /// block, and not one a verified vote names — and only for a lower one. A
    /// block a verified vote names always has room. None: no room.
    fn room(&self, at: &Instance, who: NodeId, id: Id) -> Option<Option<Id>> {
        let theirs = at.siblings.values().filter(|s| s.proposer == who).count();
        let full: Vec<Id> = if theirs >= EACH {
            at.siblings.iter().filter(|(_, s)| s.proposer == who).map(|(k, _)| *k).collect()
        } else if at.siblings.len() >= SIBLINGS_MAX {
            at.siblings.keys().copied().collect()
        } else {
            return Some(None);
        };
        let held = |k: &Id| at.siblings[k].own || self.named(at, k) > 0;
        let highest = full.into_iter().rev().find(|k| !held(k));
        if self.named(at, &id) > 0 {
            return Some(highest);
        }
        let highest = highest?;
        (id < highest).then_some(Some(highest))
    }

    /// A frame `who` signed, from `link`, refused: one of that pair's turns
    /// this settle window, the frame remembered, and said the first time `who`
    /// is refused here.
    fn refuse(
        &self,
        at: &mut Instance,
        who: NodeId,
        link: NodeId,
        frame: [u8; 32],
        why: &VmError,
        out: &mut Step,
    ) {
        let first = !at.junk.keys().any(|(w, _)| *w == who);
        spend(&mut at.junk, (who, link), JUNK_EACH, self.settle());
        let now = Instant::now();
        let kept = at.refused.len() < REFUSED_MAX || at.refused.contains_key(&frame);
        match why {
            // The chain could not be asked: the block may be fine. Asked again
            // when it is pushed again, after a wait that doubles.
            VmError::Unavailable(_) => {
                let wait =
                    at.refused.get(&frame).map_or(self.settle(), |(_, w)| (*w * 2).min(RETRY_MAX));
                if kept {
                    at.refused.insert(frame, (Some(now + wait), wait));
                }
            }
            // The block's own fault: the same frame is not looked at again here.
            _ => {
                if kept {
                    at.refused.insert(frame, (None, Duration::ZERO));
                }
            }
        }
        if first {
            out.events.push(Event::Refused(format!(
                "height {}: proposer {}'s block: {why}",
                at.height,
                short(&who)
            )));
        }
    }

    /// Put a block live at this height.
    fn live(&self, at: &mut Instance, id: Id, s: Sibling) {
        let now = Instant::now();
        if at.siblings.is_empty() {
            // The first block live here: the retry starts again from its first
            // window, whatever a quiet height let it grow to.
            at.backoff = self.settle() * 2;
            at.retry = now + at.backoff;
        }
        self.stir(at);
        let elsewhere = at.bound.as_ref().is_some_and(|v| v.position.signed_identity() != id);
        at.siblings.insert(id, s);
        // A block beside the one this validator signed: its proposer, and
        // whoever holds it, hear now what this validator signed and what it
        // holds — so a seat that missed the height sees the block that can
        // still be certified before it settles on one that cannot.
        if elsewhere {
            self.hurry(at);
        }
    }

    /// A block arrived at the height: the settle window restarts, until the
    /// height's one deadline.
    fn stir(&self, at: &mut Instance) {
        let now = Instant::now();
        if *at.closes.get_or_insert(now + SETTLE_MAX) >= now {
            at.offered = Some(now);
        }
    }

    /// Owe a push of what this validator said and holds here, now and outside
    /// the retry's schedule, at most once a settle window.
    fn hurry(&self, at: &mut Instance) {
        if at.hurried.is_some_and(|t| t.elapsed() < self.settle()) {
            return;
        }
        at.hurried = Some(Instant::now());
        at.push = true;
    }

    /// How many verified votes at this height name block `id`.
    fn named(&self, at: &Instance, id: &Id) -> usize {
        at.votes.values().filter(|v| v.position.signed_identity() == *id).count()
    }

    /// A vote at this height: the first from each validator, checked.
    fn voted(&self, at: &mut Instance, v: SignedVote) {
        if at.votes.contains_key(&v.node) || !self.signed(&v) {
            return;
        }
        at.votes.insert(v.node, v);
    }

    /// Build and propose, when this validator's window is open and nothing is
    /// live or signed at the height. A block already live is the one to
    /// certify; building beside it could only take a signature from it. So is
    /// a block f+1 validators signed that this one lacks: one of them is
    /// honest, executed it, keeps it, and pushes it again — it is waited for,
    /// not built beside, until the retry gives up on it. (Fewer signatures
    /// prove nothing: one faulty validator could sign a block that does not
    /// exist and hold every height.) A validator bound at the height builds
    /// nothing: it could never sign what it built.
    fn offer(&self, at: &mut Instance, vm: &dyn Vm, host: &Host, out: &mut Step) {
        if at.bound.is_some() || !at.siblings.is_empty() || at.built.is_some() {
            return;
        }
        let faults = self.members.saturating_sub(1) / 3;
        if at.attempts < RETRY_ATTEMPTS && self.leading(at).is_some_and(|(_, c)| c > faults) {
            return;
        }
        let slot = self.slot_of(at);
        if self.proposer(at.height, slot) != self.node {
            return;
        }
        if at.empty.is_some_and(|t| t.elapsed() < self.poll()) {
            return;
        }
        let block = match vm.build() {
            Ok(b) => b,
            // Nothing to run. It looks again a poll later, while its window
            // lasts.
            Err(VmError::Empty) => {
                at.empty = Some(Instant::now());
                return;
            }
            Err(e) => {
                at.empty = Some(Instant::now());
                out.events.push(Event::Refused(format!("building: {e}")));
                return;
            }
        };
        let id = block.id();
        if block.height() != at.height || block.parent() != at.tip {
            at.empty = Some(Instant::now());
            out.events.push(Event::Refused(format!(
                "height {}: the chain built a block that is not on the accepted tip",
                at.height
            )));
            return;
        }
        match host.propose(&self.chain, &self.set_root, &at.tip, at.height, slot, block.bytes()) {
            Ok(p) => {
                let frame = digest(&p);
                at.built = Some(id);
                self.live(at, id, Sibling { proposal: p, frame, proposer: self.node, own: true });
            }
            Err(e) => {
                at.empty = Some(Instant::now());
                out.events.push(Event::Refused(format!("signing a proposal: {e}")));
            }
        }
    }

    /// Sign the converged winner, once: the lowest id among the live blocks at
    /// the height that can still reach α, when no new block has been offered
    /// for a settle window.
    fn vote(
        &self,
        at: &mut Instance,
        vm: &dyn Vm,
        host: &Host,
        votes: &mut Journal,
        out: &mut Step,
    ) {
        if at.bound.is_some() {
            return;
        }
        let Some(latest) = at.offered else { return };
        if latest.elapsed() < self.settle() {
            return;
        }
        // Only a block that can still reach α: its verified votes and every
        // validator not yet heard from. Arithmetic over signed votes, the same
        // on every validator that has seen them — a block the votes already
        // shut out is never where one more signature goes.
        let open = self.members.saturating_sub(at.votes.len());
        let need = self.quorum();
        let Some((&winner, sibling)) =
            at.siblings.iter().find(|(id, _)| self.named(at, id) + open >= need)
        else {
            return;
        };
        let Ok(block) = vm.get(&winner) else { return };
        let tally = match Tally::new(self.position(block.as_ref()), Finality::Quasar, self.epoch) {
            Ok(t) => t,
            Err(e) => {
                out.events.push(Event::Refused(format!("voting: {e:?}")));
                return;
            }
        };
        // Bound already, by a run this one does not remember: what stands is
        // what this validator said, and it says that again.
        let slot = lux_consensus::vote::Slot { chain: self.chain, height: at.height, round: 0 };
        if let Some(standing) = votes.standing(&slot).map(<[u8]>::to_vec) {
            if let Ok(v) = host.resend(&standing, &self.key, votes) {
                at.votes.insert(self.node, v.clone());
                at.bound = Some(v);
            }
            return;
        }
        // The block reaches the device before the signature does, so a restart
        // that finds this validator bound can offer the block as well.
        if let Err(e) = self.held.keep(&sibling.proposal) {
            at.stalled = Some(Instant::now());
            out.events.push(Event::Refused(format!("keeping the block it signs: {e}")));
            return;
        }
        match host.vote(&tally, &self.key, votes) {
            Ok(v) => {
                at.votes.insert(self.node, v.clone());
                at.bound = Some(v);
            }
            Err(e) => {
                at.stalled = Some(Instant::now());
                out.events.push(Event::Refused(format!("voting: {e}")));
            }
        }
    }

    /// The block with α signatures at this height, and how many it has.
    fn leading(&self, at: &Instance) -> Option<(Id, usize)> {
        let mut count: BTreeMap<Id, usize> = BTreeMap::new();
        for v in at.votes.values() {
            *count.entry(v.position.signed_identity()).or_default() += 1;
        }
        count.into_iter().max_by_key(|(id, c)| (*c, std::cmp::Reverse(*id)))
    }

    /// Accept the block α validators signed, once this node holds and has
    /// executed it. Returns whether such a block exists that this node does
    /// NOT hold — the sign it has to ask for it.
    fn decide(&self, at: &mut Instance, vm: &dyn Vm, out: &mut Step) -> bool {
        let Some((id, count)) = self.leading(at) else { return false };
        if count < self.quorum() {
            return false;
        }
        let Ok(block) = vm.get(&id) else { return true };
        if !at.siblings.contains_key(&id) && vm.verify(&id).is_err() {
            return false;
        }
        let Ok(mut tally) = Tally::new(self.position(block.as_ref()), Finality::Quasar, self.epoch)
        else {
            return false;
        };
        // Each vote was checked against what it claimed; the certificate is
        // over this block's own position. A vote that signed other roots under
        // this block's id is not a vote for it, and is let go.
        let mut wrong = Vec::new();
        for (node, v) in &at.votes {
            if v.position.signed_identity() == id && tally.add(v, &self.set).is_err() {
                wrong.push(*node);
            }
        }
        for node in wrong {
            at.votes.remove(&node);
        }
        let Ok(cert) = tally.cert(&self.set, &self.set) else { return false };
        match self.settle_run(vm, &[(block.as_ref(), &cert)]) {
            Ok(()) => {
                if let Err(e) = self.lift() {
                    out.events.push(Event::Refused(format!("lifting the import gate: {e}")));
                }
                out.events.push(Event::Decided {
                    height: at.height,
                    slot: at.siblings.get(&id).map(|s| s.proposal.slot),
                    cert: Box::new(cert),
                    built: at.built == Some(id),
                });
            }
            Err(e) => {
                at.stalled = Some(Instant::now());
                out.events.push(Event::Refused(format!("accepting: {e}")));
            }
        }
        false
    }

    /// The retry (`rePollAllPending`): when due, say the standing vote again
    /// and push this validator's own block, and the block it signed, again, so
    /// a peer that missed any of them can still sign or count it; the window
    /// doubles to [`RETRY_MAX`]. Returns whether [`RETRY_ATTEMPTS`] have gone
    /// by with something pending at the height — Go's cue to ask for the
    /// certified block instead. A height with nothing live and nothing said is
    /// not pending: an idle chain asks nobody. A push owed out of turn
    /// ([`Engine::hurry`]) goes out at once and is no attempt.
    fn retry(&self, at: &mut Instance, host: &Host, out: &mut Step) -> bool {
        let due = Instant::now() >= at.retry;
        if !due && !at.push {
            return false;
        }
        at.push = false;
        let bound = at.bound.as_ref().map(|v| v.position.signed_identity());
        if let Some(v) = &at.bound {
            host.mesh().broadcast(v);
        }
        for (id, s) in &at.siblings {
            if s.own || bound == Some(*id) {
                host.mesh().propose(&s.proposal);
            }
        }
        // A push owed to a peer that came back, or to a block beside this
        // one, is not an attempt: attempts count the schedule, a height at a
        // time, however often a link comes and goes.
        if !due {
            return false;
        }
        at.backoff = (at.backoff * 2).min(RETRY_MAX);
        at.retry = Instant::now() + at.backoff;
        if at.siblings.is_empty() && at.votes.is_empty() {
            return false;
        }
        at.attempts += 1;
        out.events.push(Event::Waiting {
            height: at.height,
            slot: self.slot_of(at),
            most: self.leading(at).map_or(0, |(_, c)| c),
            need: self.quorum(),
            blocks: at.siblings.len(),
        });
        at.attempts >= RETRY_ATTEMPTS
    }

    /// Ask the peers for what this node missed, when there is a sign it is
    /// behind: a certificate for a block it does not hold, a height the retry
    /// gave up on, or f+1 validators heard at later heights — at least one of
    /// them honest, so the height is decided — for longer than the votes
    /// already on their way take to land. At most once a window.
    fn behind(
        &mut self,
        vm: &dyn Vm,
        host: &Host,
        height: u64,
        lost: bool,
        settled: bool,
        out: &mut Step,
    ) {
        let faults = self.members.saturating_sub(1) / 3;
        let ahead = self.heights.values().filter(|h| **h > height).count();
        if !lost && !(settled && ahead > faults) {
            return;
        }
        if self.asked.is_some_and(|t| t.elapsed() < self.window) {
            return;
        }
        self.asked = Some(Instant::now());
        // A settle window at most: this runs inside the step.
        match self.catch_up(vm, host, self.settle()) {
            Ok(c) if c.accepted == 0 && c.refused.is_empty() => {}
            Ok(c) => out.events.push(Event::Caught(c)),
            Err(e) => out.events.push(Event::Refused(format!("catching up: {e}"))),
        }
    }

    /// Accept certified blocks, in order. The ONE place a block becomes
    /// accepted, so the order is stated once: the certificates are recorded,
    /// then the blocks are accepted — a crash between the two leaves proof of
    /// blocks this node never took, which is harmless; the other order leaves
    /// blocks it can never prove. A run is recorded with one sync.
    ///
    /// The position recorded is this node's own for the block. A certificate
    /// signs the canonical bytes and not the transport ids beside them, so a
    /// peer's copy may carry any transport ids at all; the one kept is the one
    /// every honest node would write.
    fn settle_run(&self, vm: &dyn Vm, proven: &[(&dyn Block, &QuorumCert)]) -> Result<(), Failed> {
        if let Some(ledger) = &self.ledger {
            let kept: Vec<(u64, Id, QuorumCert)> = proven
                .iter()
                .map(|(b, c)| {
                    (b.height(), b.id(), QuorumCert { position: self.position(*b), ..(*c).clone() })
                })
                .collect();
            let entries: Vec<(u64, Id, &QuorumCert)> =
                kept.iter().map(|(h, id, c)| (*h, *id, c)).collect();
            ledger.record(&entries).map_err(Failed::NotDurable)?;
        }
        for (b, _) in proven {
            vm.accept(&b.id())?;
            vm.set_preference(&b.id())?;
        }
        Ok(())
    }

    /// Lift the import gate: a certified block now stands on the history it
    /// covered. Called once a block is accepted on a certificate — that block
    /// is the anchor.
    fn lift(&self) -> Result<(), Failed> {
        if self.caught_up() {
            return Ok(());
        }
        self.gate.lift().map_err(Failed::NotDurable)
    }

    /// Lift the import gate if the tip was accepted on a certificate this node
    /// kept. The anchor [`Engine::lift`] acts on, found again from the ledger,
    /// so a lift that did not reach the device is retried rather than waiting
    /// for the next certificate.
    fn anchored(&self, vm: &dyn Vm) -> Result<(), Failed> {
        let (Some((from, _)), Some(ledger)) = (self.gate.held(), &self.ledger) else {
            return Ok(());
        };
        let tip = vm.last_accepted();
        let height = vm.get(&tip)?.height();
        if height >= from && ledger.get(height).map(|(id, _)| id) == Some(tip) {
            return self.lift();
        }
        Ok(())
    }

    /// The height of the last accepted block.
    fn tip(&self, vm: &dyn Vm) -> Result<u64, Failed> {
        Ok(vm.get(&vm.last_accepted())?.height())
    }

    /// Whether `cert` is a Quasar certificate this committee issued: the rule
    /// a live certificate is held to, recomputed from the set this node holds.
    fn holds(&self, cert: &QuorumCert) -> Result<(), Failed> {
        if cert.tier != Finality::Quasar {
            return Err(Failed::NoCert(format!(
                "a {} certificate; this chain accepts on quasar",
                cert.tier.name()
            )));
        }
        cert.verify_weighted(&self.set, &self.set, self.epoch)
            .map_err(|e| Failed::NoCert(format!("{e:?}")))
    }

    /// Fetch what the peers certified beyond this node's tip, verify each
    /// certificate, and accept each block, in order.
    ///
    /// Every peer is asked at once, round after round from the new tip, for as
    /// long as one has more. Nothing a peer says is taken on its word: a
    /// record is accepted only if its certificate is a Quasar certificate of
    /// THIS committee over exactly this chain, the next height, the block's
    /// own id, and this node's tip as its parent — and then only if the chain,
    /// executing the block, arrives at the root the certificate signs. A peer
    /// that fails any of it is not asked again in this call; whatever it
    /// served before the lie proved itself record by record and stays.
    ///
    /// Silent: this node signs nothing here. Every height it takes was decided
    /// without it, and its vote at those heights would be a statement about a
    /// height that is over.
    ///
    /// Call it at startup, once the mesh is formed. [`Engine::step`] calls it
    /// on its own when the height it is on is decided without it.
    pub fn catch_up(&self, vm: &dyn Vm, host: &Host, deadline: Duration) -> Result<Caught, Failed> {
        let end = Instant::now() + deadline;
        let mut caught = Caught::default();
        let mut doubted: BTreeSet<NodeId> = BTreeSet::new();
        loop {
            let left = end.saturating_duration_since(Instant::now());
            let peers: Vec<NodeId> =
                host.mesh().members().into_iter().filter(|p| !doubted.contains(p)).collect();
            if left.is_zero() || peers.is_empty() {
                break;
            }
            // Every peer at once: one that is silent costs the wait, not a
            // turn each.
            let from = self.tip(vm)? + 1;
            let mut moved = false;
            let mut lied = false;
            for (peer, answer) in host.mesh().ask(&peers, from, BATCH_MAX, left.min(ASK_WAIT)) {
                caught.reported = caught.reported.max(answer.tip);
                if answer.records.is_empty() {
                    continue;
                }
                // The certificates first, all of them; then each block,
                // executed on the one before; then recorded and accepted
                // together. The earliest failure ends the run, and what came
                // before it stands on its own proof.
                let tip = vm.last_accepted();
                let (certs, mut lie) = self.certificates(&answer.records, from, tip);
                let mut proven: Vec<(Box<dyn Block>, QuorumCert)> = Vec::new();
                let mut parent = tip;
                for ((height, record), cert) in (from..).zip(&answer.records).zip(certs) {
                    match self.prove(vm, &record.block, &cert, parent) {
                        Ok(block) => {
                            parent = block.id();
                            proven.push((block, cert));
                        }
                        Err(why) => {
                            lie = Some((height, why));
                            break;
                        }
                    }
                }
                if let Some((height, why)) = lie {
                    doubted.insert(peer);
                    caught.refused.push((peer, format!("height {height}: {why}")));
                    lied = true;
                }
                let run: Vec<(&dyn Block, &QuorumCert)> =
                    proven.iter().map(|(b, c)| (b.as_ref(), c)).collect();
                self.settle_run(vm, &run)?;
                caught.accepted += run.len() as u64;
                // Moved on: the other answers are about heights now behind,
                // and the next round asks from the new tip.
                if !run.is_empty() {
                    moved = true;
                    break;
                }
            }
            // Nothing new, and nobody to doubt: every peer that answered has
            // told all it holds.
            if !moved && !lied {
                break;
            }
        }
        self.caught(vm, caught)
    }

    /// The end of a catch-up. A certified block on top of an imported tip is a
    /// quorum building on that history, which is what the import gate waits
    /// for — looked for whether or not this call accepted anything, so a lift
    /// that failed before is tried again.
    fn caught(&self, vm: &dyn Vm, caught: Caught) -> Result<Caught, Failed> {
        if caught.accepted > 0 {
            self.lift()?;
        }
        self.anchored(vm)?;
        Ok(caught)
    }

    /// The certificates of a run served from `from` on top of `tip`, checked
    /// before any block is looked at: each must decode, be this chain's, be at
    /// its height, build on the one before it, name this validator set, and
    /// hold under the Quasar rule. Returns the ones that do, up to the first
    /// that does not, and why that one did not.
    ///
    /// The signatures are the cost, and one certificate does not depend on
    /// another, so they are checked on several threads at once — the only part
    /// of a catch-up that can be; executing the blocks cannot.
    fn certificates(
        &self,
        records: &[Record],
        from: u64,
        tip: Id,
    ) -> (Vec<QuorumCert>, Option<(u64, Failed)>) {
        let mut certs = Vec::new();
        let mut lie = None;
        let mut parent = tip;
        for (height, record) in (from..).zip(records) {
            let cert = ledger::decode(&record.cert)
                .map_err(|e| Failed::NoCert(e.to_string()))
                .and_then(|c| self.placed(&c, height, parent).map(|()| c));
            match cert {
                Ok(c) => {
                    parent = c.position.signed_identity();
                    certs.push(c);
                }
                Err(why) => {
                    lie = Some((height, why));
                    break;
                }
            }
        }

        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 8);
        let share = certs.len().div_ceil(threads).max(1);
        let held: Vec<Result<(), Failed>> = std::thread::scope(|s| {
            let work: Vec<_> = certs
                .chunks(share)
                .map(|chunk| {
                    s.spawn(move || chunk.iter().map(|c| self.holds(c)).collect::<Vec<_>>())
                })
                .collect();
            work.into_iter().flat_map(|w| w.join().expect("a signature check")).collect()
        });
        if let Some(i) = held.iter().position(Result::is_err) {
            let why = held.into_iter().nth(i).and_then(Result::err).expect("an error at i");
            certs.truncate(i);
            lie = Some((from + i as u64, why));
        }
        (certs, lie)
    }

    /// Whether `cert` is about the block at `height` on `parent`, on this chain
    /// and under this validator set — what it SIGNS, checked against where this
    /// node is, before anything is spent on its signatures.
    fn placed(&self, cert: &QuorumCert, height: u64, parent: Id) -> Result<(), Failed> {
        let at = &cert.position;
        let signed_parent =
            if at.parent_canonical_id == EMPTY { at.parent_id } else { at.parent_canonical_id };
        if at.chain_id != self.chain {
            return Err(Failed::NoCert("a certificate for another chain".into()));
        }
        if at.height != height {
            return Err(Failed::NoCert(format!("a certificate for height {}", at.height)));
        }
        if signed_parent != parent {
            return Err(Failed::NoCert("a certificate that does not build on this tip".into()));
        }
        if at.validator_set_root != self.set_root {
            return Err(Failed::NoCert("a certificate from another validator set".into()));
        }
        Ok(())
    }

    /// The block `cert` signs, parsed from `raw` and executed on `parent`, or a
    /// refusal. Verified, not accepted.
    ///
    /// Only reached once the certificate holds, so the chain is never shown a
    /// block nobody proved: a peer that could make this node parse whatever it
    /// sends could make it hold whatever it sends. The block must be exactly
    /// what the certificate signs — its id, its parent, the root its execution
    /// arrives at — and a block that is not is dropped again.
    fn prove(
        &self,
        vm: &dyn Vm,
        raw: &[u8],
        cert: &QuorumCert,
        parent: Id,
    ) -> Result<Box<dyn Block>, Failed> {
        let block = vm.parse(raw)?;
        let signed = canonical_vote_message(&self.position(block.as_ref()), true);
        let refused = if signed != cert.message() || block.parent() != parent {
            Some(Failed::NoCert("the certificate does not sign this block".into()))
        } else {
            vm.verify(&block.id()).err().map(Failed::Chain)
        };
        if let Some(why) = refused {
            // Dropped, unless the chain already accepted it at its height:
            // this node does not keep what nobody proved to it. A peer can
            // serve this node's own proposal here; the chain gives that
            // block's transactions back to its pool as it lets the block go,
            // so the peer costs a proposal and never what users sent.
            if vm.block_id_at(block.height()).ok() != Some(block.id()) {
                let _ = vm.reject(&block.id());
            }
            return Err(why);
        }
        Ok(block)
    }
}

/// Take one of `key`'s `limit` turns this window of `every`, if one is left.
fn spend<K: Ord>(
    turns: &mut BTreeMap<K, (u32, Instant)>,
    key: K,
    limit: u32,
    every: Duration,
) -> bool {
    let (left, since) = turns.entry(key).or_insert((limit, Instant::now()));
    if since.elapsed() >= every {
        *left = limit;
        *since = Instant::now();
    }
    if *left == 0 {
        return false;
    }
    *left -= 1;
    true
}

/// A proposal frame's digest: everything it says, so two frames that differ
/// anywhere are two frames.
fn digest(p: &Proposal) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(p.height.to_be_bytes());
    h.update(p.slot.to_be_bytes());
    h.update((p.signature.len() as u64).to_be_bytes());
    h.update(&p.signature);
    h.update(&p.block);
    h.finalize().into()
}

/// An id's first bytes, for a log line.
fn short(id: &[u8]) -> String {
    id[..4].iter().map(|b| format!("{b:02x}")).collect()
}

/// The validator set as it is DESCRIBED: who is in it, how much each carries,
/// and the key each signs with.
///
/// `ValidatorSet` is the thing that verifies a signature and deliberately does
/// not enumerate — a set you can read out is a set you can copy wrongly. So the
/// description is kept here, and both the verifier and the commitment are read
/// off it. One statement of who the validators are, two things derived from it.
#[derive(Clone, Debug)]
pub struct Committee {
    members: Vec<Member>,
    /// The chain these validators are entitled on. In the proof, not the name:
    /// every member is called the same thing everywhere, and entitled here.
    chain: Id,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Member {
    /// The name, which is the hash of `identity`.
    pub node: NodeId,
    pub weight: u64,
    /// The ML-DSA-65 public key this validator is named by and proves on every
    /// link. Part of the commitment, so a set that swapped one validator's
    /// transport identity is a different set.
    pub identity: Vec<u8>,
    /// The compressed BLS public key it votes with.
    pub key: Vec<u8>,
    /// Proof this validator holds the BLS key it claims.
    pub proof: Vec<u8>,
}

fn unhex(s: &str, line: usize, what: &str) -> Result<Vec<u8>, String> {
    alloy_primitives::hex::decode(s)
        .map_err(|e| format!("line {}: {what} is not hex: {e}", line + 1))
}

impl Committee {
    /// The bytes a committee proof signs: `chain ‖ node ‖ key`.
    ///
    /// A registration proves possession of a key. A committee proves it ON A
    /// CHAIN — so a line published for one network entitles nothing on
    /// another, while the name stays the same everywhere. Stated once, and
    /// both the publisher and the door read it from here.
    pub fn claim(chain: &Id, node: &NodeId, key: &[u8]) -> Vec<u8> {
        let mut m = Vec::with_capacity(chain.len() + node.len() + key.len());
        m.extend_from_slice(chain);
        m.extend_from_slice(node);
        m.extend_from_slice(key);
        m
    }

    /// Read a committee from what its validators published.
    ///
    /// One line per validator: the ML-DSA public key it is NAMED by, the BLS
    /// public key it VOTES with, and the proof it holds that BLS key. Nothing
    /// secret is in it, which is why it can be handed around; and nothing in it
    /// is taken on trust, because the proof is checked when the set is built.
    ///
    /// A shared seed would be shorter and wrong: every validator would hold
    /// every other validator's signing key, which is not a committee, it is one
    /// validator wearing four hats.
    pub fn read(text: &str, chain: Id) -> Result<Self, String> {
        let mut members = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let parts: Vec<&str> = line.split_whitespace().collect();
            let [identity, key, proof] = parts.as_slice() else {
                return Err(format!("line {}: expected <identity> <key> <proof>", n + 1));
            };
            let identity = unhex(identity, n, "identity")?;
            members.push(Member {
                node: crate::pq::name(&identity),
                weight: 1,
                identity,
                key: unhex(key, n, "key")?,
                proof: unhex(proof, n, "proof")?,
            });
        }
        if members.is_empty() {
            return Err("a committee with no validators is not a committee".into());
        }
        // A validator listed twice would be counted twice — two seats, one key,
        // and a quorum that is smaller than it looks.
        for (i, m) in members.iter().enumerate() {
            if members[..i].iter().any(|e| e.node == m.node) {
                return Err(format!("{} is listed twice", alloy_primitives::hex::encode(m.node)));
            }
        }
        // FILE ORDER IS KEPT. It is the order the operator declared, so a
        // positional peer list lines up with the file a reader is holding. The
        // commitment sorts its own copy, so the value does not depend on it.
        Ok(Self { members, chain })
    }

    /// What this validator publishes so others can put it in their committee.
    pub fn line(identity: &[u8], key: &[u8], proof: &[u8]) -> String {
        format!(
            "{} {} {}",
            alloy_primitives::hex::encode(identity),
            alloy_primitives::hex::encode(key),
            alloy_primitives::hex::encode(proof)
        )
    }

    /// Where `node` sits in the committee, if it is in it at all.
    pub fn seat(&self, node: &NodeId) -> Option<usize> {
        self.members.iter().position(|m| &m.node == node)
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    pub fn members(&self) -> &[Member] {
        &self.members
    }

    /// The verifier this description produces.
    pub fn set(&self) -> Result<ValidatorSet, String> {
        let mut set = ValidatorSet::new();
        for m in &self.members {
            set.insert_over(
                m.node,
                m.weight,
                &m.key,
                &Self::claim(&self.chain, &m.node, &m.key),
                &m.proof,
            )
            .map_err(|e| format!("{:?}", e))?;
        }
        Ok(set)
    }

    /// A commitment to this set: who, how much, and which key.
    ///
    /// GO'S ENCODING, BYTE FOR BYTE. This value is not ours to choose: Go
    /// computes it, folds it into the bytes every validator signs
    /// (`canonicalVoteMessageFor`), and REJECTS a vote whose root differs
    /// (`engine/chain/topology.go:341` and `:552`,
    /// `engine/chain/catchup_accept.go:302`). A Rust validator that hashed the
    /// same set any other way would have every vote refused by every Go peer,
    /// and a mixed network would fork on the first block.
    ///
    /// `luxfi/node chains/quorum.go hashValidatorSet`:
    ///
    /// - an empty set commits to the zero id, not to the hash of nothing;
    /// - validators sorted by RAW NODE ID bytes, ascending;
    /// - each one written as
    ///   `nodeID(20) || weight(u64 BE) || len(key)(u64 BE) || key`;
    /// - SHA-256 over that, streamed — no Merkle tree, despite the name, and
    ///   no domain tag, no set-size prefix, no separators.
    ///
    /// THE KEY IS UNCOMPRESSED. Go writes `bls.PublicKeyToUncompressedBytes`,
    /// which under cgo — what a production node is built with — is blst's
    /// 96-byte serialisation, not the 48-byte compressed form. Both the length
    /// prefix and the bytes are hashed, so compressed and uncompressed give
    /// different roots for one set. A committee file carries the compressed
    /// key because that is what `ValidatorSet` verifies under; it is expanded
    /// here, once, for the hash.
    pub fn root(&self) -> Id {
        if self.members.is_empty() {
            return [0u8; 32];
        }
        let mut sorted: Vec<&Member> = self.members.iter().collect();
        sorted.sort_unstable_by_key(|m| m.node);

        let mut h = Sha256::new();
        for m in sorted {
            h.update(m.node);
            h.update(m.weight.to_be_bytes());
            let key = uncompressed(&m.key);
            h.update((key.len() as u64).to_be_bytes());
            h.update(&key);
        }
        h.finalize().into()
    }
}

/// A BLS public key as Go hashes it: blst's 96-byte uncompressed form.
///
/// A key that will not decompress is written through unchanged rather than
/// dropped — it cannot be a valid member (the set refuses it at `insert`), and
/// silently hashing nothing in its place would make two different committees
/// commit to the same value.
fn uncompressed(key: &[u8]) -> Vec<u8> {
    match blst::min_pk::PublicKey::uncompress(key) {
        Ok(pk) => pk.serialize().to_vec(),
        Err(_) => key.to_vec(),
    }
}

/// The 32-byte identity of a chain.
///
/// GO DOES NOT DERIVE THIS FROM THE GENESIS DOCUMENT. A blockchain id is
/// `sha256(signed CreateChainTx bytes)` — the P-chain transaction that created
/// the chain — read back on restart from P-chain state
/// (`luxfi/node vms/platformvm/txs/tx.go SetBytes`, and `node/node.go` takes
/// the C-chain's id from `VMGenesis(...).ID()`). It is not a function of
/// anything this node holds, because the transaction that fixes it lives on a
/// chain this node does not run yet.
///
/// So it is DATA, and the node is told it. `stand_in` is what a network with no
/// P-chain uses so a local cluster has one value all its members agree on; it
/// is deliberately NOT called a blockchain id, because it is not the number a
/// Go validator would compute, and a vote carrying it would be refused by one.
/// Supply the real id with `--chain-id` to talk to a Go network.
pub fn stand_in(genesis_hash: [u8; 32]) -> Id {
    genesis_hash
}

/// Where every validator listens, by identity.
pub type Peers = BTreeMap<NodeId, std::net::SocketAddr>;

#[cfg(test)]
mod tests {
    /// The chain these fixtures are entitled on. Any fixed 32 bytes: what the
    /// tests exercise is that the SAME chain admits and a different one does
    /// not, not which chain it is.
    const TEST_CHAIN: crate::vm::Id = [9u8; 32];

    use super::*;
    use crate::pq::Identity;

    /// `n` validators, each with a real ML-DSA name and a real BLS voting key,
    /// published the way they would be to a genesis file.
    fn published(n: u8) -> (Vec<(Identity, SecretKey)>, Committee) {
        let mut lines = String::new();
        let mut held = Vec::new();
        for i in 0..n {
            let id = Identity::generate().expect("identity");
            let sk = SecretKey::key_gen(&[i.wrapping_add(1); 32], &[]).expect("key_gen");
            let pk = sk.sk_to_pk().compress();
            let proof =
                lux_consensus::pop::sign_over(&sk, &Committee::claim(&TEST_CHAIN, &id.node(), &pk));
            lines.push_str(&Committee::line(id.public(), &pk, &proof));
            lines.push('\n');
            held.push((id, sk));
        }
        let c = Committee::read(&lines, TEST_CHAIN).expect("committee");
        (held, c)
    }

    fn engine_of(n: u8) -> Engine {
        let (held, c) = published(n);
        let (id, sk) = &held[0];
        Engine::new(id.node(), sk.clone(), &c, 1, [9u8; 32]).expect("engine")
    }

    #[test]
    fn a_solo_validator_needs_one_signature() {
        assert_eq!(engine_of(1).quorum(), 1, "a chain of one certifies with its own vote");
    }

    #[test]
    fn four_validators_need_three() {
        assert_eq!(engine_of(4).quorum(), 3, "the Quasar rule at equal stake");
    }

    #[test]
    fn a_committee_is_named_by_the_keys_its_validators_prove() {
        let (held, c) = published(3);
        assert_eq!(c.len(), 3);
        for (id, _) in &held {
            assert!(
                c.seat(&id.node()).is_some(),
                "a published validator is in the committee it published to"
            );
        }
        let set = c.set().expect("every member proves its BLS key");
        assert_eq!(set.len(), 3);
        for m in c.members() {
            assert!(set.can_verify(&m.node), "a described member can sign");
            assert_eq!(m.node, crate::pq::name(&m.identity), "the name is the key");
        }
    }

    #[test]
    fn a_set_commitment_is_a_function_of_the_whole_set() {
        let (_, a) = published(3);
        let (_, b) = published(3);
        assert_ne!(a.root(), b.root(), "different validators, different set");
        assert_ne!(a.root(), [0u8; 32], "and it is not a constant");

        // Re-reading the same publication is the same set, in any order: the
        // commitment sorts, so a file whose lines were shuffled is one set.
        let mut lines: Vec<String> =
            a.members().iter().map(|m| Committee::line(&m.identity, &m.key, &m.proof)).collect();
        assert_eq!(
            Committee::read(&lines.join("\n"), TEST_CHAIN).expect("re-read").root(),
            a.root()
        );
        lines.reverse();
        let shuffled = Committee::read(&lines.join("\n"), TEST_CHAIN).expect("re-read");
        assert_eq!(shuffled.root(), a.root(), "one set, whatever order it is written in");
        assert_ne!(
            shuffled.members()[0].node,
            a.members()[0].node,
            "and the file's own order is kept, so a positional peer list lines up"
        );
    }

    #[test]
    fn a_validator_listed_twice_is_refused() {
        let (_, c) = published(1);
        let line = {
            let m = &c.members()[0];
            Committee::line(&m.identity, &m.key, &m.proof)
        };
        let doubled = format!("{line}\n{line}");
        assert!(
            Committee::read(&doubled, TEST_CHAIN).is_err(),
            "one key, two seats, and a quorum smaller than it looks"
        );
    }

    #[test]
    fn a_committee_file_that_is_not_one_is_refused() {
        assert!(Committee::read("", TEST_CHAIN).is_err(), "no validators is not a committee");
        assert!(Committee::read("# just a comment", TEST_CHAIN).is_err());
        assert!(Committee::read("aa bb", TEST_CHAIN).is_err(), "a line is three fields");
        assert!(Committee::read("zz aa bb", TEST_CHAIN).is_err(), "and they are hex");
    }

    #[test]
    fn comments_and_blank_lines_are_not_validators() {
        let (_, c) = published(2);
        let text = c
            .members()
            .iter()
            .map(|m| Committee::line(&m.identity, &m.key, &m.proof))
            .collect::<Vec<_>>()
            .join("\n");
        let noisy = format!("# a committee\n\n{text}\n\n# end\n");
        assert_eq!(Committee::read(&noisy, TEST_CHAIN).expect("read").len(), 2);
    }

    /// A real certificate over a real position, signed by every validator.
    fn certified(n: u8) -> (Committee, ValidatorSet, lux_consensus::cert::QuorumCert) {
        let (held, c) = published(n);
        let set = c.set().expect("set");
        let position = Position {
            chain_id: [0xc1; 32],
            height: 1,
            round: 0,
            block_id: [0xb1; 32],
            parent_id: [0xb2; 32],
            canonical_id: [0xb1; 32],
            parent_canonical_id: [0xb2; 32],
            execution_state_root: [0xab; 32],
            payload_root: [0xcd; 32],
            validator_set_root: c.root(),
        };
        let mut tally = Tally::new(position, Finality::Quasar, 1).expect("tally");
        let message = tally.message().to_vec();
        for (id, sk) in &held {
            let v = lux_consensus::vote::SignedVote {
                position: tally.position().clone(),
                accept: true,
                node: id.node(),
                signature: sk.sign(&message, lux_consensus::cert::DST, &[]).compress().to_vec(),
            };
            tally.add(&v, &set).expect("counted");
        }
        let cert = tally.cert(&set, &set).expect("certifies");
        (c, set, cert)
    }

    /// The spec, written out by hand rather than by calling `root()`.
    ///
    /// Go's own conformance test does exactly this — `quorum_setroot_test.go`
    /// re-implements the encoding instead of calling `hashValidatorSet` — so
    /// that the test fails if the production body drifts. Same idea here.
    fn by_hand(members: &[(NodeId, u64, Vec<u8>)]) -> Id {
        if members.is_empty() {
            return [0u8; 32];
        }
        let mut sorted = members.to_vec();
        sorted.sort_by_key(|(n, _, _)| *n);
        let mut bytes = Vec::new();
        for (node, weight, key) in sorted {
            bytes.extend_from_slice(&node);
            bytes.extend_from_slice(&weight.to_be_bytes());
            let full = blst::min_pk::PublicKey::uncompress(&key)
                .expect("a member's key decompresses")
                .serialize()
                .to_vec();
            bytes.extend_from_slice(&(full.len() as u64).to_be_bytes());
            bytes.extend_from_slice(&full);
        }
        let mut h = Sha256::new();
        h.update(&bytes);
        h.finalize().into()
    }

    #[test]
    fn the_set_root_is_the_encoding_go_hashes() {
        let (_held, c) = published(4);
        let spec: Vec<(NodeId, u64, Vec<u8>)> =
            c.members().iter().map(|m| (m.node, m.weight, m.key.clone())).collect();
        assert_eq!(c.root(), by_hand(&spec), "the root must be Go's encoding, not one of our own");
    }

    #[test]
    fn the_key_in_the_root_is_the_uncompressed_one() {
        // The difference that would fork a mixed network: Go writes blst's
        // 96-byte serialisation, and BOTH the length prefix and the bytes are
        // hashed. Hashing the 48-byte compressed key gives a different number.
        let (_held, c) = published(2);
        let m = &c.members()[0];
        assert_eq!(m.key.len(), 48, "a committee file carries the compressed key");
        assert_eq!(uncompressed(&m.key).len(), 96, "and the root hashes the uncompressed one");

        let mut wrong = Sha256::new();
        let mut sorted: Vec<&Member> = c.members().iter().collect();
        sorted.sort_unstable_by_key(|m| m.node);
        for m in sorted {
            wrong.update(m.node);
            wrong.update(m.weight.to_be_bytes());
            wrong.update((m.key.len() as u64).to_be_bytes());
            wrong.update(&m.key);
        }
        let wrong: Id = wrong.finalize().into();
        assert_ne!(c.root(), wrong, "compressed and uncompressed are two roots");
    }

    #[test]
    fn an_empty_set_commits_to_zero_not_to_the_hash_of_nothing() {
        // Go returns ids.Empty for an empty set. SHA-256 of no bytes is a
        // perfectly good hash and the wrong answer.
        assert_eq!(Committee { members: Vec::new(), chain: TEST_CHAIN }.root(), [0u8; 32]);
        let mut h = Sha256::new();
        h.update(b"");
        let of_nothing: Id = h.finalize().into();
        assert_ne!(Committee { members: Vec::new(), chain: TEST_CHAIN }.root(), of_nothing);
    }

    #[test]
    fn the_root_does_not_depend_on_the_order_the_file_listed() {
        let (_held, c) = published(4);
        let mut shuffled: Vec<Member> = c.members().to_vec();
        shuffled.reverse();
        assert_eq!(
            Committee { members: shuffled, chain: TEST_CHAIN }.root(),
            c.root(),
            "sorted by node id, so the file's order cannot change the value"
        );
    }

    #[test]
    fn a_weight_change_changes_the_root() {
        let (_held, c) = published(3);
        let mut heavier: Vec<Member> = c.members().to_vec();
        heavier[0].weight = 2;
        assert_ne!(
            Committee { members: heavier, chain: TEST_CHAIN }.root(),
            c.root(),
            "weight is hashed"
        );
    }

    #[test]
    fn a_certificate_states_the_threshold_the_set_derives() {
        // The audited line's clause: a certificate may not choose its own
        // quorum. 1.22.7 accepted whatever the certificate declared; 1.23.0
        // re-derives the floor from the LIVE set and compares.
        let (_c, set, cert) = certified(4);
        assert_eq!(
            cert.threshold,
            lux_consensus::finality::two_thirds_count(4) as u32,
            "three of four is the Quasar rule at equal stake"
        );
        cert.verify_weighted(&set, &set, 1).expect("it verifies as issued");
    }

    #[test]
    fn a_certificate_that_declares_its_own_quorum_is_refused() {
        // The whole point of the repin. 1.22.7 counted signatures against the
        // certificate's OWN threshold, so one declaring 1 cleared that clause
        // on a single signature; 1.23.0 re-derives the floor from the live set
        // and compares. Every forged value is refused — and the ones that get
        // past the count clause are refused by NAME, which is the new one.
        let (_c, set, cert) = certified(4);
        for declared in [1u32, 2, 4] {
            let mut forged = cert.clone();
            forged.threshold = declared;
            let refused = forged
                .verify_weighted(&set, &set, 1)
                .expect_err("a threshold the set did not derive must not verify");
            assert!(
                matches!(refused, lux_consensus::cert::CertError::ThresholdNotDerived { .. }),
                "declared {declared}: {refused:?}"
            );
        }

        // Declaring MORE than the votes carried is refused a clause earlier,
        // by the count. Still closed, just by a different name — worth pinning
        // so nobody reads the derived clause as the only thing standing here.
        let mut greedy = cert.clone();
        greedy.threshold = 5;
        assert!(matches!(
            greedy.verify_weighted(&set, &set, 1),
            Err(lux_consensus::cert::CertError::BelowThreshold { .. })
        ));
    }

    #[test]
    fn a_certificate_does_not_carry_to_another_committee() {
        // A certificate issued under one committee is refused by another, and
        // refused at the signatures: none of its signers is a member there, so
        // it never reaches the question of what threshold that set derives.
        let (_c, _set, cert) = certified(4);
        let (_other, seven) = published(7);
        let seven = seven.set().expect("set");
        assert!(cert.verify_weighted(&seven, &seven, 1).is_err());
    }

    #[test]
    fn the_position_carries_the_root_the_block_computed() {
        // The whole point of the engine, as an assertion.
        struct Fake;
        impl Block for Fake {
            fn id(&self) -> Id {
                [1u8; 32]
            }
            fn parent(&self) -> Id {
                [2u8; 32]
            }
            fn height(&self) -> u64 {
                7
            }
            fn timestamp(&self) -> u64 {
                100
            }
            fn bytes(&self) -> Vec<u8> {
                Vec::new()
            }
            fn state_root(&self) -> Id {
                [0xab; 32]
            }
            fn payload_root(&self) -> Id {
                [0xcd; 32]
            }
        }
        let p = engine_of(1).position(&Fake);
        assert_eq!(p.execution_state_root, [0xab; 32], "the block's root, not a constant");
        assert_eq!(p.payload_root, [0xcd; 32]);
        assert_eq!(p.height, 7);
        assert_eq!(p.canonical_id, p.block_id);
        assert_ne!(
            p.execution_state_root, [0xe5; 32],
            "the harness constant this node used to vote on"
        );
    }
}

#[cfg(test)]
mod entitlement_tests {
    use super::*;
    use crate::pq::Identity;

    /// A line published for one chain is refused on another. The names do not
    /// move — the same validator is called the same thing everywhere — but the
    /// entitlement does not travel with it.
    #[test]
    fn a_line_published_elsewhere_entitles_nothing_here() {
        const HERE: crate::vm::Id = [1u8; 32];
        const ELSEWHERE: crate::vm::Id = [2u8; 32];
        let id = Identity::generate().expect("identity");
        let sk = blst::min_pk::SecretKey::key_gen(&[5u8; 32], &[]).expect("key");
        let pk = sk.sk_to_pk().compress();

        let line = Committee::line(
            id.public(),
            &pk,
            &lux_consensus::pop::sign_over(&sk, &Committee::claim(&ELSEWHERE, &id.node(), &pk)),
        );

        // It parses here — the name is the same name — and it is refused at the
        // door, which is where an entitlement is decided.
        let read = Committee::read(&line, HERE).expect("a line is a line on any chain");
        assert_eq!(read.members()[0].node, id.node(), "one node, one name");
        assert!(read.set().is_err(), "its proof does not hold on this chain");

        let mine = Committee::line(
            id.public(),
            &pk,
            &lux_consensus::pop::sign_over(&sk, &Committee::claim(&HERE, &id.node(), &pk)),
        );
        assert!(Committee::read(&mine, HERE).expect("read").set().is_ok());
    }
}
