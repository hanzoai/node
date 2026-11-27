// SPDX-License-Identifier: BSD-3-Clause-Eco
// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! Importing the Lux testnet C-chain, and what a node may do afterwards.
//!
//! Two things are checked here and they are not the same thing.
//!
//! FIRST, that the import produces the chain. Not "a chain of the right
//! length": every block's parent is checked against the block before it, every
//! block is re-executed, and the tip's hash and state root are compared against
//! what the Go node reports for the same export. A different root is a
//! different chain, whatever the block count says.
//!
//! SECOND, that the import does NOT produce a validator. The export carries no
//! certificates, so a node that read one holds history nothing agreed to. Go
//! records exactly this in `vms/proposervm/vm.go`: an import "writes inner EVM
//! blocks directly and never creates a single outer envelope", after which the
//! chain "MUST NOT be treated as a caught-up validator until the outer index is
//! rebuilt from certified peer state". A runtime that imported and then
//! proposed would be signing history it never saw agreed to — worse than one
//! that cannot import at all. So the refusal is tested as carefully as the
//! import.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alloy_primitives::{b256, B256};
use blst::min_pk::SecretKey;
use lux_consensus::cert::NodeId;
use lux_consensus::pop;
use lux_node::engine::{Committee, Engine};
use lux_node::evm::Evm;
use lux_node::pq::Identity;
use lux_node::{Config, Gate, Host, Journal, Vm as _};

/// The C-chain genesis the Go testnet node is started with, as it appears in
/// that node's own `genesis-96368.json`.
const CCHAIN: &str = include_str!("../genesis/testnet.json");
/// The 32-byte chain the committee is published for and the engine drives.
const CHAIN: [u8; 32] = [0xc1; 32];

/// What the Go node reports for that genesis, and for the export imported onto
/// it. Not our numbers — read off `luxd --import-chain-data` over JSON-RPC.
const GENESIS_HASH: B256 =
    b256!("1c5fe37764b8bc146dc88bc1c2e0259cd8369b07a06439bcfa1782b5d4fb0995");
const GENESIS_ROOT: B256 =
    b256!("2d1cedac263020c5c56ef962f6abe0da1f5217bdc6468f8c9258a0ea23699e80");
const TIP_HEIGHT: u64 = 218;
const TIP_HASH: B256 = b256!("722e2b39ae8973ab5d94b51451623352650b728e411ce0261c5efcd23aa381a5");
const TIP_ROOT: B256 = b256!("4e19366fcc65d7ddd0b803bfbd7537f0c0ddc5d190c3ded40712408fcb137f35");
const TIP_TIME: u64 = 1_746_815_479;

/// Where the canonical export lives.
///
/// It is a read-only dataset held outside this repository — gigabytes across
/// five networks — so the path is named rather than vendored, and the test says
/// so out loud instead of passing quietly when it is absent.
fn export() -> Option<PathBuf> {
    let path = match std::env::var("LUX_RLP_TESTNET") {
        Ok(p) => PathBuf::from(p),
        Err(_) => return None,
    };
    path.exists().then_some(path)
}

fn testnet() -> Evm {
    Evm::new(lux_node::genesis::parse(CCHAIN).expect("the testnet genesis document parses"))
}

#[test]
fn the_genesis_is_the_block_the_go_node_calls_zero() {
    // Before any import can mean anything: this node and the Go node have to
    // agree on what block zero is. The state root is computed from the
    // allocation here; the hash is keccak of the header this crate builds.
    let c = testnet();
    let zero = c.get(&c.last_accepted()).expect("genesis");
    assert_eq!(B256::from(zero.id()), GENESIS_HASH);
    assert_eq!(B256::from(zero.state_root()), GENESIS_ROOT);
    assert_eq!(zero.height(), 0);
}

#[test]
fn the_export_replays_to_the_tip_the_go_node_reports() {
    let Some(path) = export() else {
        panic!(
            "LUX_RLP_TESTNET is not set to the lux-testnet-96368 export. \
             This test compares against the Go node's own tip and cannot be run without it."
        );
    };
    let c = testnet();
    let gate = Gate::memory();
    let done = lux_node::import::from_file(&c, &path, &gate).expect("the export imports");
    assert_eq!(gate.held(), Some((1, TIP_HEIGHT)), "every height it read is behind the gate");

    assert_eq!(done.blocks, TIP_HEIGHT as usize);
    assert_eq!(done.from, 1);
    assert_eq!(done.height, TIP_HEIGHT);
    assert_eq!(B256::from(done.tip), TIP_HASH);
    assert_eq!(B256::from(done.state_root), TIP_ROOT);

    // BLOCK BY BLOCK, after the fact as well as during. Walking the accepted
    // chain backwards from the tip and checking each parent against the block
    // below it is the same check the import made, made again against what was
    // actually kept — a chain that imported cleanly and stored something else
    // would pass the first and fail this.
    let mut at = c.last_accepted();
    for height in (1..=TIP_HEIGHT).rev() {
        let block = c.get(&at).expect("every accepted block is held");
        assert_eq!(block.height(), height);
        assert_eq!(
            c.block_id_at(height).expect("the height is indexed"),
            at,
            "height {height} does not name the block it holds"
        );
        at = block.parent();
    }
    assert_eq!(B256::from(at), GENESIS_HASH, "the chain reaches its genesis");

    let tip = c.get(&c.last_accepted()).expect("tip");
    assert_eq!(tip.timestamp(), TIP_TIME);
}

/// A committee of `n` validators with real keys, and the journals they sign
/// against — the same construction a live cluster uses, because a gate proved
/// against a stub validator is a gate proved against nothing.
struct Cast {
    ids: Vec<Arc<Identity>>,
    keys: Vec<SecretKey>,
    committee: Committee,
}

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
        let dir = std::env::temp_dir().join(format!("lux-import-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const WINDOW: Duration = Duration::from_millis(40);
/// Past the engine's settle window (Go's 150 ms floor), and then some.
const SETTLED: Duration = Duration::from_millis(400);

fn len(path: &std::path::Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Step `engine` on `chain` until a slot whose window is `engine`'s own has
/// come and gone — the one moment it would build — and `at_least` since it
/// began: the settle window, when it would sign.
fn through_its_window(
    engine: &mut Engine,
    chain: &Evm,
    host: &Host,
    votes: &mut Journal,
    at_least: Duration,
) {
    let me = host.node();
    let began = Instant::now();
    let end = began + Duration::from_secs(20);
    let mut seen = None;
    loop {
        assert!(Instant::now() < end, "its window never came");
        let step = engine.step(chain, host, votes);
        let (height, slot) = engine.slot().expect("a height in progress");
        if seen.is_none() && engine.proposer(height, slot) == me {
            seen = Some(slot);
        }
        if seen.is_some_and(|s| slot > s) && began.elapsed() >= at_least {
            return;
        }
        if step.quiet && step.events.is_empty() {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// Validator 0 and the validator whose window is the first slot of height 1
/// that is not validator 0's, linked; and that slot.
fn linked(cast: &Cast, engine: &Engine) -> (Host, Host, u64) {
    let slot = (0..).find(|s| engine.proposer(1, *s) != cast.ids[0].node()).expect("a slot");
    let peer = cast.ids.iter().position(|id| id.node() == engine.proposer(1, slot)).expect("one");
    let bind = |i: usize| {
        Host::bind(Config {
            identity: cast.ids[i].clone(),
            bind: "127.0.0.1:0".parse().expect("a"),
        })
        .expect("bind")
    };
    let (a, b) = (bind(0), bind(peer));
    let pair: BTreeMap<NodeId, SocketAddr> =
        [(a.node(), a.addr()), (b.node(), b.addr())].into_iter().collect();
    let b = std::thread::scope(|s| {
        let p = &pair;
        let j = s.spawn(move || {
            b.connect(p, Duration::from_secs(20));
            b
        });
        assert_eq!(a.connect(p, Duration::from_secs(20)), 1);
        j.join().expect("peer")
    });
    (a, b, slot)
}

#[test]
fn a_node_that_imported_will_not_build_and_will_not_sign() {
    let cast = cast(4);
    let scratch = Scratch::new();

    // Before the import this validator is ordinary: in its window it builds,
    // and it signs for what it built. That is what makes the refusal below
    // mean something.
    let mut ordinary =
        Engine::new(cast.ids[0].node(), cast.keys[0].clone(), &cast.committee, 1, CHAIN)
            .expect("engine")
            .with_window(WINDOW);
    let host = Host::bind(Config {
        identity: cast.ids[0].clone(),
        bind: "127.0.0.1:0".parse().expect("addr"),
    })
    .expect("bind");
    let mut votes = Journal::open(scratch.0.join("votes")).expect("journal");
    assert!(ordinary.caught_up());
    let c = chain_with_one_pending_transaction();
    let mine = (0..).find(|s| ordinary.proposer(1, *s) == cast.ids[0].node()).expect("a slot");
    let settled = WINDOW * (mine as u32 + 1) + SETTLED;
    through_its_window(&mut ordinary, &c, &host, &mut votes, settled);
    assert_eq!(c.pending(), 0, "an ordinary validator builds in its window");
    assert!(len(&scratch.0.join("votes")) > 0, "and, settled, signs for its own block");
    drop(host);

    // Now the state an import leaves: accepted blocks, no certificate under any
    // of them.
    let gate = Arc::new(Gate::memory());
    gate.hold(1, 218).expect("the gate closes");
    let mut engine =
        Engine::new(cast.ids[0].node(), cast.keys[0].clone(), &cast.committee, 1, CHAIN)
            .expect("engine")
            .with_gate(gate.clone())
            .with_window(WINDOW);
    assert!(!engine.caught_up());
    let (host, peer, slot) = linked(&cast, &engine);
    let mut votes = Journal::open(scratch.0.join("gated.votes")).expect("journal");

    // IT DOES NOT BUILD. Its window comes and goes, and the transaction
    // waiting in its pool is never taken into a block — a proposal on an
    // unagreed tip is damage already done by the time it exists.
    let c = chain_with_one_pending_transaction();
    // IT DOES NOT SIGN. Not for a peer's block either: the slot's proposer
    // offers one, signed, the height settles, and this node says nothing.
    let theirs = chain_with_one_pending_transaction();
    let block = theirs.build().expect("a block");
    let tip = c.last_accepted();
    peer.propose(&CHAIN, &cast.committee.root(), &tip, 1, slot, block.bytes())
        .expect("a signed proposal");
    let wait = WINDOW * (slot as u32 + 2) + SETTLED;
    through_its_window(&mut engine, &c, &host, &mut votes, wait);
    assert_eq!(c.pending(), 1, "the transaction was never taken into a block");

    // AND THE KEY WAS NEVER REACHED. The journals are what a validator writes
    // before it signs; that they are empty is proof no signature was made,
    // not merely that none was returned.
    assert_eq!(len(&scratch.0.join("gated.votes")), 0, "an imported node signed");
    assert!(!engine.caught_up());
}

#[test]
fn the_gate_lifts_only_on_a_certificate_and_never_on_its_own() {
    let cast = cast(4);
    let gate = Arc::new(Gate::memory());
    let mut engine =
        Engine::new(cast.ids[0].node(), cast.keys[0].clone(), &cast.committee, 1, CHAIN)
            .expect("engine")
            .with_gate(gate.clone())
            .with_window(WINDOW);
    gate.hold(1, 218).expect("the gate closes");
    let (host, peer, slot) = linked(&cast, &engine);
    let scratch = Scratch::new();
    let mut votes = Journal::open(scratch.0.join("votes")).expect("journal");

    // A peer's block, with nobody to certify it. The node executes it, waits
    // for the others, and stays gated window after window — there is no amount
    // of local work that turns an import into an agreement.
    let c = chain_with_one_pending_transaction();
    let theirs = chain_with_one_pending_transaction();
    let block = theirs.build().expect("block");
    let tip = c.last_accepted();
    peer.propose(&CHAIN, &cast.committee.root(), &tip, 1, slot, block.bytes())
        .expect("a signed proposal");
    let wait = WINDOW * (slot as u32 + 4) + SETTLED;
    through_its_window(&mut engine, &c, &host, &mut votes, wait);
    assert!(!engine.caught_up(), "the gate lifted with no certificate");
    assert_eq!(len(&scratch.0.join("votes")), 0, "the node signed while waiting to be anchored");
}

/// A one-account chain with a transaction waiting, so `build` has work.
fn chain_with_one_pending_transaction() -> Evm {
    use alloy_consensus::{SignableTransaction, TxEip1559};
    use alloy_eips::eip2718::Encodable2718 as _;
    use alloy_primitives::{keccak256, Address, Signature, U256};
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
        to: TxKind::Call(Address::repeat_byte(9)),
        value: U256::from(1u64),
        ..Default::default()
    };
    let (sig, recid) = key.sign_prehash_recoverable(tx.signature_hash().as_slice()).expect("sign");
    let signed = alloy_consensus::TxEnvelope::Eip1559(tx.into_signed(Signature::new(
        U256::from_be_slice(&sig.r().to_bytes()),
        U256::from_be_slice(&sig.s().to_bytes()),
        recid.is_y_odd(),
    )));
    c.submit(&signed.encoded_2718()).expect("submitted");
    c
}
