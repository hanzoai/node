// SPDX-License-Identifier: BSD-3-Clause-Eco
// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! What a validator does at a height it has already spoken at.
//!
//! The journal refuses a second, different statement at one slot, and that
//! refusal is correct: it is the whole reason the journal is written before the
//! key is used, and it is what stops a restart from contradicting the run
//! before it. Nothing here weakens it — every test below ends by measuring the
//! journal's length, because a validator that signed twice would have grown it.
//!
//! What is tested is the move AFTER the refusal. A node that has said something
//! at a height cannot say a second thing there, but the chain does not stop for
//! it: the other validators certify, and a certificate is checked by a rule
//! this node can run without having voted. So the refusal is a reason not to
//! sign, never a reason to stop — a node holding a losing statement rejoins by
//! accepting the block its peers certified, and it does that silently.
//!
//! Without that, one durable line in a file ends the node: it rebuilds at the
//! same height, is refused for the same reason, and retries forever. That is
//! the state a cluster was found in — the log filling with `already signed at
//! this height` while the height never moved.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use alloy_primitives::{Address, U256};
use blst::min_pk::SecretKey;
use lux_consensus::cert::NodeId;
use lux_consensus::finality::{canonical_vote_message, Finality};
use lux_consensus::pop;
use lux_consensus::Tally;
use lux_node::engine::{Committee, Engine};
use lux_node::evm::Evm;
use lux_node::pq::Identity;
use lux_node::{Config, Host, Journal, Vm as _};

const LOCAL: &str = "127.0.0.1:0";
const REACH: Duration = Duration::from_secs(10);
const EPOCH: u64 = 1;
const CHAIN: [u8; 32] = [0xc1; 32];

struct Cast {
    ids: Vec<Arc<Identity>>,
    keys: Vec<SecretKey>,
    committee: Committee,
}

/// `n` validators, ordered by node id so dial-and-accept is decided, not drawn.
fn cast(n: u8) -> Cast {
    let mut ids: Vec<Arc<Identity>> =
        (0..n).map(|_| Arc::new(Identity::generate().expect("identity"))).collect();
    ids.sort_by_key(|id| id.node());

    let mut keys = Vec::new();
    let mut lines = String::new();
    for (i, id) in ids.iter().enumerate() {
        let sk = SecretKey::key_gen(&[(i as u8).wrapping_add(1); 32], &[]).expect("key_gen");
        let node = id.node();
        let pk = sk.sk_to_pk().compress();
        let proof = pop::sign_over(&sk, &Committee::claim(&CHAIN, &node, &pk));
        lines.push_str(&Committee::line(id.public(), &pk, &proof));
        lines.push('\n');
        keys.push(sk);
    }
    Cast { ids, keys, committee: Committee::read(&lines, CHAIN).expect("committee") }
}

/// A journal directory that removes itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Scratch {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("lux-rejoin-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        Scratch(dir)
    }

    fn journal(&self, name: &str) -> Journal {
        Journal::open(self.0.join(name)).expect("journal")
    }

    fn size(&self, name: &str) -> u64 {
        std::fs::metadata(self.0.join(name)).map(|m| m.len()).unwrap_or(0)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A funded chain with one transaction waiting, so `build` has work to do.
///
/// `mark` goes into the recipient address, so two chains built from two marks
/// produce different blocks at the same height — which is the pair that shares
/// a slot and cannot share a signature.
fn chain(mark: u8) -> Evm {
    use alloy_consensus::{SignableTransaction, TxEip1559};
    use alloy_eips::eip2718::Encodable2718 as _;
    use alloy_primitives::{keccak256, Signature};
    use k256::ecdsa::SigningKey;
    use revm::primitives::TxKind;

    let key = SigningKey::from_bytes(&[7u8; 32].into()).expect("key");
    let point = key.verifying_key().to_encoded_point(false);
    let from = Address::from_slice(&keccak256(&point.as_bytes()[1..])[12..]);

    let genesis = lux_evm::Genesis {
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
    };
    let chain_id = genesis.chain_id;
    let c = Evm::new(genesis);

    let tx = TxEip1559 {
        chain_id,
        nonce: 0,
        gas_limit: 100_000,
        max_fee_per_gas: 30_000_000_000,
        max_priority_fee_per_gas: 1_000_000_000,
        to: TxKind::Call(Address::repeat_byte(mark)),
        value: U256::from(1u64),
        ..Default::default()
    };
    let (sig, recid) = key.sign_prehash_recoverable(tx.signature_hash().as_slice()).expect("sign");
    let signed = alloy_consensus::TxEnvelope::Eip1559(tx.into_signed(Signature::new(
        U256::from_be_slice(&sig.r().to_bytes()),
        U256::from_be_slice(&sig.s().to_bytes()),
        recid.is_y_odd(),
    )));
    c.submit(&signed.encoded_2718()).expect("submit");
    c
}

fn bind(id: Arc<Identity>) -> Host {
    Host::bind(Config { identity: id, bind: LOCAL.parse().expect("addr") }).expect("bind")
}

/// Step `engine` on `chain` for `d`.
fn steps(engine: &mut Engine, chain: &Evm, host: &Host, votes: &mut Journal, d: Duration) {
    let end = std::time::Instant::now() + d;
    while std::time::Instant::now() < end {
        let step = engine.step(chain, host, votes);
        if step.quiet && step.events.is_empty() {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

const WINDOW: Duration = Duration::from_millis(50);

#[test]
fn a_statement_at_this_height_from_an_earlier_run_is_not_a_dead_end() {
    let cast = cast(4);
    let mut engine =
        Engine::new(cast.ids[0].node(), cast.keys[0].clone(), &cast.committee, EPOCH, CHAIN)
            .expect("engine")
            .with_window(WINDOW);
    let host = bind(cast.ids[0].clone());
    let scratch = Scratch::new();
    let mut journal = scratch.journal("votes");

    // The earlier run's statement, durable, for a block this run no longer
    // holds — exactly what a journal is for and exactly what it comes back to.
    let earlier = chain(0xaa);
    let a = earlier.build().expect("a block");
    journal
        .admit(&canonical_vote_message(&engine.position(a.as_ref()), true))
        .expect("the earlier run's statement is admitted to a journal that has said nothing");
    let stood = scratch.size("votes");
    assert!(stood > 0, "the earlier run left a statement to stand by");

    // This run holds a different block's worth of work at that same height.
    let now = chain(0xbb);

    // Alone, so nothing can be certified: the point is that the height does
    // not end on the refusal. Bound, the node builds nothing in its window —
    // it could never sign it — and the slots keep moving, the node with them.
    steps(&mut engine, &now, &host, &mut journal, WINDOW * 12);
    let (height, slot) = engine.slot().expect("a height in progress");
    assert_eq!(height, 1);
    assert!(slot >= 8, "the slots moved on: {slot}");

    // THE GUARD HELD. The journal is written before the key is used, so its
    // length is the count of statements made: unchanged means this validator
    // did not sign a second one — and, locked on what it said, nothing else.
    assert_eq!(
        scratch.size("votes"),
        stood,
        "a second statement was written at a slot this validator had already spoken at"
    );
}

#[test]
fn a_validator_that_already_spoke_rejoins_on_the_quorum_it_did_not_join() {
    let cast = cast(4);
    let mut engine =
        Engine::new(cast.ids[0].node(), cast.keys[0].clone(), &cast.committee, EPOCH, CHAIN)
            .expect("engine")
            .with_window(WINDOW);

    // Four real hosts on real sockets, met in both directions.
    let hosts: Vec<Host> = cast.ids.iter().map(|id| bind(id.clone())).collect();
    let peers: BTreeMap<NodeId, SocketAddr> = hosts.iter().map(|h| (h.node(), h.addr())).collect();
    let hosts: Vec<Host> = hosts
        .into_iter()
        .map(|h| {
            let peers = peers.clone();
            std::thread::spawn(move || {
                h.connect(&peers, REACH);
                h
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|j| j.join().expect("sweep"))
        .collect();

    let scratch = Scratch::new();
    let mut journal = scratch.journal("votes");

    // Same setup as above: this node stands by a block at this height that the
    // rest of the network is not on.
    let earlier = chain(0xaa);
    let a = earlier.build().expect("a block");
    journal
        .admit(&canonical_vote_message(&engine.position(a.as_ref()), true))
        .expect("the earlier run's statement");
    let stood = scratch.size("votes");

    // The block the others are on.
    let now = chain(0xbb);
    let b = now.build().expect("a block");
    let at = engine.position(b.as_ref());

    // The other three sign it — a quorum of four is three, so they can
    // certify without this node and that is the point.
    let mut peer_journals: Vec<Journal> =
        (1..4).map(|i| scratch.journal(&format!("peer{i}.votes"))).collect();
    for (i, h) in hosts.iter().enumerate().skip(1) {
        let tally = Tally::new(at.clone(), Finality::Quasar, EPOCH).expect("tally");
        h.vote(&tally, &cast.keys[i], &mut peer_journals[i - 1]).expect("a peer's statement");
    }

    // It counts what the others said and accepts on a certificate it verified
    // rather than one it helped make.
    let end = std::time::Instant::now() + REACH;
    while now.last_accepted() != b.id() {
        assert!(std::time::Instant::now() < end, "the peers' certificate was never accepted");
        let step = engine.step(&now, &hosts[0], &mut journal);
        if step.quiet && step.events.is_empty() {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    // AND IT NEVER SIGNED. Same measurement as the test above: rejoining is
    // done without making a second statement at the slot.
    assert_eq!(
        scratch.size("votes"),
        stood,
        "the node signed at a slot it had already spoken at in order to rejoin"
    );
}

#[test]
fn the_statement_it_stands_by_goes_back_out_and_is_counted() {
    let cast = cast(4);
    let mut engine =
        Engine::new(cast.ids[0].node(), cast.keys[0].clone(), &cast.committee, EPOCH, CHAIN)
            .expect("engine")
            .with_window(WINDOW);

    let hosts: Vec<Host> = cast.ids.iter().map(|id| bind(id.clone())).collect();
    let peers: BTreeMap<NodeId, SocketAddr> = hosts.iter().map(|h| (h.node(), h.addr())).collect();
    let hosts: Vec<Host> = hosts
        .into_iter()
        .map(|h| {
            let peers = peers.clone();
            std::thread::spawn(move || {
                h.connect(&peers, REACH);
                h
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|j| j.join().expect("sweep"))
        .collect();

    let scratch = Scratch::new();
    let mut journal = scratch.journal("votes");

    // What this node said at this height, in an earlier run: a vote for A.
    let earlier = chain(0xaa);
    let a = earlier.build().expect("a block");
    let at_a = engine.position(a.as_ref());
    journal.admit(&canonical_vote_message(&at_a, true)).expect("the earlier run's statement");
    let stood = scratch.size("votes");

    // A peer is still on A and counting votes for it. It has not heard this
    // node's, because this node restarted between recording it and sending it.
    let mut waiting = Tally::new(at_a.clone(), Finality::Quasar, EPOCH).expect("tally");
    hosts[1].collect(&mut waiting, engine.set());
    assert_eq!(waiting.len(), 0, "the peer has not heard it yet");

    // This node takes up the height again, on a chain that holds other work:
    // the first thing it does is say again what it said there.
    let now = chain(0xbb);
    let _ = engine.step(&now, &hosts[0], &mut journal);

    // The peer counts it: the bytes that went out are the bytes that were
    // signed, at the position the message itself names.
    for _ in 0..500 {
        hosts[1].collect(&mut waiting, engine.set());
        if !waiting.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(waiting.len(), 1, "the statement it stands by reached the peer waiting on it");

    // Said again is not said twice.
    assert_eq!(scratch.size("votes"), stood, "re-sending wrote a second statement");
}

/// The mesh after the opening rendezvous.
///
/// `connect` ends. A validator that restarts comes back to peers that are no
/// longer accepting, so its dial lands in a backlog nobody reads — and being
/// out of the mesh is what makes the recovery above unreachable, because the
/// certificate a returning node rejoins on arrives over a link it does not
/// have. `sweep` is that rendezvous run again, for as long as the node runs.
///
/// Both sides sweep at once, on their own threads, because a meeting has two
/// halves and one of them is always blocked waiting for the other.
#[test]
fn a_validator_that_comes_back_is_met_again() {
    let cast = cast(2);
    let a = bind(cast.ids[0].clone());

    // A restart keeps the node's name AND its address — its peers hold both —
    // so the returning host has to come back to the same one.
    let first = bind(cast.ids[1].clone());
    let addr_b = first.addr();
    let addrs: BTreeMap<NodeId, SocketAddr> =
        [(a.node(), a.addr()), (first.node(), addr_b)].into_iter().collect();

    // They meet at startup, the way a cluster comes up.
    let peers = addrs.clone();
    let met = std::thread::spawn(move || {
        first.connect(&peers, REACH);
        first
    });
    assert_eq!(a.connect(&addrs, REACH), 1, "the two meet at startup");
    let first = met.join().expect("sweep");
    assert_eq!(first.peer_count(), 1, "and both hold the link");

    // That validator's process goes away and comes back at its address.
    drop(first);
    let b = Host::bind(Config { identity: cast.ids[1].clone(), bind: addr_b }).expect("rebind");
    assert_eq!(b.addr(), addr_b, "a restart comes back where its peers expect it");
    assert_eq!(b.peer_count(), 0, "a node that just started has met no one");

    // Neither side is inside `connect` any more. Only the standing rendezvous
    // can put them back together, and it has to work whichever of the two is
    // the one that dials — which their names decide, not the test.
    let peers = addrs.clone();
    let returning = std::thread::spawn(move || {
        for _ in 0..500 {
            b.sweep(&peers);
            if b.peer_count() == 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        b
    });
    for _ in 0..500 {
        a.sweep(&addrs);
        if a.peer_count() >= 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let b = returning.join().expect("the returning validator");

    assert_eq!(b.peer_count(), 1, "the validator that came back was never met again");
    assert!(a.peer_count() >= 1, "and the one that stayed up never met it");
}
