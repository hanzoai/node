// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! One signature a height, converged — on real sockets, real EVM blocks, real
//! BLS and real ML-DSA proposer signatures.
//!
//! v0.2.1 let every seat holding transactions propose at once: five blocks at
//! one height, each seat signing its own, each refusing the others for good.
//! These hold down Go's rule in its place (luxfi/consensus engine/chain, B1):
//!
//! - one proposer a window, its proposal signed; a proposal signed by anyone
//!   else for that window is refused, and two live blocks converge on the
//!   lowest id, so one certificate forms (`convergedWinnerAtHeightLocked`);
//! - a proposer that goes silent costs its window and no more;
//! - a validator signs one block a height, the converged winner — not the one
//!   it saw first — and after a restart says that same statement again, never a
//!   fresh one (the clone recovery of `emitConvergedVote`,
//!   `multinode_clone_test.go`);
//! - one validator that missed the proposal and the votes cannot hold the
//!   height: the retry says them again (`rePollAllPending`);
//! - the block a validator signs is kept before it signs, so a whole committee
//!   restarted mid-height completes the height on it;
//! - a height v0.2.x left split stays uncertified unless one of its blocks can
//!   still be certified — Go's non-equivocation guarantee, stated as a test.

use std::collections::{BTreeMap, BTreeSet};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use alloy_primitives::{keccak256, Address, U256};
use blst::min_pk::SecretKey;
use lux_consensus::cert::{NodeId, QuorumCert, DST};
use lux_consensus::finality::canonical_vote_message;
use lux_consensus::pop;
use lux_consensus::vote::{read_message, SignedVote, Slot, VoteTransport as _};
use lux_node::engine::{Committee, Engine, Event};
use lux_node::evm::Evm;
use lux_node::ledger::{self, Ledger};
use lux_node::pq::Identity;
use lux_node::vm::{Block, Vm};
use lux_node::{Config, Held, Host, Journal, Relay};
use serde_json::json;

const CHAIN: [u8; 32] = [0xc1; 32];
const EPOCH: u64 = 1;
const WINDOW: Duration = Duration::from_millis(500);
const REACH: Duration = Duration::from_secs(20);

struct Cast {
    ids: Vec<Arc<Identity>>,
    keys: Vec<SecretKey>,
    committee: Committee,
}

/// `n` validators, ordered by node id.
fn cast(n: u8) -> Cast {
    let mut ids: Vec<Arc<Identity>> =
        (0..n).map(|_| Arc::new(Identity::generate().expect("identity"))).collect();
    ids.sort_by_key(|id| id.node());
    let mut keys = Vec::new();
    let mut lines = String::new();
    for (i, id) in ids.iter().enumerate() {
        let sk = SecretKey::key_gen(&[i as u8 + 1; 32], &[]).expect("key_gen");
        let pk = sk.sk_to_pk().compress();
        let proof = pop::sign_over(&sk, &Committee::claim(&CHAIN, &id.node(), &pk));
        lines.push_str(&Committee::line(id.public(), &pk, &proof));
        lines.push('\n');
        keys.push(sk);
    }
    Cast { ids, keys, committee: Committee::read(&lines, CHAIN).expect("committee") }
}

impl Cast {
    fn index(&self, node: NodeId) -> usize {
        self.ids.iter().position(|id| id.node() == node).expect("a member")
    }
}

/// A directory that removes itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("lux-converge-{}-{tag}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Account `i`'s key: every validator's load comes from its own sender.
fn signer(i: u8) -> k256::ecdsa::SigningKey {
    k256::ecdsa::SigningKey::from_bytes(&[i + 7; 32].into()).expect("key")
}

fn account(i: u8) -> Address {
    let point = signer(i).verifying_key().to_encoded_point(false);
    Address::from_slice(&keccak256(&point.as_bytes()[1..])[12..])
}

fn genesis() -> lux_evm::Genesis {
    lux_evm::Genesis {
        alloc: (0..12)
            .map(|i| (account(i), lux_evm::Alloc::funded(U256::from(10u64).pow(U256::from(21u64)))))
            .collect(),
        // Several blocks a second, and the test is about agreement, not fees:
        // a schedule under which neither the base fee nor the block gas cost
        // moves, at the genesis's own 12M gas limit.
        fee: lux_evm::fee::Config {
            gas_limit: 12_000_000,
            target_gas: 1_000_000_000_000,
            block_gas_cost_step: 0,
            ..lux_evm::fee::Config::DEFAULT
        },
        ..lux_evm::genesis::localnet()
    }
}

/// A transfer from account `from`; `to` picks the recipient, so two
/// transactions at one nonce make two different blocks.
fn tx(from: u8, nonce: u64, to: u8) -> Vec<u8> {
    use alloy_consensus::{SignableTransaction, TxEip1559};
    use alloy_eips::eip2718::Encodable2718 as _;
    use alloy_primitives::Signature;
    use revm::primitives::TxKind;

    let t = TxEip1559 {
        chain_id: genesis().chain_id,
        nonce,
        gas_limit: 100_000,
        max_fee_per_gas: 30_000_000_000,
        max_priority_fee_per_gas: 1_000_000_000,
        to: TxKind::Call(Address::repeat_byte(to)),
        value: U256::from(1u64),
        ..Default::default()
    };
    let (sig, recid) =
        signer(from).sign_prehash_recoverable(t.signature_hash().as_slice()).expect("sign");
    alloy_consensus::TxEnvelope::Eip1559(t.into_signed(Signature::new(
        U256::from_be_slice(&sig.r().to_bytes()),
        U256::from_be_slice(&sig.s().to_bytes()),
        recid.is_y_odd(),
    )))
    .encoded_2718()
}

/// A block at height 1 carrying one transfer, built on a chain of its own.
fn block(from: u8, to: u8) -> Box<dyn Block> {
    let other = Evm::new(genesis());
    other.submit(&tx(from, 0, to)).expect("submit");
    other.build().expect("build")
}

/// Validator `i`'s signature for `block`, as it would sign it.
fn vote(cast: &Cast, engine: &Engine, i: usize, block: &dyn Block) -> SignedVote {
    let position = engine.position(block);
    let message = canonical_vote_message(&position, true);
    SignedVote {
        position,
        accept: true,
        node: cast.ids[i].node(),
        signature: cast.keys[i].sign(&message, DST, &[]).compress().to_vec(),
    }
}

/// A validator, wired the way `run.rs` wires one, on a data directory it
/// keeps across a restart.
struct Node {
    engine: Engine,
    host: Host,
    chain: Arc<Relay>,
    evm: Arc<Evm>,
    ledger: Arc<Ledger>,
    votes: Journal,
    data: PathBuf,
    /// (height, built) of each block this validator decided live.
    decided: Vec<(u64, bool)>,
    /// The most votes any retry saw on one block, by height.
    waited: BTreeMap<u64, usize>,
}

impl Node {
    fn start(cast: &Cast, i: usize, dir: &Path, bind: SocketAddr, window: Duration) -> Node {
        let data = dir.join(format!("n{i}"));
        std::fs::create_dir_all(&data).expect("data");
        let host = Host::bind(Config { identity: cast.ids[i].clone(), bind }).expect("bind");
        let evm = Arc::new(Evm::open(genesis(), &data).expect("chain"));
        let ledger = Arc::new(Ledger::open(data.join("certs")).expect("ledger"));
        let engine =
            Engine::new(cast.ids[i].node(), cast.keys[i].clone(), &cast.committee, EPOCH, CHAIN)
                .expect("engine")
                .with_ledger(ledger.clone())
                .with_held(Held::open(data.join("held")).expect("held"))
                .with_window(window);
        let chain = Relay::attach(evm.clone(), ledger.clone(), &host);
        let votes = Journal::open(data.join("votes")).expect("votes");
        Node {
            engine,
            host,
            chain,
            evm,
            ledger,
            votes,
            data,
            decided: Vec::new(),
            waited: BTreeMap::new(),
        }
    }

    fn step(&mut self) -> bool {
        let step = self.engine.step(self.chain.as_ref(), &self.host, &mut self.votes);
        for e in &step.events {
            match e {
                Event::Decided { height, built, .. } => self.decided.push((*height, *built)),
                Event::Waiting { height, most, .. } => {
                    let w = self.waited.entry(*height).or_default();
                    *w = (*w).max(*most);
                }
                _ => {}
            }
        }
        step.quiet && step.events.is_empty()
    }

    fn height(&self) -> u64 {
        self.evm.height()
    }

    fn nonce(&self, i: u8) -> u64 {
        let n = self
            .chain
            .call("eth_getTransactionCount", &json!([format!("{:#x}", account(i)), "latest"]))
            .expect("nonce");
        u64::from_str_radix(n.as_str().expect("a quantity").trim_start_matches("0x"), 16)
            .expect("hex")
    }

    fn send(&self, raw: &[u8]) {
        self.chain
            .call(
                "eth_sendRawTransaction",
                &json!([format!("0x{}", alloy_primitives::hex::encode(raw))]),
            )
            .expect("admitted");
    }

    /// The certificate this validator accepted `height` on.
    fn cert(&self, height: u64) -> QuorumCert {
        ledger::decode(&self.ledger.get(height).expect("a certificate").1).expect("decodes")
    }

    /// The block this validator signed at `height`, if it signed one.
    fn signed(&self, height: u64) -> Option<[u8; 32]> {
        let slot = Slot { chain: CHAIN, height, round: 0 };
        self.votes
            .standing(&slot)
            .map(|m| read_message(m).expect("a statement").0.signed_identity())
    }
}

fn addresses(n: usize) -> Vec<SocketAddr> {
    (0..n)
        .map(|_| TcpListener::bind("127.0.0.1:0").expect("bind").local_addr().expect("addr"))
        .collect()
}

fn peers(cast: &Cast, addrs: &[SocketAddr]) -> BTreeMap<NodeId, SocketAddr> {
    cast.ids.iter().zip(addrs).map(|(id, a)| (id.node(), *a)).collect()
}

/// Each host sweeps its own peer map, on its own thread — a meeting has two
/// halves and one is always waiting on the other — until it holds every peer
/// in it.
fn meet(hosts: Vec<(&mut Host, BTreeMap<NodeId, SocketAddr>)>) {
    std::thread::scope(|s| {
        for (h, peers) in hosts {
            s.spawn(move || {
                let peers = &peers;
                let want: BTreeSet<NodeId> =
                    peers.keys().copied().filter(|n| *n != h.node()).collect();
                let held = || h.mesh().members().into_iter().collect::<BTreeSet<_>>();
                let end = Instant::now() + REACH;
                while !want.is_subset(&held()) && Instant::now() < end {
                    h.sweep(peers);
                    std::thread::sleep(Duration::from_millis(10));
                }
                assert!(want.is_subset(&held()), "a validator that is up meets the others");
            });
        }
    });
}

/// The addresses of the nodes that are up. A node held back is still bound,
/// and a dial to it would wait out a handshake nobody answers.
fn present(
    nodes: &[Option<Node>],
    peers: &BTreeMap<NodeId, SocketAddr>,
) -> BTreeMap<NodeId, SocketAddr> {
    let up: BTreeSet<NodeId> = nodes.iter().flatten().map(|n| n.host.node()).collect();
    peers.iter().filter(|(k, _)| up.contains(*k)).map(|(k, v)| (*k, *v)).collect()
}

/// Every node that is up meets every other that is up.
fn mesh(nodes: &mut [Option<Node>], peers: &BTreeMap<NodeId, SocketAddr>) {
    let present = present(nodes, peers);
    meet(nodes.iter_mut().flatten().map(|n| (&mut n.host, present.clone())).collect());
}

/// Step every node that is up, each on its own thread and meeting the others
/// as it goes, until `done` holds for all of them. Panics past `limit`.
fn run(
    nodes: &mut [Option<Node>],
    peers: &BTreeMap<NodeId, SocketAddr>,
    limit: Duration,
    done: impl Fn(&Node) -> bool + Sync,
) {
    let up = nodes.iter().flatten().count();
    let peers = &present(nodes, peers);
    let finished = AtomicUsize::new(0);
    let end = Instant::now() + limit;
    std::thread::scope(|s| {
        for n in nodes.iter_mut().flatten() {
            let (finished, done) = (&finished, &done);
            s.spawn(move || {
                let mut counted = false;
                let mut met = Instant::now();
                while Instant::now() < end && finished.load(Ordering::SeqCst) < up {
                    if met.elapsed() >= Duration::from_millis(100) {
                        n.host.sweep(peers);
                        met = Instant::now();
                    }
                    let quiet = n.step();
                    if !counted && done(n) {
                        counted = true;
                        finished.fetch_add(1, Ordering::SeqCst);
                    }
                    if quiet {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
            });
        }
    });
    assert_eq!(finished.load(Ordering::SeqCst), up, "the nodes did not get there in {limit:?}");
}

/// Step every node that is up for `d`, whatever happens.
fn idle(nodes: &mut [Option<Node>], peers: &BTreeMap<NodeId, SocketAddr>, d: Duration) {
    let peers = &present(nodes, peers);
    let end = Instant::now() + d;
    std::thread::scope(|s| {
        for n in nodes.iter_mut().flatten() {
            s.spawn(move || {
                let mut met = Instant::now();
                while Instant::now() < end {
                    if met.elapsed() >= Duration::from_millis(100) {
                        n.host.sweep(peers);
                        met = Instant::now();
                    }
                    if n.step() {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
            });
        }
    });
}

/// The cluster agrees: the same block at every height up to the lowest tip,
/// each on a certificate that verifies.
fn agree(nodes: &[Option<Node>]) -> u64 {
    let up: Vec<&Node> = nodes.iter().flatten().collect();
    let low = up.iter().map(|n| n.height()).min().expect("a node");
    for h in 1..=low {
        let id = up[0].chain.block_id_at(h).expect("a block");
        for n in &up {
            assert_eq!(n.chain.block_id_at(h).expect("a block"), id, "height {h}: one chain");
            n.cert(h).verify_weighted(n.engine.set(), n.engine.set(), EPOCH).expect("certified");
        }
    }
    low
}

/// Every seat holds transactions, every seat outside the first window
/// proposes anyway, and the second window's proposer offers a block of its own
/// while the first's is live. The out-of-window proposals are refused; the two
/// real ones converge on the lower id; every seat signs that one, and one
/// certificate forms.
#[test]
fn concurrent_proposals_at_one_height_resolve_to_one_certified_block() {
    const PER: u64 = 24;
    let cast = cast(5);
    let scratch = Scratch::new("concurrent");
    let addrs = addresses(5);
    let peers = peers(&cast, &addrs);
    let mut nodes: Vec<Option<Node>> =
        (0..5).map(|i| Some(Node::start(&cast, i, &scratch.0, addrs[i], WINDOW))).collect();
    mesh(&mut nodes, &peers);

    for (i, n) in nodes.iter().flatten().enumerate() {
        n.send(&tx(i as u8, 0, 0xa0 + i as u8));
    }
    let end = Instant::now() + REACH;
    while nodes.iter().flatten().any(|n| n.evm.pending() < 5) {
        assert!(Instant::now() < end, "the transactions were relayed");
        std::thread::sleep(Duration::from_millis(5));
    }
    let engine = &nodes[0].as_ref().expect("up").engine;
    let (first, second) = (engine.proposer(1, 0), engine.proposer(1, 1));
    let genesis_id = nodes[0].as_ref().expect("up").chain.last_accepted();

    // Out of their window: every other seat signs a block of its own and
    // claims the first window with it.
    let mut rogue = BTreeSet::new();
    for (k, n) in nodes.iter().flatten().filter(|n| n.host.node() != first).enumerate() {
        n.evm.submit(&tx(10, 0, 0xd0 + k as u8)).expect("its own");
        let b = n.chain.build().expect("a rogue block");
        rogue.insert(b.id());
        n.host
            .propose(&CHAIN, &cast.committee.root(), &genesis_id, 1, 0, b.bytes())
            .expect("signed");
    }
    // In its window: the second slot's proposer, when it is another seat,
    // offers a block of its own beside the first's — a real sibling.
    let mut sibling = None;
    if second != first {
        let n = nodes[cast.index(second)].as_ref().expect("up");
        n.evm.submit(&tx(11, 0, 0xee)).expect("its own");
        let b = n.chain.build().expect("a sibling");
        sibling = Some(b.id());
        rogue.remove(&b.id());
        n.host
            .propose(&CHAIN, &cast.committee.root(), &genesis_id, 1, 1, b.bytes())
            .expect("signed");
    }

    // Load keeps arriving at every seat while they run.
    let chains: Vec<Arc<Relay>> = nodes.iter().flatten().map(|n| n.chain.clone()).collect();
    let stop = AtomicBool::new(false);
    std::thread::scope(|s| {
        s.spawn(|| {
            for nonce in 1..PER {
                for (i, c) in chains.iter().enumerate() {
                    let raw = tx(i as u8, nonce, 0xa0 + i as u8);
                    let _ = c.call(
                        "eth_sendRawTransaction",
                        &json!([format!("0x{}", alloy_primitives::hex::encode(&raw))]),
                    );
                }
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(40));
            }
        });
        run(&mut nodes, &peers, Duration::from_secs(120), |n| (0..5).all(|i| n.nonce(i) == PER));
        stop.store(true, Ordering::SeqCst);
    });

    let top = agree(&nodes);
    assert!(top >= 2, "the chain kept moving: {top}");
    let one = nodes[0].as_ref().expect("up").chain.block_id_at(1).expect("height 1");
    assert!(!rogue.contains(&one), "a block proposed outside its window was accepted");
    if let Some(s) = sibling {
        assert!(one <= s, "the lower of the two live blocks is the one certified");
    }
    // ONE signature a height, and every seat that signed signed the block
    // decided there: the converged winner, not whichever each saw first. (A
    // seat whose settle window had not run out when the other four finished
    // accepted on their certificate without signing.)
    for h in 1..=top {
        let at = nodes[0].as_ref().expect("up").chain.block_id_at(h).expect("a block");
        let signers: Vec<[u8; 32]> = nodes.iter().flatten().filter_map(|n| n.signed(h)).collect();
        assert!(signers.iter().all(|b| *b == at), "height {h}: a seat signed another block");
        assert!(signers.len() >= 4, "height {h}: {} signers", signers.len());
    }
    for n in nodes.iter().flatten() {
        for (h, built) in &n.decided {
            if *built {
                assert!(
                    (0..64).any(|s| n.engine.proposer(*h, s) == n.host.node()),
                    "height {h}: built outside the validator's windows"
                );
            }
        }
    }
    // Each journal read back whole once its validator has let it go.
    let data: Vec<PathBuf> = nodes.iter().flatten().map(|n| n.data.clone()).collect();
    drop(nodes);
    for d in data {
        Journal::open(d.join("votes")).expect("one statement a height");
    }
}

/// The first window's proposer of a height is gone. The others wait out its
/// window and certify the next window's proposer's block, and keep going with
/// it gone, four of five being a quorum.
#[test]
fn a_proposer_that_goes_silent_is_skipped_after_its_window() {
    let cast = cast(5);
    let scratch = Scratch::new("silent");
    let addrs = addresses(5);
    let peers = peers(&cast, &addrs);
    let mut nodes: Vec<Option<Node>> =
        (0..5).map(|i| Some(Node::start(&cast, i, &scratch.0, addrs[i], WINDOW))).collect();
    mesh(&mut nodes, &peers);

    nodes[0].as_ref().expect("up").send(&tx(5, 0, 0xaa));
    run(&mut nodes, &peers, Duration::from_secs(60), |n| n.height() >= 1);

    let next = agree(&nodes) + 1;
    let silent = nodes[0].as_ref().expect("up").engine.proposer(next, 0);
    nodes[cast.index(silent)] = None;
    let sender = nodes.iter().position(Option::is_some).expect("up");
    nodes[sender].as_ref().expect("up").send(&tx(6, 0, 0xbb));
    let began = Instant::now();
    run(&mut nodes, &peers, Duration::from_secs(60), |n| n.height() >= next);
    let took = began.elapsed();

    agree(&nodes);
    let builder = nodes
        .iter()
        .flatten()
        .find(|n| n.decided.iter().any(|(h, built)| *h == next && *built))
        .expect("a builder that decided it");
    let slot = (1..64)
        .find(|s| builder.engine.proposer(next, *s) == builder.host.node())
        .expect("a later window");
    assert!(nodes.iter().flatten().all(|n| n.cert(next).votes.iter().all(|v| v.node_id != silent)));
    assert!(took < WINDOW * (slot as u32 + 4), "skipped after its window, not a stall: {took:?}");

    for k in 1..=2u64 {
        nodes[sender].as_ref().expect("up").send(&tx(6, k, 0xbb));
        run(&mut nodes, &peers, Duration::from_secs(60), |n| n.height() >= next + k);
    }
    assert!(agree(&nodes) >= next + 2);
}

/// One real validator, X, and the four others played by the test: every
/// proposal is signed by its window's proposer and pushed on its link, every
/// vote is signed by the validator it names.
struct Stage {
    cast: Cast,
    x: Node,
    others: Vec<Host>,
    peers: BTreeMap<NodeId, SocketAddr>,
    addrs: Vec<SocketAddr>,
    scratch: Scratch,
}

fn stage(tag: &str) -> Stage {
    let cast = cast(5);
    let scratch = Scratch::new(tag);
    let addrs = addresses(5);
    let peers = peers(&cast, &addrs);
    let x = Node::start(&cast, 0, &scratch.0, addrs[0], WINDOW);
    let others: Vec<Host> = (1..5)
        .map(|i| {
            Host::bind(Config { identity: cast.ids[i].clone(), bind: addrs[i] }).expect("bind")
        })
        .collect();
    let mut s = Stage { cast, x, others, peers, addrs, scratch };
    s.link_up();
    s
}

impl Stage {
    /// X meets each of the others, and they meet only X.
    fn link_up(&mut self) {
        let x = self.x.host.node();
        let mut hosts = vec![(&mut self.x.host, self.peers.clone())];
        for h in self.others.iter_mut() {
            let pair: BTreeMap<NodeId, SocketAddr> = self
                .peers
                .iter()
                .filter(|(n, _)| **n == x || **n == h.node())
                .map(|(n, a)| (*n, *a))
                .collect();
            hosts.push((h, pair));
        }
        meet(hosts);
    }

    fn link(&self, i: usize) -> &Host {
        &self.others[i - 1]
    }

    /// The first `n` slots whose proposer is not X, stepping X until its
    /// clock is within reach of the last of them.
    fn theirs(&mut self, n: usize) -> Vec<u64> {
        let me = self.x.host.node();
        let slots: Vec<u64> =
            (0..).filter(|s| self.x.engine.proposer(1, *s) != me).take(n).collect();
        let last = *slots.last().expect("a slot");
        self.until("reached the slot", |x| x.engine.slot().is_some_and(|(_, s)| s + 1 >= last));
        slots
    }

    /// The slot's proposer signs `block` and pushes it to X.
    fn propose(&self, slot: u64, block: &dyn Block) {
        let i = self.cast.index(self.x.engine.proposer(1, slot));
        let tip = self.x.chain.last_accepted();
        self.link(i)
            .propose(&CHAIN, &self.cast.committee.root(), &tip, 1, slot, block.bytes())
            .expect("signed");
    }

    fn until(&mut self, what: &str, done: impl Fn(&Node) -> bool) {
        let end = Instant::now() + REACH;
        while !done(&self.x) {
            assert!(Instant::now() < end, "X never {what}");
            if self.x.step() {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    fn idle(&mut self, d: Duration) {
        let end = Instant::now() + d;
        while Instant::now() < end {
            if self.x.step() {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    /// X's signatures as validator 1's link heard them.
    fn heard(&self) -> Vec<[u8; 32]> {
        self.link(1)
            .mesh()
            .drain()
            .into_iter()
            .filter(|(_, v)| v.node == self.x.host.node())
            .map(|(_, v)| v.position.signed_identity())
            .collect()
    }
}

/// X is offered a block and, inside the settle window, a lower one: it signs
/// the lower — the converged winner, not the first it saw. Stopped and started
/// again, it says that SAME statement again and signs nothing else at the
/// height, even offered a lower block still; the others then certify its block
/// with it (Go's clone recovery).
#[test]
fn one_signature_a_height_the_converged_winner_and_the_same_one_after_a_restart() {
    let mut s = stage("clone");
    let mut pair = [block(1, 0xaa), block(1, 0xbb)];
    pair.sort_by_key(|b| b.id());
    let [low, high] = pair;

    let slots = s.theirs(2);
    s.propose(slots[0], high.as_ref());
    s.idle(Duration::from_millis(50));
    s.propose(slots[1], low.as_ref());
    s.until("signed", |x| x.signed(1).is_some());
    assert_eq!(s.x.signed(1), Some(low.id()), "the lowest live block, not the first");
    let kept = Held::open(s.x.data.join("held")).expect("held").get(1).expect("kept");
    assert_eq!(kept.block, low.bytes(), "the block it signed is on the device with the signature");
    s.idle(Duration::from_millis(100));
    let before = std::fs::read(s.x.data.join("votes")).expect("votes");
    let _ = s.heard();

    // Stopped, and started again on the same directory and address.
    let Stage { cast, x, others, peers, addrs, scratch } = s;
    drop(x);
    std::thread::sleep(Duration::from_millis(300));
    let x = Node::start(&cast, 0, &scratch.0, addrs[0], WINDOW);
    let mut s = Stage { cast, x, others, peers, addrs, scratch };
    s.link_up();
    s.idle(Duration::from_millis(100));
    assert!(s.heard().contains(&low.id()), "it said its statement again");

    // Offered a lower block still, it holds: one signature a height.
    let lower = loop {
        let b = block(2, rand_to());
        if b.id() < low.id() {
            break b;
        }
    };
    let slots = s.theirs(2);
    s.propose(slots[0], lower.as_ref());
    s.idle(WINDOW);
    assert_eq!(std::fs::read(s.x.data.join("votes")).expect("votes"), before, "nothing new signed");

    // The others sign its block, and the height is decided on it — with X's
    // own signature among the four.
    s.propose(slots[1], low.as_ref());
    for i in 1..4 {
        s.link(1).mesh().broadcast(&vote(&s.cast, &s.x.engine, i, low.as_ref()));
    }
    s.until("accepted", |x| x.height() == 1);
    assert_eq!(s.x.chain.block_id_at(1).expect("height 1"), low.id());
    assert!(s.x.cert(1).votes.iter().any(|v| v.node_id == s.x.host.node()), "X is in it");
}

/// A recipient byte no test above uses, different each call.
fn rand_to() -> u8 {
    static NEXT: AtomicUsize = AtomicUsize::new(0x10);
    NEXT.fetch_add(1, Ordering::Relaxed) as u8
}

/// Four of five are up and all four are needed. One of them comes onto the
/// mesh only after the others proposed and signed — it heard none of it. The
/// retry says the proposal and the signatures again, and the height is
/// decided with it.
#[test]
fn one_validator_that_missed_the_height_cannot_hold_it() {
    let cast = cast(5);
    let scratch = Scratch::new("laggard");
    let addrs = addresses(5);
    let peers = peers(&cast, &addrs);
    let mut nodes: Vec<Option<Node>> =
        (0..4).map(|i| Some(Node::start(&cast, i, &scratch.0, addrs[i], WINDOW))).collect();
    nodes.push(None);
    let late = 3;
    let laggard = nodes[late].take();
    mesh(&mut nodes, &peers);

    nodes[0].as_ref().expect("up").send(&tx(5, 0, 0xaa));
    // Three of the needed four: the first proposer that is up builds, the
    // three sign, and nothing is decided.
    let engine = &nodes[0].as_ref().expect("up").engine;
    let first = (0..)
        .find(|s| cast.index(engine.proposer(1, *s)) < late)
        .expect("a window of a validator that is up");
    idle(&mut nodes, &peers, WINDOW * (first as u32 + 2));
    assert!(nodes.iter().flatten().all(|n| n.height() == 0), "three cannot certify");
    assert!(nodes.iter().flatten().any(|n| n.signed(1).is_some()), "and they did sign");

    nodes[late] = laggard;
    run(&mut nodes, &peers, Duration::from_secs(60), |n| n.height() >= 1);
    agree(&nodes);
    let n = nodes[late].as_ref().expect("up");
    assert!(n.cert(1).votes.iter().any(|v| v.node_id == cast.ids[late].node()), "it signed");
}

/// What v0.2.x left at a stalled height: each seat's journal holding one
/// statement there, for the block `blocks[i]`.
fn split(cast: &Cast, dir: &Path, blocks: &[&dyn Block]) {
    let engine =
        Engine::new(cast.ids[0].node(), cast.keys[0].clone(), &cast.committee, EPOCH, CHAIN)
            .expect("engine");
    for (i, b) in blocks.iter().enumerate() {
        let data = dir.join(format!("n{i}"));
        std::fs::create_dir_all(&data).expect("data");
        let mut j = Journal::open(data.join("votes")).expect("votes");
        j.admit(&canonical_vote_message(&engine.position(*b), true)).expect("a v0.2 vote");
    }
}

/// Every seat signed its own block at the height and refused the others — the
/// benchmark's halt. Under one signature a height no seat may sign again, no
/// block holds α, and the height stays uncertified: Go's non-equivocation
/// guarantee, and what Go does too (d13b4559 leaves this case to its Nova
/// majority tier, which this node does not run). Nothing is signed twice.
#[test]
fn a_height_v0_2_split_every_way_stays_uncertified_and_nobody_signs_twice() {
    let cast = cast(5);
    let scratch = Scratch::new("split");
    let own: Vec<Box<dyn Block>> = (0..5).map(|i| block(i, 0xc0 + i)).collect();
    split(&cast, &scratch.0, &own.iter().map(|b| b.as_ref()).collect::<Vec<_>>());
    let addrs = addresses(5);
    let peers = peers(&cast, &addrs);
    let mut nodes: Vec<Option<Node>> =
        (0..5).map(|i| Some(Node::start(&cast, i, &scratch.0, addrs[i], WINDOW))).collect();
    mesh(&mut nodes, &peers);

    nodes[0].as_ref().expect("up").send(&tx(6, 0, 0xee));
    idle(&mut nodes, &peers, WINDOW * 6);
    for (i, n) in nodes.iter().flatten().enumerate() {
        assert_eq!(n.height(), 0, "seat {i}: no block at the split height holds a certificate");
        assert_eq!(n.signed(1), Some(own[i].id()), "seat {i}: its v0.2 statement stands");
        assert_eq!(
            std::fs::metadata(n.data.join("votes")).expect("votes").len() as usize,
            lux_node::journal::ENTRY,
            "seat {i}: and it is the only one"
        );
        assert!(n.waited.get(&1).is_some_and(|m| *m == 1), "seat {i}: one vote a block, seen");
    }
}

/// Four seats signed B at the height and one signed C. B holds α: the seats
/// say their statements again (clone recovery), the seat that still holds B
/// assembles its certificate, and the others take B from it — the height
/// resumes on B, and nobody signed anything new.
#[test]
fn a_height_v0_2_split_resumes_on_the_block_that_holds_a_certificate() {
    let cast = cast(5);
    let scratch = Scratch::new("four");
    let b = block(1, 0xb0);
    let c = block(2, 0xc0);
    split(&cast, &scratch.0, &[b.as_ref(), b.as_ref(), b.as_ref(), b.as_ref(), c.as_ref()]);
    let addrs = addresses(5);
    let peers = peers(&cast, &addrs);
    let mut nodes: Vec<Option<Node>> =
        (0..5).map(|i| Some(Node::start(&cast, i, &scratch.0, addrs[i], WINDOW))).collect();
    // Seat 0 never restarted since it signed: it still holds B.
    nodes[0].as_ref().expect("up").chain.parse(&b.bytes()).expect("B");
    mesh(&mut nodes, &peers);

    run(&mut nodes, &peers, Duration::from_secs(60), |n| n.height() >= 1);
    agree(&nodes);
    for (i, n) in nodes.iter().flatten().enumerate() {
        assert_eq!(n.chain.block_id_at(1).expect("height 1"), b.id(), "seat {i}: resumed on B");
        assert_eq!(
            std::fs::metadata(n.data.join("votes")).expect("votes").len() as usize,
            lux_node::journal::ENTRY,
            "seat {i}: nothing new signed"
        );
    }
}

/// The whole committee stops at a height `k` seats already signed — too few to
/// certify — and starts again. Each signer kept the block before it signed, so
/// it says its signature again AND offers the block again, and the height
/// completes on that block: nobody is left bound to a block nobody holds.
fn a_committee_restarted_with(k: usize) {
    let cast = cast(5);
    let scratch = Scratch::new(&format!("restart-{k}"));
    // Each start binds a port the kernel picks and holds it from that moment,
    // and the peers are told where each one is: no port is freed and taken
    // back, so none can be taken in between by another test.
    let start = |up: &[usize]| -> (Vec<Option<Node>>, BTreeMap<NodeId, SocketAddr>) {
        let any: SocketAddr = "127.0.0.1:0".parse().expect("addr");
        let nodes: Vec<Option<Node>> = (0..5)
            .map(|i| up.contains(&i).then(|| Node::start(&cast, i, &scratch.0, any, WINDOW)))
            .collect();
        let peers = nodes.iter().flatten().map(|n| (n.host.node(), n.host.addr())).collect();
        (nodes, peers)
    };

    // Three at most are up, so nothing is decided: they sign B and wait.
    let first: Vec<usize> = (0..k.min(3)).collect();
    let (mut nodes, peers) = start(&first);
    mesh(&mut nodes, &peers);
    nodes[0].as_ref().expect("up").send(&tx(5, 0, 0xaa));
    run(&mut nodes, &peers, Duration::from_secs(60), |n| n.signed(1).is_some());
    let b = nodes[0].as_ref().expect("up").signed(1).expect("signed");
    assert!(nodes.iter().flatten().all(|n| n.signed(1) == Some(b) && n.height() == 0));
    drop(nodes);

    // A fourth signature: seat 3 comes up beside seat 0 alone, takes B from
    // what seat 0 kept, and signs it. Two are up; four have signed.
    if k == 4 {
        let (mut nodes, peers) = start(&[0, 3]);
        mesh(&mut nodes, &peers);
        run(&mut nodes, &peers, Duration::from_secs(60), |n| n.signed(1).is_some());
        assert_eq!(nodes[3].as_ref().expect("up").signed(1), Some(b), "seat 3 signed B");
        assert!(nodes.iter().flatten().all(|n| n.height() == 0), "two cannot count four");
        drop(nodes);
    }

    // Every seat, at once.
    let (mut nodes, peers) = start(&[0, 1, 2, 3, 4]);
    mesh(&mut nodes, &peers);
    run(&mut nodes, &peers, Duration::from_secs(60), |n| n.height() >= 1);
    agree(&nodes);
    idle(&mut nodes, &peers, WINDOW);
    for (i, n) in nodes.iter().flatten().enumerate() {
        assert_eq!(n.chain.block_id_at(1).expect("height 1"), b, "seat {i}: completed on B");
        assert!(n.signed(1).is_none_or(|s| s == b), "seat {i}: signed another block");
        assert!(
            std::fs::metadata(n.data.join("votes")).expect("votes").len() as usize
                <= lux_node::journal::ENTRY,
            "seat {i}: one statement"
        );
        assert!(!n.data.join("held").exists(), "seat {i}: a decided block is let go");
    }
}

#[test]
fn a_committee_restarted_with_two_signatures_cast_completes_on_the_signed_block() {
    a_committee_restarted_with(2);
}

#[test]
fn a_committee_restarted_with_three_signatures_cast_completes_on_the_signed_block() {
    a_committee_restarted_with(3);
}

#[test]
fn a_committee_restarted_with_four_signatures_cast_completes_on_the_signed_block() {
    a_committee_restarted_with(4);
}

// ---------------------------------------------------------------------------
// RED probes (review of fix/live). Each asserts what a safe, live engine does.

/// A relayer sends the proposer's genuine signature over other bytes first.
/// X must still take and sign the genuine proposal when it arrives.
#[test]
fn red_a_tampered_relay_silences_the_genuine_proposal() {
    let mut s = stage("red-poison");
    let a = block(1, 0xaa);
    let other = block(2, 0xbb);
    let slot = s.theirs(1)[0];
    let who = s.x.engine.proposer(1, slot);
    let i = s.cast.index(who);
    let tip = s.x.chain.last_accepted();
    let header = lux_node::proposer::header(
        &CHAIN,
        &s.cast.committee.root(),
        &tip,
        1,
        slot,
        s.cast.ids[i].public(),
        &a.bytes(),
    );
    let signature = s.cast.ids[i].sign(&header, lux_node::proposer::CONTEXT).expect("sign");
    let genuine = lux_node::Proposal { from: who, height: 1, slot, signature, block: a.bytes() };
    let tampered = lux_node::Proposal { block: other.bytes(), ..genuine.clone() };
    let j = (1..5).find(|j| *j != i).expect("another link");
    s.link(j).mesh().propose(&tampered);
    s.idle(Duration::from_millis(50));
    s.link(i).mesh().propose(&genuine);
    s.idle(WINDOW * 2);
    assert_eq!(s.x.signed(1), Some(a.id()), "X holds and signs the genuine proposal");
}

/// One window's proposer signs two blocks. Go tracks both and every node
/// signs the lower; X must too, whichever it saw first.
#[test]
fn red_two_signed_blocks_in_one_window_the_lower_is_signed() {
    let mut s = stage("red-twice");
    let mut pair = [block(1, 0xa1), block(1, 0xb1)];
    pair.sort_by_key(|b| b.id());
    let [low, high] = pair;
    let slot = s.theirs(1)[0];
    s.propose(slot, high.as_ref());
    s.idle(Duration::from_millis(30));
    s.propose(slot, low.as_ref());
    s.until("signed", |x| x.signed(1).is_some());
    assert_eq!(s.x.signed(1), Some(low.id()), "first-seen, not the lowest live block");
}

/// A v0.2 stalled height, every seat restarted on it: the heights reached and
/// each seat's journal after sixteen windows.
fn red_stalled(tag: &str, bound: &[u8]) -> Vec<(u64, usize)> {
    let cast = cast(5);
    let scratch = Scratch::new(tag);
    let mut made: BTreeMap<u8, Box<dyn Block>> = BTreeMap::new();
    for k in bound {
        made.entry(*k).or_insert_with(|| block(*k, 0xc0 + *k));
    }
    split(&cast, &scratch.0, &bound.iter().map(|k| made[k].as_ref()).collect::<Vec<_>>());
    let addrs = addresses(5);
    let peers = peers(&cast, &addrs);
    let mut nodes: Vec<Option<Node>> =
        (0..5).map(|i| Some(Node::start(&cast, i, &scratch.0, addrs[i], WINDOW))).collect();
    mesh(&mut nodes, &peers);
    nodes[0].as_ref().expect("up").send(&tx(6, 0, 0xee));
    idle(&mut nodes, &peers, WINDOW * 16);
    for (i, n) in nodes.iter().flatten().enumerate() {
        if let Some(k) = bound.get(i) {
            assert_eq!(n.signed(1), Some(made[k].id()), "seat {i}: its v0.2 statement stands");
        }
    }
    nodes
        .iter()
        .flatten()
        .map(|n| {
            let size = std::fs::metadata(n.data.join("votes")).map_or(0, |m| m.len() as usize);
            (n.height(), size)
        })
        .collect()
}

/// v0.2 left four seats on B and one on C, and every seat restarted: v0.2 kept
/// an undecided block only in memory, so nobody holds B. B carries α
/// signatures — a certificate in all but its bytes — so certifying any other
/// block at the height would be the fork one signature a height exists to
/// prevent. The height waits for B, and nobody signs twice. From this version
/// on a seat keeps the block before it signs, and the same restart completes
/// (`a_committee_restarted_with_four_signatures_cast_completes_on_the_signed_block`).
#[test]
fn red_a_v0_2_four_of_five_after_a_full_restart_waits_for_b_and_nobody_signs_twice() {
    let after = red_stalled("red-four", &[1, 1, 1, 1, 2]);
    for (i, (height, size)) in after.iter().enumerate() {
        assert_eq!(*height, 0, "seat {i}: nothing but B can be decided at the height");
        assert_eq!(*size, lux_node::journal::ENTRY, "seat {i}: one statement");
    }
}

/// Two seats signed at the stalled height under v0.2, on blocks nobody holds
/// after the restart; three are clean. α is four, so a new block can reach
/// three signatures and no more without a bound seat signing twice: the split
/// Go calls unrecoverable under one signature a height (engine.go:1716) and
/// leaves to Nova, which this node does not run. It waits, and nobody signs
/// twice.
#[test]
fn red_two_v0_2_signatures_at_the_stalled_height_wait_and_nobody_signs_twice() {
    let after = red_stalled("red-two", &[1, 2]);
    for (i, (height, size)) in after.iter().enumerate() {
        assert_eq!(*height, 0, "seat {i}: three clean seats cannot reach α");
        assert!(*size <= lux_node::journal::ENTRY, "seat {i}: one statement at most");
    }
}

struct Asked(AtomicUsize);

impl lux_node::mesh::Service for Asked {
    fn tip(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst);
        0
    }
    fn record(&self, _: u64) -> Option<lux_node::mesh::Record> {
        None
    }
    fn transaction(&self, _: &[u8]) {}
}

/// One peer sends votes with bad signatures naming the other validators at a
/// far height. X must not start catching up on them.
#[test]
fn red_forged_votes_from_one_peer_start_catch_up_every_window() {
    let mut s = stage("red-ahead");
    let asked = Arc::new(Asked(AtomicUsize::new(0)));
    s.link(1).mesh().serve(asked.clone());
    s.idle(WINDOW * 6);
    let quiet = asked.0.load(Ordering::SeqCst);
    let a = block(1, 0xaa);
    for k in 1..5 {
        let mut v = vote(&s.cast, &s.x.engine, k, a.as_ref());
        v.position.height = 1_000_000;
        s.link(1).mesh().broadcast(&v);
    }
    s.idle(WINDOW * 6);
    let after = asked.0.load(Ordering::SeqCst) - quiet;
    assert_eq!((quiet, after), (0, 0), "asks of link 1: (before, after the forged votes)");
}

// ---------------------------------------------------------------------------
// RED round two. Each asserts what a safe, live engine does; a failure is the
// finding.

/// One slot's proposer signs 320 frames of junk and pushes them to X, paced
/// under the per-link queue. Another proposer's honest block follows. X must
/// still take and sign the honest block.
#[test]
fn red2_one_proposer_filling_seen_blinds_x_to_the_honest_block() {
    let mut s = stage("red2-seen");
    let me = s.x.host.node();
    let slots: Vec<u64> = (0..).filter(|sl| s.x.engine.proposer(1, *sl) != me).take(12).collect();
    let bad_slot = slots[0];
    let bad = s.cast.index(s.x.engine.proposer(1, bad_slot));
    let good_slot = *slots
        .iter()
        .find(|sl| s.cast.index(s.x.engine.proposer(1, **sl)) != bad)
        .expect("another proposer");
    let reach = bad_slot.max(good_slot);
    s.until("reached the slots", |x| x.engine.slot().is_some_and(|(_, sl)| sl + 1 >= reach));
    let tip = s.x.chain.last_accepted();
    let root = s.cast.committee.root();
    for burst in 0..40u32 {
        for k in 0..8u32 {
            let junk = (burst * 8 + k).to_be_bytes().to_vec();
            s.link(bad).propose(&CHAIN, &root, &tip, 1, bad_slot, junk).expect("signed");
        }
        s.idle(Duration::from_millis(15));
    }
    let a = block(1, 0xaa);
    s.propose(good_slot, a.as_ref());
    s.idle(WINDOW * 3);
    assert_eq!(s.x.signed(1), Some(a.id()), "X holds and signs the honest proposal");
}

/// A proposer gives X sixty-four signed siblings, then an honest peer relays
/// the same proposer's lower sibling inside the settle window. X must sign the
/// lowest block it was offered before it settled — or two halves given
/// disjoint sets sign two winners.
#[test]
fn red2_a_full_sibling_set_refuses_a_lower_relayed_block() {
    let mut s = stage("red2-cap");
    let mut all: Vec<Box<dyn Block>> = (0..65u8).map(|k| block(1, k.wrapping_add(0x40))).collect();
    all.sort_by_key(|b| b.id());
    let low = all.remove(0);
    let slot = s.theirs(1)[0];
    let who = s.cast.index(s.x.engine.proposer(1, slot));
    let relay = (1..5).find(|j| *j != who).expect("another link");
    for chunk in all.chunks(4) {
        for b in chunk {
            s.propose(slot, b.as_ref());
        }
        s.idle(Duration::from_millis(20));
    }
    let tip = s.x.chain.last_accepted();
    let header = lux_node::proposer::header(
        &CHAIN,
        &s.cast.committee.root(),
        &tip,
        1,
        slot,
        s.cast.ids[who].public(),
        &low.bytes(),
    );
    let signature = s.cast.ids[who].sign(&header, lux_node::proposer::CONTEXT).expect("sign");
    let p = lux_node::Proposal {
        from: s.cast.ids[who].node(),
        height: 1,
        slot,
        signature,
        block: low.bytes(),
    };
    s.link(relay).mesh().propose(&p);
    s.until("signed", |x| x.signed(1).is_some());
    assert_eq!(s.x.signed(1), Some(low.id()), "the lowest block offered before it settled");
}

/// Three seats signed A at height 1 and wait (one seat is down). The fourth
/// comes back with traffic, and its own window opens before A's signers push
/// A again. A is the only block that can ever be certified at the height; the
/// returning seat must sign it, not a block of its own.
#[test]
fn red2_a_returning_seat_signs_the_block_three_signed_not_its_own() {
    let cast = cast(5);
    let scratch = Scratch::new("red2-late");
    let addrs = addresses(5);
    let peers = peers(&cast, &addrs);
    let probe =
        Engine::new(cast.ids[0].node(), cast.keys[0].clone(), &cast.committee, EPOCH, CHAIN)
            .expect("engine");
    let late = cast.index(probe.proposer(1, 0));
    let down = (0..5).find(|i| *i != late).expect("another seat");
    let first = (0..)
        .find(|sl| {
            let i = cast.index(probe.proposer(1, *sl));
            i != late && i != down
        })
        .expect("a window of a seat that is up");
    let mut nodes: Vec<Option<Node>> = (0..5)
        .map(|i| {
            (i != late && i != down).then(|| Node::start(&cast, i, &scratch.0, addrs[i], WINDOW))
        })
        .collect();
    mesh(&mut nodes, &peers);
    nodes.iter().flatten().next().expect("up").send(&tx(5, 0, 0xaa));
    idle(&mut nodes, &peers, WINDOW * (first as u32 + 3));
    let a = nodes.iter().flatten().find_map(|n| n.signed(1)).expect("the three signed");
    assert!(nodes.iter().flatten().all(|n| n.signed(1) == Some(a) && n.height() == 0));
    idle(&mut nodes, &peers, Duration::from_secs(4));

    nodes[late] = Some(Node::start(&cast, late, &scratch.0, addrs[late], WINDOW));
    mesh(&mut nodes, &peers);
    nodes[late].as_ref().expect("up").send(&tx(7, 0, 0x77));
    idle(&mut nodes, &peers, Duration::from_secs(20));
    let n = nodes[late].as_ref().expect("up");
    eprintln!(
        "late seat {late}: signed {:?}, A {:?}, heights {:?}",
        n.signed(1).map(|b| alloy_primitives::hex::encode(&b[..4])),
        alloy_primitives::hex::encode(&a[..4]),
        nodes.iter().flatten().map(|n| n.height()).collect::<Vec<_>>()
    );
    assert_eq!(n.signed(1), Some(a), "the returning seat signs A");
    assert!(nodes.iter().flatten().all(|n| n.height() >= 1), "four seats up decide the height");
}

/// X restarts bound on B at height 1 with no copy of B (v0.2, or a lost
/// held file). It may never sign again there; it must not build a rival
/// block there either.
#[test]
fn red2_a_seat_bound_without_its_block_does_not_build_beside_it() {
    let cast = cast(5);
    let scratch = Scratch::new("red2-bound");
    let b = block(1, 0xb0);
    split(&cast, &scratch.0, &[b.as_ref()]);
    let addrs = addresses(5);
    let peers = peers(&cast, &addrs);
    let x = Node::start(&cast, 0, &scratch.0, addrs[0], WINDOW);
    let others: Vec<Host> = (1..5)
        .map(|i| {
            Host::bind(Config { identity: cast.ids[i].clone(), bind: addrs[i] }).expect("bind")
        })
        .collect();
    let mut s = Stage { cast, x, others, peers, addrs, scratch };
    s.link_up();
    s.x.send(&tx(5, 0, 0xaa));
    let me = s.x.host.node();
    let mine = (0..).find(|sl| s.x.engine.proposer(1, *sl) == me).expect("a window of X's");
    s.until("passed its window", |x| x.engine.slot().is_some_and(|(_, sl)| sl > mine));
    s.idle(Duration::from_millis(50));
    let built = s.link(1).mesh().proposals().into_iter().filter(|p| p.height == 1).count();
    assert_eq!(s.x.signed(1), Some(b.id()), "still bound on B");
    assert_eq!(built, 0, "X, bound on B, proposed a rival block at the height");
}

/// A held file cut short (a copied data directory, a failing disk, a later
/// proposal version) must not keep a validator from starting: the journal,
/// not this file, is what stops a second signature.
#[test]
fn red2_a_damaged_held_file_does_not_keep_a_validator_down() {
    let scratch = Scratch::new("red2-torn");
    let path = scratch.0.join("held");
    let p = lux_node::Proposal {
        from: [0; 20],
        height: 3,
        slot: 0,
        signature: vec![1; lux_node::pq::IDENTITY_SIGNATURE_LEN],
        block: block(1, 0xaa).bytes(),
    };
    let whole = p.encode();
    std::fs::write(&path, &whole[..whole.len() / 2]).expect("write");
    assert!(Held::open(&path).is_ok(), "a torn held file refuses the start");
}

/// 256 forged votes at far heights from one peer: the price of one step.
#[test]
fn red2_forged_far_votes_cost_one_check_each() {
    let mut s = stage("red2-flood");
    s.idle(WINDOW);
    let a = block(1, 0xaa);
    let v0 = vote(&s.cast, &s.x.engine, 1, a.as_ref());
    for h in 0..lux_node::mesh::VOTES_MAX as u64 {
        let mut v = v0.clone();
        v.position.height = 10 + h;
        s.link(1).mesh().broadcast(&v);
    }
    std::thread::sleep(Duration::from_millis(500));
    let began = Instant::now();
    s.x.step();
    let spent = began.elapsed();
    eprintln!("one step over {} forged far votes: {spent:?}", lux_node::mesh::VOTES_MAX);
    assert!(spent < Duration::from_millis(20), "one peer bought {spent:?} of the engine thread");
}

// ---------------------------------------------------------------------------
// RED round three. Each asserts what a safe, live engine does; a failure is
// the finding.

/// `n` distinct blocks at height 1, lowest id first.
fn red3_sorted(n: usize) -> Vec<Box<dyn Block>> {
    let mut all: Vec<Box<dyn Block>> =
        (0..n).map(|k| block((k % 12) as u8, (k / 12) as u8 + 1)).collect();
    all.sort_by_key(|b| b.id());
    all
}

/// The first of `slots` whose proposer is not seat `not`.
fn red3_other(s: &Stage, slots: &[u64], not: usize) -> u64 {
    *slots.iter().find(|sl| s.cast.index(s.x.engine.proposer(1, **sl)) != not).expect("another")
}

/// (e) An honest block A is live. Another slot's proposer F pushes a new
/// signed block every 50 ms (the settle window is 150 ms) for 6 s — twelve
/// windows — all above A, signed up front. X must still sign A while F streams.
#[test]
fn red3_a_proposer_streaming_signed_blocks_holds_every_vote_back() {
    let mut s = stage("red3-stream");
    let all = red3_sorted(130);
    let (a, rest) = all.split_first().expect("blocks");
    let slots = s.theirs(4);
    let p_slot = slots[0];
    let p = s.cast.index(s.x.engine.proposer(1, p_slot));
    let f_slot = red3_other(&s, &slots, p);
    let f = s.cast.index(s.x.engine.proposer(1, f_slot));
    let tip = s.x.chain.last_accepted();
    let root = s.cast.committee.root();
    let frames: Vec<lux_node::Proposal> = rest
        .iter()
        .take(120)
        .map(|b| {
            let header = lux_node::proposer::header(
                &CHAIN,
                &root,
                &tip,
                1,
                f_slot,
                s.cast.ids[f].public(),
                &b.bytes(),
            );
            let signature = s.cast.ids[f].sign(&header, lux_node::proposer::CONTEXT).expect("sign");
            lux_node::Proposal {
                from: s.cast.ids[f].node(),
                height: 1,
                slot: f_slot,
                signature,
                block: b.bytes(),
            }
        })
        .collect();
    s.propose(p_slot, a.as_ref());
    let began = Instant::now();
    for frame in &frames {
        s.link(f).mesh().propose(frame);
        s.idle(Duration::from_millis(50));
    }
    let during = s.x.signed(1);
    let streamed = began.elapsed();
    s.until("signed", |x| x.signed(1).is_some());
    eprintln!(
        "red3-stream: signed during the {streamed:?} stream: {}; signed A {:?} after A",
        during.is_some(),
        began.elapsed()
    );
    assert_eq!(during, Some(a.id()), "X signed A while F streamed");
}

/// (fix 1) F signs 149 blocks, highest id first, so each displaces the one
/// above it; then an honest proposer offers L, lower than all of them, inside
/// the settle window. X must sign L. The displaced blocks are never let go by
/// the chain, so past its undecided bound the chain drops each new block the
/// moment it is parsed.
#[test]
fn red3_displaced_siblings_fill_the_chain_and_the_lowest_is_refused() {
    let mut s = stage("red3-fill");
    let all = red3_sorted(150);
    let (low, rest) = all.split_first().expect("blocks");
    let slots = s.theirs(4);
    let f_slot = slots[0];
    let f = s.cast.index(s.x.engine.proposer(1, f_slot));
    let p_slot = red3_other(&s, &slots, f);
    let tip = s.x.chain.last_accepted();
    let root = s.cast.committee.root();
    let down: Vec<&Box<dyn Block>> = rest.iter().rev().collect();
    for chunk in down.chunks(4) {
        for b in chunk {
            s.link(f).propose(&CHAIN, &root, &tip, 1, f_slot, b.bytes()).expect("signed");
        }
        s.idle(Duration::from_millis(20));
    }
    s.propose(p_slot, low.as_ref());
    s.until("signed", |x| x.signed(1).is_some());
    let signed = s.x.signed(1).expect("signed");
    let rank = all.iter().position(|b| b.id() == signed);
    eprintln!(
        "red3-fill: X signed the block ranked {rank:?} of {}; L held by the chain: {}",
        all.len(),
        s.x.chain.get(&low.id()).is_ok()
    );
    assert_eq!(signed, low.id(), "the lowest block offered before it settled");
}

/// (f) Four of five up, one of them restarting — a rolling restart with a seat
/// already out. The height has been quiet for 30 windows when a transaction
/// arrives while the seat is down; the three build and sign. The seat comes
/// back, counts its slots from 0, and is needed. The height must be decided in
/// a few windows, not in as many as it was quiet.
#[test]
fn red3_a_seat_restarted_in_a_quiet_height_drops_its_block_for_as_long_as_it_was_quiet() {
    let cast = cast(5);
    let scratch = Scratch::new("red3-quiet");
    let addrs = addresses(5);
    let peers = peers(&cast, &addrs);
    let late = 3;
    let mut nodes: Vec<Option<Node>> = (0..5)
        .map(|i| (i < 4).then(|| Node::start(&cast, i, &scratch.0, addrs[i], WINDOW)))
        .collect();
    mesh(&mut nodes, &peers);
    idle(&mut nodes, &peers, WINDOW * 30);
    drop(nodes[late].take());
    nodes[0].as_ref().expect("up").send(&tx(5, 0, 0xaa));
    idle(&mut nodes, &peers, WINDOW * 4);
    let slot = nodes[0].as_ref().expect("up").engine.slot();
    assert!(nodes.iter().flatten().all(|n| n.signed(1).is_some() && n.height() == 0));
    nodes[late] = Some(Node::start(&cast, late, &scratch.0, addrs[late], WINDOW));
    mesh(&mut nodes, &peers);
    let began = Instant::now();
    run(&mut nodes, &peers, Duration::from_secs(90), |n| n.height() >= 1);
    let took = began.elapsed();
    eprintln!("red3-quiet: the others at {slot:?}; decided {took:?} after the seat came back");
    assert!(took < WINDOW * 8, "decided {took:?} after the needed seat came back");
}

/// (d) F, a slot's proposer, signs junk and sends it to X alone; X marks F
/// faulty for the height. F's real block L goes to the three others, who take
/// it, relay it to X, and sign it. L is the only block that can be certified
/// and it needs X. X must sign L.
#[test]
fn red3_junk_to_one_seat_hides_the_proposers_real_block_there() {
    let mut s = stage("red3-junk");
    let slot = s.theirs(1)[0];
    let f = s.cast.index(s.x.engine.proposer(1, slot));
    let tip = s.x.chain.last_accepted();
    let root = s.cast.committee.root();
    s.link(f).propose(&CHAIN, &root, &tip, 1, slot, b"not a block".to_vec()).expect("signed");
    s.idle(Duration::from_millis(50));
    let l = block(1, 0xaa);
    let header = lux_node::proposer::header(
        &CHAIN,
        &root,
        &tip,
        1,
        slot,
        s.cast.ids[f].public(),
        &l.bytes(),
    );
    let signature = s.cast.ids[f].sign(&header, lux_node::proposer::CONTEXT).expect("sign");
    let p = lux_node::Proposal {
        from: s.cast.ids[f].node(),
        height: 1,
        slot,
        signature,
        block: l.bytes(),
    };
    for j in (1..5).filter(|j| *j != f) {
        s.link(j).mesh().propose(&p);
        s.link(j).mesh().broadcast(&vote(&s.cast, &s.x.engine, j, l.as_ref()));
    }
    s.idle(WINDOW * 20);
    eprintln!("red3-junk: X signed {:?}; height {}", s.x.signed(1).is_some(), s.x.height());
    assert_eq!(s.x.signed(1), Some(l.id()), "X signs the block three honest seats signed");
}

/// A chain whose first verify of one block fails the way a plugin link does
/// when it times out.
struct Flaky {
    inner: Arc<Relay>,
    fail: std::sync::Mutex<Option<[u8; 32]>>,
}

impl Vm for Flaky {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn version(&self) -> String {
        self.inner.version()
    }
    fn build(&self) -> Result<Box<dyn Block>, lux_node::vm::Error> {
        self.inner.build()
    }
    fn parse(&self, raw: &[u8]) -> Result<Box<dyn Block>, lux_node::vm::Error> {
        self.inner.parse(raw)
    }
    fn get(&self, id: &[u8; 32]) -> Result<Box<dyn Block>, lux_node::vm::Error> {
        self.inner.get(id)
    }
    fn verify(&self, id: &[u8; 32]) -> Result<(), lux_node::vm::Error> {
        let mut fail = self.fail.lock().expect("lock");
        if fail.as_ref() == Some(id) {
            *fail = None;
            return Err(lux_node::vm::Error::Unavailable(
                "the far end did not answer: timed out".into(),
            ));
        }
        drop(fail);
        self.inner.verify(id)
    }
    fn accept(&self, id: &[u8; 32]) -> Result<(), lux_node::vm::Error> {
        self.inner.accept(id)
    }
    fn reject(&self, id: &[u8; 32]) -> Result<(), lux_node::vm::Error> {
        self.inner.reject(id)
    }
    fn set_preference(&self, id: &[u8; 32]) -> Result<(), lux_node::vm::Error> {
        self.inner.set_preference(id)
    }
    fn last_accepted(&self) -> [u8; 32] {
        self.inner.last_accepted()
    }
    fn block_id_at(&self, height: u64) -> Result<[u8; 32], lux_node::vm::Error> {
        self.inner.block_id_at(height)
    }
    fn call(
        &self,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, lux_node::vm::Error> {
        self.inner.call(method, params)
    }
}

/// (d) X's chain fails one verify of honest block A for a local reason (a
/// plugin link timing out). A's proposer and two others signed A; the fifth
/// seat is down, so A needs X. The proposer pushes A again. X must sign A.
#[test]
fn red3_a_block_that_failed_to_run_for_a_local_reason_is_never_taken_again() {
    let mut s = stage("red3-flaky");
    let a = block(1, 0xaa);
    let flaky = Flaky { inner: s.x.chain.clone(), fail: std::sync::Mutex::new(Some(a.id())) };
    let slot = s.theirs(1)[0];
    let p = s.cast.index(s.x.engine.proposer(1, slot));
    let tip = s.x.chain.last_accepted();
    let root = s.cast.committee.root();
    let prop = s.link(p).propose(&CHAIN, &root, &tip, 1, slot, a.bytes()).expect("signed");
    let drive = |s: &mut Stage, d: Duration| {
        let end = Instant::now() + d;
        while Instant::now() < end {
            let step = s.x.engine.step(&flaky, &s.x.host, &mut s.x.votes);
            if step.quiet && step.events.is_empty() {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    };
    drive(&mut s, Duration::from_millis(100));
    let signers: Vec<usize> =
        std::iter::once(p).chain((1..5).filter(|j| *j != p).take(2)).collect();
    for j in &signers {
        s.link(*j).mesh().broadcast(&vote(&s.cast, &s.x.engine, *j, a.as_ref()));
    }
    for _ in 0..6 {
        s.link(p).mesh().propose(&prop);
        drive(&mut s, WINDOW * 2);
    }
    eprintln!("red3-flaky: X signed {:?}", s.x.signed(1).is_some());
    assert_eq!(s.x.signed(1), Some(a.id()), "X signs A once its chain answers again");
}

// ---------------------------------------------------------------------------
// RED round four. Each asserts what a safe, live engine does; a failure is
// the finding, and the eprintln is the measurement.

/// A proposal `slot` at height 1 from that slot's proposer, signed over `raw`.
fn red4_frame(s: &Stage, slot: u64, raw: Vec<u8>) -> lux_node::Proposal {
    let who = s.cast.index(s.x.engine.proposer(1, slot));
    let tip = s.x.chain.last_accepted();
    let root = s.cast.committee.root();
    let header =
        lux_node::proposer::header(&CHAIN, &root, &tip, 1, slot, s.cast.ids[who].public(), &raw);
    let signature = s.cast.ids[who].sign(&header, lux_node::proposer::CONTEXT).expect("sign");
    lux_node::Proposal { from: s.cast.ids[who].node(), height: 1, slot, signature, block: raw }
}

/// (c) The settle budget of one faulty proposer F, spent the way that costs
/// most: ever-lower valid blocks, all above honest A, one each 100 ms for as
/// long as F's live blocks restart the window (RETRY_MAX), then blocks above
/// all of F's, each not kept, one each 140 ms (settle is 150 ms). X signs A
/// only when both are spent.
#[test]
fn red4_one_proposer_holds_the_vote_for_its_whole_budget() {
    let mut s = stage("red4-budget");
    let (live, unkept) = (165usize, 70usize);
    let all = red3_sorted(1 + live + unkept);
    let a = &all[0];
    let slots = s.theirs(4);
    let p_slot = slots[0];
    let p = s.cast.index(s.x.engine.proposer(1, p_slot));
    let f_slot = red3_other(&s, &slots, p);
    let down: Vec<lux_node::Proposal> =
        all[1..=live].iter().rev().map(|b| red4_frame(&s, f_slot, b.bytes())).collect();
    let up: Vec<lux_node::Proposal> =
        all[live + 1..].iter().map(|b| red4_frame(&s, f_slot, b.bytes())).collect();
    let f = s.cast.index(s.x.engine.proposer(1, f_slot));
    s.propose(p_slot, a.as_ref());
    let began = Instant::now();
    for frame in &down {
        if began.elapsed() >= Duration::from_millis(15_800) {
            break;
        }
        s.link(f).mesh().propose(frame);
        s.idle(Duration::from_millis(100));
        if s.x.signed(1).is_some() {
            break;
        }
    }
    let live_phase = began.elapsed();
    for frame in &up {
        if s.x.signed(1).is_some() {
            break;
        }
        s.link(f).mesh().propose(frame);
        s.idle(Duration::from_millis(140));
    }
    let end = Instant::now() + Duration::from_secs(10);
    while s.x.signed(1).is_none() && Instant::now() < end {
        s.idle(Duration::from_millis(5));
    }
    let took = began.elapsed();
    eprintln!(
        "red4-budget: live phase {live_phase:?}; X signed {:?} {took:?} after A was offered",
        s.x.signed(1).map(|id| id == a.id())
    );
    assert!(took < WINDOW * 8, "one proposer held X's vote {took:?}");
}

/// (b) At height 2, on a tip built just now, F signs junk at its slot nearest
/// 18 windows ahead of X's clock (9 s at this window; SKEW is 10 s), then a
/// real block there. X's slot clock must not be moved by a frame that is not
/// a block, and X must not sign a block from a slot nine seconds away.
#[test]
fn red4_a_proposer_nine_seconds_early() {
    let mut s = stage("red4-early");
    let side = Evm::new(genesis());
    let slot = s.theirs(1)[0];
    side.submit(&tx(1, 0, 0xaa)).expect("submit");
    let b1 = side.build().expect("b1");
    side.accept(&b1.id()).expect("accept b1 on the side chain");
    s.propose(slot, b1.as_ref());
    for i in 1..4 {
        s.link(1).mesh().broadcast(&vote(&s.cast, &s.x.engine, i, b1.as_ref()));
    }
    s.until("accepted", |x| x.height() == 1);
    s.idle(Duration::from_millis(20));
    side.submit(&tx(2, 0, 0xbb)).expect("submit");
    let b2 = side.build().expect("b2");
    let me = s.x.host.node();
    let far = (4..=18u64).rev().find(|sl| s.x.engine.proposer(2, *sl) != me).expect("far");
    let f = s.cast.index(s.x.engine.proposer(2, far));
    let root = s.cast.committee.root();
    let before = s.x.engine.slot();
    s.link(f).propose(&CHAIN, &root, &b1.id(), 2, far, b"not a block".to_vec()).expect("signed");
    s.idle(Duration::from_millis(50));
    let after_junk = s.x.engine.slot();
    s.link(f).propose(&CHAIN, &root, &b1.id(), 2, far, b2.bytes()).expect("signed");
    s.idle(WINDOW);
    let signed = s.x.signed(2) == Some(b2.id());
    eprintln!(
        "red4-early: F's slot {far}; X's clock {before:?} -> {after_junk:?} on junk; \
         X signed F's block {:?} after it was offered: {signed}",
        WINDOW
    );
    let moved = after_junk.map_or(0, |a| a.1) > before.map_or(0, |b| b.1) + 1;
    assert!(
        !moved && !signed,
        "junk moved X's clock: {moved}; X signed a block from slot {far}: {signed}"
    );
}

/// (d) F's block b16 goes live at X, then F's b1..b15 and b0, all lower,
/// displace it. F, Y and Z then sign b16 and P signs A, so b16 is the only
/// block that can be certified, and only with X — "a block a verified vote
/// names always has room". Y and Z push the frame they signed. X must sign it.
#[test]
fn red4_a_displaced_block_three_seats_signed_is_taken_back() {
    let mut s = stage("red4-displaced");
    let all = red3_sorted(18);
    let (b0, b16, a) = (&all[0], &all[16], &all[17]);
    let slots = s.theirs(1);
    let f_slot = slots[0];
    let f = s.cast.index(s.x.engine.proposer(1, f_slot));
    let rest: Vec<usize> = (1..5).filter(|j| *j != f).collect();
    let frame16 = red4_frame(&s, f_slot, b16.bytes());
    s.link(f).mesh().propose(&frame16);
    s.idle(Duration::from_millis(10));
    for chunk in all[1..16].chunks(4) {
        for b in chunk {
            s.link(f).mesh().propose(&red4_frame(&s, f_slot, b.bytes()));
        }
        s.idle(Duration::from_millis(10));
    }
    s.link(f).mesh().propose(&red4_frame(&s, f_slot, b0.bytes()));
    s.idle(Duration::from_millis(10));
    for j in [f, rest[0], rest[1]] {
        s.link(1).mesh().broadcast(&vote(&s.cast, &s.x.engine, j, b16.as_ref()));
    }
    s.link(1).mesh().broadcast(&vote(&s.cast, &s.x.engine, rest[2], a.as_ref()));
    s.idle(Duration::from_millis(20));
    let early = s.x.signed(1);
    for _ in 0..6 {
        for j in [rest[0], rest[1]] {
            s.link(j).mesh().propose(&frame16);
        }
        s.idle(WINDOW);
    }
    eprintln!(
        "red4-displaced: signed before the votes: {:?}; X signed b16: {}; height {}",
        early.is_some(),
        s.x.signed(1) == Some(b16.id()),
        s.x.height()
    );
    assert_eq!(early, None, "the probe's timing: X signed before the votes landed");
    assert_eq!(s.x.signed(1), Some(b16.id()), "X signs the one block that can be certified");
}

/// A chain whose verify of one block fails while `down` holds.
struct Down {
    inner: Arc<Relay>,
    id: [u8; 32],
    down: AtomicBool,
    failed: AtomicUsize,
}

impl Vm for Down {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn version(&self) -> String {
        self.inner.version()
    }
    fn build(&self) -> Result<Box<dyn Block>, lux_node::vm::Error> {
        self.inner.build()
    }
    fn parse(&self, raw: &[u8]) -> Result<Box<dyn Block>, lux_node::vm::Error> {
        self.inner.parse(raw)
    }
    fn get(&self, id: &[u8; 32]) -> Result<Box<dyn Block>, lux_node::vm::Error> {
        self.inner.get(id)
    }
    fn verify(&self, id: &[u8; 32]) -> Result<(), lux_node::vm::Error> {
        if *id == self.id && self.down.load(Ordering::SeqCst) {
            self.failed.fetch_add(1, Ordering::SeqCst);
            return Err(lux_node::vm::Error::Unavailable(
                "the far end did not answer: timed out".into(),
            ));
        }
        self.inner.verify(id)
    }
    fn accept(&self, id: &[u8; 32]) -> Result<(), lux_node::vm::Error> {
        self.inner.accept(id)
    }
    fn reject(&self, id: &[u8; 32]) -> Result<(), lux_node::vm::Error> {
        self.inner.reject(id)
    }
    fn set_preference(&self, id: &[u8; 32]) -> Result<(), lux_node::vm::Error> {
        self.inner.set_preference(id)
    }
    fn last_accepted(&self) -> [u8; 32] {
        self.inner.last_accepted()
    }
    fn block_id_at(&self, height: u64) -> Result<[u8; 32], lux_node::vm::Error> {
        self.inner.block_id_at(height)
    }
    fn call(
        &self,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, lux_node::vm::Error> {
        self.inner.call(method, params)
    }
}

/// (d) X's chain link is down while honest A's proposer and three relays push
/// A — seventeen times on each link, the pushes of a few minutes of retries.
/// The chain comes back and A is pushed again on every link. X must take A.
#[test]
fn red4_a_chain_down_for_a_while_refuses_the_honest_block_for_good() {
    let mut s = stage("red4-down");
    let a = block(1, 0xaa);
    let chain = Down {
        inner: s.x.chain.clone(),
        id: a.id(),
        down: AtomicBool::new(true),
        failed: AtomicUsize::new(0),
    };
    let slot = s.theirs(1)[0];
    let prop = red4_frame(&s, slot, a.bytes());
    let drive = |s: &mut Stage, d: Duration| {
        let end = Instant::now() + d;
        while Instant::now() < end {
            let step = s.x.engine.step(&chain, &s.x.host, &mut s.x.votes);
            if step.quiet && step.events.is_empty() {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    };
    for _ in 0..17 {
        for j in 1..5 {
            s.link(j).mesh().propose(&prop);
        }
        drive(&mut s, Duration::from_millis(30));
    }
    chain.down.store(false, Ordering::SeqCst);
    let failed = chain.failed.load(Ordering::SeqCst);
    for _ in 0..6 {
        for j in 1..5 {
            s.link(j).mesh().propose(&prop);
        }
        drive(&mut s, WINDOW);
    }
    eprintln!(
        "red4-down: {failed} verifies failed while down; X signed A after: {}",
        s.x.signed(1) == Some(a.id())
    );
    assert_eq!(s.x.signed(1), Some(a.id()), "X signs A once its chain answers again");
}

// ---------------------------------------------------------------------------
// RED round five. Each asserts what a safe, live engine does; a failure is the
// finding, and the eprintln is the measurement.

/// A block at height 1 on the network's genesis claiming `gas_limit`, whose one
/// transaction is a creation that loops until its gas runs out. Every other
/// header field is what an honest build would put there.
fn red5_heavy(gas_limit: u64) -> (Vec<u8>, [u8; 32]) {
    use alloy_consensus::{SignableTransaction, TxEip1559};
    use alloy_primitives::{Bytes, Signature};
    use revm::primitives::TxKind;

    let g = genesis();
    let mut world = g.world();
    let parent = g.sealed();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    let honest = g.env().next(&parent, None, now.max(parent.header.timestamp)).expect("env");
    let env = lux_evm::block::Env { gas_limit, ..honest };
    let t = TxEip1559 {
        chain_id: g.chain_id,
        nonce: 0,
        gas_limit,
        max_fee_per_gas: u128::from(env.base_fee),
        max_priority_fee_per_gas: 0,
        to: TxKind::Create,
        value: U256::ZERO,
        input: Bytes::from_static(&[0x5b, 0x60, 0x00, 0x56]),
        ..Default::default()
    };
    let (sig, recid) =
        signer(9).sign_prehash_recoverable(t.signature_hash().as_slice()).expect("sign");
    let tx = alloy_consensus::TxEnvelope::Eip1559(t.into_signed(Signature::new(
        U256::from_be_slice(&sig.r().to_bytes()),
        U256::from_be_slice(&sig.s().to_bytes()),
        recid.is_y_odd(),
    )));
    let built =
        lux_evm::block::execute(&mut world, &parent, &env, vec![(tx, account(9))]).expect("runs");
    let raw = alloy_rlp::encode(&alloy_consensus::Block {
        header: built.header.clone(),
        body: alloy_consensus::BlockBody {
            transactions: built.transactions.clone(),
            ommers: Vec::new(),
            withdrawals: None,
        },
    });
    (raw, built.header.seal().0)
}

/// F proposes, in its own slot, a block that claims 50x the chain's gas limit
/// and spends all of it. Go refuses it (VerifyGasLimit). X must not run it.
#[test]
fn red5_a_block_claiming_its_own_gas_limit_is_run_and_signed() {
    let g = genesis();
    let heavy = 50 * g.gas_limit;
    let made = Instant::now();
    let (raw, id) = red5_heavy(heavy);
    let build = made.elapsed();
    let side = Evm::new(genesis());
    let t = Instant::now();
    let parsed = side.parse(&raw).expect("parses");
    let verified = side.verify(&parsed.id());
    let took = t.elapsed();

    let mut s = stage("red5-gas");
    let slot = s.theirs(1)[0];
    let f = s.cast.index(s.x.engine.proposer(1, slot));
    s.link(f).mesh().propose(&red4_frame(&s, slot, raw));
    let began = Instant::now();
    let end = began + Duration::from_secs(30);
    while s.x.signed(1).is_none() && Instant::now() < end {
        s.idle(Duration::from_millis(5));
    }
    eprintln!(
        "red5-gas: claimed {heavy} gas (chain limit {}); built in {build:?}; a seat's verify \
         ok={} in {took:?}; X signed it: {} {:?} after it was offered",
        g.gas_limit,
        verified.is_ok(),
        s.x.signed(1) == Some(id),
        began.elapsed()
    );
    assert!(verified.is_err(), "a block over the chain's gas limit verified in {took:?}");
}

/// F replays ONE frame that does not run, every 5 ms for 3 s, on its own link.
/// Round four ran it at most JUNK_EACH times a height; each run is a full
/// sender recovery and execution before the root is compared.
#[test]
fn red5_one_refused_frame_replayed_runs_every_settle_window() {
    let mut s = stage("red5-junk");
    let a = block(1, 0xaa);
    let chain = Down {
        inner: s.x.chain.clone(),
        id: a.id(),
        down: AtomicBool::new(true),
        failed: AtomicUsize::new(0),
    };
    let slot = s.theirs(1)[0];
    let f = s.cast.index(s.x.engine.proposer(1, slot));
    let prop = red4_frame(&s, slot, a.bytes());
    let run = Duration::from_secs(3);
    let end = Instant::now() + run;
    while Instant::now() < end {
        s.link(f).mesh().propose(&prop);
        let stop = Instant::now() + Duration::from_millis(5);
        while Instant::now() < stop {
            let step = s.x.engine.step(&chain, &s.x.host, &mut s.x.votes);
            if step.quiet && step.events.is_empty() {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
    let n = chain.failed.load(Ordering::SeqCst);
    eprintln!(
        "red5-junk: one frame on one link for {run:?} ran {n} times (JUNK_EACH {})",
        lux_node::engine::JUNK_EACH
    );
    assert!(n <= lux_node::engine::JUNK_EACH as usize, "one refused frame ran {n} times");
}
