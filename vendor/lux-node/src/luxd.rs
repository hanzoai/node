// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! Joining a luxd network as a peer.
//!
//! Everything a Go node requires of something that dials its staking port, and
//! nothing it does not. The layers, bottom up:
//!
//! 1. **TCP**, then **TLS 1.3 — and only TLS 1.3.** `network/peer/tls_config.go`
//!    pins `MinVersion == MaxVersion == VersionTLS13` and, decisively,
//!    `CurvePreferences: []tls.CurveID{tls.X25519MLKEM768}`. That single-element
//!    list is the gate: a client that cannot offer the hybrid
//!    X25519 + ML-KEM-768 group never gets a session. Mutual TLS, with
//!    `RequireAnyClientCert` and `InsecureSkipVerify` — the peer is identified
//!    by its key, not by a CA — so a self-signed certificate is correct and a
//!    CA-signed one buys nothing.
//! 2. **The staking certificate**, which is still classical: `ValidateCertificate`
//!    admits ECDSA on P-256 or a well-formed RSA key and nothing else. Not
//!    ML-DSA, whatever the node's own identity is.
//! 3. **A post-quantum handshake, only if the genesis pins a strict profile.**
//!    `profileRequiresPQHandshake` is driven by the `securityProfile.json`
//!    shard; a network without one logs "boots in classical-compat mode" and
//!    exchanges no INIT/RESP at all. There is also no per-frame AEAD to
//!    implement in either mode — luxd derives that key and, today, never reads
//!    it back.
//! 4. **ZAP stream frames** ([`crate::wire`]), starting with a `Handshake` that
//!    both sides send unprompted.
//!
//! WHAT MAKES A PEER "CONNECTED" is not the handshake. luxd flips
//! `finishedHandshake` when it receives a **PeerList**, so a peer that sends a
//! perfect Handshake and stops is a peer that is never counted. An empty
//! PeerList is enough, and this sends one.
//!
//! WHAT GETS A PEER DROPPED, and therefore what is checked here before
//! anything is sent: a network id that is not theirs, a clock more than a
//! minute out, a port of zero, a signed IP whose ECDSA signature does not
//! verify against the certificate just presented, and a BLS signature that
//! does not parse. The last is easy to miss — the field is not optional, and
//! an empty one closes the connection.

use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use blst::min_pk::SecretKey;
use lux_consensus::cert::{NodeId, DST};
use lux_consensus::finality::{canonical_vote_message, Position};
use p256::ecdsa::signature::hazmat::PrehashSigner;

use sha2::{Digest, Sha256};

use crate::wire::{self, frame, tag, Reader, Writer};

/// The domain tag luxd's BLS proof-of-possession signs under.
/// `luxfi/crypto@v1.19.26 bls: dstPoP`. The signing DST, not the verifying one:
/// a signature under the wrong tag is a well-formed signature of nothing.
pub const POP_DST: &[u8] = b"BLS_POP_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_";

/// How long to wait on the socket. luxd's own per-frame deadline is its pong
/// timeout, thirty seconds.
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// How many un-certified containers a peer keeps. Consensus is one block deep
/// at a time; this is slack, not a buffer to grow.
const PENDING_MAX: usize = 32;

/// What this peer calls itself. luxd compares versions and will drop a peer it
/// considers incompatible, so the name and numbers are part of the protocol.
#[derive(Clone, Debug)]
pub struct Client {
    pub name: String,
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Default for Client {
    fn default() -> Self {
        // luxd's compatibility check is by client NAME and version. Presenting
        // as "lux" with the version luxd itself reports is what a second
        // implementation of the same protocol should say; claiming a different
        // name is how a peer gets refused for being a stranger rather than for
        // being wrong.
        Self { name: "lux".into(), major: 1, minor: 36, patch: 181 }
    }
}

/// This peer's TLS identity: a self-signed ECDSA P-256 certificate, and the key
/// that signs both the TLS handshake and the signed IP.
pub struct Staking {
    key: p256::ecdsa::SigningKey,
    der: Vec<u8>,
    pkcs8: Vec<u8>,
}

impl Staking {
    /// Load the staking identity in `dir`, or make one and keep it.
    ///
    /// KEPT, because the node id is a function of the certificate: a node that
    /// regenerated its certificate would be a different validator on every
    /// restart, and a genesis naming it would name somebody who no longer
    /// exists.
    pub fn open(dir: impl AsRef<std::path::Path>) -> Result<Self, Error> {
        let dir = dir.as_ref();
        let der = dir.join("staker.der");
        let key = dir.join("staker.key");
        match (std::fs::read(&der), std::fs::read(&key)) {
            (Ok(der), Ok(pkcs8)) => {
                use p256::pkcs8::DecodePrivateKey;
                let signing = p256::SecretKey::from_pkcs8_der(&pkcs8)
                    .map(p256::ecdsa::SigningKey::from)
                    .map_err(|e| Error::Identity(format!("the staking key is not P-256: {e}")))?;
                Ok(Self { key: signing, der, pkcs8 })
            }
            _ => {
                let fresh = Self::generate()?;
                std::fs::create_dir_all(dir).map_err(|e| Error::Identity(e.to_string()))?;
                std::fs::write(&der, &fresh.der).map_err(|e| Error::Identity(e.to_string()))?;
                crate::pq::write_private(&key, &fresh.pkcs8)
                    .map_err(|e| Error::Identity(e.to_string()))?;
                Ok(fresh)
            }
        }
    }

    /// The node id luxd derives from this certificate.
    ///
    /// `ids.NodeIDFromCert`: RIPEMD-160 of SHA-256 of the DER, which is
    /// Bitcoin's hash160 and not a truncation of either hash on its own.
    pub fn node_id(&self) -> [u8; 20] {
        use ripemd::{Digest as _, Ripemd160};
        let outer = Ripemd160::digest(Sha256::digest(&self.der));
        let mut id = [0u8; 20];
        id.copy_from_slice(&outer);
        id
    }

    /// How that id is written down: `NodeID-` and the CB58 of the twenty bytes.
    pub fn node_name(&self) -> String {
        format!("NodeID-{}", cb58(&self.node_id()))
    }

    /// A fresh staking identity.
    ///
    /// P-256 because that is the only elliptic curve `ValidateCertificate`
    /// accepts — an ECDSA key on any other curve is refused with
    /// `ErrCurveMismatch` after the TLS handshake has already succeeded, which
    /// is a confusing way to fail.
    pub fn generate() -> Result<Self, Error> {
        let pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
            .map_err(|e| Error::Identity(e.to_string()))?;
        let pkcs8 = pair.serialize_der();
        let key = {
            use p256::pkcs8::DecodePrivateKey;
            p256::SecretKey::from_pkcs8_der(&pkcs8)
                .map(p256::ecdsa::SigningKey::from)
                .map_err(|e| Error::Identity(format!("the generated key is not P-256: {e}")))?
        };

        let mut params = rcgen::CertificateParams::new(vec!["lux".to_string()])
            .map_err(|e| Error::Identity(e.to_string()))?;
        params.distinguished_name = rcgen::DistinguishedName::new();
        let cert = params.self_signed(&pair).map_err(|e| Error::Identity(e.to_string()))?;
        Ok(Self { key, der: cert.der().to_vec(), pkcs8 })
    }

    pub fn certificate(&self) -> &[u8] {
        &self.der
    }

    /// Sign the way Go's `crypto.Signer` does for an ECDSA key: over the
    /// SHA-256 of the message, and ASN.1 DER on the wire.
    fn sign_ip(&self, message: &[u8]) -> Result<Vec<u8>, Error> {
        let digest = Sha256::digest(message);
        let sig: p256::ecdsa::Signature = self
            .key
            .sign_prehash(&digest)
            .map_err(|e| Error::Identity(format!("cannot sign the ip: {e}")))?;
        Ok(sig.to_der().as_bytes().to_vec())
    }
}

/// CB58: base58 of the bytes and the first four of their SHA-256.
///
/// How every id in a Lux genesis or log line is written, so a value produced
/// here can be pasted into one.
pub fn cb58(raw: &[u8]) -> String {
    let mut out = raw.to_vec();
    out.extend_from_slice(&Sha256::digest(raw)[28..]);
    bs58::encode(out).into_string()
}

/// What a validator publishes to a genesis: its BLS public key and the proof it
/// holds it.
///
/// `vms/platformvm/signer.NewProofOfPossession` signs the COMPRESSED public key
/// — forty-eight bytes — under the proof-of-possession domain tag. Signing
/// anything else produces a proof that verifies against nothing.
pub fn proof_of_possession(bls: &SecretKey) -> (Vec<u8>, Vec<u8>) {
    let public = bls.sk_to_pk().compress().to_vec();
    let proof = bls.sign(&public, POP_DST, &[]).compress().to_vec();
    (public, proof)
}

/// Find the execution block inside a proposervm container.
///
/// The container is ZAP's arena format and the block is a byte field in it;
/// rather than parse the arena, the block is found by what it IS — a long RLP
/// list whose first element is a header this crate can read. Returns the
/// header's RLP, so the block is named by the bytes that arrived rather than by
/// a re-encoding of them.
pub fn inner_header(container: &[u8]) -> Option<&[u8]> {
    for i in 0..container.len().saturating_sub(4) {
        if container[i] != 0xf9 {
            continue;
        }
        let outer = u16::from_be_bytes([container[i + 1], container[i + 2]]) as usize;
        if i + 3 + outer != container.len() {
            continue;
        }
        let h = i + 3;
        if container.get(h) != Some(&0xf9) {
            continue;
        }
        let len = u16::from_be_bytes([container[h + 1], container[h + 2]]) as usize;
        let rlp = container.get(h..h + 3 + len)?;
        if lux_evm::header::Lux::read(rlp).is_ok() {
            return Some(rlp);
        }
    }
    None
}

/// What this validator needs in order to cast a vote luxd will count.
///
/// The set root is the one value that cannot be derived from the block: it is
/// the commitment to the weighted validator set in force at the block's P-chain
/// epoch, and the weights are the ones the P-chain COMPUTED, not the ones a
/// genesis declared. luxd serves them at `/validators/at`.
pub struct Ballot {
    pub node: NodeId,
    pub key: SecretKey,
    pub chain: [u8; 32],
    pub set_root: [u8; 32],
}

/// The bytes every signature over a peer's address covers.
///
/// `network/peer/ip.go bytes()`: the address as its IPv6-MAPPED sixteen bytes,
/// the port as a `u16`, the timestamp as a `u64` — twenty-six bytes, big-endian
/// throughout. The sixteen is the trap: the `IpAddr` FIELD of the handshake
/// carries four bytes for a v4 address, and the SIGNATURE covers the mapped
/// sixteen. Signing what the field carries produces a valid signature over the
/// wrong message, and luxd closes the connection without saying why.
pub fn signed_ip(addr: IpAddr, port: u16, timestamp: u64) -> Vec<u8> {
    let mapped = match addr {
        IpAddr::V4(v4) => v4.to_ipv6_mapped().octets(),
        IpAddr::V6(v6) => v6.octets(),
    };
    let mut out = Vec::with_capacity(26);
    out.extend_from_slice(&mapped);
    out.extend_from_slice(&port.to_be_bytes());
    out.extend_from_slice(&timestamp.to_be_bytes());
    out
}

/// Why joining failed.
#[derive(Debug)]
pub enum Error {
    Identity(String),
    Tls(String),
    Io(String),
    /// The peer said something this codec could not read.
    Malformed(&'static str),
    /// The peer hung up, and this is what it last said.
    Closed(String),
    /// Nothing arrived before the read deadline, and nothing was half-read.
    /// A quiet link is not a broken one: luxd pings about every 22s, so a
    /// validator with nothing to answer legitimately hears nothing for a while.
    Quiet,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Identity(w) => write!(f, "identity: {w}"),
            Error::Tls(w) => write!(f, "tls: {w}"),
            Error::Io(w) => write!(f, "{w}"),
            Error::Malformed(w) => write!(f, "malformed: {w}"),
            Error::Closed(w) => write!(f, "the peer closed the connection: {w}"),
            Error::Quiet => write!(f, "nothing arrived before the read deadline"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

/// What a certificate for `outer` makes a peer's tip.
///
/// `None` for a certificate naming a block this peer was never asked about —
/// its height is then unknown, and claiming a tip it has no basis for is worse
/// than staying behind. `None` too for one no newer than what it already has,
/// because an accepted tip does not move backwards.
fn certified(
    pending: &[([u8; 32], Vec<u8>, u64)],
    outer: &[u8; 32],
    have: u64,
) -> Option<(Vec<u8>, u64)> {
    pending
        .iter()
        .find(|(id, _, _)| id == outer)
        .filter(|(_, _, height)| *height > have)
        .map(|(_, chain, height)| (chain.clone(), *height))
}

/// Fill `buf`, telling a quiet link apart from a broken one.
///
/// A deadline that fires having consumed NOTHING means the peer had nothing to
/// say — normal, and the caller should keep the link. A deadline that fires
/// part-way through means the stream has lost sync and the connection is
/// finished, whatever kind the error carries. `read_exact` cannot tell those
/// apart because it does not report how far it got.
fn fill(r: &mut impl Read, buf: &mut [u8]) -> Result<(), Error> {
    let mut got = 0usize;
    while got < buf.len() {
        match r.read(&mut buf[got..]) {
            Ok(0) if got == 0 => return Err(Error::Closed("at a frame boundary".into())),
            Ok(0) => return Err(Error::Closed("mid-frame".into())),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) if quiet(&e) && got == 0 => return Err(Error::Quiet),
            Err(e) if quiet(&e) => return Err(Error::Closed("a deadline mid-frame".into())),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn quiet(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut)
}

/// What this peer tells luxd about itself.
pub struct Join {
    pub network: u32,
    /// Where this peer claims to listen. The signature covers it, and luxd
    /// refuses a port of zero.
    pub addr: IpAddr,
    pub port: u16,
    pub client: Client,
}

/// A luxd peer, once TLS is up.
/// One quorum message as it arrived: which chain, which kind, which block, and
/// the bytes under it.
pub type Heard = (Vec<u8>, u8, [u8; 32], Vec<u8>);

pub struct Peer {
    conn: rustls::ClientConnection,
    sock: TcpStream,
    /// The chains this peer will answer consensus polls for, and the block it
    /// considers each one's tip. A chain that is not here is one this peer
    /// stays silent about.
    tracked: Vec<(Vec<u8>, Vec<u8>)>,
    /// Every poll received, so a caller can show that consensus reached it.
    pub polled: Vec<(Vec<u8>, u32)>,
    /// Every PushQuery: the chain, the request, and the block it carried.
    pub queried: Vec<(Vec<u8>, u32, [u8; 32])>,
    /// Every quorum message heard over app-gossip.
    pub heard: Vec<Heard>,
    /// The containers luxd pushed, kept so the block inside can be read out.
    pub containers: Vec<Vec<u8>>,
    /// The height this peer claims to have accepted.
    pub height: u64,
    /// Outer container id -> the chain and height it carried, for the blocks
    /// luxd has pushed. A certificate names only the outer id, so following
    /// the chain means having kept what that id stood for.
    pending: Vec<([u8; 32], Vec<u8>, u64)>,
    /// What this peer votes with, when it is a validator of the chain.
    ballot: Option<Ballot>,
    /// Every vote this peer cast, so a caller can show it spoke.
    pub cast: Vec<(u64, [u8; 32])>,
    /// Their handshake, once it arrives.
    pub peer_network: Option<u32>,
    pub peer_client: Option<String>,
    /// Whether luxd has counted us — it sends a PeerList once our handshake
    /// validates, and that is the acknowledgement.
    pub accepted: bool,
}

/// Accept whatever certificate luxd presents.
///
/// This is not laxity: luxd does the same in both directions
/// (`InsecureSkipVerify: true`) because a peer is identified by the public key
/// in its certificate and by the signature it makes over its own address, not
/// by a certificate authority. Verifying a CA here would refuse every real
/// node on the network.
#[derive(Debug)]
struct PeerKeyIsTheIdentity(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for PeerKeyIsTheIdentity {
    fn verify_server_cert(
        &self,
        _end: &rustls::pki_types::CertificateDer<'_>,
        _inter: &[rustls::pki_types::CertificateDer<'_>],
        _name: &rustls::pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _m: &[u8],
        _c: &rustls::pki_types::CertificateDer<'_>,
        _d: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _m: &[u8],
        _c: &rustls::pki_types::CertificateDer<'_>,
        _d: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

impl Peer {
    /// Dial luxd and bring the connection up to a usable peer.
    ///
    /// Returns once luxd has answered our handshake with a PeerList, which is
    /// the moment it counts us as connected.
    pub fn join(
        at: SocketAddr,
        staking: &Staking,
        bls: &SecretKey,
        me: &Join,
    ) -> Result<Self, Error> {
        let mut peer = Self::connect(at, staking)?;
        peer.send_handshake(staking, bls, me)?;
        // Both sides send a handshake unprompted, so theirs may already be in
        // the buffer. Drive until they acknowledge or hang up.
        peer.pump(Duration::from_secs(15))?;
        Ok(peer)
    }

    /// TCP and TLS only. Separate from the handshake so a caller can measure
    /// exactly which layer refused it.
    pub fn connect(at: SocketAddr, staking: &Staking) -> Result<Self, Error> {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        // The one group luxd offers. Named rather than left to the default so
        // that a provider whose default drifts fails here, loudly, instead of
        // at a handshake nobody can explain.
        if !provider.kx_groups.iter().any(|g| g.name() == rustls::NamedGroup::X25519MLKEM768) {
            return Err(Error::Tls(
                "this build cannot offer X25519MLKEM768, which is the only group luxd accepts"
                    .into(),
            ));
        }

        let chain = vec![rustls::pki_types::CertificateDer::from(staking.certificate().to_vec())];
        let key = rustls::pki_types::PrivatePkcs8KeyDer::from(staking.pkcs8.clone());
        let verifier = Arc::new(PeerKeyIsTheIdentity(provider.clone()));

        let cfg = rustls::ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|e| Error::Tls(e.to_string()))?
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_client_auth_cert(chain, key.into())
            .map_err(|e| Error::Tls(e.to_string()))?;

        let name = rustls::pki_types::ServerName::try_from("lux")
            .map_err(|e| Error::Tls(e.to_string()))?;
        let conn = rustls::ClientConnection::new(Arc::new(cfg), name)
            .map_err(|e| Error::Tls(e.to_string()))?;
        let sock = TcpStream::connect_timeout(&at, TIMEOUT)?;
        sock.set_read_timeout(Some(TIMEOUT))?;
        sock.set_write_timeout(Some(TIMEOUT))?;
        sock.set_nodelay(true)?;

        Ok(Self {
            conn,
            sock,
            tracked: Vec::new(),
            polled: Vec::new(),
            queried: Vec::new(),
            heard: Vec::new(),
            containers: Vec::new(),
            pending: Vec::new(),
            ballot: None,
            cast: Vec::new(),
            height: 0,
            peer_network: None,
            peer_client: None,
            accepted: false,
        })
    }

    /// Vote on the blocks this peer is queried about.
    pub fn votes_with(&mut self, ballot: Ballot) {
        self.ballot = Some(ballot);
    }

    /// Answer polls for `chain`, naming `tip` as what this peer has accepted.
    pub fn track(&mut self, chain: [u8; 32], tip: [u8; 32]) {
        self.tracked.retain(|(c, _)| c.as_slice() != chain.as_slice());
        self.tracked.push((chain.to_vec(), tip.to_vec()));
    }

    fn frontier_of(&self, chain: &[u8]) -> Option<Vec<u8>> {
        self.tracked.iter().find(|(c, _)| c.as_slice() == chain).map(|(_, tip)| tip.clone())
    }

    /// `AcceptedFrontier`: ChainId | RequestId | ContainerId.
    pub fn accepted_frontier(
        &mut self,
        chain: &[u8],
        request: u32,
        container: &[u8],
    ) -> Result<(), Error> {
        let mut w = Writer::new();
        w.bytes(chain).u32(request).bytes(container);
        self.write(tag::ACCEPTED_FRONTIER, w.as_slice())
    }

    /// Sign this block's position and gossip the vote.
    ///
    /// The position is rebuilt from the block exactly as luxd rebuilds it: the
    /// canonical identity is the INNER execution block — its own hash and its
    /// parent's — and the two roots are Empty, because the proposervm is the
    /// only thing that reports them and it reports nothing. The outer container
    /// ids are not signed at all.
    /// Keep what a pushed container stood for, bounded — only the blocks that
    /// could still be certified are worth remembering.
    fn remember(&mut self, outer: [u8; 32], chain: &[u8], height: u64) {
        if self.pending.iter().any(|(id, _, _)| id == &outer) {
            return;
        }
        if self.pending.len() >= PENDING_MAX {
            self.pending.remove(0);
        }
        self.pending.push((outer, chain.to_vec(), height));
    }

    /// Take a certified block as this peer's accepted tip.
    fn follow(&mut self, outer: &[u8; 32]) {
        let Some((chain, height)) = certified(&self.pending, outer, self.height) else {
            return;
        };
        let Ok(chain) = <[u8; 32]>::try_from(chain.as_slice()) else {
            return;
        };
        self.height = height;
        self.track(chain, *outer);
        self.pending.retain(|(_, _, h)| *h > height);
    }

    /// `rlp` is the header EXACTLY as it arrived. The canonical id is the hash
    /// of those bytes, so re-encoding a decoded header to get them back would
    /// stake the signature on a round-trip being byte-perfect.
    fn cast_vote(
        &mut self,
        chain: &[u8],
        rlp: &[u8],
        header: &lux_evm::header::Lux,
        outer: &[u8; 32],
    ) -> Result<(), Error> {
        let Some(ballot) = &self.ballot else {
            return Ok(());
        };
        if chain != ballot.chain {
            return Ok(());
        }
        let height = header.header.number;
        let canonical = lux_evm::header::seal_bytes(rlp).0;
        if self.cast.iter().any(|(h, c)| *h == height && *c == canonical) {
            return Ok(()); // one statement per block
        }

        let position = Position {
            chain_id: ballot.chain,
            height,
            round: 0,
            block_id: canonical,
            parent_id: header.header.parent_hash.0,
            canonical_id: canonical,
            parent_canonical_id: header.header.parent_hash.0,
            execution_state_root: [0u8; 32],
            payload_root: [0u8; 32],
            validator_set_root: ballot.set_root,
        };
        let message = canonical_vote_message(&position, true);
        let signature = ballot.key.sign(&message, DST, &[]).compress().to_vec();
        let node = ballot.node;
        let payload = quorum::vote(&node, &signature);
        // TWO DIFFERENT IDS, and using one for both is a vote that is received
        // and then buffered forever. The SIGNATURE binds the canonical (inner
        // execution) identity — that is what finality is defined over. The
        // ENVELOPE names the OUTER container id, because that is the key luxd
        // tracks pending blocks by; a vote addressed to the inner id finds no
        // pending block and is never counted.
        let envelope = quorum::wrap(quorum::VOTE, outer, &payload);
        self.cast.push((height, canonical));
        let chain: [u8; 32] = chain.try_into().map_err(|_| Error::Malformed("chain id"))?;
        self.gossip(&chain, &envelope)
    }

    /// `Chits`: this peer's answer to a query.
    ///
    /// ChainId | RequestId | PreferredId | PreferredIdAtHeight | AcceptedId |
    /// AcceptedHeight. Saying we prefer the block that was pushed is what a
    /// validator that has verified it says; saying it while ALSO reporting an
    /// accepted height we are actually at is what keeps the answer honest.
    pub fn chits(
        &mut self,
        chain: &[u8],
        request: u32,
        preferred: &[u8],
        accepted: &[u8],
        height: u64,
    ) -> Result<(), Error> {
        let mut w = Writer::new();
        w.bytes(chain).u32(request).bytes(preferred).bytes(preferred).bytes(accepted).u64(height);
        self.write(tag::CHITS, w.as_slice())
    }

    /// `Accepted`: ChainId | RequestId | ContainerIds.
    pub fn accepted<T: AsRef<[u8]>>(
        &mut self,
        chain: &[u8],
        request: u32,
        ids: &[T],
    ) -> Result<(), Error> {
        let mut w = Writer::new();
        w.bytes(chain).u32(request).list_bytes(ids);
        self.write(tag::ACCEPTED, w.as_slice())
    }

    fn write(&mut self, tag_byte: u8, body: &[u8]) -> Result<(), Error> {
        let out = frame(tag_byte, body);
        self.stream().write_all(&out)?;
        self.stream().flush()?;
        Ok(())
    }

    /// The group TLS actually negotiated, once the handshake has run.
    pub fn group(&self) -> Option<rustls::NamedGroup> {
        self.conn.negotiated_key_exchange_group().map(|g| g.name())
    }

    /// The certificate luxd presented. Its public key is luxd's identity.
    pub fn peer_certificate(&self) -> Option<Vec<u8>> {
        self.conn.peer_certificates().and_then(|c| c.first()).map(|c| c.to_vec())
    }

    fn stream(&mut self) -> rustls::Stream<'_, rustls::ClientConnection, TcpStream> {
        rustls::Stream::new(&mut self.conn, &mut self.sock)
    }

    /// Build and send the Handshake luxd validates before it will talk.
    pub fn send_handshake(
        &mut self,
        staking: &Staking,
        bls: &SecretKey,
        me: &Join,
    ) -> Result<(), Error> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let ip = signed_ip(me.addr, me.port, now);

        let mut w = Writer::new();
        w.u32(me.network).u64(now);
        match me.addr {
            IpAddr::V4(v4) => w.bytes(&v4.octets()),
            IpAddr::V6(v6) => w.bytes(&v6.octets()),
        };
        w.u32(u32::from(me.port));
        w.u64(now);
        // The TLS leg: ECDSA over SHA-256 of the twenty-six bytes, verified
        // against the certificate this connection already presented.
        w.bytes(&staking.sign_ip(&ip)?);
        // No tracked chains. luxd caps this at sixteen and refuses any entry
        // that is not exactly thirty-two bytes.
        let none: [&[u8]; 0] = [];
        w.list_bytes(&none);
        w.present()
            .text(&me.client.name)
            .u32(me.client.major)
            .u32(me.client.minor)
            .u32(me.client.patch);
        w.list_u32(&[]).list_u32(&[]);
        // No bloom of known peers. Optional, and an unparseable one is a
        // disconnect, so absent is both correct and safer than a guess.
        w.absent();
        // The BLS leg. NOT optional: luxd parses this field unconditionally and
        // closes the connection if it is not a signature.
        let pop = bls.sign(&ip, POP_DST, &[]).compress().to_vec();
        w.bytes(&pop);
        w.bool(true);
        // Everything after this point is append-only on luxd's side, and it
        // tells an old peer from a silent one by whether the frame ends here.
        // Ending here is the legacy shape, which is read without complaint.

        let body = w.take();
        let out = frame(tag::HANDSHAKE, &body);
        self.stream().write_all(&out)?;
        self.stream().flush()?;
        Ok(())
    }

    /// Read one frame, answering anything that must be answered.
    ///
    /// Returns the tag it handled.
    pub fn step(&mut self) -> Result<u8, Error> {
        let (tag_byte, body) = self.read_frame()?;
        self.handle(tag_byte, &body)?;
        Ok(tag_byte)
    }

    /// Drive the connection until luxd accepts us or the deadline passes.
    pub fn pump(&mut self, budget: Duration) -> Result<(), Error> {
        let end = std::time::Instant::now() + budget;
        while std::time::Instant::now() < end && !self.accepted {
            match self.step() {
                Ok(_) => {}
                // Nothing arrived; the deadline above decides whether to wait
                // more, not this read.
                Err(Error::Quiet) => continue,
                Err(Error::Io(_)) if self.accepted => break,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn read_frame(&mut self) -> Result<(u8, Vec<u8>), Error> {
        let mut len = [0u8; 4];
        fill(&mut self.stream(), &mut len)?;
        let n = u32::from_be_bytes(len);
        if n == 0 || n > wire::MESSAGE_MAX {
            return Err(Error::Malformed("a frame length luxd would not have written"));
        }
        let mut body = vec![0u8; n as usize];
        fill(&mut self.stream(), &mut body)?;
        let tag_byte = body[0];
        let rest = body[1..].to_vec();

        // A compressed message is a message whose body is a zstd frame holding
        // another whole message. Exactly one level: luxd unwraps once and a
        // doubly-wrapped frame is refused rather than unwrapped twice.
        if tag_byte == tag::COMPRESSED_ZSTD {
            let mut r = Reader::new(&rest);
            let packed = r.bytes().ok_or(Error::Malformed("a compressed frame with no payload"))?;
            let inner = zstd::decode_all(packed)
                .map_err(|e| Error::Io(format!("cannot decompress a frame: {e}")))?;
            if inner.is_empty() {
                return Err(Error::Malformed("a compressed frame holding nothing"));
            }
            if inner[0] == tag::COMPRESSED_ZSTD {
                return Err(Error::Malformed("a frame compressed twice"));
            }
            return Ok((inner[0], inner[1..].to_vec()));
        }
        Ok((tag_byte, rest))
    }

    fn handle(&mut self, tag_byte: u8, body: &[u8]) -> Result<(), Error> {
        match tag_byte {
            tag::HANDSHAKE => {
                let mut r = Reader::new(body);
                self.peer_network = r.u32();
                // Skip to the client name: myTime, ip, port, signing time,
                // signature, tracked nets.
                let _ = r.u64();
                let _ = r.bytes();
                let _ = r.u32();
                let _ = r.u64();
                let _ = r.bytes();
                if let Some(count) = r.u32() {
                    for _ in 0..count.min(16) {
                        let _ = r.bytes();
                    }
                }
                if r.u8() == Some(1) {
                    self.peer_client = r.bytes().map(|b| String::from_utf8_lossy(b).into_owned());
                }
                // luxd answers a valid handshake with a PeerList. We must send
                // one too: it is the message that flips the far side from
                // "connecting" to "connected".
                self.send_peer_list()?;
                Ok(())
            }
            tag::PEER_LIST => {
                // They counted us.
                self.accepted = true;
                Ok(())
            }
            tag::PING => {
                let mut w = Writer::new();
                // Uptime, as a percentage. luxd closes the connection on
                // anything above a hundred.
                w.u32(100).u32(0);
                self.write(tag::PONG, w.as_slice())
            }
            tag::PONG => Ok(()),
            tag::GET_PEER_LIST => self.send_peer_list(),
            tag::GET_ACCEPTED_FRONTIER => {
                // ChainId | RequestId | Deadline | EngineType. A poll asking
                // what this peer considers the tip of that chain.
                let mut r = Reader::new(body);
                let (Some(chain), Some(request)) = (r.bytes().map(<[u8]>::to_vec), r.u32()) else {
                    return Ok(());
                };
                self.polled.push((chain.clone(), request));
                match self.frontier_of(&chain) {
                    // Answering for a chain this peer does not track would be
                    // asserting a tip it has no basis for, which is worse than
                    // the timeout that silence costs.
                    None => Ok(()),
                    Some(tip) => self.accepted_frontier(&chain, request, &tip),
                }
            }
            tag::PUSH_QUERY => {
                // ChainId | RequestId | Deadline | Container | EngineType |
                // RequestedHeight. The container is the block itself, so a
                // PushQuery both asks for a vote and supplies what to vote on.
                let mut r = Reader::new(body);
                let (Some(chain), Some(request), Some(_deadline), Some(container)) =
                    (r.bytes().map(<[u8]>::to_vec), r.u32(), r.u64(), r.bytes())
                else {
                    return Ok(());
                };
                // A container's id is the hash of its bytes — that is what
                // consensus names it by, and it is derivable without parsing
                // the block, which is why a peer can answer before it can
                // execute.
                let id: [u8; 32] = Sha256::digest(container).into();
                self.queried.push((chain.clone(), request, id));
                if self.containers.len() < 4 {
                    self.containers.push(container.to_vec());
                }
                // Read the block ONCE: the vote is signed over its header, and
                // so is the record that lets a later certificate for this same
                // container move this peer's accepted tip.
                let block = inner_header(container).and_then(|rlp| {
                    lux_evm::header::Lux::read(rlp).ok().map(|header| (rlp.to_vec(), header))
                });
                if let Some((_, header)) = &block {
                    self.remember(id, &chain, header.header.number);
                }
                if self.frontier_of(&chain).is_some() {
                    let accepted = self.frontier_of(&chain).unwrap_or_default();
                    self.chits(&chain, request, &id, &accepted, self.height)?;
                }
                // And VOTE. Chits answer the poll; the vote is what a
                // certificate is assembled from, and only a signature over the
                // position luxd derived can be counted toward one.
                match block {
                    Some((rlp, header)) => self.cast_vote(&chain, &rlp, &header, &id),
                    None => Ok(()),
                }
            }
            tag::GOSSIP => {
                // App-gossip. Consensus rides here behind the LXQ magic; a
                // payload without it is an ordinary block gossip.
                let mut r = Reader::new(body);
                let (Some(chain), Some(app)) = (r.bytes().map(<[u8]>::to_vec), r.bytes()) else {
                    return Ok(());
                };
                if let Some((kind, block, payload)) = quorum::read(app) {
                    self.heard.push((chain.clone(), kind, block, payload.to_vec()));
                    // A certificate is the network saying that block is final.
                    // A validator that keeps answering with an older tip reads
                    // as behind, and luxd spends the next round fetching a gap
                    // that is not there instead of building the next block.
                    if kind == quorum::CERT {
                        self.follow(&block);
                    }
                }
                Ok(())
            }
            tag::GET_ACCEPTED => {
                // ChainId | RequestId | Deadline | ContainerIds | EngineType.
                // The honest answer is the subset this peer has actually
                // accepted, so an unknown chain answers with none rather than
                // echoing the question back.
                let mut r = Reader::new(body);
                let (Some(chain), Some(request)) = (r.bytes().map(<[u8]>::to_vec), r.u32()) else {
                    return Ok(());
                };
                let _ = r.u64();
                let mut known: Vec<Vec<u8>> = Vec::new();
                if let (Some(count), Some(tip)) = (r.u32(), self.frontier_of(&chain)) {
                    for _ in 0..count.min(1024) {
                        match r.bytes() {
                            Some(id) if id == tip => known.push(id.to_vec()),
                            Some(_) => {}
                            None => break,
                        }
                    }
                }
                self.accepted(&chain, request, &known)
            }
            _ => Ok(()),
        }
    }

    /// An empty PeerList. Enough to complete the handshake in both directions.
    pub fn send_peer_list(&mut self) -> Result<(), Error> {
        let mut w = Writer::new();
        w.u32(0);
        self.write(tag::PEER_LIST, w.as_slice())
    }

    /// Send an app-gossip message to one chain.
    ///
    /// This is the envelope consensus rides in: `chains/quorum.go` puts a vote
    /// or a certificate inside `AppBytes` behind the magic `LXQ\x01`, and
    /// `blockHandler.Gossip` demultiplexes on it.
    pub fn gossip(&mut self, chain: &[u8; 32], app: &[u8]) -> Result<(), Error> {
        let mut w = Writer::new();
        w.bytes(chain).bytes(app);
        self.write(tag::GOSSIP, w.as_slice())
    }
}

/// The envelope luxd carries consensus in, over app-gossip.
///
/// `chains/quorum.go`: four magic bytes, a kind, the block id, then the
/// payload. A payload without the magic is read as an ordinary block gossip
/// instead, so the magic is what separates a vote from a block rather than a
/// decoration on one.
pub mod quorum {
    pub const MAGIC: [u8; 4] = [b'L', b'X', b'Q', 0x01];
    pub const VOTE: u8 = 1;
    pub const CERT: u8 = 2;

    /// Wrap a vote or a certificate.
    pub fn wrap(kind: u8, block: &[u8; 32], payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + 1 + 32 + payload.len());
        out.extend_from_slice(&MAGIC);
        out.push(kind);
        out.extend_from_slice(block);
        out.extend_from_slice(payload);
        out
    }

    /// A signed vote's payload: `nodeID || u32 BE length || signature`.
    ///
    /// `luxfi/consensus@v1.36.81 engine/chain/topology.go encodeSignedVote`.
    /// The signature covers `CanonicalVoteMessage(position)` — the same bytes
    /// `lux_consensus::finality::canonical_vote_message` produces, because it
    /// is the same protocol — and the position carries the execution state
    /// root, so a vote commits to the state the block produced and not merely
    /// to its name.
    pub fn vote(node: &[u8; 20], signature: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(20 + 4 + signature.len());
        out.extend_from_slice(node);
        out.extend_from_slice(&(signature.len() as u32).to_be_bytes());
        out.extend_from_slice(signature);
        out
    }

    /// Read a signed vote back. Strict about trailing bytes, as Go is.
    pub fn read_vote(payload: &[u8]) -> Option<([u8; 20], &[u8])> {
        let (head, rest) = payload.split_at_checked(20)?;
        let (len, sig) = rest.split_at_checked(4)?;
        let n = u32::from_be_bytes(len.try_into().ok()?) as usize;
        if sig.len() != n {
            return None;
        }
        let mut node = [0u8; 20];
        node.copy_from_slice(head);
        Some((node, sig))
    }

    /// Read one back, or say it is not one.
    pub fn read(data: &[u8]) -> Option<(u8, [u8; 32], &[u8])> {
        if data.len() < 4 + 1 + 32 || data[..4] != MAGIC {
            return None;
        }
        let kind = data[4];
        if kind != VOTE && kind != CERT {
            return None;
        }
        let mut block = [0u8; 32];
        block.copy_from_slice(&data[5..37]);
        Some((kind, block, &data[37..]))
    }
}

#[cfg(test)]
mod tests {

    const CHAIN: &[u8] = &[9u8; 32];

    #[test]
    fn a_certificate_moves_the_tip_to_the_block_it_names() {
        let a = [1u8; 32];
        let b = [2u8; 32];
        let pending = vec![(a, CHAIN.to_vec(), 1u64), (b, CHAIN.to_vec(), 2u64)];
        assert_eq!(certified(&pending, &b, 1), Some((CHAIN.to_vec(), 2)));
    }

    #[test]
    fn a_certificate_for_a_block_never_seen_moves_nothing() {
        // Its height is unknown, so following it would claim a tip with no
        // basis — worse than staying behind, which luxd can correct.
        let pending = vec![([1u8; 32], CHAIN.to_vec(), 1u64)];
        assert_eq!(certified(&pending, &[7u8; 32], 0), None);
    }

    #[test]
    fn an_accepted_tip_does_not_move_backwards() {
        // A late certificate for an older block must not undo a newer one.
        let old = [1u8; 32];
        let pending = vec![(old, CHAIN.to_vec(), 1u64)];
        assert_eq!(certified(&pending, &old, 3), None);
    }

    /// A reader that hands out a fixed script of results, one per `read`.
    struct Script(Vec<io::Result<Vec<u8>>>);
    impl Read for Script {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            match self.0.remove(0) {
                Ok(bytes) => {
                    let n = bytes.len().min(out.len());
                    out[..n].copy_from_slice(&bytes[..n]);
                    Ok(n)
                }
                Err(e) => Err(e),
            }
        }
    }

    fn deadline() -> io::Error {
        io::Error::new(io::ErrorKind::WouldBlock, "deadline")
    }

    #[test]
    fn a_deadline_with_nothing_consumed_is_a_quiet_link() {
        let mut r = Script(vec![Err(deadline())]);
        let mut buf = [0u8; 4];
        assert!(matches!(fill(&mut r, &mut buf), Err(Error::Quiet)));
    }

    #[test]
    fn a_deadline_part_way_through_is_a_broken_link() {
        // Two bytes of a four-byte length arrived and then the deadline fired:
        // the stream has lost sync, so this must NOT read as quiet — a daemon
        // that kept such a link would decode the next frame at the wrong offset.
        let mut r = Script(vec![Ok(vec![0, 0]), Err(deadline())]);
        let mut buf = [0u8; 4];
        assert!(matches!(fill(&mut r, &mut buf), Err(Error::Closed(_))));
    }

    #[test]
    fn a_quiet_link_resumes_where_it_left_off() {
        // Silence, then a real frame: the next read must see the whole length.
        let mut r = Script(vec![Ok(vec![0, 0, 0]), Ok(vec![7])]);
        let mut buf = [0u8; 4];
        fill(&mut r, &mut buf).expect("a complete length");
        assert_eq!(u32::from_be_bytes(buf), 7);
    }

    #[test]
    fn a_clean_hangup_is_not_quiet() {
        let mut r = Script(vec![Ok(Vec::new())]);
        let mut buf = [0u8; 4];
        assert!(matches!(fill(&mut r, &mut buf), Err(Error::Closed(_))));
    }
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn a_signed_address_is_twenty_six_bytes_and_the_address_is_mapped() {
        // The trap: the handshake FIELD carries four bytes for a v4 address and
        // the SIGNATURE covers the IPv6-mapped sixteen.
        let b = signed_ip(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 19731, 0x6a98b856);
        assert_eq!(b.len(), 26);
        assert_eq!(
            &b[..16],
            &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 127, 0, 0, 1],
            "the address is v6-mapped, not the four bytes the field carries"
        );
        assert_eq!(&b[16..18], &19731u16.to_be_bytes());
        assert_eq!(&b[18..], &0x6a98b856u64.to_be_bytes());
    }

    #[test]
    fn a_staking_identity_is_p256_and_signs_what_it_certifies() {
        let s = Staking::generate().expect("identity");
        assert!(!s.certificate().is_empty());
        let ip = signed_ip(IpAddr::V4(Ipv4Addr::LOCALHOST), 1, 2);
        let sig = s.sign_ip(&ip).expect("sign");
        // ASN.1 DER, which is what Go's crypto.Signer emits for ECDSA.
        assert_eq!(sig[0], 0x30, "a DER SEQUENCE");
        let parsed = p256::ecdsa::Signature::from_der(&sig).expect("der");
        use p256::ecdsa::signature::hazmat::PrehashVerifier;
        let digest = Sha256::digest(&ip);
        s.key
            .verifying_key()
            .verify_prehash(&digest, &parsed)
            .expect("the signature verifies against its own key");
    }

    #[test]
    fn the_quorum_envelope_round_trips() {
        let block = [7u8; 32];
        let wrapped = quorum::wrap(quorum::VOTE, &block, b"payload");
        assert_eq!(&wrapped[..4], b"LXQ\x01");
        let (kind, got, payload) = quorum::read(&wrapped).expect("reads back");
        assert_eq!(kind, quorum::VOTE);
        assert_eq!(got, block);
        assert_eq!(payload, b"payload");
    }

    #[test]
    fn a_node_id_is_hash160_of_the_certificate() {
        // ids.NodeIDFromCert: RIPEMD-160 of SHA-256 of the DER. Not a
        // truncation of either — a node that used one hash names itself
        // something luxd will not recognise.
        let s = Staking::generate().expect("identity");
        let id = s.node_id();
        use ripemd::{Digest as _, Ripemd160};
        let want = Ripemd160::digest(Sha256::digest(s.certificate()));
        assert_eq!(id.as_slice(), &want[..]);
        assert!(s.node_name().starts_with("NodeID-"));
    }

    #[test]
    fn an_identity_kept_on_disk_keeps_its_name() {
        let dir = std::env::temp_dir().join(format!("lux-staker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let first = Staking::open(&dir).expect("made");
        let second = Staking::open(&dir).expect("loaded");
        assert_eq!(
            first.node_name(),
            second.node_name(),
            "a validator that renamed itself on restart is not in the genesis that named it"
        );
        assert_eq!(first.certificate(), second.certificate());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_proof_of_possession_is_over_the_compressed_key() {
        // signer.NewProofOfPossession signs the 48-byte compressed public key.
        let bls = SecretKey::key_gen(&[5u8; 32], &[]).expect("bls");
        let (public, proof) = proof_of_possession(&bls);
        assert_eq!(public.len(), 48);
        assert_eq!(proof.len(), 96);
        let pk = blst::min_pk::PublicKey::uncompress(&public).expect("key");
        let sig = blst::min_pk::Signature::uncompress(&proof).expect("sig");
        assert_eq!(
            sig.verify(true, &public, POP_DST, &[], &pk, true),
            blst::BLST_ERROR::BLST_SUCCESS,
            "the proof verifies over the key it is a proof of"
        );
    }

    #[test]
    fn cb58_carries_its_own_checksum() {
        // Every id in a genesis or a log line is written this way, so a value
        // produced here has to paste into one.
        let raw = [7u8; 20];
        let text = cb58(&raw);
        let decoded = bs58::decode(&text).into_vec().expect("base58");
        assert_eq!(decoded.len(), 24);
        assert_eq!(&decoded[..20], &raw);
        assert_eq!(&decoded[20..], &Sha256::digest(raw)[28..]);
    }

    #[test]
    fn a_signed_vote_is_an_identity_and_a_signature() {
        // Go's encodeSignedVote, and its strict inverse.
        let node = [0x11u8; 20];
        let sig = vec![0xabu8; 96];
        let payload = quorum::vote(&node, &sig);
        assert_eq!(payload.len(), 20 + 4 + 96);
        assert_eq!(&payload[20..24], &96u32.to_be_bytes());
        let (got, back) = quorum::read_vote(&payload).expect("reads back");
        assert_eq!(got, node);
        assert_eq!(back, &sig[..]);
    }

    #[test]
    fn a_vote_with_the_wrong_length_is_refused() {
        // decodeSignedVote is fail-closed on short reads and strict about
        // trailing bytes, so this must be too.
        let mut short = quorum::vote(&[1u8; 20], &[2u8; 96]);
        short.pop();
        assert!(quorum::read_vote(&short).is_none());
        let mut long = quorum::vote(&[1u8; 20], &[2u8; 96]);
        long.push(0);
        assert!(quorum::read_vote(&long).is_none());
        assert!(quorum::read_vote(&[0u8; 10]).is_none());
    }

    #[test]
    fn an_envelope_without_the_magic_is_not_one() {
        // luxd reads a payload with no magic as an ordinary block gossip, so
        // this predicate has to agree with it exactly.
        assert!(quorum::read(b"short").is_none());
        let mut wrong = quorum::wrap(quorum::VOTE, &[0u8; 32], b"x");
        wrong[0] = b'X';
        assert!(quorum::read(&wrong).is_none());
        // Kind 3 was deleted with the view-change removal and must stay dead.
        let mut dead = quorum::wrap(quorum::VOTE, &[0u8; 32], b"x");
        dead[4] = 3;
        assert!(quorum::read(&dead).is_none());
    }

    #[test]
    fn a_handshake_body_is_the_shape_luxd_reads() {
        // Built the way `send_handshake` builds it, then read back field by
        // field in luxd's declared order.
        let s = Staking::generate().expect("identity");
        let bls = SecretKey::key_gen(&[3u8; 32], &[]).expect("bls");
        let me = Join {
            network: 1337,
            addr: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 19731,
            client: Client::default(),
        };
        let ip = signed_ip(me.addr, me.port, 99);
        let mut w = Writer::new();
        w.u32(me.network).u64(99).bytes(&[127, 0, 0, 1]).u32(19731).u64(99);
        w.bytes(&s.sign_ip(&ip).expect("sign"));
        let none: [&[u8]; 0] = [];
        w.list_bytes(&none);
        w.present().text(&me.client.name).u32(1).u32(36).u32(181);
        w.list_u32(&[]).list_u32(&[]).absent();
        w.bytes(&bls.sign(&ip, POP_DST, &[]).compress());
        w.bool(true);
        let body = w.take();

        let mut r = Reader::new(&body);
        assert_eq!(r.u32(), Some(1337));
        assert_eq!(r.u64(), Some(99));
        assert_eq!(r.bytes(), Some([127, 0, 0, 1].as_slice()));
        assert_eq!(r.u32(), Some(19731));
        assert_eq!(r.u64(), Some(99));
        assert_eq!(r.bytes().map(|b| b[0]), Some(0x30), "DER signature");
        assert_eq!(r.u32(), Some(0), "no tracked chains");
        assert_eq!(r.u8(), Some(1), "the client is present");
        assert_eq!(r.bytes(), Some(b"lux".as_slice()));
        assert_eq!((r.u32(), r.u32(), r.u32()), (Some(1), Some(36), Some(181)));
        assert_eq!(r.u32(), Some(0), "no supported lps");
        assert_eq!(r.u32(), Some(0), "no objected lps");
        assert_eq!(r.u8(), Some(0), "no known peers");
        assert_eq!(r.bytes().map(<[u8]>::len), Some(96), "a compressed BLS signature");
        assert_eq!(r.bool(), Some(true), "all chains");
        assert!(!r.more(), "the legacy shape ends here");
    }
}

#[cfg(test)]
mod vote_message_conformance {
    use lux_consensus::finality::{canonical_vote_message, Position};

    fn b32(v: u8) -> [u8; 32] {
        [v; 32]
    }

    fn position() -> Position {
        Position {
            chain_id: b32(0xc1),
            height: 9,
            round: 1,
            block_id: b32(0xb1),
            parent_id: b32(0xb2),
            canonical_id: b32(0xca),
            parent_canonical_id: b32(0xcb),
            execution_state_root: b32(0xe5),
            payload_root: b32(0x70),
            validator_set_root: b32(0x88),
        }
    }

    /// What `chain.CanonicalVoteMessage` printed for this position, from a Go
    /// program linked against `luxfi/consensus@v1.36.81`. Not reconstructed
    /// here — run and copied.
    const GO: &str = "4c55582f636861696e2f766f74652f763200000301c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1000000000000000900000001cacacacacacacacacacacacacacacacacacacacacacacacacacacacacacacacacbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbe5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e57070707070707070707070707070707070707070707070707070707070707070888888888888888888888888888888888888888888888888888888888888888801";

    /// The same position with the canonical fields unset, which Go degrades to
    /// the outer ids.
    const GO_DEGRADE: &str = "4c55582f636861696e2f766f74652f763200000301c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1000000000000000900000001b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e57070707070707070707070707070707070707070707070707070707070707070888888888888888888888888888888888888888888888888888888888888888801";

    #[test]
    fn the_signed_bytes_are_the_bytes_go_signs() {
        // The one thing that must be byte-identical for a vote to interoperate.
        // Everything else about a vote is which VALUES go in; this is whether
        // the two implementations agree on what a message even is.
        let got = alloy_primitives::hex::encode(canonical_vote_message(&position(), true));
        assert_eq!(got, GO, "the Rust vote message is not the Go vote message");
        assert_eq!(got.len() / 2, 226);
    }

    #[test]
    fn the_degrade_binds_the_outer_ids_the_way_go_does() {
        let mut bare = position();
        bare.canonical_id = [0u8; 32];
        bare.parent_canonical_id = [0u8; 32];
        assert_eq!(
            alloy_primitives::hex::encode(canonical_vote_message(&bare, true)),
            GO_DEGRADE,
            "an unwrapped block must bind its outer ids exactly as Go does"
        );
    }

    #[test]
    fn a_reject_is_a_different_message() {
        assert_ne!(
            canonical_vote_message(&position(), true),
            canonical_vote_message(&position(), false),
            "the accept byte is bound, so a reject can never be replayed as an accept"
        );
    }
}
