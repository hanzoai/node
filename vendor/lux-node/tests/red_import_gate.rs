// Red-team finding: the uncertified-import gate was in memory only, so a
// restart WITHOUT --import-chain-data cleared it and the node built and signed
// on an imported (uncertified) tip: caught_up() came back true and the signing
// journal grew by one vote on top of imported height 1.
//
// The gate is now a file in the data directory (`<data>/uncertified`), closed
// by the import before its first block is accepted and lifted only where a
// peer's certificate anchors the imported tip. These hold that down against
// the same on-disk state, reopened the way `run.rs` reopens it:
//
// - import, restart without the flag: still refuses to build or sign;
// - a peer's certificate anchors the tip (by the height it is decided at, or
//   by catch-up): the file is gone, and a later restart builds;
// - blocks with no certificates and no gate — what hanzod 0.2.2 left on every
//   seat — start ungated.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alloy_consensus::{SignableTransaction, TxEip1559, TxEnvelope};
use alloy_eips::eip2718::Encodable2718 as _;
use alloy_primitives::{keccak256, Address, Signature, U256};
use blst::min_pk::SecretKey;
use k256::ecdsa::SigningKey;
use lux_consensus::cert::{NodeId, QuorumCert, Vote, DST};
use lux_consensus::finality::{canonical_vote_message, Finality, Position};
use lux_consensus::pop;
use lux_consensus::vote::{SignedVote, VoteTransport as _};
use lux_node::engine::{Committee, Engine};
use lux_node::evm::Evm;
use lux_node::ledger::{self, Ledger};
use lux_node::mesh::{Record, Service};
use lux_node::pq::Identity;
use lux_node::vm::Vm as _;
use lux_node::{Config, Gate, Host, Journal};
use revm::primitives::TxKind;

const CHAIN: [u8; 32] = [0xc1; 32];
const EPOCH: u64 = 1;
const REACH: Duration = Duration::from_secs(20);
const WINDOW: Duration = Duration::from_millis(40);
/// Past the engine's settle window (Go's 150 ms floor), and then some.
const SETTLED: Duration = Duration::from_millis(400);

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Scratch {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("red-gate-{}-{n}", std::process::id()));
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

fn committee() -> (Vec<Arc<Identity>>, Vec<SecretKey>, Committee) {
    let mut ids: Vec<Arc<Identity>> =
        (0..4u8).map(|_| Arc::new(Identity::generate().expect("id"))).collect();
    ids.sort_by_key(|i| i.node());
    let mut keys = Vec::new();
    let mut lines = String::new();
    for (i, id) in ids.iter().enumerate() {
        let sk = SecretKey::key_gen(&[(i as u8) + 1; 32], &[]).expect("kg");
        let pk = sk.sk_to_pk().compress();
        let proof = pop::sign_over(&sk, &Committee::claim(&CHAIN, &id.node(), &pk));
        lines.push_str(&Committee::line(id.public(), &pk, &proof));
        lines.push('\n');
        keys.push(sk);
    }
    (ids.clone(), keys, Committee::read(&lines, CHAIN).expect("committee"))
}

fn funded_genesis(from: Address) -> lux_evm::Genesis {
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

fn signer() -> (SigningKey, Address) {
    let key = SigningKey::from_bytes(&[7u8; 32].into()).expect("key");
    let point = key.verifying_key().to_encoded_point(false);
    let from = Address::from_slice(&keccak256(&point.as_bytes()[1..])[12..]);
    (key, from)
}

fn tx(key: &SigningKey, chain_id: u64, nonce: u64, to: u8) -> Vec<u8> {
    let t = TxEip1559 {
        chain_id,
        nonce,
        gas_limit: 100_000,
        max_fee_per_gas: 30_000_000_000,
        max_priority_fee_per_gas: 1_000_000_000,
        to: TxKind::Call(Address::repeat_byte(to)),
        value: U256::from(1u64),
        ..Default::default()
    };
    let (sig, recid) = key.sign_prehash_recoverable(t.signature_hash().as_slice()).expect("sign");
    TxEnvelope::Eip1559(t.into_signed(Signature::new(
        U256::from_be_slice(&sig.r().to_bytes()),
        U256::from_be_slice(&sig.s().to_bytes()),
        recid.is_y_odd(),
    )))
    .encoded_2718()
}

fn bind(id: Arc<Identity>) -> Host {
    Host::bind(Config { identity: id, bind: "127.0.0.1:0".parse().expect("addr") }).expect("bind")
}

/// The network's side of the story: a chain that produced `n` blocks, and
/// the export of them — genesis first, then each block, as `luxd` writes one.
fn network(n: u64) -> (Evm, Vec<u8>) {
    let (key, from) = signer();
    let source = Evm::new(funded_genesis(from));
    let mut export = source.get(&source.last_accepted()).expect("genesis").bytes();
    for nonce in 0..n {
        source.submit(&tx(&key, source.chain_id(), nonce, 0xaa)).expect("submit");
        let b = source.build().expect("build");
        source.verify(&b.id()).expect("verify");
        source.accept(&b.id()).expect("accept");
        export.extend_from_slice(&b.bytes());
    }
    (source, export)
}

/// One start of validator 0 on `data`, wired the way `run.rs` wires it: the
/// chain from its directory, the certificates beside it, the gate read from
/// the directory whatever the command line says.
struct Start {
    chain: Evm,
    ledger: Arc<Ledger>,
    gate: Arc<Gate>,
    engine: Engine,
}

fn start(data: &Path, ids: &[Arc<Identity>], keys: &[SecretKey], committee: &Committee) -> Start {
    let (_, from) = signer();
    let chain = Evm::open(funded_genesis(from), data).expect("open");
    let ledger = Arc::new(Ledger::open(data.join("certs")).expect("certs"));
    let gate = Arc::new(Gate::open(data.join("uncertified"), chain.height()).expect("gate"));
    let engine = Engine::new(ids[0].node(), keys[0].clone(), committee, EPOCH, CHAIN)
        .expect("engine")
        .with_ledger(ledger.clone())
        .with_gate(gate.clone())
        .with_window(WINDOW);
    Start { chain, ledger, gate, engine }
}

impl Start {
    /// Step the engine until it has been through a slot whose window is its
    /// own — the one moment it would build — and a settle window past it,
    /// when it would sign.
    fn through_its_window(&mut self, host: &Host, votes: &mut Journal) {
        let me = host.node();
        let end = Instant::now() + REACH;
        let mut seen: Option<(u64, Instant)> = None;
        loop {
            assert!(Instant::now() < end, "its window never came");
            let step = self.engine.step(&self.chain, host, votes);
            let (height, slot) = self.engine.slot().expect("a height in progress");
            if seen.is_none() && self.engine.proposer(height, slot) == me {
                seen = Some((slot, Instant::now()));
            }
            if seen.is_some_and(|(s, t)| slot > s && t.elapsed() >= SETTLED) {
                return;
            }
            if step.quiet && step.events.is_empty() {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

fn journal_len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// A validator that imported and is restarted WITHOUT the import flag is still
/// held: it will not build and will not sign until a peer's certificate
/// anchors the imported tip.
#[test]
fn imported_chain_refuses_after_a_restart_without_the_flag() {
    let (ids, keys, committee) = committee();
    let (key, _) = signer();
    let dir = Scratch::new();
    let data = dir.0.clone();
    let (_network, export) = network(1);

    // ---- Run 1: bootstrap with --import-chain-data. ----
    {
        let mut s = start(&data, &ids, &keys, &committee);
        assert!(s.engine.caught_up(), "nothing imported yet");
        let done =
            lux_node::import::from_reader(&s.chain, Cursor::new(export), &s.gate).expect("import");
        assert_eq!((done.from, done.height), (1, 1), "one imported block on disk");
        assert_eq!(s.gate.held(), Some((1, 1)));
        assert_eq!(std::fs::read_to_string(data.join("uncertified")).expect("the gate"), "1\n");
        assert!(!s.engine.caught_up(), "with the flag, the node knows it is uncertified");

        s.chain.submit(&tx(&key, s.chain.chain_id(), 1, 0xbb)).expect("submit");
        let host = bind(ids[0].clone());
        let mut journal = Journal::open(data.join("votes")).expect("journal");
        s.through_its_window(&host, &mut journal);
        assert_eq!(journal_len(&data.join("votes")), 0, "run 1 signed nothing");
        assert_eq!(s.chain.pending(), 1, "and built nothing");
    }

    // ---- Run 2: steady-state restart, flag removed. Same on-disk state. ----
    let mut s = start(&data, &ids, &keys, &committee);
    assert_eq!(s.chain.height(), 1, "the imported (uncertified) block came back");
    assert!(s.ledger.is_empty(), "no certificate was ever recorded for the imported block");
    assert!(!s.engine.caught_up(), "the gate came back from the data directory");

    let raw = tx(&key, s.chain.chain_id(), 1, 0xbb);
    s.chain.submit(&raw).expect("submit");
    let host = bind(ids[0].clone());
    let votes = data.join("votes");
    let mut journal = Journal::open(&votes).expect("journal");
    s.through_its_window(&host, &mut journal);
    assert!(!s.engine.caught_up(), "the gate held across the restart");
    assert_eq!(journal_len(&votes), 0, "SAFETY: nothing signed on an uncertified tip");
    assert_eq!(s.chain.pending(), 1, "and nothing was built");
}

/// Blocks with no certificates and no gate: what hanzod 0.2.2 left on every
/// seat of the network that ran it. That is NOT an import, and inferring one
/// from the shape would halt the network on upgrade.
#[test]
fn blocks_with_no_certificates_and_no_gate_start_ungated() {
    let (ids, keys, committee) = committee();
    let (key, _) = signer();
    let dir = Scratch::new();
    let data = dir.0.clone();
    {
        // Accepted with no certificate recorded, as 0.2.2 accepted them.
        let s = start(&data, &ids, &keys, &committee);
        for nonce in 0..3 {
            s.chain.submit(&tx(&key, s.chain.chain_id(), nonce, 0xaa)).expect("submit");
            let b = s.chain.build().expect("build");
            s.chain.verify(&b.id()).expect("verify");
            s.chain.accept(&b.id()).expect("accept");
        }
    }
    let _ = std::fs::remove_file(data.join("certs"));

    let mut s = start(&data, &ids, &keys, &committee);
    assert_eq!(s.chain.height(), 3);
    assert!(s.ledger.is_empty(), "no certificate under any of them");
    assert!(!data.join("uncertified").exists());
    assert!(s.engine.caught_up(), "no import, no gate");

    s.chain.submit(&tx(&key, s.chain.chain_id(), 3, 0xaa)).expect("submit");
    let host = bind(ids[0].clone());
    let mut journal = Journal::open(data.join("votes")).expect("journal");
    s.through_its_window(&host, &mut journal);
    assert_eq!(s.chain.pending(), 0, "an ordinary validator builds in its window");
    assert!(journal_len(&data.join("votes")) > 0, "and, settled, signs for its own block");
}

/// Two hosts linked on loopback: validator 0 and one peer.
fn linked(ids: &[Arc<Identity>], peer: usize) -> (Host, Host) {
    let a = bind(ids[0].clone());
    let b = bind(ids[peer].clone());
    let pair: BTreeMap<NodeId, SocketAddr> =
        [(a.node(), a.addr()), (b.node(), b.addr())].into_iter().collect();
    // A host is driven by one thread, so the peer is moved into its own and
    // handed back.
    let b = std::thread::scope(|s| {
        let p = &pair;
        let j = s.spawn(move || {
            let reached = b.connect(p, REACH);
            (b, reached)
        });
        assert_eq!(a.connect(p, REACH), 1);
        let (b, reached) = j.join().expect("peer");
        assert_eq!(reached, 1);
        b
    });
    (a, b)
}

/// A vote from validator `i` for `position`, as that validator signs one.
fn vote(ids: &[Arc<Identity>], keys: &[SecretKey], i: usize, position: &Position) -> SignedVote {
    SignedVote {
        position: position.clone(),
        accept: true,
        node: ids[i].node(),
        signature: keys[i]
            .sign(&canonical_vote_message(position, true), DST, &[])
            .compress()
            .to_vec(),
    }
}

/// The way out, at the height: the peers certify a block on the imported tip,
/// this node executes it and stays silent, and the certificate lifts the gate
/// on disk — so a later restart builds.
#[test]
fn a_certificate_on_the_imported_tip_lifts_the_gate_and_a_restart_builds() {
    let (ids, keys, committee) = committee();
    let (key, _) = signer();
    let dir = Scratch::new();
    let data = dir.0.clone();
    let (network, export) = network(1);
    // The next block the network builds, on the tip this node imported.
    network.submit(&tx(&key, network.chain_id(), 1, 0xcc)).expect("submit");
    let next = network.build().expect("build");

    {
        let mut s = start(&data, &ids, &keys, &committee);
        lux_node::import::from_reader(&s.chain, Cursor::new(export), &s.gate).expect("import");
        assert!(data.join("uncertified").exists());

        // The first slot's proposer — the first that is not this node — signs
        // and offers it on its own link, and the three other validators sign
        // it; the proposer relays all three votes, which is all a link is.
        let slot = (0..).find(|s2| s.engine.proposer(2, *s2) != ids[0].node()).expect("a slot");
        let proposer =
            ids.iter().position(|id| id.node() == s.engine.proposer(2, slot)).expect("a member");
        let (host, peer) = linked(&ids, proposer);
        let tip = s.chain.last_accepted();
        peer.propose(&CHAIN, &committee.root(), &tip, 2, slot, next.bytes())
            .expect("a signed proposal");
        let position = s.engine.position(next.as_ref());
        for i in 1..4 {
            peer.mesh().broadcast(&vote(&ids, &keys, i, &position));
        }
        let mut journal = Journal::open(data.join("votes")).expect("journal");
        let end = Instant::now() + REACH;
        while s.chain.height() < 2 {
            assert!(Instant::now() < end, "never anchored on the peers' certificate");
            let step = s.engine.step(&s.chain, &host, &mut journal);
            if step.quiet && step.events.is_empty() {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        let cert = ledger::decode(&s.ledger.get(2).expect("a certificate").1).expect("decodes");
        assert!(cert.votes.iter().all(|v| v.node_id != ids[0].node()), "silent while anchored");
        assert_eq!(journal_len(&data.join("votes")), 0, "and nothing in its journal");
        assert!(s.engine.caught_up(), "a validator like any other");
        assert!(!data.join("uncertified").exists(), "lifted on the device, not only in memory");
        assert_eq!(s.ledger.get(2).map(|(id, _)| id), Some(next.id()), "on the certificate");
    }

    // A later start, no flag: ungated, and it builds and signs.
    let mut s = start(&data, &ids, &keys, &committee);
    assert_eq!(s.chain.height(), 2);
    assert!(s.engine.caught_up(), "the gate did not come back");
    s.chain.submit(&tx(&key, s.chain.chain_id(), 2, 0xdd)).expect("submit");
    let host = bind(ids[0].clone());
    let mut journal = Journal::open(data.join("votes")).expect("journal");
    s.through_its_window(&host, &mut journal);
    assert_eq!(s.chain.pending(), 0, "an anchored node builds in its window");
    assert!(journal_len(&data.join("votes")) > 0, "and, settled, signs for its own block");
}

/// Serves what the test tells it to.
struct Shelf(Mutex<BTreeMap<u64, Record>>);

impl Service for Shelf {
    fn tip(&self) -> u64 {
        self.0.lock().expect("shelf").keys().next_back().copied().unwrap_or(0)
    }
    fn record(&self, height: u64) -> Option<Record> {
        self.0.lock().expect("shelf").get(&height).cloned()
    }
    fn transaction(&self, _: &[u8]) {}
}

/// The way out, by catch-up: the network moved on while this node was
/// importing, and the first certified block it takes on the imported tip
/// lifts the gate on disk.
#[test]
fn a_certified_catch_up_on_the_imported_tip_lifts_the_gate() {
    let (ids, keys, committee) = committee();
    let (key, _) = signer();
    let dir = Scratch::new();
    let data = dir.0.clone();
    let (network, export) = network(1);
    network.submit(&tx(&key, network.chain_id(), 1, 0xcc)).expect("submit");
    let next = network.build().expect("build");

    let s = start(&data, &ids, &keys, &committee);
    lux_node::import::from_reader(&s.chain, Cursor::new(export), &s.gate).expect("import");
    assert!(!s.engine.caught_up());

    let (host, peer) = linked(&ids, 1);
    let position = s.engine.position(next.as_ref());
    let message = canonical_vote_message(&position, true);
    let mut votes: Vec<Vote> = (1..4)
        .map(|i| Vote {
            node_id: ids[i].node(),
            accept: true,
            signature: keys[i].sign(&message, DST, &[]).compress().to_vec(),
        })
        .collect();
    votes.sort_by_key(|v| v.node_id);
    let cert = QuorumCert {
        version: 3,
        qc_type: 1,
        tier: Finality::Quasar,
        position,
        threshold: 3,
        votes,
    };
    let shelf = Arc::new(Shelf(Mutex::new(BTreeMap::from([(
        2,
        Record { block: next.bytes(), cert: ledger::encode(&cert) },
    )]))));
    peer.mesh().serve(shelf);

    let caught = s.engine.catch_up(&s.chain, &host, Duration::from_secs(10)).expect("caught up");
    assert_eq!(caught.accepted, 1, "{:?}", caught.refused);
    assert_eq!(s.chain.height(), 2);
    assert!(s.engine.caught_up());
    assert!(!data.join("uncertified").exists(), "lifted on the device, not only in memory");
    drop(s);
    assert_eq!(Gate::open(data.join("uncertified"), 2).expect("gate").held(), None);
}
