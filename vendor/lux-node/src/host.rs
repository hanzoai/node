//! A running validator: a listener, a mesh, and the driving of one tally.
//!
//! Identity and the peer set are fixed at construction. The listen port may be
//! ephemeral, so [`Host::bind`] reports the address it actually got and
//! [`Host::connect`] is told nothing but a deadline — "who am I" stays
//! orthogonal to "who do I dial".
//!
//! THE MESH IS NOT THE QUORUM. `connect` returns how many peers it reached
//! inside one deadline; it never demands all of them. The finality rule already
//! says how many votes are enough, and saying it a second time as "every peer
//! must connect" would only be a way to disagree with it — one absent validator
//! would take down a cluster the ⅔ rule calls healthy. The caller reads the
//! returned count against the rule it already holds.
//!
//! ONE PAIR, ONE CONNECTION. The lower node id dials, the higher accepts. The
//! rule is the identity itself, so there is no seat number to keep in step with
//! the validator set, and both ends derive the same answer without agreeing on
//! anything first.
//!
//! The handshake is a ZAP frame like everything else on the link, so one link
//! carries one framing. What it carries is a PROOF of identity: [`crate::pq`]
//! runs an ML-KEM-768 exchange and an ML-DSA-65 signature over the whole
//! transcript, and a peer's node id is derived from the key it signed with
//! rather than from anything it said. There is no unauthenticated path — a
//! link that could fall back to a claim is a link an attacker chooses the
//! weaker half of.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use blst::min_pk::SecretKey;
use lux_consensus::cert::{NodeId, VoteVerifier, DST};
use lux_consensus::vote::{read_message, SignedVote, VoteTransport};
use lux_consensus::Tally;

use crate::journal::{Journal, Refused};
use crate::mesh::{Mesh, Proposal};
use crate::pq::{self, Identity};
use crate::proposer;
use crate::vm::Id;

/// How long a peer has to complete the handshake before the sweep moves on.
///
/// Longer than the old identity swap because this one is four frames and two
/// lattice operations, and shorter than a round: a peer that cannot finish in
/// this is a peer the sweep reaches on a later pass.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(4000);

/// How long one dial attempt may take. The sweep owns the retry policy, so a
/// single attempt is short and a peer that is merely late is reached on a later
/// pass rather than holding this one.
const DIAL_TIMEOUT: Duration = Duration::from_millis(250);

/// How long the sweep rests between passes.
const SWEEP_REST: Duration = Duration::from_millis(10);

/// Who this validator is and where it listens. Not who it dials: in a real
/// cluster a peer's port may be ephemeral and is only known after that peer
/// binds, so the peer set is supplied to [`Host::connect`] instead.
#[derive(Clone)]
pub struct Config {
    /// The key this validator is named by. Shared rather than owned because
    /// every link proves the same identity, and a second copy of a signing key
    /// in one process is a second validator.
    pub identity: Arc<Identity>,
    pub bind: SocketAddr,
}

/// A bound validator.
pub struct Host {
    identity: Arc<Identity>,
    node: NodeId,
    listener: TcpListener,
    addr: SocketAddr,
    mesh: Mesh,
}

impl Host {
    /// Bind the listener. A port of 0 is resolved here and reported by
    /// [`Host::addr`]; the config keeps the request, the host reports the fact.
    pub fn bind(cfg: Config) -> io::Result<Host> {
        let listener = TcpListener::bind(cfg.bind)?;
        let addr = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        Ok(Host {
            node: cfg.identity.node(),
            identity: cfg.identity,
            listener,
            addr,
            mesh: Mesh::new(),
        })
    }

    pub fn node(&self) -> NodeId {
        self.node
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn mesh(&self) -> &Mesh {
        &self.mesh
    }

    /// Peers reached so far.
    pub fn peer_count(&self) -> usize {
        self.mesh.len()
    }

    /// Reach as many peers as possible inside ONE deadline that covers the
    /// whole phase — accepting, dialing and the handshake alike. Accepts and
    /// dials are swept together, so an absent low-id peer cannot starve the
    /// dials. Returns the number of peers connected.
    ///
    /// An entry for this validator itself is ignored rather than dialed: a node
    /// is not its own peer.
    pub fn connect(&self, peers: &BTreeMap<NodeId, SocketAddr>, deadline: Duration) -> usize {
        let end = Instant::now() + deadline;
        let mut dial: Vec<(NodeId, SocketAddr)> =
            peers.iter().filter(|(n, _)| self.node < **n).map(|(n, a)| (*n, *a)).collect();
        let mut expect: BTreeSet<NodeId> =
            peers.keys().copied().filter(|n| *n < self.node).collect();

        while Instant::now() < end && !(dial.is_empty() && expect.is_empty()) {
            self.sweep_accepts(&mut expect);
            dial.retain(|(who, addr)| !self.dial_once(*who, *addr));
            if dial.is_empty() && expect.is_empty() {
                break;
            }
            std::thread::sleep(SWEEP_REST);
        }
        self.mesh.len()
    }

    /// Meet whoever is missing, once, without blocking on anyone who is not
    /// there.
    ///
    /// [`Host::connect`] is the opening rendezvous and it ENDS. Nothing
    /// accepted an inbound link afterwards, so a validator that restarted
    /// dialed into a backlog no one was reading: it saw `peers 0 of 3` while
    /// its three peers carried on without it, each still holding a socket that
    /// had already died. Being out of the mesh is worse than it sounds — a node
    /// rejoins by taking the certificate its peers formed, and a certificate
    /// arrives over the link it does not have.
    ///
    /// So the rendezvous is run again, and keeps being run. It is the same two
    /// halves under the same rule that decides who dials, so a pair still ends
    /// up with one link and not two: the peers below this one are accepted, the
    /// peers above it are dialed, and a peer already held is left alone.
    ///
    /// Cheap enough for the block loop: accepting takes only what is already
    /// waiting, and a dial gives up after [`DIAL_TIMEOUT`].
    pub fn sweep(&self, peers: &BTreeMap<NodeId, SocketAddr>) {
        let held: BTreeSet<NodeId> = self.mesh.members().into_iter().collect();
        let mut expect: BTreeSet<NodeId> =
            peers.keys().copied().filter(|n| *n < self.node && !held.contains(n)).collect();
        self.sweep_accepts(&mut expect);
        for (who, addr) in peers.iter().filter(|(n, _)| self.node < **n && !held.contains(*n)) {
            self.dial_once(*who, *addr);
        }
    }

    /// Take every inbound connection the backlog is holding right now.
    fn sweep_accepts(&self, expect: &mut BTreeSet<NodeId>) {
        loop {
            let stream = match self.listener.accept() {
                Ok((s, _)) => s,
                Err(_) => return, // nothing ready, or the listener is unhappy
            };
            // The listener is polled, so it is non-blocking, and on BSD and
            // macOS what it accepts inherits that; on Linux it does not. Left
            // inherited, the handshake's first read returns EAGAIN and every
            // inbound peer is dropped, so a node dials out and is never dialed
            // — a half-open mesh, on one platform only.
            let _ = stream.set_nonblocking(false);
            // The accepting side does not know who is calling, so it learns
            // the peer's name from the key the peer signs with.
            let Ok(session) = self.prove(&stream, pq::Role::Responder, None) else {
                continue;
            };
            let who = session.peer;
            // A claim we were not waiting for: an unknown validator, or a
            // second connection from one we already hold.
            if !expect.remove(&who) {
                continue;
            }
            if self.mesh.add(who, stream).is_err() {
                expect.insert(who); // put the slot back; a later pass may fill it
            }
        }
    }

    /// One dial attempt. Returns whether the peer is now connected.
    fn dial_once(&self, who: NodeId, addr: SocketAddr) -> bool {
        let Ok(stream) = TcpStream::connect_timeout(&addr, DIAL_TIMEOUT) else {
            return false;
        };
        // We dialed a specific validator, and say so: the handshake refuses
        // anyone else, whatever they claim, because the name is the key.
        if self.prove(&stream, pq::Role::Initiator, Some(who)).is_err() {
            return false;
        }
        self.mesh.add(who, stream).is_ok()
    }

    /// Record this statement, sign it, and disseminate it.
    ///
    /// The bytes signed are the tally's own, so a validator cannot sign one
    /// statement and be counted for another.
    ///
    /// The journal is not optional, and it is consulted BEFORE the key is used.
    /// A validator that has already said something else at this slot does not
    /// sign — that is the equivocation a slashing rule punishes, and no
    /// argument about whether the earlier statement "really went out" changes
    /// it. Re-signing the identical statement is admitted, because a signer
    /// that crashed between recording and sending has to be able to send it.
    pub fn vote(
        &self,
        tally: &Tally,
        sk: &SecretKey,
        journal: &mut Journal,
    ) -> Result<SignedVote, Refused> {
        let message = tally.message().to_vec();
        journal.admit(&message)?;
        let v = SignedVote {
            position: tally.position().clone(),
            accept: true,
            node: self.node,
            signature: sk.sign(&message, DST, &[]).compress().to_vec(),
        };
        self.mesh.broadcast(&v);
        Ok(v)
    }

    /// Say again the statement this validator already stands by.
    ///
    /// A validator that has spoken at a slot may not speak differently there,
    /// but it is still the author of what it said — and a peer that never
    /// received it is waiting on a vote that exists. Re-sending is how the
    /// vote reaches them; it is not a second statement, and the journal is
    /// what proves that, because admitting the identical bytes is exactly the
    /// case it allows.
    ///
    /// THE JOURNAL IS STILL THE ONLY WAY TO THE KEY. This goes through
    /// `admit` like any other statement rather than around it, so there is one
    /// path to signing and not two.
    ///
    /// The position is read back out of the signed bytes by the same function
    /// a receiver uses, so a re-sent vote cannot claim a position its message
    /// does not say. The bytes go back out exactly as they were signed.
    pub fn resend(
        &self,
        standing: &[u8],
        sk: &SecretKey,
        journal: &mut Journal,
    ) -> Result<SignedVote, Refused> {
        journal.admit(standing)?;
        let (position, accept) = read_message(standing).map_err(|_| Refused::NotAStatement)?;
        let v = SignedVote {
            position,
            accept,
            node: self.node,
            signature: sk.sign(standing, DST, &[]).compress().to_vec(),
        };
        self.mesh.broadcast(&v);
        Ok(v)
    }

    /// Sign `block` as this validator's proposal at `height` and `slot` on
    /// `parent`, for the validator set committed to by `set`, and push it to
    /// every peer.
    ///
    /// Signed with the ML-DSA-65 identity this validator proves on every link,
    /// in the shape Go's proposervm signs a block ([`proposer::header`]), so a
    /// peer can relay it and the proposal still says who made it.
    pub fn propose(
        &self,
        chain: &Id,
        set: &Id,
        parent: &Id,
        height: u64,
        slot: u64,
        block: Vec<u8>,
    ) -> Result<Proposal, pq::Error> {
        let header =
            proposer::header(chain, set, parent, height, slot, self.identity.public(), &block);
        let signature = self.identity.sign(&header, proposer::CONTEXT)?;
        let p = Proposal { from: self.node, height, slot, signature, block };
        self.mesh.propose(&p);
        Ok(p)
    }

    /// Feed everything that has arrived into the tally. Returns how many votes
    /// were newly counted; refusals are dropped, because a refused vote is a
    /// vote that did not happen.
    pub fn collect(&self, tally: &mut Tally, keys: &dyn VoteVerifier) -> usize {
        self.mesh.drain().iter().filter(|(_, v)| tally.add(v, keys).unwrap_or(false)).count()
    }
}

impl Host {
    /// Run the post-quantum handshake over `stream`.
    ///
    /// Bounded at both ends: a peer that goes quiet mid-handshake holds this
    /// sweep for the timeout and no longer. The read timeout is cleared
    /// afterwards because the mesh's readers block on whole frames by design.
    fn prove(
        &self,
        stream: &TcpStream,
        role: pq::Role,
        expect: Option<NodeId>,
    ) -> Result<pq::Session, pq::Error> {
        stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
        stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;
        let session =
            pq::greet(&mut &*stream, &pq::Config::strict_pq(), &self.identity, role, expect)?;
        stream.set_read_timeout(None)?;
        Ok(session)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two bound validators, and which of them is the lower id — the rule that
    /// decides who dials.
    fn pair() -> (Host, Host) {
        let a = Host::bind(Config {
            identity: Arc::new(Identity::generate().expect("identity")),
            bind: "127.0.0.1:0".parse().expect("addr"),
        })
        .expect("bind");
        let b = Host::bind(Config {
            identity: Arc::new(Identity::generate().expect("identity")),
            bind: "127.0.0.1:0".parse().expect("addr"),
        })
        .expect("bind");
        if a.node() < b.node() {
            (a, b)
        } else {
            (b, a)
        }
    }

    #[test]
    fn a_host_is_named_by_the_key_it_holds() {
        let id = Arc::new(Identity::generate().expect("identity"));
        let want = id.node();
        let h = Host::bind(Config { identity: id, bind: "127.0.0.1:0".parse().expect("addr") })
            .expect("bind");
        assert_eq!(h.node(), want, "a validator cannot be named anything else");
    }

    #[test]
    fn the_lower_id_dials_and_the_higher_accepts() {
        let (low, high) = pair();
        let peers: BTreeMap<NodeId, SocketAddr> =
            BTreeMap::from([(low.node(), low.addr()), (high.node(), high.addr())]);
        let p2 = peers.clone();
        let j = std::thread::spawn(move || {
            high.connect(&p2, Duration::from_secs(10));
            high
        });
        assert_eq!(low.connect(&peers, Duration::from_secs(10)), 1);
        let high = j.join().expect("sweep");
        assert_eq!(high.peer_count(), 1);
        assert_eq!(low.mesh().members(), vec![high.node()]);
        assert_eq!(high.mesh().members(), vec![low.node()]);
    }

    #[test]
    fn a_node_is_not_its_own_peer() {
        let (low, _high) = pair();
        let only_me: BTreeMap<NodeId, SocketAddr> = BTreeMap::from([(low.node(), low.addr())]);
        assert_eq!(low.connect(&only_me, Duration::from_millis(300)), 0);
    }

    #[test]
    fn dialing_a_validator_that_is_somewhere_else_is_not_a_connection() {
        // The far end is a real validator, and it is not the one we dialed.
        // The handshake proves who answered, so the name settles it.
        let (a, b) = pair();
        let stranger = Identity::generate().expect("identity").node();
        let addr = b.addr();
        std::thread::spawn(move || {
            b.connect(&BTreeMap::from([(stranger, addr)]), Duration::from_millis(600));
        });
        assert!(!a.dial_once(stranger, addr), "someone else answering is not that validator");
        assert_eq!(a.peer_count(), 0);
    }

    #[test]
    fn a_silent_peer_does_not_hold_the_sweep_forever() {
        // A listener that takes the connection and says nothing. The handshake
        // is bounded, so the sweep moves on.
        let l = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = l.local_addr().expect("addr");
        std::thread::spawn(move || {
            if let Ok((s, _)) = l.accept() {
                std::thread::sleep(HANDSHAKE_TIMEOUT * 2);
                drop(s);
            }
        });
        let (a, _b) = pair();
        let began = Instant::now();
        assert!(!a.dial_once(Identity::generate().expect("id").node(), addr));
        assert!(began.elapsed() < HANDSHAKE_TIMEOUT * 2, "the handshake is bounded");
    }

    #[test]
    fn a_peer_that_cannot_prove_itself_gains_nothing() {
        // Well-formed TCP, no handshake. The mesh stays empty.
        let (a, _b) = pair();
        let addr = a.addr();
        let noise = std::thread::spawn(move || {
            let mut s = TcpStream::connect(addr).expect("dial");
            use std::io::Write;
            let _ = s.write_all(b"not a greeting at all");
            std::thread::sleep(Duration::from_millis(200));
        });
        let stranger = Identity::generate().expect("identity").node();
        a.connect(
            &BTreeMap::from([(stranger, "127.0.0.1:1".parse().expect("addr"))]),
            Duration::from_millis(400),
        );
        assert_eq!(a.peer_count(), 0, "an unproven peer is not a peer");
        noise.join().expect("noise");
    }
}
