// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! A validator that was not there, catching up — and a transaction sent to one
//! validator, reaching the one that proposes.
//!
//! Real hosts on real sockets, real EVM blocks, real BLS certificates. Nobody
//! re-votes a decided height, so a validator that was down while the others
//! moved on can only come back by being handed each block it missed with the
//! certificate the network formed for it. These tests stand a cluster up
//! without one validator, let the others certify, bring it in, and require it
//! to hold the same block at every height — and then to sign the next one.

use std::collections::BTreeMap;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alloy_primitives::{keccak256, Address, U256};
use blst::min_pk::SecretKey;
use lux_consensus::cert::{NodeId, QuorumCert, Vote, DST};
use lux_consensus::finality::{canonical_vote_message, Finality, Position};
use lux_consensus::pop;
use lux_node::engine::{Committee, Engine, Event};
use lux_node::evm::Evm;
use lux_node::ledger::{self, Ledger};
use lux_node::mesh::{Record, Service};
use lux_node::pq::Identity;
use lux_node::vm::{Block, Vm};
use lux_node::{Config, Host, Journal, Relay};
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

/// `n` validators, ordered by node id so who dials whom is decided, not drawn.
/// `seed` keeps two casts' BLS keys apart.
fn cast(n: u8, seed: u8) -> Cast {
    let mut ids: Vec<Arc<Identity>> =
        (0..n).map(|_| Arc::new(Identity::generate().expect("identity"))).collect();
    ids.sort_by_key(|id| id.node());
    let mut keys = Vec::new();
    let mut lines = String::new();
    for (i, id) in ids.iter().enumerate() {
        let sk = SecretKey::key_gen(&[seed.wrapping_add(i as u8).wrapping_add(1); 32], &[])
            .expect("key_gen");
        let pk = sk.sk_to_pk().compress();
        let proof = pop::sign_over(&sk, &Committee::claim(&CHAIN, &id.node(), &pk));
        lines.push_str(&Committee::line(id.public(), &pk, &proof));
        lines.push('\n');
        keys.push(sk);
    }
    Cast { ids, keys, committee: Committee::read(&lines, CHAIN).expect("committee") }
}

/// A directory that removes itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("lux-catchup-{}-{tag}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn signer() -> k256::ecdsa::SigningKey {
    k256::ecdsa::SigningKey::from_bytes(&[7u8; 32].into()).expect("key")
}

/// One funded account, the same genesis on every node.
fn genesis() -> lux_evm::Genesis {
    let point = signer().verifying_key().to_encoded_point(false);
    let from = Address::from_slice(&keccak256(&point.as_bytes()[1..])[12..]);
    lux_evm::Genesis {
        alloc: vec![(from, lux_evm::Alloc::funded(U256::from(10u64).pow(U256::from(21u64))))],
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

/// A signed transfer from the funded account. `mark` picks the recipient, so
/// two transactions at one nonce make two different blocks.
fn tx(nonce: u64, mark: u8) -> Vec<u8> {
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
        to: TxKind::Call(Address::repeat_byte(mark)),
        value: U256::from(1u64),
        ..Default::default()
    };
    let (sig, recid) =
        signer().sign_prehash_recoverable(t.signature_hash().as_slice()).expect("sign");
    alloy_consensus::TxEnvelope::Eip1559(t.into_signed(Signature::new(
        U256::from_be_slice(&sig.r().to_bytes()),
        U256::from_be_slice(&sig.s().to_bytes()),
        recid.is_y_odd(),
    )))
    .encoded_2718()
}

/// A running validator: what `main` holds, wired the way `main` wires it.
struct Node {
    engine: Engine,
    host: Host,
    chain: Arc<Relay>,
    evm: Arc<Evm>,
    ledger: Arc<Ledger>,
    votes: Journal,
    /// The certificate of each block this validator decided live.
    certs: Vec<QuorumCert>,
    built: Vec<u64>,
}

impl Node {
    fn start(cast: &Cast, i: usize, dir: &Path, bind: SocketAddr) -> Node {
        let host = Host::bind(Config { identity: cast.ids[i].clone(), bind }).expect("bind");
        let evm = Arc::new(Evm::new(genesis()));
        let ledger = Arc::new(Ledger::open(dir.join(format!("{i}.certs"))).expect("ledger"));
        let engine =
            Engine::new(cast.ids[i].node(), cast.keys[i].clone(), &cast.committee, EPOCH, CHAIN)
                .expect("engine")
                .with_ledger(ledger.clone())
                .with_window(WINDOW);
        let chain = Relay::attach(evm.clone(), ledger.clone(), &host);
        let votes = Journal::open(dir.join(format!("{i}.votes"))).expect("journal");
        Node { engine, host, chain, evm, ledger, votes, certs: Vec::new(), built: Vec::new() }
    }

    fn id_at(&self, height: u64) -> [u8; 32] {
        self.chain.block_id_at(height).expect("an accepted block at that height")
    }

    fn step(&mut self) -> bool {
        let step = self.engine.step(self.chain.as_ref(), &self.host, &mut self.votes);
        for e in &step.events {
            if let Event::Decided { cert, built, height, .. } = e {
                self.certs.push((**cert).clone());
                if *built {
                    self.built.push(*height);
                }
            }
        }
        step.quiet && step.events.is_empty()
    }
}

/// Where every validator listens. A validator that is not up yet still has an
/// address — its peers hold it — so one is reserved for it.
fn addresses(n: usize) -> Vec<SocketAddr> {
    (0..n)
        .map(|_| TcpListener::bind("127.0.0.1:0").expect("bind").local_addr().expect("addr"))
        .collect()
}

/// Every node that is up sweeps until it holds `want` peers.
fn meet(nodes: &mut [Option<Node>], peers: &BTreeMap<NodeId, SocketAddr>, want: usize) {
    std::thread::scope(|s| {
        for n in nodes.iter_mut().flatten() {
            s.spawn(move || {
                let end = Instant::now() + REACH;
                while n.host.peer_count() < want && Instant::now() < end {
                    n.host.sweep(peers);
                    std::thread::sleep(Duration::from_millis(10));
                }
                assert_eq!(
                    n.host.peer_count(),
                    want,
                    "every validator that is up meets the others"
                );
            });
        }
    });
}

/// Step every node that is up, each on its own thread, until every one of
/// them has accepted `height`. Returns the certificate the height was decided
/// on, as the first node that decided it live holds it.
fn decide(nodes: &mut [Option<Node>], height: u64) -> QuorumCert {
    let up = nodes.iter().flatten().count();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let end = Instant::now() + REACH * 3;
    std::thread::scope(|s| {
        for n in nodes.iter_mut().flatten() {
            let done = &done;
            s.spawn(move || {
                let mut counted = false;
                while done.load(std::sync::atomic::Ordering::SeqCst) < up {
                    assert!(Instant::now() < end, "height {height} was not decided");
                    let quiet = n.step();
                    if !counted && n.evm.height() >= height {
                        counted = true;
                        done.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                    if quiet {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
            });
        }
    });
    nodes
        .iter()
        .flatten()
        .find_map(|n| n.certs.iter().find(|c| c.position.height == height).cloned())
        .expect("a node decided it")
}

/// Validator `late` is not there while the others certify `blocks` heights.
/// Returns the nodes (the late one absent), their addresses, and the peer map.
fn without(
    cast: &Cast,
    dir: &Path,
    late: usize,
    blocks: u64,
) -> (Vec<Option<Node>>, Vec<SocketAddr>, BTreeMap<NodeId, SocketAddr>) {
    let n = cast.ids.len();
    let addrs = addresses(n);
    let peers: BTreeMap<NodeId, SocketAddr> =
        cast.ids.iter().zip(&addrs).map(|(id, a)| (id.node(), *a)).collect();
    let mut nodes: Vec<Option<Node>> =
        (0..n).map(|i| (i != late).then(|| Node::start(cast, i, dir, addrs[i]))).collect();
    let present: BTreeMap<NodeId, SocketAddr> =
        peers.iter().filter(|(k, _)| **k != cast.ids[late].node()).map(|(k, v)| (*k, *v)).collect();
    meet(&mut nodes, &present, n - 2);

    let up: Vec<usize> = (0..n).filter(|i| *i != late).collect();
    for h in 0..blocks {
        // Sent to one validator; whichever has the window builds it.
        let to = up[h as usize % up.len()];
        nodes[to].as_ref().expect("up").evm.submit(&tx(h, 0xaa)).expect("submit");
        decide(&mut nodes, h + 1);
    }
    let lead = nodes[up[0]].as_ref().expect("up");
    for i in &up {
        let node = nodes[*i].as_ref().expect("up");
        assert_eq!(node.evm.height(), blocks, "validator {i} certified every height");
        assert_eq!(node.ledger.len() as u64, blocks, "and kept every certificate");
        for h in 1..=blocks {
            assert_eq!(node.id_at(h), lead.id_at(h), "one chain");
        }
    }
    (nodes, addrs, peers)
}

#[test]
fn a_validator_that_starts_late_catches_up_and_signs_the_next_block() {
    // Five validators, so the Quasar quorum is four. One is absent while the
    // other four certify; then one of the four goes away, and the next block
    // can only be certified if the one that was absent signs it.
    const BLOCKS: u64 = 100;
    let cast = cast(5, 0);
    let scratch = Scratch::new("late");
    let late = 2;
    let (mut nodes, addrs, peers) = without(&cast, &scratch.0, late, BLOCKS);

    // It comes up with nothing: genesis, an empty ledger, no peers.
    nodes[late] = Some(Node::start(&cast, late, &scratch.0, addrs[late]));
    assert_eq!(nodes[late].as_ref().expect("up").evm.height(), 0);
    meet(&mut nodes, &peers, 4);

    let began = Instant::now();
    let node = nodes[late].as_mut().expect("up");
    let caught = node
        .engine
        .catch_up(node.chain.as_ref(), &node.host, Duration::from_secs(120))
        .expect("catch up");
    let took = began.elapsed();
    eprintln!(
        "caught up {} blocks in {:?}: {:.0} blocks/s",
        caught.accepted,
        took,
        caught.accepted as f64 / took.as_secs_f64()
    );
    assert!(caught.refused.is_empty(), "every peer was honest: {:?}", caught.refused);
    assert_eq!(caught.accepted, BLOCKS, "every height it missed");
    assert_eq!(caught.reported, BLOCKS);

    // The same block at every height, and the certificate it was accepted on.
    let lead = nodes[0].as_ref().expect("up");
    let node = nodes[late].as_ref().expect("up");
    assert_eq!(node.evm.height(), BLOCKS);
    assert_eq!(node.ledger.len() as u64, BLOCKS);
    for h in 1..=BLOCKS {
        assert_eq!(node.id_at(h), lead.id_at(h), "height {h}");
        let (id, raw) = node.ledger.get(h).expect("a certificate");
        assert_eq!(id, lead.id_at(h));
        let cert = ledger::decode(&raw).expect("decodes");
        assert_eq!(cert.position.height, h);
        cert.verify_weighted(node.engine.set(), node.engine.set(), EPOCH)
            .expect("the kept certificate verifies");
    }
    // Catching up signed nothing: every height was decided without it.
    assert_eq!(std::fs::metadata(scratch.0.join(format!("{late}.votes"))).expect("stat").len(), 0);

    // One of the four that carried the chain goes away. Four validators are
    // left and four is the quorum, so the next block is certified only if the
    // one that caught up signs it.
    nodes[3] = None;
    nodes[0].as_ref().expect("up").evm.submit(&tx(BLOCKS, 0xaa)).expect("submit");
    let cert = decide(&mut nodes, BLOCKS + 1);
    let voter = cast.ids[late].node();
    assert!(
        cert.votes.iter().any(|v| v.node_id == voter),
        "the validator that caught up signed the next block"
    );
    let lead = nodes[0].as_ref().expect("up");
    let node = nodes[late].as_ref().expect("up");
    assert_eq!(node.evm.height(), BLOCKS + 1);
    assert_eq!(node.id_at(BLOCKS + 1), lead.id_at(BLOCKS + 1));
}

#[test]
fn a_validator_that_hears_the_others_ahead_fetches_the_gap_and_then_votes() {
    // No catch-up at startup: the first sign that this validator is behind is
    // the others speaking about a later height. It fetches the gap, then
    // votes on the next block like any validator at the tip — it has to: one
    // of the five is gone, and the four left are the quorum.
    const BLOCKS: u64 = 12;
    let cast = cast(5, 0);
    let scratch = Scratch::new("gap");
    let late = 1;
    let (mut nodes, addrs, peers) = without(&cast, &scratch.0, late, BLOCKS);
    nodes[4] = None;
    nodes[late] = Some(Node::start(&cast, late, &scratch.0, addrs[late]));
    let present: BTreeMap<NodeId, SocketAddr> =
        peers.iter().filter(|(k, _)| **k != cast.ids[4].node()).map(|(k, v)| (*k, *v)).collect();
    meet(&mut nodes, &present, 3);

    nodes[0].as_ref().expect("up").evm.submit(&tx(BLOCKS, 0xaa)).expect("submit");
    let cert = decide(&mut nodes, BLOCKS + 1);

    let lead = nodes[0].as_ref().expect("up");
    let node = nodes[late].as_ref().expect("up");
    assert_eq!(node.evm.height(), BLOCKS + 1, "it caught up on hearing it was behind");
    for h in 1..=BLOCKS + 1 {
        assert_eq!(node.id_at(h), lead.id_at(h), "height {h}");
    }
    assert!(cert.votes.iter().any(|v| v.node_id == cast.ids[late].node()), "and it voted on it");
}

#[test]
fn a_transaction_sent_to_one_validator_is_included_by_another() {
    let cast = cast(4, 0);
    let scratch = Scratch::new("gossip");
    let addrs = addresses(4);
    let peers: BTreeMap<NodeId, SocketAddr> =
        cast.ids.iter().zip(&addrs).map(|(id, a)| (id.node(), *a)).collect();
    let mut nodes: Vec<Option<Node>> =
        (0..4).map(|i| Some(Node::start(&cast, i, &scratch.0, addrs[i]))).collect();
    meet(&mut nodes, &peers, 3);

    // Submitted the way a wallet does, over RPC, to a validator whose window
    // height 1's first slot is NOT: only the proposer builds, so the
    // transaction is in a block only if it reached the proposer.
    let lead = nodes[0].as_ref().expect("up").engine.proposer(1, 0);
    let proposer = cast.ids.iter().position(|id| id.node() == lead).expect("a member");
    let sender = (proposer + 1) % 4;
    let raw = tx(0, 0xcc);
    let hash = format!("{:#x}", keccak256(&raw));
    let sent = nodes[sender]
        .as_ref()
        .expect("up")
        .chain
        .call(
            "eth_sendRawTransaction",
            &json!([format!("0x{}", alloy_primitives::hex::encode(&raw))]),
        )
        .expect("admitted");
    assert_eq!(sent, json!(hash));

    // Every other pool has it.
    let end = Instant::now() + REACH;
    while nodes.iter().flatten().any(|n| n.evm.pending() == 0) {
        assert!(Instant::now() < end, "the transaction never reached every pool");
        std::thread::sleep(Duration::from_millis(2));
    }
    // Sent again, and to a second validator: still one transaction everywhere.
    for i in [sender, (sender + 1) % 4] {
        nodes[i]
            .as_ref()
            .expect("up")
            .chain
            .call(
                "eth_sendRawTransaction",
                &json!([format!("0x{}", alloy_primitives::hex::encode(&raw))]),
            )
            .expect("admitted again");
    }
    std::thread::sleep(Duration::from_millis(50));
    assert!(nodes.iter().flatten().all(|n| n.evm.pending() == 1), "one entry per pool");

    // The proposer builds it — it was never sent the transaction; it has it
    // because it was relayed.
    decide(&mut nodes, 1);
    let builder: Vec<usize> =
        (0..4).filter(|i| nodes[*i].as_ref().is_some_and(|n| n.built.contains(&1))).collect();
    assert_eq!(
        builder,
        vec![proposer],
        "the first slot's proposer built height 1, and nobody else"
    );
    assert_ne!(proposer, sender);
    let mut at = None;
    for (i, n) in nodes.iter().flatten().enumerate() {
        let receipt = n.chain.call("eth_getTransactionReceipt", &json!([hash])).expect("receipt");
        assert_eq!(receipt["blockNumber"], json!("0x1"), "validator {i} holds it in block 1");
        let block = receipt["blockHash"].clone();
        assert_eq!(*at.get_or_insert(block.clone()), block, "the same block everywhere");
    }
}

/// A peer that answers whatever the test tells it to.
struct Forger(Mutex<BTreeMap<u64, Record>>);

impl Forger {
    fn serve(&self, records: Vec<(u64, Record)>) {
        *self.0.lock().expect("forger") = records.into_iter().collect();
    }
}

impl Service for Forger {
    fn tip(&self) -> u64 {
        99
    }
    fn record(&self, height: u64) -> Option<Record> {
        self.0.lock().expect("forger").get(&height).cloned()
    }
    fn transaction(&self, _: &[u8]) {}
}

/// A certificate over `position` carrying the votes of `signers`, as a tally
/// would assemble it, declaring `threshold`.
fn signed(cast: &Cast, signers: &[usize], position: &Position, threshold: u32) -> QuorumCert {
    let message = canonical_vote_message(position, true);
    let mut votes: Vec<Vote> = signers
        .iter()
        .map(|i| Vote {
            node_id: cast.ids[*i].node(),
            accept: true,
            signature: cast.keys[*i].sign(&message, DST, &[]).compress().to_vec(),
        })
        .collect();
    votes.sort_by_key(|v| v.node_id);
    QuorumCert {
        version: 3,
        qc_type: 1,
        tier: Finality::Quasar,
        position: position.clone(),
        threshold,
        votes,
    }
}

#[test]
fn a_certificate_that_does_not_prove_its_block_is_refused() {
    let cast = cast(4, 0);
    let scratch = Scratch::new("forged");
    let addrs = addresses(2);
    let mut victim = Node::start(&cast, 0, &scratch.0, addrs[0]);
    let liar = Host::bind(Config { identity: cast.ids[1].clone(), bind: addrs[1] }).expect("bind");
    let forger = Arc::new(Forger(Mutex::new(BTreeMap::new())));
    liar.mesh().serve(forger.clone());
    let pair: BTreeMap<NodeId, SocketAddr> =
        [(victim.host.node(), addrs[0]), (liar.node(), addrs[1])].into_iter().collect();
    let liar = std::thread::scope(|s| {
        let p = &pair;
        let j = s.spawn(move || {
            liar.connect(p, REACH);
            liar
        });
        assert_eq!(victim.host.connect(p, REACH), 1);
        j.join().expect("liar")
    });

    // Real blocks: the one at height 1, a different one at height 1, and the
    // one at height 2 on top of the first.
    let other = Evm::new(genesis());
    other.submit(&tx(0, 0xbb)).expect("submit");
    let b1x = other.build().expect("b1'");
    let side = Evm::new(genesis());
    side.submit(&tx(0, 0xaa)).expect("submit");
    let b1 = side.build().expect("b1");
    side.accept(&b1.id()).expect("accept b1");
    side.submit(&tx(1, 0xaa)).expect("submit");
    let b2 = side.build().expect("b2");

    let at1 = victim.engine.position(b1.as_ref());
    let at1x = victim.engine.position(b1x.as_ref());
    let at2 = victim.engine.position(b2.as_ref());
    let record =
        |b: &dyn Block, c: &QuorumCert| Record { block: b.bytes(), cert: ledger::encode(c) };

    let mut foreign_sig = signed(&cast, &[0, 2, 3], &at1, 3);
    foreign_sig.votes[1].signature = signed(&cast, &[2], &at1x, 3).votes[0].signature.clone();
    foreign_sig.votes.sort_by_key(|v| v.node_id);
    let strangers = self::cast(4, 100);
    let mut nova = signed(&cast, &[0, 2, 3], &at1, 3);
    nova.tier = Finality::Nova;
    let elsewhere = Position { chain_id: [0xc2; 32], ..at1.clone() };

    // What the liar serves, and the reason the refusal must name.
    type Case<'a> = (&'a str, Vec<(u64, Record)>, &'a str);
    let cases: Vec<Case> = vec![
        (
            "two signatures of the three a quorum needs",
            vec![(1, record(b1.as_ref(), &signed(&cast, &[2, 3], &at1, 3)))],
            "BelowThreshold",
        ),
        (
            "a certificate declaring its own quorum of two",
            vec![(1, record(b1.as_ref(), &signed(&cast, &[2, 3], &at1, 2)))],
            "ThresholdNotDerived",
        ),
        (
            "one signature that is over another block",
            vec![(1, record(b1.as_ref(), &foreign_sig))],
            "SigInvalid",
        ),
        (
            "a full quorum from another committee",
            vec![(1, record(b1.as_ref(), &signed(&strangers, &[0, 1, 2, 3], &at1, 3)))],
            "SigInvalid",
        ),
        ("a nova certificate", vec![(1, record(b1.as_ref(), &nova))], "nova certificate"),
        (
            "a quorum over another chain",
            vec![(1, record(b1.as_ref(), &signed(&cast, &[0, 2, 3], &elsewhere, 3)))],
            "another chain",
        ),
        (
            "a real quorum for a different block at this height",
            vec![(1, record(b1.as_ref(), &signed(&cast, &[0, 2, 3], &at1x, 3)))],
            "does not sign this block",
        ),
        (
            "a real quorum for height 2, served as height 1",
            vec![(1, record(b2.as_ref(), &signed(&cast, &[0, 2, 3], &at2, 3)))],
            "a certificate for height 2",
        ),
        (
            "bytes that are not a certificate",
            vec![(1, Record { block: b1.bytes(), cert: b"a certificate, honestly".to_vec() })],
            "not a certificate",
        ),
    ];

    for (what, records, why) in cases {
        forger.serve(records);
        let caught = victim
            .engine
            .catch_up(victim.chain.as_ref(), &victim.host, Duration::from_secs(10))
            .expect("a peer's lie is not this node's failure");
        assert_eq!(caught.accepted, 0, "{what}: accepted");
        assert_eq!(caught.refused.len(), 1, "{what}: {:?}", caught.refused);
        let (who, reason) = &caught.refused[0];
        assert_eq!(*who, liar.node(), "{what}: the liar is the one not believed");
        assert!(reason.contains(why), "{what}: refused for {reason}, expected {why}");
        assert_eq!(victim.evm.height(), 0, "{what}: nothing accepted");
        assert!(victim.ledger.is_empty(), "{what}: nothing recorded");
        // Not even held: a block is shown to the chain only once a
        // certificate proves it, and dropped again if the proof is not for it.
        assert!(victim.evm.get(&b1.id()).is_err(), "{what}: the block is not held");
    }

    // The control: the same plumbing, told the truth, is believed — so every
    // refusal above was for the reason it names. And a lie after the truth
    // costs only the lie.
    forger.serve(vec![
        (1, record(b1.as_ref(), &signed(&cast, &[0, 2, 3], &at1, 3))),
        (2, record(b2.as_ref(), &signed(&cast, &[2, 3], &at2, 3))),
    ]);
    let caught = victim
        .engine
        .catch_up(victim.chain.as_ref(), &victim.host, Duration::from_secs(10))
        .expect("catch up");
    assert_eq!(caught.accepted, 1, "{:?}", caught.refused);
    assert_eq!(caught.refused.len(), 1, "the lie at height 2");
    assert_eq!(victim.evm.height(), 1);
    assert_eq!(victim.id_at(1), b1.id());
    assert_eq!(victim.ledger.get(1).expect("kept").0, b1.id());

    forger.serve(vec![(2, record(b2.as_ref(), &signed(&cast, &[1, 2, 3], &at2, 3)))]);
    let caught = victim
        .engine
        .catch_up(victim.chain.as_ref(), &victim.host, Duration::from_secs(10))
        .expect("catch up");
    assert_eq!(caught.accepted, 1, "{:?}", caught.refused);
    assert_eq!(victim.id_at(2), b2.id());
    drop(liar);
    let _ = &mut victim;
}

#[test]
fn a_block_this_node_built_is_not_lost_to_a_peer_that_serves_it_wrongly() {
    // This node proposed and missed quorum. A peer then serves that same
    // block under a real certificate for ANOTHER block at its height. The
    // block is refused and let go — and what its users sent is not.
    let cast = cast(4, 0);
    let scratch = Scratch::new("replayed");
    let addrs = addresses(2);
    let victim = Node::start(&cast, 0, &scratch.0, addrs[0]);
    let liar = Host::bind(Config { identity: cast.ids[1].clone(), bind: addrs[1] }).expect("bind");
    let forger = Arc::new(Forger(Mutex::new(BTreeMap::new())));
    liar.mesh().serve(forger.clone());
    let pair: BTreeMap<NodeId, SocketAddr> =
        [(victim.host.node(), addrs[0]), (liar.node(), addrs[1])].into_iter().collect();
    let liar = std::thread::scope(|s| {
        let p = &pair;
        let j = s.spawn(move || {
            liar.connect(p, REACH);
            liar
        });
        assert_eq!(victim.host.connect(p, REACH), 1);
        j.join().expect("liar")
    });

    victim.evm.submit(&tx(0, 0xaa)).expect("submit");
    // Its own block, the one it would propose in its window, and no quorum
    // for it: one validator of four cannot certify.
    let mine = victim.chain.build().expect("its block").bytes();
    assert_eq!(victim.evm.pending(), 0, "the transaction went into the block");

    let other = Evm::new(genesis());
    other.submit(&tx(0, 0xbb)).expect("submit");
    let theirs = other.build().expect("build");
    let at = victim.engine.position(theirs.as_ref());
    let cert = signed(&cast, &[1, 2, 3], &at, 3);
    forger.serve(vec![(1, Record { block: mine, cert: ledger::encode(&cert) })]);
    let caught = victim
        .engine
        .catch_up(victim.chain.as_ref(), &victim.host, Duration::from_secs(10))
        .expect("a peer's lie is not this node's failure");
    assert_eq!(caught.accepted, 0);
    assert!(
        caught.refused.iter().any(|(_, why)| why.contains("does not sign this block")),
        "{:?}",
        caught.refused
    );
    assert_eq!(victim.evm.pending(), 1, "the transaction is back in the pool");
    drop(liar);
}

/// A peer that takes a question and does not answer it.
struct Silent;

impl Service for Silent {
    fn tip(&self) -> u64 {
        0
    }
    fn record(&self, _: u64) -> Option<Record> {
        std::thread::sleep(Duration::from_secs(5));
        None
    }
    fn transaction(&self, _: &[u8]) {}
}

/// One peer never answers, one serves the certified block. Every peer is asked
/// at once, so the silent one costs no turn: the block is taken inside a
/// deadline shorter than one peer's wait used to be.
#[test]
fn a_silent_peer_does_not_hold_a_catch_up() {
    let cast = cast(4, 0);
    let scratch = Scratch::new("silent");
    let addrs = addresses(3);
    let victim = Node::start(&cast, 0, &scratch.0, addrs[0]);
    let honest =
        Host::bind(Config { identity: cast.ids[1].clone(), bind: addrs[1] }).expect("bind");
    let quiet = Host::bind(Config { identity: cast.ids[2].clone(), bind: addrs[2] }).expect("bind");
    let side = Evm::new(genesis());
    side.submit(&tx(0, 0xaa)).expect("submit");
    let b1 = side.build().expect("b1");
    let at1 = victim.engine.position(b1.as_ref());
    let forger = Arc::new(Forger(Mutex::new(BTreeMap::new())));
    forger.serve(vec![(
        1,
        Record { block: b1.bytes(), cert: ledger::encode(&signed(&cast, &[1, 2, 3], &at1, 3)) },
    )]);
    honest.mesh().serve(forger);
    quiet.mesh().serve(Arc::new(Silent));
    let trio: BTreeMap<NodeId, SocketAddr> =
        [(victim.host.node(), addrs[0]), (honest.node(), addrs[1]), (quiet.node(), addrs[2])]
            .into_iter()
            .collect();
    let (honest, quiet) = std::thread::scope(|s| {
        let t = &trio;
        let a = s.spawn(move || {
            honest.connect(t, REACH);
            honest
        });
        let b = s.spawn(move || {
            quiet.connect(t, REACH);
            quiet
        });
        assert_eq!(victim.host.connect(t, REACH), 2);
        (a.join().expect("honest"), b.join().expect("quiet"))
    });

    let began = Instant::now();
    let caught = victim
        .engine
        .catch_up(victim.chain.as_ref(), &victim.host, Duration::from_millis(400))
        .expect("catch up");
    let took = began.elapsed();
    assert_eq!(caught.accepted, 1, "{:?}", caught.refused);
    assert_eq!(victim.id_at(1), b1.id());
    assert!(took < Duration::from_secs(1), "a silent peer held the catch-up {took:?}");
    drop((honest, quiet));
}
