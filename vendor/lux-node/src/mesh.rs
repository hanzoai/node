//! The validator mesh: N peer sockets, ZAP frames on each, votes in and out.
//!
//! A mesh knows sockets, frames, and the vote codec. It does not know what a
//! quorum is, it does not verify a signature, and it holds no consensus state.
//! Inbound votes go into a queue; the caller feeds them to its own
//! [`Tally`](lux_consensus::Tally), which is where verification and counting
//! live. Keeping the rule out of the transport is what lets a peer be dropped
//! without asking whether that costs finality — the rule already answers that.
//!
//! A PEER IS A MEMBER, NOT A WALL. One eviction rule, stated once in
//! [`Peer::fail`]: a peer whose stream ended, whose framing was violated, or
//! whose socket will not take a write is dropped. The quorum rule decides
//! finality from whoever is left, so dropping a peer costs liveness nothing and
//! removes every way one socket can hold this node — an unbounded buffer (the
//! frame limit is the size of a vote), a corpse polled forever, or a stalled
//! reader blocking a broadcast (writes carry a timeout, so a write fails
//! instead of hanging).
//!
//! A structurally invalid PAYLOAD is not a framing violation: the frame was
//! well formed and its contents were a lie. The lie is dropped, the peer keeps
//! its seat, and the signature check it would have failed never runs.
//!
//! WHICH IDS THIS LINK MAY USE. `luxfi/api/zap/wire.go` reserves the whole
//! space in bands — VM service 1-31, atomic 32-39, p2p sender 40-49, warp
//! 50-59, Quasar export 60-63 — and DEFINES 1-31, 32-34, 40-43, 50-53 and
//! 60-61. There is therefore no id outside every band; what there is, is ids Go
//! has not defined. This link takes only those: 57 to ask for accepted blocks
//! (and, under ZAP's response flag, the answer), 58 for a transaction, 59 for a
//! signed proposal, 62 for a vote (`lux_consensus::vote::VOTE`), 63 for the
//! greeting.
//!
//! A PEER THAT IS BEHIND ASKS, AND IS ANSWERED FROM THE READER. A question is
//! answered on the thread that read it, from the [`Service`] this node put on
//! its mesh, so a node busy in a round still serves the one catching up. The
//! mesh carries the bytes and knows nothing about them: which blocks exist, and
//! which certificate each was accepted on, is the service's business, and
//! whether an answer proves anything is the asker's. An answer is disk reads,
//! so answers are PACED per validator ([`ASK_BURST`], then [`ASK_RATE`] a
//! second), by the node id the link proved and not by the link: a peer that
//! asks faster waits, on its own link, for its turn, and one that reconnects
//! comes back to the pace it left.
//!
//! WHAT A PEER SENT IS HELD PER PEER, AND BOUNDED. Each peer's proposals and
//! votes wait in that peer's own queue until the node takes them, at most
//! [`ARRIVALS_MAX`] proposals and [`VOTES_MAX`] votes; past that the oldest is
//! let go. One peer streaming frames holds a fixed amount of this node's
//! memory, and cannot crowd out what the others sent.
//!
//! WHAT GOES TO A PEER WAITS IN ITS OWN QUEUE, AND ITS OWN THREAD WRITES IT.
//! A send puts one shared copy of the frame on each peer's queue and returns;
//! nothing that sends — the engine, a relay, the RPC side — waits on a socket,
//! and no lock is held across a write. A queue holds at most [`OUTBOUND_MAX`]
//! frames and [`OUTBOUND_BYTES`]; past that the oldest is let go, which costs
//! nothing a retry does not say again.
//!
//! ONE THREAD PER PEER, AND NO READ TIMEOUT. A frame is read with `read_exact`,
//! so a deadline that expired mid-frame would consume a header and leave the
//! stream one frame out of step forever — the failure a non-blocking reader has
//! to carry a reassembly buffer to avoid. Blocking instead means a peer that
//! stalls mid-frame holds exactly its own thread and nobody else's, and
//! [`Mesh::close`] shuts the socket down, which returns the blocked read at
//! once. Rust's runtime ignores SIGPIPE, so a peer that hangs up mid-broadcast
//! returns an error here instead of ending the process.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use alloy_primitives::{keccak256, B256};
use lux_consensus::cert::NodeId;
use lux_consensus::vote::{SignedVote, VoteTransport, VOTE, VOTE_PAYLOAD_LEN};
use lux_consensus::zap;

/// Ask a peer for the blocks it accepted, from a height.
///
/// The answer travels under the same id with ZAP's response flag set — the
/// convention `luxfi/api/zap` already states for a request and its reply — so
/// one exchange takes one number. 57 is in Go's warp band and undefined there,
/// beside the block.
///
/// A question is `from(u64) ‖ count(u32)`, twelve bytes exactly. An answer is
/// `from(u64) ‖ tip(u64) ‖ n(u32)` and then `n` records, each a length-prefixed
/// block and a length-prefixed certificate; `from` says which question it
/// answers, `tip` is the height the answering node has accepted, and the whole
/// frame is at most [`LINK_MAX`].
pub const ASK: u8 = 57;

/// An answer: [`ASK`] with the response flag.
const ANSWER: u8 = ASK | zap::RESPONSE_FLAG;

/// A signed transaction, relayed so that whichever validator proposes next can
/// include it. 58, beside the block, for the block's reasons.
pub const TRANSACTION: u8 = 58;

/// A proposal, pushed to the validators that must vote on it and relayed by
/// them: `version(1) ‖ height(u64) ‖ slot(u64) ‖ signature(3309) ‖ block`,
/// big-endian. The signature is the proposer's ML-DSA-65 over
/// [`crate::proposer::header`], so a proposal proves who made it whichever
/// link it came in on.
///
/// THE VALIDATOR LINK IS NOT THE PLUGIN LINK, and it still does not get to
/// reuse the plugin wire's numbers. `luxfi/api/zap/wire.go` DEFINES 60 as
/// `MsgSetQuasarFinalized` and 61 as `MsgQuasarHeight`; this id was 61, which
/// is a number Go already means something by. 59 is inside Go's warp band
/// (50-59) and undefined there, which is the closest thing to free that exists
/// — wire.go's bands cover the whole space.
///
/// NOT AGREED WITH GO. Go gossips blocks over its own p2p layer, not over this
/// link, so there is no counterpart to match — only ids to stay clear of. This
/// still has to be reconciled before a Rust validator and a Go validator share
/// a p2p link.
pub const BLOCK: u8 = 59;

/// The largest vote frame. A vote's size is fixed, so this is exact.
pub const VOTE_MAX: u32 = VOTE_PAYLOAD_LEN as u32;

/// A proposal's first byte.
///
/// v0.2 sent a bare block under [`BLOCK`], and a block is an RLP list, whose
/// first byte is 0xc0 or above. A frame that does not open with this is not a
/// proposal: it is refused, never read as a height and a slot.
pub const PROPOSAL_VERSION: u8 = 1;

/// A proposal's header: version, height, slot, the proposer's signature.
pub const PROPOSAL_HEAD: usize = 1 + 8 + 8 + crate::pq::IDENTITY_SIGNATURE_LEN;

/// The largest block a proposal carries.
///
/// A block is bounded by its gas limit: 12M gas buys at most ~750 KiB of
/// calldata at 16 gas a byte, so two mebibytes covers any block this chain can
/// produce and refuses anything that could only be a lie. It is deliberately
/// far below ZAP's own 16 MiB — headroom on a link is memory a hostile peer can
/// make us hold.
pub const BLOCK_MAX: u32 = 2 * 1024 * 1024;

/// The largest frame this link carries at all.
pub const LINK_MAX: u32 = BLOCK_MAX + PROPOSAL_HEAD as u32;

/// How long a write may take before the peer is judged stalled.
pub const WRITE_TIMEOUT: Duration = Duration::from_millis(2000);

/// The most frames waiting to go to one peer.
pub const OUTBOUND_MAX: usize = 256;

/// The most bytes waiting to go to one peer: four of the largest frames. A
/// frame sent to every peer is one copy, shared.
pub const OUTBOUND_BYTES: usize = 4 * LINK_MAX as usize;

/// The most blocks one answer carries. An answer is also bounded by
/// [`LINK_MAX`] in bytes, and stops at whichever comes first.
pub const BATCH_MAX: u32 = 64;

/// The largest transaction this link relays: 128 KiB, the size go-ethereum's
/// pool refuses above. Anything larger is not relayed and not taken.
pub const TRANSACTION_MAX: u32 = 128 * 1024;

/// How many transactions are remembered as already seen. A transaction is
/// relayed and taken once; past this many the oldest is forgotten, so the
/// memory this costs is fixed however many a peer sends.
pub const SEEN_MAX: usize = 1 << 14;

/// The most proposals one peer may have waiting to be taken. A height sees one
/// in the ordinary case; past this many from one peer the oldest is let go, so
/// a peer can make this node hold at most this many of [`BLOCK_MAX`].
pub const ARRIVALS_MAX: usize = 8;

/// The most votes one peer may have waiting to be taken. A validator casts one
/// a round, and the node takes them every few milliseconds while it counts;
/// past this many from one peer the oldest is let go.
pub const VOTES_MAX: usize = 256;

/// Questions a peer may ask back to back before it is paced.
pub const ASK_BURST: u32 = 32;

/// Questions a second a peer is answered at once past the burst. An answer is
/// up to [`BATCH_MAX`] blocks and their certificates read off the disk, so
/// this is a peer catching up at a thousand blocks a second — and a peer
/// asking to exhaust this node, held to the same.
pub const ASK_RATE: u32 = 16;

/// How many validators' paces are remembered. Only a validator whose key the
/// handshake proved is added to a mesh, so this is the committee and never
/// near the bound; past it, the pace nearest a full burst is let go, and
/// letting a full one go forgets nothing.
pub const ASKERS_MAX: usize = 1024;

/// A question's exact size.
const ASK_LEN: usize = 12;

/// No answer awaited. Not a height anyone asks from.
const NONE: u64 = u64::MAX;

/// An answer's header: from, tip, and the record count.
const ANSWER_HEAD: usize = 8 + 8 + 4;

/// A proposal as it arrived: the link it came in on — its proposer's, or a
/// relaying peer's — and what it says. The block is bytes; what it is, is the
/// chain's to say, and whether the signature is its proposer's, the engine's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposal {
    pub from: NodeId,
    pub height: u64,
    pub slot: u64,
    pub signature: Vec<u8>,
    pub block: Vec<u8>,
}

impl Proposal {
    /// The frame's bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Vec::with_capacity(PROPOSAL_HEAD + self.block.len());
        w.push(PROPOSAL_VERSION);
        w.extend_from_slice(&self.height.to_be_bytes());
        w.extend_from_slice(&self.slot.to_be_bytes());
        w.extend_from_slice(&self.signature);
        w.extend_from_slice(&self.block);
        w
    }

    /// A proposal out of a frame that came in on `from`'s link, or nothing.
    pub fn decode(from: NodeId, payload: Vec<u8>) -> Option<Proposal> {
        if payload.len() <= PROPOSAL_HEAD
            || payload.len() > PROPOSAL_HEAD + BLOCK_MAX as usize
            || payload[0] != PROPOSAL_VERSION
        {
            return None;
        }
        let height = u64::from_be_bytes(payload[1..9].try_into().ok()?);
        let slot = u64::from_be_bytes(payload[9..17].try_into().ok()?);
        let signature = payload[17..PROPOSAL_HEAD].to_vec();
        let mut block = payload;
        block.drain(..PROPOSAL_HEAD);
        Some(Proposal { from, height, slot, signature, block })
    }
}

/// One accepted block and the certificate it was accepted on, as bytes. The
/// certificate is `cert_codec.go`'s encoding; the block is the chain's own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub block: Vec<u8>,
    pub cert: Vec<u8>,
}

/// What a peer said when asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    /// The height asked from, which the records start at.
    pub from: u64,
    /// The height that peer says it has accepted. A claim, not a proof: the
    /// records are the proof.
    pub tip: u64,
    pub records: Vec<Record>,
}

/// What this node offers its peers: its accepted blocks with their
/// certificates, and a pool that takes the transactions they relay.
///
/// Called from the peer's reader thread, so a peer's question is answered while
/// this node's own loop is busy — which is exactly when a peer that is behind
/// needs it.
pub trait Service: Send + Sync {
    /// The height of the last block this node accepted.
    fn tip(&self) -> u64;
    /// The accepted block at `height` and the certificate it was accepted on,
    /// when this node holds both.
    fn record(&self, height: u64) -> Option<Record>;
    /// A transaction a peer relayed, not seen before.
    fn transaction(&self, raw: &[u8]);
}

/// The transactions already relayed or taken, bounded.
struct Seen {
    order: VecDeque<B256>,
    set: HashSet<B256>,
}

impl Seen {
    /// Whether `hash` is new. Remembered either way.
    fn first(&mut self, hash: B256) -> bool {
        if !self.set.insert(hash) {
            return false;
        }
        self.order.push_back(hash);
        if self.order.len() > SEEN_MAX {
            if let Some(old) = self.order.pop_front() {
                self.set.remove(&old);
            }
        }
        true
    }
}

/// One connected validator.
struct Peer {
    node: NodeId,
    /// The write half. Held under a lock so two broadcasts cannot interleave
    /// halfway through a frame.
    out: Mutex<TcpStream>,
    /// A second handle on the same socket, used for nothing but shutdown, so
    /// eviction never waits on a broadcast that is stuck in a write.
    ctl: TcpStream,
    live: AtomicBool,
    /// The height this node is waiting on an answer from, or [`NONE`]. An
    /// answer to anything else — a question that already gave up, or one never
    /// asked — is dropped as it is read, so a peer cannot fill memory with
    /// replies to nothing and a late reply cannot stand in for the one awaited.
    awaiting: AtomicU64,
    /// The proposals this peer pushed that the node has not taken.
    proposals: Mutex<VecDeque<Proposal>>,
    /// The votes this peer carried that the node has not taken.
    votes: Mutex<VecDeque<SignedVote>>,
    /// The frames on their way to this peer, and the writer they wake.
    outbound: Mutex<Outbound>,
    ready: Condvar,
    reader: Mutex<Option<JoinHandle<()>>>,
    writer: Mutex<Option<JoinHandle<()>>>,
}

/// Frames waiting to be written to one peer, oldest first.
#[derive(Default)]
struct Outbound {
    frames: VecDeque<(u8, Arc<Vec<u8>>)>,
    bytes: usize,
}

/// Put `item` at the back of `queue`, letting the oldest go past `most`.
fn keep<T>(queue: &Mutex<VecDeque<T>>, item: T, most: usize) {
    let mut q = queue.lock().expect("a peer's queue");
    q.push_back(item);
    while q.len() > most {
        q.pop_front();
    }
}

/// A token bucket: [`ASK_BURST`] questions at once, [`ASK_RATE`] a second
/// after that.
struct Bucket {
    tokens: f64,
    at: Instant,
}

impl Bucket {
    fn new(now: Instant) -> Self {
        Self { tokens: f64::from(ASK_BURST), at: now }
    }

    /// The questions it would answer at once at `now`.
    fn level(&self, now: Instant) -> f64 {
        let since = now.saturating_duration_since(self.at).as_secs_f64();
        (self.tokens + since * f64::from(ASK_RATE)).min(f64::from(ASK_BURST))
    }

    /// Take one question's worth, and say how long to wait before answering
    /// it. The wait is spent by the caller, so a peer cannot run up a debt: it
    /// is at most one question behind.
    fn take(&mut self, now: Instant) -> Duration {
        self.tokens = self.level(now) - 1.0;
        self.at = now;
        if self.tokens >= 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(-self.tokens / f64::from(ASK_RATE))
        }
    }
}

impl Peer {
    /// The eviction rule: this peer is finished. Idempotent — the first caller
    /// closes the socket, which is what returns the reader's blocked read, and
    /// wakes the writer, which then finds the peer gone.
    fn fail(&self) {
        if self.live.swap(false, Ordering::SeqCst) {
            let _ = self.ctl.shutdown(Shutdown::Both);
            // Under the queue's lock, so the writer is either before its look
            // at `live` or already waiting: the wake is never lost.
            let _queue = self.outbound.lock().expect("a peer's outbound");
            self.ready.notify_all();
        }
    }

    /// Put a frame on this peer's queue for its writer, letting the oldest go
    /// past the bounds.
    fn post(&self, kind: u8, frame: Arc<Vec<u8>>) {
        let mut q = self.outbound.lock().expect("a peer's outbound");
        q.bytes += frame.len();
        q.frames.push_back((kind, frame));
        while q.frames.len() > OUTBOUND_MAX || q.bytes > OUTBOUND_BYTES {
            match q.frames.pop_front() {
                Some((_, old)) => q.bytes -= old.len(),
                None => break,
            }
        }
        self.ready.notify_one();
    }

    fn is_live(&self) -> bool {
        self.live.load(Ordering::SeqCst)
    }

    /// Gone, and holding nothing the node has yet to take.
    fn spent(&self) -> bool {
        !self.is_live()
            && self.proposals.lock().expect("a peer's queue").is_empty()
            && self.votes.lock().expect("a peer's queue").is_empty()
    }
}

/// What the readers share with the mesh: who is connected, what this node
/// serves, and which transactions have gone by.
struct Core {
    peers: Mutex<Vec<Arc<Peer>>>,
    /// Links taken so far.
    joined: AtomicUsize,
    evicted: AtomicUsize,
    service: RwLock<Option<Arc<dyn Service>>>,
    seen: Mutex<Seen>,
    /// The pace each validator's questions are answered at, by node id —
    /// kept across its links, so a reconnect is not a fresh burst.
    asks: Mutex<HashMap<NodeId, Bucket>>,
}

impl Core {
    /// Take one of `node`'s questions, and say how long to wait before
    /// answering it.
    fn pace(&self, node: NodeId) -> Duration {
        let now = Instant::now();
        let mut asks = self.asks.lock().expect("the paces");
        let wait = asks.entry(node).or_insert_with(|| Bucket::new(now)).take(now);
        while asks.len() > ASKERS_MAX {
            let fullest = asks
                .iter()
                .filter(|(k, _)| **k != node)
                .max_by(|a, b| a.1.level(now).total_cmp(&b.1.level(now)))
                .map(|(k, _)| *k);
            match fullest {
                Some(k) => {
                    asks.remove(&k);
                }
                None => break,
            }
        }
        wait
    }

    /// Write one frame to one peer. The ONE place a peer is written to, so the
    /// eviction rule is stated once whatever the frame carries and whichever
    /// thread sends it.
    fn write(&self, p: &Peer, kind: u8, payload: &[u8]) -> bool {
        if !p.is_live() {
            return false;
        }
        let sent = p.out.lock().map_err(|_| io::Error::other("peer lock")).and_then(|mut s| {
            zap::write_frame(&mut *s, kind, payload)?;
            s.flush()
        });
        if sent.is_err() {
            // The socket will not take it. That peer is gone.
            p.fail();
            self.evicted.fetch_add(1, Ordering::SeqCst);
            return false;
        }
        true
    }

    /// Queue one frame for every live peer. Returns at once: each peer's own
    /// writer puts it on the wire, and the peers lock is let go before any
    /// queue is touched.
    fn send(&self, kind: u8, payload: Vec<u8>) {
        let frame = Arc::new(payload);
        let peers: Vec<Arc<Peer>> =
            self.peers.lock().expect("peers").iter().filter(|p| p.is_live()).cloned().collect();
        for p in peers {
            p.post(kind, Arc::clone(&frame));
        }
    }

    fn service(&self) -> Option<Arc<dyn Service>> {
        self.service.read().expect("service").clone()
    }

    /// Whether a transaction is one this link has not carried yet.
    fn first(&self, tx: &[u8]) -> bool {
        tx.len() <= TRANSACTION_MAX as usize && self.seen.lock().expect("seen").first(keccak256(tx))
    }

    /// Answer a question on the peer's own link.
    ///
    /// A question that is not twelve bytes is not a question; the peer keeps
    /// its seat and gets no answer, the way a malformed vote is dropped. Heights
    /// are served in order until one is missing, the count is reached, or the
    /// next record would take the frame past [`LINK_MAX`] — so an answer is
    /// always a run of consecutive heights starting where it was asked.
    fn answer(&self, p: &Peer, question: &[u8]) {
        let Ok(question) = <[u8; ASK_LEN]>::try_from(question) else {
            return;
        };
        let from = u64::from_be_bytes(question[..8].try_into().expect("8 bytes"));
        let count = u32::from_be_bytes(question[8..].try_into().expect("4 bytes")).min(BATCH_MAX);
        let (tip, records) = match self.service() {
            None => (0, Vec::new()),
            Some(s) => {
                let mut records = Vec::new();
                let mut size = ANSWER_HEAD;
                for height in (from..).take(count as usize) {
                    let Some(r) = s.record(height) else { break };
                    size += 8 + r.block.len() + r.cert.len();
                    if size > LINK_MAX as usize {
                        break;
                    }
                    records.push(r);
                }
                (s.tip(), records)
            }
        };
        self.write(p, ANSWER, &encode_answer(from, tip, &records));
    }
}

/// An answer's bytes.
fn encode_answer(from: u64, tip: u64, records: &[Record]) -> Vec<u8> {
    let mut w = zap::Writer::new();
    w.u64(from).u64(tip).u32(records.len() as u32);
    for r in records {
        w.bytes(&r.block).bytes(&r.cert);
    }
    w.take()
}

/// An answer out of its bytes, or nothing.
///
/// Nothing is believed that the frame does not hold exactly: more records than
/// were asked for, a field that runs past the payload, or a byte left over are
/// all a peer that answers garbage, and garbage is not an answer.
fn decode_answer(payload: &[u8], asked: u32) -> Option<Answer> {
    let mut r = zap::Reader::new(payload);
    let from = r.u64()?;
    let tip = r.u64()?;
    let n = r.u32()?;
    if n > asked.min(BATCH_MAX) {
        return None;
    }
    let mut records = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let block = r.bytes()?.to_vec();
        let cert = r.bytes()?.to_vec();
        records.push(Record { block, cert });
    }
    if r.remaining() != 0 {
        return None;
    }
    Some(Answer { from, tip, records })
}

/// A handle that relays transactions onto the mesh, from any thread.
///
/// Held by whatever takes transactions in — the RPC side of a chain — which
/// runs on threads the mesh does not own.
#[derive(Clone)]
pub struct Outlet(Arc<Core>);

impl Outlet {
    /// Send a transaction to every peer, once.
    ///
    /// Relayed by the node it was submitted to, and not again by the nodes that
    /// receive it: the mesh is full, so one hop reaches every validator that is
    /// connected, and a transaction is pushed N times rather than N².
    pub fn relay(&self, tx: &[u8]) {
        if self.0.first(tx) {
            self.0.send(TRANSACTION, tx.to_vec());
        }
    }
}

/// The peer set, and the votes arriving from it.
///
/// What peers send waits in each peer's own bounded queue; blocks are held as
/// bytes. The mesh does not know what a block is — parsing one is the chain's
/// business, and a transport that could misread a block is a transport that
/// can fork it.
pub struct Mesh {
    core: Arc<Core>,
    /// This node's own votes, on their way to its own tally.
    inbox: Receiver<SignedVote>,
    post: Sender<SignedVote>,
    /// Answers to this node's questions, by who gave them. Behind a lock so
    /// that asking is one question at a time.
    answers: Mutex<Receiver<(NodeId, Vec<u8>)>>,
    reply: Sender<(NodeId, Vec<u8>)>,
}

impl Mesh {
    pub fn new() -> Self {
        let (post, inbox) = mpsc::channel();
        let (reply, answers) = mpsc::channel();
        Mesh {
            core: Arc::new(Core {
                peers: Mutex::new(Vec::new()),
                joined: AtomicUsize::new(0),
                evicted: AtomicUsize::new(0),
                service: RwLock::new(None),
                seen: Mutex::new(Seen { order: VecDeque::new(), set: HashSet::new() }),
                asks: Mutex::new(HashMap::new()),
            }),
            inbox,
            post,
            answers: Mutex::new(answers),
            reply,
        }
    }

    /// Serve `service` to every peer: their questions are answered from it, and
    /// the transactions they relay are handed to it.
    pub fn serve(&self, service: Arc<dyn Service>) {
        *self.core.service.write().expect("service") = Some(service);
    }

    /// A handle that relays transactions from another thread.
    pub fn outlet(&self) -> Outlet {
        Outlet(Arc::clone(&self.core))
    }

    /// Ask `peers` at once for the blocks each accepted from `from`, at most
    /// `count`, and collect what they answer: until every one has, `wait`
    /// passes, or an answer carries records — what the asker came for. A
    /// silent peer costs the wait and no more, however many are asked.
    ///
    /// Only answers that parse come back. What comes back is only well formed
    /// — whether any record in it is a block anyone agreed to is the asker's
    /// question, and a transport that answered it would be one a peer could
    /// lie through.
    pub fn ask(
        &self,
        peers: &[NodeId],
        from: u64,
        count: u32,
        wait: Duration,
    ) -> Vec<(NodeId, Answer)> {
        if from == NONE {
            return Vec::new();
        }
        let answers = self.answers.lock().expect("answers");
        // Anything still here answered a question that already gave up.
        while answers.try_recv().is_ok() {}
        let mut asked: Vec<Arc<Peer>> = Vec::new();
        for p in self.core.peers.lock().expect("peers").iter() {
            if p.is_live() && peers.contains(&p.node) && !asked.iter().any(|a| a.node == p.node) {
                asked.push(Arc::clone(p));
            }
        }
        let count = count.min(BATCH_MAX);
        let mut question = [0u8; ASK_LEN];
        question[..8].copy_from_slice(&from.to_be_bytes());
        question[8..].copy_from_slice(&count.to_be_bytes());
        let question = Arc::new(question.to_vec());
        for p in &asked {
            p.awaiting.store(from, Ordering::SeqCst);
            p.post(ASK, Arc::clone(&question));
        }
        let mut waiting: HashSet<NodeId> = asked.iter().map(|p| p.node).collect();
        let mut got = Vec::new();
        let end = Instant::now() + wait;
        while !waiting.is_empty() {
            let left = end.saturating_duration_since(Instant::now());
            match answers.recv_timeout(left) {
                Ok((who, payload)) if waiting.remove(&who) => {
                    if let Some(a) = decode_answer(&payload, count).filter(|a| a.from == from) {
                        let full = !a.records.is_empty();
                        got.push((who, a));
                        if full {
                            break;
                        }
                    }
                }
                Ok(_) => continue,
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
            }
        }
        // Stop waiting, unless the reader already did.
        for p in &asked {
            let _ = p.awaiting.compare_exchange(from, NONE, Ordering::SeqCst, Ordering::SeqCst);
        }
        got
    }

    /// Take one connected peer, and start reading it.
    pub fn add(&self, node: NodeId, stream: TcpStream) -> io::Result<()> {
        stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
        stream.set_read_timeout(None)?;
        stream.set_nodelay(true)?;
        let rx = stream.try_clone()?;
        let ctl = stream.try_clone()?;
        let peer = Arc::new(Peer {
            node,
            out: Mutex::new(stream),
            ctl,
            live: AtomicBool::new(true),
            awaiting: AtomicU64::new(NONE),
            proposals: Mutex::new(VecDeque::new()),
            votes: Mutex::new(VecDeque::new()),
            outbound: Mutex::new(Outbound::default()),
            ready: Condvar::new(),
            reader: Mutex::new(None),
            writer: Mutex::new(None),
        });
        let handle =
            spawn_reader(Arc::clone(&peer), rx, Arc::clone(&self.core), self.reply.clone());
        *peer.reader.lock().expect("reader slot") = Some(handle);
        let handle = spawn_writer(Arc::clone(&peer), Arc::clone(&self.core));
        *peer.writer.lock().expect("writer slot") = Some(handle);
        let mut peers = self.core.peers.lock().expect("peers");
        // A peer that is gone and holds nothing is let go here, with its
        // sockets: one that reconnects over and over costs a slot, not a slot
        // a time.
        peers.retain(|p| !p.spent());
        peers.push(peer);
        self.core.joined.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// Links taken so far, reconnects included: a change is a peer that may
    /// have missed what was said before it came.
    pub fn joined(&self) -> usize {
        self.core.joined.load(Ordering::SeqCst)
    }

    /// Peers still connected.
    pub fn len(&self) -> usize {
        self.core.peers.lock().expect("peers").iter().filter(|p| p.is_live()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Peers dropped so far by the eviction rule.
    pub fn evicted(&self) -> usize {
        self.core.evicted.load(Ordering::SeqCst)
    }

    /// The identities of the peers still connected.
    pub fn members(&self) -> Vec<NodeId> {
        self.core
            .peers
            .lock()
            .expect("peers")
            .iter()
            .filter(|p| p.is_live())
            .map(|p| p.node)
            .collect()
    }

    /// Every vote that has arrived and not yet been taken, with the link it
    /// came in on — this node's own, under its own signer, then each peer's.
    /// The link is proven; who a vote says signed it is not, until it is
    /// verified. Never blocks.
    pub fn drain(&self) -> Vec<(NodeId, SignedVote)> {
        let mut votes: Vec<(NodeId, SignedVote)> =
            self.inbox.try_iter().map(|v| (v.node, v)).collect();
        for p in self.core.peers.lock().expect("peers").iter() {
            votes.extend(p.votes.lock().expect("a peer's queue").drain(..).map(|v| (p.node, v)));
        }
        votes
    }

    /// Every proposal that has arrived and not yet been taken.
    pub fn proposals(&self) -> Vec<Proposal> {
        let mut proposals = Vec::new();
        for p in self.core.peers.lock().expect("peers").iter() {
            proposals.extend(p.proposals.lock().expect("a peer's queue").drain(..));
        }
        proposals
    }

    /// Send a proposal to every peer — this node's own, or one it relays.
    ///
    /// A validator votes on what it has executed, so a proposer that did not
    /// push its block would be asking the others to sign for something they
    /// have never seen.
    pub fn propose(&self, proposal: &Proposal) {
        self.core.send(BLOCK, proposal.encode());
    }

    /// Stop the readers and close every socket.
    ///
    /// The service is let go too: it holds an [`Outlet`] onto this mesh, and
    /// the two holding each other would outlive the node.
    pub fn close(&self) {
        *self.core.service.write().expect("service") = None;
        let mut peers = self.core.peers.lock().expect("peers");
        for p in peers.iter() {
            p.fail();
        }
        for p in peers.drain(..) {
            for slot in [&p.reader, &p.writer] {
                let handle = slot.lock().expect("thread slot").take();
                if let Some(h) = handle {
                    let _ = h.join();
                }
            }
        }
    }
}

impl Default for Mesh {
    fn default() -> Self {
        Mesh::new()
    }
}

impl Drop for Mesh {
    fn drop(&mut self) {
        self.close();
    }
}

impl VoteTransport for Mesh {
    /// Disseminate this node's own vote.
    ///
    /// The vote is posted to our own inbox first: a validator's own vote must
    /// reach its own tally, and routing it the same way as every other vote
    /// means there is one path a vote can arrive by, not two.
    fn broadcast(&self, vote: &SignedVote) {
        let _ = self.post.send(vote.clone());
        self.core.send(VOTE, vote.encode());
    }
}

/// Write what is queued for one peer until the peer is gone.
fn spawn_writer(peer: Arc<Peer>, core: Arc<Core>) -> JoinHandle<()> {
    std::thread::spawn(move || loop {
        let (kind, frame) = {
            let mut q = peer.outbound.lock().expect("a peer's outbound");
            loop {
                if !peer.is_live() {
                    return;
                }
                if let Some((kind, frame)) = q.frames.pop_front() {
                    q.bytes -= frame.len();
                    break (kind, frame);
                }
                q = peer.ready.wait(q).expect("a peer's outbound");
            }
        };
        if !core.write(&peer, kind, &frame) {
            return;
        }
    })
}

/// Read frames from one peer until the stream ends or the framing breaks.
fn spawn_reader(
    peer: Arc<Peer>,
    mut rx: TcpStream,
    core: Arc<Core>,
    reply: Sender<(NodeId, Vec<u8>)>,
) -> JoinHandle<()> {
    std::thread::spawn(move || loop {
        match zap::read_frame(&mut rx, LINK_MAX) {
            // An answer, taken only if it answers the question awaited.
            Ok((ANSWER, payload)) => {
                let Some(from) =
                    payload.get(..8).map(|b| u64::from_be_bytes(b.try_into().expect("8")))
                else {
                    continue;
                };
                let awaited = peer
                    .awaiting
                    .compare_exchange(from, NONE, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok();
                if awaited && reply.send((peer.node, payload)).is_err() {
                    return;
                }
            }
            Ok((raw, payload)) => match zap::strip_flags(raw) {
                VOTE => {
                    // A vote has a fixed size. Anything else claiming to be one
                    // is refused before the decoder is asked, so a peer cannot
                    // spend our time on a frame that cannot be a vote.
                    if payload.len() > VOTE_MAX as usize {
                        continue;
                    }
                    match SignedVote::decode(&payload) {
                        // A well-formed frame whose contents were a lie. The
                        // peer keeps its seat; the lie goes nowhere.
                        Err(_) => continue,
                        Ok(v) => keep(&peer.votes, v, VOTES_MAX),
                    }
                }
                BLOCK => match Proposal::decode(peer.node, payload) {
                    Some(p) => keep(&peer.proposals, p, ARRIVALS_MAX),
                    None => continue,
                },
                ASK => {
                    // Paced here, on this peer's own thread: a peer asking
                    // faster than it is answered waits, and nobody else does.
                    std::thread::sleep(core.pace(peer.node));
                    core.answer(&peer, &payload)
                }
                TRANSACTION => {
                    if core.first(&payload) {
                        if let Some(s) = core.service() {
                            s.transaction(&payload);
                        }
                    }
                }
                // Not something this link carries. The frame was well formed,
                // so the peer keeps its seat.
                _ => continue,
            },
            // The stream ended or the framing was violated. Either way this
            // peer is finished; the quorum rule decides what that costs.
            Err(_) => {
                peer.fail();
                return;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A mesh holding one peer, and the raw far end of that peer's socket.
    fn linked() -> (Mesh, NodeId, TcpStream) {
        let l = TcpListener::bind("127.0.0.1:0").expect("bind");
        let near = TcpStream::connect(l.local_addr().expect("addr")).expect("dial");
        let (far, _) = l.accept().expect("accept");
        far.set_read_timeout(Some(Duration::from_secs(5))).expect("timeout");
        let mesh = Mesh::new();
        let peer = [7u8; 20];
        mesh.add(peer, near).expect("add");
        (mesh, peer, far)
    }

    fn question(from: u64, count: u32) -> Vec<u8> {
        let mut q = from.to_be_bytes().to_vec();
        q.extend_from_slice(&count.to_be_bytes());
        q
    }

    /// Heights 1 to `top`, each block `size` bytes stamped with its height.
    struct Shelf {
        top: u64,
        size: usize,
        taken: Mutex<Vec<Vec<u8>>>,
    }

    impl Service for Shelf {
        fn tip(&self) -> u64 {
            self.top
        }
        fn record(&self, height: u64) -> Option<Record> {
            (1..=self.top).contains(&height).then(|| {
                let mut block = vec![0u8; self.size];
                block[..8].copy_from_slice(&height.to_be_bytes());
                Record { block, cert: vec![0xce; 10] }
            })
        }
        fn transaction(&self, raw: &[u8]) {
            self.taken.lock().expect("taken").push(raw.to_vec());
        }
    }

    fn shelf(top: u64, size: usize) -> Arc<Shelf> {
        Arc::new(Shelf { top, size, taken: Mutex::new(Vec::new()) })
    }

    fn ask(far: &mut TcpStream, from: u64, count: u32) -> Answer {
        zap::write_frame(far, ASK, &question(from, count)).expect("ask");
        let (kind, payload) = zap::read_frame(far, LINK_MAX).expect("answer");
        assert_eq!(kind, ANSWER, "an answer carries the response flag");
        decode_answer(&payload, count).expect("a well-formed answer")
    }

    fn height(r: &Record) -> u64 {
        u64::from_be_bytes(r.block[..8].try_into().expect("8"))
    }

    #[test]
    fn a_question_is_answered_in_order_and_in_bounds() {
        let (mesh, _, mut far) = linked();
        mesh.serve(shelf(100, 1000));

        // Asked for a thousand: given the batch, from where it was asked.
        let a = ask(&mut far, 1, 1000);
        assert_eq!((a.from, a.tip), (1, 100));
        assert_eq!(a.records.len(), BATCH_MAX as usize, "no more than a batch");
        assert!(a.records.iter().zip(1..).all(|(r, h)| height(r) == h), "consecutive heights");

        // A missing height ends the run.
        let a = ask(&mut far, 90, 64);
        assert_eq!(a.records.len(), 11, "90 to 100, and nothing past what it holds");

        // Beyond the tip: an empty answer, still an answer.
        assert!(ask(&mut far, 101, 64).records.is_empty());

        // Twelve bytes or it is not a question. No answer, and the peer keeps
        // its seat: the next question is answered.
        zap::write_frame(&mut far, ASK, &question(1, 4)[..11]).expect("short question");
        assert_eq!(ask(&mut far, 5, 2).from, 5, "the next frame is the answer to this one");
        assert_eq!(mesh.len(), 1);
    }

    #[test]
    fn an_answer_stops_before_the_link_limit() {
        let (mesh, _, mut far) = linked();
        // Three 900 KiB blocks do not fit a 2 MiB frame; two do.
        mesh.serve(shelf(10, 900 * 1024));
        let a = ask(&mut far, 1, 10);
        assert_eq!(a.records.len(), 2);
        assert!(
            a.records.iter().map(|r| r.block.len() + r.cert.len()).sum::<usize>()
                < LINK_MAX as usize
        );
    }

    #[test]
    fn an_answer_to_nothing_is_not_taken() {
        let (mesh, peer, mut far) = linked();
        let answer = |from: u64, n: u32, extra: &[u8]| {
            let mut w = zap::Writer::new();
            w.u64(from).u64(9).u32(n);
            for _ in 0..n {
                w.bytes(b"block").bytes(b"cert");
            }
            let mut p = w.take();
            p.extend_from_slice(extra);
            p
        };

        // Sent before anyone asked: dropped as it is read, so it cannot stand
        // in for the answer to a later question. The transaction behind it says
        // when the reader has been through it — frames are read in order.
        let pool = shelf(0, 0);
        mesh.serve(pool.clone());
        zap::write_frame(&mut far, ANSWER, &answer(1, 1, &[])).expect("unsolicited");
        zap::write_frame(&mut far, TRANSACTION, b"read past it").expect("marker");
        while pool.taken.lock().expect("taken").is_empty() {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(mesh.ask(&[peer], 1, 4, Duration::from_millis(300)).is_empty(), "nobody answered");
        // The question went out; its answer comes after the asker gave up, and
        // is dropped the same way.
        let (kind, _) = zap::read_frame(&mut far, LINK_MAX).expect("the question that gave up");
        assert_eq!(kind, ASK);
        zap::write_frame(&mut far, ANSWER, &answer(1, 1, &[])).expect("late");

        // A late answer to an earlier question, then the real one.
        // The far end is handed back rather than dropped: a closed socket is a
        // peer gone, and this test is about a peer that stays and lies.
        let responder = std::thread::spawn(move || {
            let mut next = |reply: Vec<Vec<u8>>| {
                let (kind, _) = zap::read_frame(&mut far, LINK_MAX).expect("question");
                assert_eq!(kind, ASK);
                for r in reply {
                    zap::write_frame(&mut far, ANSWER, &r).expect("reply");
                }
            };
            next(vec![answer(6, 1, &[]), answer(7, 2, &[])]);
            next(vec![answer(8, 3, &[])]); // more records than asked
            next(vec![answer(9, 1, &[0])]); // a byte left over
            next(vec![b"\x00\x00\x00\x00\x00\x00\x00\x0a garbage".to_vec()]);
            far
        });
        let began = Instant::now();
        let a = mesh.ask(&[peer], 7, 2, Duration::from_secs(5)).pop().expect("the answer").1;
        assert_eq!((a.from, a.records.len()), (7, 2));
        assert!(mesh.ask(&[peer], 8, 2, Duration::from_secs(5)).is_empty(), "more than asked");
        assert!(mesh.ask(&[peer], 9, 2, Duration::from_secs(5)).is_empty(), "trailing bytes");
        assert!(mesh.ask(&[peer], 10, 2, Duration::from_secs(5)).is_empty(), "garbage");
        let _far = responder.join().expect("responder");
        assert!(began.elapsed() < Duration::from_secs(4), "refused on sight, not on a timeout");
        assert_eq!(mesh.len(), 1, "a liar keeps its seat; it is simply not believed");
    }

    #[test]
    fn a_transaction_is_relayed_once_and_taken_once() {
        let (mesh, _, mut far) = linked();
        let pool = shelf(0, 0);
        mesh.serve(pool.clone());

        // Out: one frame however many times it is relayed.
        let out = mesh.outlet();
        out.relay(b"tx one");
        out.relay(b"tx one");
        out.relay(&vec![1u8; TRANSACTION_MAX as usize + 1]); // too large to carry
        let (kind, payload) = zap::read_frame(&mut far, LINK_MAX).expect("relayed");
        assert_eq!((kind, payload.as_slice()), (TRANSACTION, &b"tx one"[..]));
        far.set_read_timeout(Some(Duration::from_millis(200))).expect("timeout");
        assert!(zap::read_frame(&mut far, LINK_MAX).is_err(), "sent once, and nothing else");

        // In: taken once, and not at all if this node already relayed it.
        for tx in [&b"tx two"[..], b"tx two", b"tx one"] {
            zap::write_frame(&mut far, TRANSACTION, tx).expect("relay in");
        }
        zap::write_frame(&mut far, TRANSACTION, &vec![2u8; TRANSACTION_MAX as usize + 1])
            .expect("too large");
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(*pool.taken.lock().expect("taken"), vec![b"tx two".to_vec()]);
    }

    /// Frames are read in order, so a transaction sent after `n` frames is
    /// taken only once the reader is past all of them.
    fn read_past(far: &mut TcpStream, pool: &Shelf, marker: &[u8]) {
        zap::write_frame(far, TRANSACTION, marker).expect("marker");
        while !pool.taken.lock().expect("taken").iter().any(|t| t == marker) {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn a_peer_streaming_proposals_holds_no_more_than_its_bound() {
        let (mesh, peer, mut far) = linked();
        let pool = shelf(0, 0);
        mesh.serve(pool.clone());
        const SENT: u64 = 300;
        for slot in 0..SENT {
            let p = Proposal {
                from: [0; 20],
                height: 9,
                slot,
                signature: vec![3; crate::pq::IDENTITY_SIGNATURE_LEN],
                block: vec![7; 64 * 1024],
            };
            zap::write_frame(&mut far, BLOCK, &p.encode()).expect("a proposal");
        }
        // Not proposals: a header short, a header over nothing, and what v0.2
        // sent under this id — a bare block, an RLP list longer than a header.
        zap::write_frame(&mut far, BLOCK, &vec![0u8; PROPOSAL_HEAD - 1]).expect("short");
        zap::write_frame(&mut far, BLOCK, &vec![0u8; PROPOSAL_HEAD]).expect("empty");
        let mut bare = vec![0xf9, 0x20, 0x00];
        bare.resize(8 * 1024, 0x80);
        zap::write_frame(&mut far, BLOCK, &bare).expect("a v0.2 block");
        read_past(&mut far, &pool, b"past the proposals");
        let held = mesh.proposals();
        assert_eq!(held.len(), ARRIVALS_MAX, "{SENT} sent, and no more than the bound held");
        let kept: Vec<u64> = held.iter().map(|p| p.slot).collect();
        let newest: Vec<u64> = (SENT - ARRIVALS_MAX as u64..SENT).collect();
        assert_eq!(kept, newest, "the newest are the ones kept");
        assert!(held.iter().all(|p| p.from == peer && p.height == 9), "kept with its link");
        assert_eq!(held[0].signature, vec![3; crate::pq::IDENTITY_SIGNATURE_LEN]);
        assert_eq!(held[0].block.len(), 64 * 1024, "the block is what follows the header");
        assert_eq!(mesh.len(), 1, "sending proposals is not a reason to be dropped");
    }

    #[test]
    fn a_peer_streaming_votes_holds_no_more_than_its_bound() {
        use lux_consensus::finality::Position;
        let (mesh, peer, mut far) = linked();
        let pool = shelf(0, 0);
        mesh.serve(pool.clone());
        let sk = blst::min_pk::SecretKey::key_gen(&[1u8; 32], &[]).expect("key");
        let signature = sk.sign(b"any", lux_consensus::cert::DST, &[]).compress().to_vec();
        const SENT: u64 = 5_000;
        for height in 0..SENT {
            let v = SignedVote {
                position: Position {
                    chain_id: [1; 32],
                    height,
                    round: 0,
                    block_id: [2; 32],
                    parent_id: [3; 32],
                    canonical_id: [2; 32],
                    parent_canonical_id: [3; 32],
                    execution_state_root: [4; 32],
                    payload_root: [5; 32],
                    validator_set_root: [6; 32],
                },
                accept: true,
                node: [9; 20],
                signature: signature.clone(),
            };
            zap::write_frame(&mut far, VOTE, &v.encode()).expect("a vote");
        }
        read_past(&mut far, &pool, b"past the votes");
        let held = mesh.drain();
        assert_eq!(held.len(), VOTES_MAX, "{SENT} sent, and no more than the bound held");
        assert_eq!(held.first().map(|(_, v)| v.position.height), Some(SENT - VOTES_MAX as u64));
        assert_eq!(held.last().map(|(_, v)| v.position.height), Some(SENT - 1), "the newest");
        assert!(held.iter().all(|(link, v)| *link == peer && v.node == [9; 20]), "with its link");
    }

    #[test]
    fn a_peer_that_does_not_read_holds_no_sender_and_no_more_than_its_bound() {
        // The far end never reads: the kernel's buffers fill, and a write to it
        // would block for its whole timeout.
        let (mesh, _, far) = linked();
        let p = Proposal {
            from: [0; 20],
            height: 1,
            slot: 0,
            signature: vec![3; crate::pq::IDENTITY_SIGNATURE_LEN],
            block: vec![7; BLOCK_MAX as usize],
        };
        let began = Instant::now();
        for _ in 0..16 {
            mesh.propose(&p);
        }
        assert!(began.elapsed() < WRITE_TIMEOUT / 4, "a send waited: {:?}", began.elapsed());
        let peer = mesh.core.peers.lock().expect("peers")[0].clone();
        let q = peer.outbound.lock().expect("outbound");
        assert!(q.bytes <= OUTBOUND_BYTES, "{} bytes queued for one peer", q.bytes);
        assert_eq!(q.bytes, q.frames.iter().map(|(_, f)| f.len()).sum::<usize>());
        drop(q);
        drop(far);
    }

    #[test]
    fn a_bucket_answers_a_burst_then_a_pace() {
        let start = Instant::now();
        let mut b = Bucket::new(start);
        for _ in 0..ASK_BURST {
            assert_eq!(b.take(start), Duration::ZERO, "a burst is answered at once");
        }
        let wait = b.take(start);
        assert!(wait > Duration::ZERO, "the next waits");
        assert!(wait <= Duration::from_secs(1) / ASK_RATE + Duration::from_millis(1), "{wait:?}");
        // Having waited, it is answered, and the one after waits again.
        let later = start + wait;
        assert!(b.take(later + Duration::from_secs(1) / ASK_RATE) <= wait, "one behind at most");
        // A peer that stops asking earns the burst back, and no more.
        let mut b = Bucket::new(start);
        let rested = start + Duration::from_secs(3600);
        for _ in 0..ASK_BURST {
            assert_eq!(b.take(rested), Duration::ZERO);
        }
        assert!(b.take(rested) > Duration::ZERO, "an hour's rest is still one burst");
    }

    #[test]
    fn a_peer_that_asks_too_often_is_answered_at_a_pace() {
        let (mesh, _, mut far) = linked();
        mesh.serve(shelf(10, 100));
        const ASKED: u32 = ASK_BURST + 24;
        for _ in 0..ASKED {
            zap::write_frame(&mut far, ASK, &question(1, 1)).expect("ask");
        }
        let began = Instant::now();
        let mut at = Vec::new();
        for _ in 0..ASKED {
            let (kind, _) = zap::read_frame(&mut far, LINK_MAX).expect("an answer");
            assert_eq!(kind, ANSWER);
            at.push(began.elapsed());
        }
        let early = at.iter().filter(|t| **t < Duration::from_millis(500)).count() as u32;
        assert!(early >= ASK_BURST, "the burst is answered at once: {early}");
        assert!(early <= ASK_BURST + ASK_RATE / 2 + 1, "then a pace, not a flood: {early}");
        let last = *at.last().expect("answers");
        let floor = Duration::from_secs_f64(f64::from(ASKED - ASK_BURST - 1) / f64::from(ASK_RATE));
        assert!(last >= floor, "{ASKED} answers in {last:?}");
        assert_eq!(mesh.len(), 1, "a peer that asks often is paced, not dropped");
    }

    #[test]
    fn a_validator_that_reconnects_comes_back_to_its_pace() {
        let (mesh, peer, mut far) = linked();
        mesh.serve(shelf(10, 100));
        let answered = |far: &mut TcpStream, n: u32| {
            for _ in 0..n {
                zap::write_frame(far, ASK, &question(1, 1)).expect("ask");
            }
            let began = Instant::now();
            for _ in 0..n {
                let (kind, _) = zap::read_frame(far, LINK_MAX).expect("an answer");
                assert_eq!(kind, ANSWER);
            }
            began.elapsed()
        };
        assert!(answered(&mut far, ASK_BURST) < Duration::from_millis(500), "a burst, at once");

        // The same validator on a new link: the burst it spent stays spent.
        let dial = |node: NodeId| {
            let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            let near = TcpStream::connect(l.local_addr().expect("addr")).expect("dial");
            let (far, _) = l.accept().expect("accept");
            far.set_read_timeout(Some(Duration::from_secs(5))).expect("timeout");
            mesh.add(node, near).expect("add");
            far
        };
        let mut again = dial(peer);
        let paced = answered(&mut again, 8);
        assert!(paced >= Duration::from_millis(300), "a reconnect was a fresh burst: {paced:?}");

        // Another validator has its own.
        let mut other = dial([8u8; 20]);
        assert!(answered(&mut other, 8) < Duration::from_millis(250), "its own burst");
    }

    #[test]
    fn the_paces_remembered_are_bounded() {
        let mesh = Mesh::new();
        for i in 0..ASKERS_MAX as u32 + 50 {
            let mut node = [0u8; 20];
            node[..4].copy_from_slice(&i.to_be_bytes());
            mesh.core.pace(node);
            assert!(mesh.core.asks.lock().expect("paces").len() <= ASKERS_MAX);
        }
        // The one asking is never the one let go.
        let last = (ASKERS_MAX as u32 + 49).to_be_bytes();
        assert!(mesh.core.asks.lock().expect("paces").keys().any(|k| k[..4] == last));
    }

    #[test]
    fn what_is_remembered_is_bounded() {
        let mut seen = Seen { order: VecDeque::new(), set: HashSet::new() };
        let hash = |i: usize| B256::from(keccak256(i.to_be_bytes()));
        for i in 0..=SEEN_MAX {
            assert!(seen.first(hash(i)));
        }
        assert!(!seen.first(hash(SEEN_MAX)), "the newest is remembered");
        assert_eq!(seen.set.len(), SEEN_MAX, "and no more than the bound");
        assert!(seen.first(hash(0)), "the oldest was let go");
    }
}
