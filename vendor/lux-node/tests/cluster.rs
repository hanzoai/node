//! A validator cluster on real sockets.
//!
//! Not a simulation: each host binds a real listener, forms a real mesh, signs
//! with a real BLS key, and the certificate at the end is checked by the same
//! rule that checks one off the network.

use std::collections::BTreeMap;
use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use blst::min_pk::SecretKey;
use lux_consensus::cert::{NodeId, ValidatorSet, DST};
use lux_consensus::finality::{canonical_vote_message, two_thirds_count, Finality, Position};
use lux_consensus::pop;
use lux_consensus::vote::{SignedVote, VOTE, VOTE_PAYLOAD_LEN};
use lux_consensus::{zap, Tally};
use lux_node::pq::{self, Identity};
use lux_node::{Config, Host, Journal};

const LOCAL: &str = "127.0.0.1:0";
const REACH: Duration = Duration::from_secs(10);

/// A directory of signing journals that removes itself.
///
/// A validator cannot sign without a journal, so a cluster test needs one per
/// host — which is the point: the type system will not let a test stand up a
/// node that can equivocate.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("lux-node-{}-{tag}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        Scratch(dir)
    }

    fn journals(&self, n: usize) -> Vec<Journal> {
        (0..n).map(|i| Journal::open(self.0.join(format!("{i}.votes"))).expect("journal")).collect()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn position() -> Position {
    Position {
        chain_id: [0xc1; 32],
        height: 9,
        round: 1,
        block_id: [0xb1; 32],
        parent_id: [0xb2; 32],
        canonical_id: [0xca; 32],
        parent_canonical_id: [0xcb; 32],
        execution_state_root: [0xe5; 32],
        payload_root: [0x70; 32],
        validator_set_root: [0x88; 32],
    }
}

/// The epoch this cluster registers and votes at. A tally reads one epoch on
/// both sides, so there is one value here and the tests say which.
const EPOCH: u64 = 1;

/// `n` validators with real keys, each admitted through the proof path — a
/// validator that cannot prove it holds its key is not a validator, and a test
/// set is built the way a real one is.
///
/// A validator has TWO keys and they answer two questions: the ML-DSA identity
/// is its NAME, proved on every link; the BLS key is what it VOTES with.
struct Cast {
    ids: Vec<Arc<Identity>>,
    sks: Vec<SecretKey>,
    nodes: Vec<NodeId>,
    set: ValidatorSet,
}

fn committee(n: u8) -> Cast {
    // ASCENDING BY NODE ID, and that is not cosmetic. An ML-DSA identity is
    // random, and `Host` decides who dials and who accepts by comparing node
    // ids — so a test that indexes validators in generation order is a test
    // whose two halves swap roles at random. Sorting first makes `nodes[i] <
    // nodes[j]` for `i < j` hold, which is what the tests below actually mean
    // when they say "the lower id".
    let mut ids: Vec<Arc<Identity>> =
        (0..n).map(|_| Arc::new(Identity::generate().expect("identity"))).collect();
    ids.sort_by_key(|id| id.node());

    let mut cast = Cast { ids, sks: Vec::new(), nodes: Vec::new(), set: ValidatorSet::new() };
    for (i, id) in cast.ids.iter().enumerate() {
        let sk = SecretKey::key_gen(&[(i as u8).wrapping_add(1); 32], &[]).expect("key_gen");
        let node = id.node();
        let key = sk.sk_to_pk().compress();
        cast.set.insert(node, 1, &key, &pop::sign(&sk, &node, &key)).expect("insert");
        cast.sks.push(sk);
        cast.nodes.push(node);
    }
    assert!(
        cast.nodes.windows(2).all(|w| w[0] < w[1]),
        "the cast is ordered, so dial-and-accept is decided and not drawn"
    );
    cast
}

/// Bind a host per validator, then bring every pair together inside one
/// deadline.
fn cluster(ids: &[Arc<Identity>]) -> Vec<Host> {
    let hosts: Vec<Host> = ids
        .iter()
        .map(|id| {
            Host::bind(Config { identity: id.clone(), bind: LOCAL.parse().expect("addr") })
                .expect("bind")
        })
        .collect();
    let peers: BTreeMap<NodeId, SocketAddr> = hosts.iter().map(|h| (h.node(), h.addr())).collect();
    // Each host sweeps on its own thread — a mesh forms by both ends acting at
    // once — and comes back when its deadline is done.
    let joins: Vec<_> = hosts
        .into_iter()
        .map(|h| {
            let peers = peers.clone();
            std::thread::spawn(move || {
                h.connect(&peers, REACH);
                h
            })
        })
        .collect();
    joins.into_iter().map(|j| j.join().expect("sweep")).collect()
}

/// Drive every host until each has counted `want` votes or the round runs out.
fn drive(hosts: &[Host], tallies: &mut [Tally], keys: &ValidatorSet, want: usize) {
    for _ in 0..2000 {
        let mut short = false;
        for (h, t) in hosts.iter().zip(tallies.iter_mut()) {
            h.collect(t, keys);
            if t.len() < want {
                short = true;
            }
        }
        if !short {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_cluster_certifies_over_real_sockets() {
    let Cast { ids, sks, nodes, set } = committee(4);
    let hosts = cluster(&ids);
    for h in &hosts {
        assert_eq!(h.peer_count(), 3, "every validator reaches the other three");
    }

    let mut tallies: Vec<Tally> = (0..hosts.len())
        .map(|_| Tally::new(position(), Finality::Quasar, EPOCH).expect("tally"))
        .collect();

    let scratch = Scratch::new("certifies");
    let mut journals = scratch.journals(hosts.len());
    for (i, h) in hosts.iter().enumerate() {
        h.vote(&tallies[i], &sks[i], &mut journals[i]).expect("a first statement is admitted");
    }
    drive(&hosts, &mut tallies, &set, nodes.len());

    for (i, t) in tallies.iter().enumerate() {
        assert_eq!(t.len(), 4, "validator {i} counted every vote, its own included");
        let cert = t.cert(&set, &set).unwrap_or_else(|e| panic!("validator {i} certifies: {e}"));
        // And it is a certificate anyone else can check, not one this tally
        // alone believes.
        cert.verify_weighted(&set, &set, EPOCH).unwrap_or_else(|e| panic!("validator {i}: {e:?}"));
        assert_eq!(cert.tier, Finality::Quasar);
        // The quorum is the set's, read at the moment of issue. No tally stated
        // it, so there is nothing for the set to disagree with.
        assert_eq!(
            cert.threshold,
            two_thirds_count(nodes.len() as i64) as u32,
            "three of four is the Quasar rule at equal stake"
        );
    }
}

#[test]
fn a_validator_that_votes_and_leaves_still_counts() {
    let Cast { ids, sks, nodes, set } = committee(4);
    let mut hosts = cluster(&ids);
    let mut tallies: Vec<Tally> = (0..hosts.len())
        .map(|_| Tally::new(position(), Finality::Quasar, EPOCH).expect("tally"))
        .collect();

    let scratch = Scratch::new("departs");
    let mut journals = scratch.journals(hosts.len());
    for (i, h) in hosts.iter().enumerate() {
        h.vote(&tallies[i], &sks[i], &mut journals[i]).expect("a first statement is admitted");
    }
    drive(&hosts, &mut tallies, &set, nodes.len());

    // The last validator hangs up. Its vote was already delivered, so it still
    // counts: a peer is a member, not a wall.
    let gone = hosts.pop().expect("four hosts");
    let gone_node = gone.node();
    drop(gone);
    tallies.pop();

    // Broadcasting again is how the survivors notice: a write to a closed peer
    // fails, and the eviction rule acts on that. The statement is the same one,
    // so the journal admits it again — re-sending is not equivocating.
    journals.pop();
    for (i, h) in hosts.iter().enumerate() {
        h.vote(&tallies[i], &sks[i], &mut journals[i])
            .expect("re-sending one statement is admitted");
    }
    drive(&hosts, &mut tallies, &set, 4);

    for (i, h) in hosts.iter().enumerate() {
        assert!(!h.mesh().members().contains(&gone_node), "host {i} dropped the departed peer");
        assert_eq!(h.peer_count(), 2, "host {i} keeps the two that are left");
        let cert =
            tallies[i].cert(&set, &set).unwrap_or_else(|e| panic!("host {i} still certifies: {e}"));
        cert.verify_weighted(&set, &set, EPOCH).expect("the certificate verifies");
        assert_eq!(cert.votes.len(), 4, "the departed validator's vote still counts");
    }
}

#[test]
fn a_peer_that_speaks_nonsense_gains_a_socket_and_no_vote() {
    let Cast { ids, sks, nodes, set } = committee(2);
    // The HIGHER id binds and waits: the lower id dials, so this is the end
    // that accepts, which is the whole point of the exercise.
    let host = Host::bind(Config { identity: ids[1].clone(), bind: LOCAL.parse().expect("addr") })
        .expect("bind");
    let addr = host.addr();
    let peers: BTreeMap<NodeId, SocketAddr> = BTreeMap::from([(nodes[0], addr)]);

    // The impostor IS validator 0 — it holds both of that validator's keys, so
    // it completes the post-quantum handshake honestly and gets a seat. Then it
    // lies on the wire in every way the codec allows. Even a genuine signer
    // cannot get a forgery counted.
    let impostor_key = sks[0].clone();
    let impostor_id = ids[0].clone();
    let liar = std::thread::spawn(move || {
        let mut s = TcpStream::connect(addr).expect("dial");
        pq::greet(&mut s, &pq::Config::strict_pq(), &impostor_id, pq::Role::Initiator, None)
            .expect("a real validator proves itself");

        // a vote frame whose payload is garbage
        zap::write_frame(&mut &s, VOTE, b"not a vote").expect("garbage");
        // a vote-shaped payload with a wrecked signature
        let message = canonical_vote_message(&position(), true);
        let sig = impostor_key.sign(&message, DST, &[]).compress();
        let mut bad = sig.to_vec();
        bad[0] ^= 0xff;
        let mut w = zap::Writer::new();
        w.bytes(&message).bytes(&nodes[0]).bytes(&bad);
        zap::write_frame(&mut &s, VOTE, w.as_slice()).expect("forgery");
        // a frame announcing far more than this link carries
        let mut header = Vec::new();
        header.extend_from_slice(&(16u32 * 1024 * 1024).to_be_bytes());
        header.push(VOTE);
        (&s).write_all(&header).expect("flood");
        std::thread::sleep(Duration::from_millis(200));
    });

    let reached = {
        // The host is not its own peer; it accepts the impostor as validator 0.
        host.connect(&peers, Duration::from_secs(3))
    };
    assert_eq!(reached, 1, "the impostor got a socket");

    let mut tally = Tally::new(position(), Finality::Quasar, EPOCH).expect("tally");
    for _ in 0..200 {
        host.collect(&mut tally, &set);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(tally.len(), 0, "nothing it said was a vote");
    assert!(tally.cert(&set, &set).is_err());
    // The over-long announcement broke the framing, so the peer is gone — and
    // this validator is still standing.
    assert_eq!(host.peer_count(), 0, "the framing violation evicted it");
    liar.join().expect("impostor");

    // Still a working validator: its own vote counts.
    let scratch = Scratch::new("impostor");
    let mut journal = scratch.journals(1);
    host.vote(&tally, &sks[1], &mut journal[0]).expect("admitted");
    host.collect(&mut tally, &set);
    assert_eq!(tally.len(), 1);
}

#[test]
fn a_vote_frame_is_the_size_the_link_is_told_to_expect() {
    let Cast { sks, nodes, .. } = committee(1);
    let message = canonical_vote_message(&position(), true);
    let v = SignedVote {
        position: position(),
        accept: true,
        node: nodes[0],
        signature: sks[0].sign(&message, DST, &[]).compress().to_vec(),
    };
    assert_eq!(v.encode().len(), VOTE_PAYLOAD_LEN);
    // A vote's size is fixed, and the reader refuses anything larger claiming
    // to be one before the decoder is asked.
    assert_eq!(lux_node::mesh::VOTE_MAX as usize, VOTE_PAYLOAD_LEN);

    // The link itself is bounded by the biggest thing it carries, which is a
    // proposal: a block and the height and rounds in front of it. Far below
    // ZAP's own 16 MiB: headroom is memory a hostile peer can make us hold.
    assert_eq!(
        lux_node::LINK_MAX as usize,
        lux_node::mesh::BLOCK_MAX as usize + lux_node::mesh::PROPOSAL_HEAD
    );
    const { assert!(lux_node::LINK_MAX < lux_consensus::zap::MAX_MESSAGE) };
    const { assert!(lux_node::mesh::VOTE_MAX < lux_node::LINK_MAX) };
}

#[test]
fn a_validator_will_not_sign_two_blocks_at_one_slot() {
    let Cast { ids, sks, set, .. } = committee(2);
    let hosts = cluster(&ids);
    let scratch = Scratch::new("equivocation");
    let mut journals = scratch.journals(1);

    let first = Tally::new(position(), Finality::Quasar, EPOCH).expect("tally");
    // The same chain, height and round, a different block. This is the pair a
    // fork is built out of.
    let forked = Tally::new(
        Position { canonical_id: [0xff; 32], block_id: [0xfe; 32], ..position() },
        Finality::Quasar,
        EPOCH,
    )
    .expect("tally");
    assert_eq!(
        lux_consensus::vote::Slot::of(first.position()),
        lux_consensus::vote::Slot::of(forked.position()),
        "the two proposals share a slot"
    );

    hosts[0].vote(&first, &sks[0], &mut journals[0]).expect("the first statement");
    let refusal = hosts[0]
        .vote(&forked, &sks[0], &mut journals[0])
        .expect_err("the second must not be signed");
    assert!(matches!(refusal, lux_node::Refused::Equivocation(_)));

    // The key was never used on the second statement, so the second block has
    // no signature of this validator's anywhere on the network.
    let mut counted =
        Tally::new(forked.position().clone(), Finality::Quasar, EPOCH).expect("tally");
    for h in &hosts {
        h.collect(&mut counted, &set);
    }
    assert_eq!(counted.len(), 0, "nothing was said about the forked block");
}
