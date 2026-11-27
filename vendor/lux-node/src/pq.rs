// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! Who a validator is, and proving it on every link.
//!
//! This is the peer post-quantum handshake the Lux network runs — the same one
//! `mesh/peer/handshake.go` runs, down to the byte. Two messages, not three,
//! and asymmetric: the dialer sends INIT, the acceptor answers RESP, and both
//! ends derive one AEAD session key from what crossed.
//!
//! - **ML-DSA-65** (FIPS 204) is IDENTITY. A node id is a domain-separated
//!   SHAKE256 digest of the ML-DSA public key, so a node cannot claim a name it
//!   cannot sign for — the name and the key are the same fact.
//! - **ML-KEM-768** (FIPS 203) is the SESSION. The initiator offers an
//!   encapsulation key, the responder encapsulates to it once, and the shared
//!   secret is bound to everything both ends saw.
//!
//! WHAT EACH SIDE SIGNS. The initiator signs the INIT fields up to the
//! signature. The responder signs the initiator's whole message and then its
//! own fields — so the reply cannot be lifted onto a different INIT. The two
//! signatures are made under different FIPS 204 context strings
//! (`NODE_PQ_HANDSHAKE_V1/initiator` and `.../responder`), which are part of
//! the signed message: an initiator's signature presented as a responder's is a
//! signature over bytes nobody signed.
//!
//! WHY THE SESSION KEY IS NOT THE KEM OUTPUT. Both ends hash the two canonical
//! messages, the profile, the chain, and both identity keys into one
//! transcript, and derive the key from the KEM secret AND that hash. Flip one
//! field of the handshake and the attacker holds a key the honest peer never
//! derived, so the first sealed frame fails to open. That is what makes a
//! downgrade fail closed instead of quietly succeeding.
//!
//! DETERMINISTIC SIGNING. Signatures are made in FIPS 204's deterministic mode,
//! the mode Go uses — which is what lets a handshake be a test vector.
//!
//! WHAT IT DOES NOT DO. The frames after the handshake are not encrypted on the
//! validator link. What that link carries is votes and blocks, which every
//! validator is entitled to, and the integrity that matters — that a vote is
//! this validator's — is the BLS signature inside the vote. The plugin link
//! does seal its frames, and the key it seals them with is the one established
//! here.

use std::io::{self, Read, Write};
use std::time::Duration;

use lux_consensus::cert::NodeId;
use ml_dsa::{
    common::Generate, EncodedSignature, EncodedVerifyingKey, ExpandedSigningKey, MlDsa65, Seed,
    Signature, SigningKey, VerifyingKey,
};
use ml_kem::{
    kem::{Kem as KemAlgorithm, Key},
    ml_kem_1024::{self, MlKem1024},
    ml_kem_768::{self, MlKem768},
    Decapsulate, Encapsulate, KeyExport,
};
use sha3::digest::{core_api::CoreWrapper, ExtendableOutput, Update, XofReader};

/// The protocol version byte. A peer that does not understand the version it
/// receives stops before any lattice arithmetic runs.
pub const PROTOCOL_VERSION: u8 = 0x01;

/// The canonical node id width.
pub const NODE_ID_LEN: usize = 20;

/// The canonical chain id width.
pub const CHAIN_ID_LEN: usize = 32;

/// FIPS 204 ML-DSA-65 public key width.
pub const IDENTITY_KEY_LEN: usize = 1952;

/// FIPS 204 ML-DSA-65 signature width.
pub const IDENTITY_SIGNATURE_LEN: usize = 3309;

/// The largest handshake frame.
///
/// The widest legitimate message is a RESP under ML-KEM-1024: 1952 bytes of
/// identity key, 1568 of ciphertext, 3309 of signature and the fixed fields.
/// 16 KiB is well above that and far below anything a peer could exhaust
/// memory with — and the cap is checked before the buffer is allocated.
pub const FRAME_MAX: usize = 16 * 1024;

/// The FIPS 204 context an initiator signs under.
pub const CONTEXT_INITIATOR: &[u8] = b"NODE_PQ_HANDSHAKE_V1/initiator";

/// The FIPS 204 context a responder signs under.
pub const CONTEXT_RESPONDER: &[u8] = b"NODE_PQ_HANDSHAKE_V1/responder";

/// The SP 800-185 customization the node id digest is taken under.
const NODE_ID_DOMAIN: &[u8] = b"NODE_ID_V1";

/// The node id scheme byte for ML-DSA-65 keys.
const NODE_ID_SCHEME_MLDSA65: u8 = 0x42;

/// The full node id digest is 48 bytes; the id is its first twenty.
const NODE_ID_DIGEST_LEN: usize = 48;

/// The transcript hash width, and the customization it is taken under.
const TRANSCRIPT_HASH_LEN: usize = 48;
const TRANSCRIPT_CUSTOMIZATION: &[u8] = b"NODE_TRANSCRIPT_V1";
const TRANSCRIPT_FUNCTION: &[u8] = b"TupleHash";

/// The session key width, and the customization it is derived under.
const AEAD_KEY_LEN: usize = 32;
const AEAD_CUSTOMIZATION: &[u8] = b"NODE_AEAD_V1";
const AEAD_FUNCTION: &[u8] = b"KEMDerive";

/// The security profile both ends commit to, as one wire byte.
///
/// A byte rather than a closed set, because the wire carries a byte: a peer
/// that names a profile this build has never heard of is refused for
/// disagreeing with ours, which is the same answer and a truer one than
/// "unparseable".
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Profile(pub u8);

impl Profile {
    /// No profile committed. Refused by every peer, including as a local
    /// setting.
    pub const NONE: Profile = Profile(0x00);
    /// The canonical strict post-quantum profile.
    pub const STRICT_PQ: Profile = Profile(0x01);
    /// Testnet and devnet.
    pub const PERMISSIVE: Profile = Profile(0x02);
    /// FIPS 204 and FIPS 202 only.
    pub const FIPS: Profile = Profile(0x03);

    /// Whether this profile mandates refusing classical key exchange.
    pub fn is_strict(self) -> bool {
        self == Profile::STRICT_PQ || self == Profile::FIPS
    }
}

impl core::fmt::Debug for Profile {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Profile::NONE => f.write_str("none"),
            Profile::STRICT_PQ => f.write_str("strict-pq"),
            Profile::PERMISSIVE => f.write_str("permissive"),
            Profile::FIPS => f.write_str("fips"),
            Profile(b) => write!(f, "profile(0x{b:02x})"),
        }
    }
}

/// The key-exchange scheme, as one wire byte.
///
/// The classical marker exists so a strict peer that receives one can say what
/// was wrong instead of dropping the connection without a reason.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Kem(pub u8);

impl Kem {
    /// Unspecified. Refused everywhere.
    pub const INVALID: Kem = Kem(0x00);
    /// FIPS 203 ML-KEM-768, NIST category 3. The production default.
    pub const MLKEM768: Kem = Kem(0x01);
    /// FIPS 203 ML-KEM-1024, NIST category 5.
    pub const MLKEM1024: Kem = Kem(0x02);
    /// Classical X25519, carried only as a marker to refuse.
    pub const X25519_UNSAFE: Kem = Kem(0x90);

    /// Whether this scheme is post-quantum.
    pub fn is_post_quantum(self) -> bool {
        self == Kem::MLKEM768 || self == Kem::MLKEM1024
    }

    /// Whether a strict-profile peer refuses this scheme outright.
    pub fn is_forbidden_in_pq_mode(self) -> bool {
        self == Kem::X25519_UNSAFE
    }

    /// The encapsulation-key width, for a scheme this build implements.
    pub fn key_len(self) -> Option<usize> {
        match self {
            Kem::MLKEM768 => Some(1184),
            Kem::MLKEM1024 => Some(1568),
            _ => None,
        }
    }

    /// The ciphertext width, for a scheme this build implements.
    pub fn ciphertext_len(self) -> Option<usize> {
        match self {
            Kem::MLKEM768 => Some(1088),
            Kem::MLKEM1024 => Some(1568),
            _ => None,
        }
    }
}

impl core::fmt::Debug for Kem {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Kem::INVALID => f.write_str("invalid"),
            Kem::MLKEM768 => f.write_str("ml-kem-768"),
            Kem::MLKEM1024 => f.write_str("ml-kem-1024"),
            Kem::X25519_UNSAFE => f.write_str("x25519-unsafe"),
            Kem(b) => write!(f, "kem(0x{b:02x})"),
        }
    }
}

/// The policy a link is held to, and the two values it commits to on the wire.
///
/// One value per connection. The remote must advertise the same profile, the
/// same chain and the same scheme, or the handshake stops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    pub profile: Profile,
    pub chain: [u8; CHAIN_ID_LEN],
    pub kem: Kem,
    pub forbid_classical_kem: bool,
}

impl Config {
    /// What the running network puts on the wire.
    ///
    /// The chain commitment is zero because that is the value a Go node sends:
    /// `mesh.go` builds its `HandshakeConfig` with a profile, a scheme and the
    /// refusal switch, and leaves `ChainID` at its zero value. A node that
    /// invented a chain commitment here would be refused by every peer on the
    /// network for disagreeing about which chain it is on.
    pub fn strict_pq() -> Self {
        Self {
            profile: Profile::STRICT_PQ,
            chain: [0u8; CHAIN_ID_LEN],
            kem: Kem::MLKEM768,
            forbid_classical_kem: true,
        }
    }

    /// Refuse a local configuration that could not produce a valid handshake,
    /// before a peer is contacted with it.
    fn check(&self) -> Result<(), Error> {
        if self.kem.is_forbidden_in_pq_mode() {
            return Err(Error::ClassicalKem(self.kem));
        }
        if !self.kem.is_post_quantum() {
            return Err(Error::KemScheme { want: self.kem, got: self.kem });
        }
        if self.profile == Profile::NONE {
            return Err(Error::ProfileMismatch { want: self.profile, got: self.profile });
        }
        Ok(())
    }
}

/// Which half of the handshake this end runs.
///
/// By dial direction, which is well defined per connection: the dialer
/// initiates, the accepter responds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Initiator,
    Responder,
}

/// What a validator signs with, and is named by.
///
/// The whole identity is a 32-byte FIPS 204 seed: the signing key and the
/// public key are both functions of it, so that is what is kept on disk. An
/// identity that changed every restart would be a new validator every restart.
pub struct Identity {
    seed: Seed,
    signing: ExpandedSigningKey<MlDsa65>,
    public: Vec<u8>,
    node: NodeId,
}

impl Identity {
    /// A fresh identity, from the system's random source.
    pub fn generate() -> Result<Self, Error> {
        let key = SigningKey::<MlDsa65>::try_generate()
            .map_err(|e| Error::Io(format!("no entropy for an identity: {e}")))?;
        Ok(Self::from_seed(&key.to_seed()))
    }

    /// The identity a seed names.
    pub fn from_seed(seed: &Seed) -> Self {
        let signing = ExpandedSigningKey::<MlDsa65>::from_seed(seed);
        let public = signing.verifying_key().encode().to_vec();
        Self { node: name(&public), seed: *seed, signing, public }
    }

    /// Load the identity at `path`, or make one and write it there.
    ///
    /// The file is the seed, and the seed is the secret, so it is written
    /// readable by nobody else. A node whose identity file is world-readable
    /// has published the ability to be it.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        match std::fs::read(path) {
            Ok(raw) => {
                let seed: [u8; 32] = raw
                    .as_slice()
                    .try_into()
                    .map_err(|_| Error::Malformed("the identity file is not a seed"))?;
                Ok(Self::from_seed(&seed.into()))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let fresh = Self::generate()?;
                write_private(path, &fresh.seed)?;
                Ok(fresh)
            }
            Err(e) => Err(Error::Io(e.to_string())),
        }
    }

    /// This validator's name.
    pub fn node(&self) -> NodeId {
        self.node
    }

    /// The ML-DSA-65 public key, in FIPS 204 encoding.
    pub fn public(&self) -> &[u8] {
        &self.public
    }

    /// Sign `message` under `context`, in FIPS 204's deterministic mode.
    ///
    /// The context is part of the signed message, which is what keeps an
    /// initiator's signature from being replayed as a responder's.
    pub fn sign(&self, message: &[u8], context: &[u8]) -> Result<Vec<u8>, Error> {
        self.signing
            .sign_deterministic(message, context)
            .map(|s| s.encode().to_vec())
            .map_err(|_| Error::Malformed("the signature context is too long"))
    }
}

/// The node id an ML-DSA-65 public key answers to.
///
/// `SHAKE256(left_encode ‖ "NODE_ID_V1" ‖ left_encode ‖ chain ‖ left_encode ‖
/// scheme ‖ left_encode ‖ key)`, truncated to twenty bytes. The chain is zero:
/// that is the node-identity domain, the one a peer re-derives under to check
/// that the name a peer claims is the name its key produces.
///
/// Every field is length-prefixed under SP 800-185, so a key whose first bytes
/// spell another field cannot be read as one.
pub fn name(public_key: &[u8]) -> NodeId {
    let mut input = Vec::with_capacity(public_key.len() + 64);
    for (field, bits) in [
        (NODE_ID_DOMAIN, (NODE_ID_DOMAIN.len() * 8) as u64),
        (&[0u8; CHAIN_ID_LEN][..], (CHAIN_ID_LEN * 8) as u64),
        (&[NODE_ID_SCHEME_MLDSA65][..], 8u64),
        (public_key, (public_key.len() * 8) as u64),
    ] {
        input.extend_from_slice(&left_encode(bits));
        input.extend_from_slice(field);
    }

    let mut digest = [0u8; NODE_ID_DIGEST_LEN];
    let mut h = sha3::Shake256::default();
    h.update(&input);
    XofReader::read(&mut h.finalize_xof(), &mut digest);

    let mut id = [0u8; NODE_ID_LEN];
    id.copy_from_slice(&digest[..NODE_ID_LEN]);
    id
}

/// The initiator's message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Init {
    pub version: u8,
    pub profile: Profile,
    pub chain: [u8; CHAIN_ID_LEN],
    pub kem: Kem,
    pub node: NodeId,
    pub identity_key: Vec<u8>,
    pub kem_key: Vec<u8>,
    pub signature: Vec<u8>,
}

impl Init {
    /// The bytes the initiator signs: every field in wire order, minus the
    /// signature itself.
    pub fn prefix(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(256 + self.identity_key.len() + self.kem_key.len());
        out.push(self.version);
        out.push(self.profile.0);
        out.extend_from_slice(&self.chain);
        out.push(self.kem.0);
        out.extend_from_slice(&self.node);
        append_length_prefixed(&mut out, &self.identity_key);
        append_length_prefixed(&mut out, &self.kem_key);
        out
    }

    /// The full message, signature included. This is the frame body, and it is
    /// also what the responder signs over first.
    pub fn canonical(&self) -> Vec<u8> {
        let mut out = self.prefix();
        append_length_prefixed(&mut out, &self.signature);
        out
    }

    /// Read a message off the wire.
    ///
    /// `kem` is what this end offered: a scheme byte that is not it stops the
    /// read before any length is trusted.
    pub fn decode(bytes: &[u8], kem: Kem) -> Result<Self, Error> {
        let mut r = Cursor::new(bytes);
        let (version, profile, chain, scheme, node) = r.header()?;
        if scheme != kem {
            return Err(Error::KemScheme { want: kem, got: scheme });
        }
        let identity_key = r.bytes()?.to_vec();
        let kem_key = r.bytes()?.to_vec();
        let signature = r.bytes()?.to_vec();
        r.end()?;
        Ok(Self { version, profile, chain, kem: scheme, node, identity_key, kem_key, signature })
    }
}

/// The responder's message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resp {
    pub version: u8,
    pub profile: Profile,
    pub chain: [u8; CHAIN_ID_LEN],
    pub kem: Kem,
    pub node: NodeId,
    pub identity_key: Vec<u8>,
    pub kem_ciphertext: Vec<u8>,
    pub signature: Vec<u8>,
}

impl Resp {
    /// The bytes the responder signs: the initiator's whole message, then the
    /// responder's own fields.
    ///
    /// Carrying the INIT here is what stops the reply being lifted onto a
    /// different handshake — the signature covers what this responder actually
    /// saw.
    pub fn prefix(&self, init: &Init) -> Vec<u8> {
        let mut out = init.canonical();
        out.reserve(128 + self.identity_key.len() + self.kem_ciphertext.len());
        out.push(self.version);
        out.push(self.profile.0);
        out.extend_from_slice(&self.chain);
        out.push(self.kem.0);
        out.extend_from_slice(&self.node);
        append_length_prefixed(&mut out, &self.identity_key);
        append_length_prefixed(&mut out, &self.kem_ciphertext);
        out
    }

    /// The full message, signature included — the frame body. It does not
    /// carry the INIT, because the wire delivers RESP alone.
    pub fn canonical(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(256 + self.identity_key.len() + self.kem_ciphertext.len());
        out.push(self.version);
        out.push(self.profile.0);
        out.extend_from_slice(&self.chain);
        out.push(self.kem.0);
        out.extend_from_slice(&self.node);
        append_length_prefixed(&mut out, &self.identity_key);
        append_length_prefixed(&mut out, &self.kem_ciphertext);
        append_length_prefixed(&mut out, &self.signature);
        out
    }

    /// Read a message off the wire.
    pub fn decode(bytes: &[u8], kem: Kem) -> Result<Self, Error> {
        let mut r = Cursor::new(bytes);
        let (version, profile, chain, scheme, node) = r.header()?;
        if scheme != kem {
            return Err(Error::KemScheme { want: kem, got: scheme });
        }
        let identity_key = r.bytes()?.to_vec();
        let kem_ciphertext = r.bytes()?.to_vec();
        let signature = r.bytes()?.to_vec();
        r.end()?;
        Ok(Self {
            version,
            profile,
            chain,
            kem: scheme,
            node,
            identity_key,
            kem_ciphertext,
            signature,
        })
    }
}

/// Everything the session key is bound to.
///
/// Both messages, then the profile, the chain and both identity keys again.
/// The repetition is deliberate: a disagreement about the profile or the chain
/// shows up here even if the encoding of either message drifted.
pub fn bind_aead_transcript(init: &Init, resp: &Resp) -> Vec<u8> {
    let mut out = init.canonical();
    out.extend_from_slice(&resp.canonical());
    out.push(init.profile.0);
    out.extend_from_slice(&init.chain);
    out.extend_from_slice(&init.identity_key);
    out.extend_from_slice(&resp.identity_key);
    out
}

/// The 48-byte commitment to a bound transcript.
///
/// SP 800-185 TupleHash256 over one element — which is how the handshake calls
/// it — so the input is `encode_string(transcript) ‖ right_encode(384)` fed to
/// cSHAKE256 under the function name `TupleHash` and the customization
/// `NODE_TRANSCRIPT_V1`.
pub fn hash_transcript(transcript: &[u8]) -> [u8; TRANSCRIPT_HASH_LEN] {
    let mut input = left_encode((transcript.len() as u64) * 8);
    input.extend_from_slice(transcript);
    input.extend_from_slice(&right_encode((TRANSCRIPT_HASH_LEN as u64) * 8));

    let mut out = [0u8; TRANSCRIPT_HASH_LEN];
    cshake256(TRANSCRIPT_FUNCTION, TRANSCRIPT_CUSTOMIZATION, &input, &mut out);
    out
}

/// The session key: the KEM secret and the transcript, and neither alone.
///
/// Same secret with a different transcript gives a different key; same
/// transcript with a different secret gives a different key. That is the
/// property a downgrade has to beat, and cannot.
pub fn derive_aead_key(
    kem: Kem,
    shared: &[u8; 32],
    transcript_hash: &[u8; TRANSCRIPT_HASH_LEN],
) -> [u8; AEAD_KEY_LEN] {
    let mut input = Vec::with_capacity(1 + shared.len() + transcript_hash.len());
    input.push(kem.0);
    input.extend_from_slice(shared);
    input.extend_from_slice(transcript_hash);

    let mut out = [0u8; AEAD_KEY_LEN];
    cshake256(AEAD_FUNCTION, AEAD_CUSTOMIZATION, &input, &mut out);
    out
}

/// What a completed handshake leaves behind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    /// The peer's proven identity — derived from the key it signed with, not
    /// from anything it asserted.
    pub peer: NodeId,
    /// The peer's ML-DSA-65 public key.
    pub key: Vec<u8>,
    /// The AEAD session key both ends derived.
    pub secret: [u8; AEAD_KEY_LEN],
}

#[derive(Debug)]
pub enum Error {
    Io(String),
    Malformed(&'static str),
    /// The frame, or a field in it, is larger than this link admits.
    Oversize(usize),
    /// A protocol version this build does not speak.
    BadVersion(u8),
    /// The peer runs a different security profile.
    ProfileMismatch {
        want: Profile,
        got: Profile,
    },
    /// The peer is on a different chain.
    ChainMismatch,
    /// The peer offered a scheme this link does not admit — including a
    /// responder answering with a weaker one than was offered.
    KemScheme {
        want: Kem,
        got: Kem,
    },
    /// The peer offered classical key exchange to a strict peer.
    ClassicalKem(Kem),
    /// The peer presented the zero node id.
    NodeIdZero,
    /// A key or signature was not the width its scheme defines.
    BadIdentity(&'static str),
    /// The peer's signature over the transcript did not verify: it does not
    /// hold the key it presented.
    Unproven,
    /// The peer holds a key, and it is not the key its claimed name derives
    /// from. A throwaway key signing a validator's name.
    Impersonation {
        claimed: NodeId,
        derived: NodeId,
    },
    /// The peer proved an identity, and it is not the one we dialed.
    WrongPeer {
        want: NodeId,
        got: NodeId,
    },
    /// The far end presented our own identity: a mirror, or ourselves.
    Reflected,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Malformed(w) => write!(f, "malformed handshake: {w}"),
            Error::Oversize(n) => write!(
                f,
                "the peer announced {n} bytes, over the {FRAME_MAX} this link admits"
            ),
            Error::BadVersion(v) => write!(
                f,
                "the peer speaks handshake version 0x{v:02x}, this node speaks 0x{PROTOCOL_VERSION:02x}"
            ),
            Error::ProfileMismatch { want, got } => {
                write!(f, "this node runs {want:?} and the peer runs {got:?}")
            }
            Error::ChainMismatch => write!(f, "the peer is on another chain"),
            Error::KemScheme { want, got } => {
                write!(f, "this link offered {want:?} and the peer answered {got:?}")
            }
            Error::ClassicalKem(k) => write!(f, "{k:?} is classical, and this profile refuses it"),
            Error::NodeIdZero => write!(f, "the peer presented the zero node id"),
            Error::BadIdentity(w) => write!(f, "the peer's identity is malformed: {w}"),
            Error::Unproven => write!(f, "the peer does not hold the key it presented"),
            Error::Impersonation { claimed, derived } => write!(
                f,
                "the peer calls itself {} and its key derives {}",
                alloy_primitives::hex::encode(claimed),
                alloy_primitives::hex::encode(derived)
            ),
            Error::Reflected => write!(f, "the far end is this node"),
            Error::WrongPeer { want, got } => write!(
                f,
                "dialed {} and reached {}",
                alloy_primitives::hex::encode(want),
                alloy_primitives::hex::encode(got)
            ),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

/// Perform the handshake over `link`.
///
/// `role` is the dial direction. `expect` is the identity we believe we
/// dialed; an accepting side passes `None` because it does not know yet.
pub fn greet<L: Read + Write>(
    link: &mut L,
    cfg: &Config,
    me: &Identity,
    role: Role,
    expect: Option<NodeId>,
) -> Result<Session, Error> {
    cfg.check()?;
    let (peer_claimed, peer_key, secret) = match role {
        Role::Initiator => initiate(link, cfg, me)?,
        Role::Responder => respond(link, cfg, me)?,
    };

    // THE NAME IS THE KEY. The signature proves the peer holds the key it
    // sent; nothing so far ties that key to the name it claimed. Re-derive the
    // name and require it, or a throwaway key could sign a validator's name.
    let derived = name(&peer_key);
    if derived != peer_claimed {
        return Err(Error::Impersonation { claimed: peer_claimed, derived });
    }
    // A MIRROR IS NOT A PEER. A link to ourselves completes honestly and
    // counts as a validator; the only thing that distinguishes it is that the
    // far end holds our key.
    if peer_key == me.public() {
        return Err(Error::Reflected);
    }
    if let Some(want) = expect {
        if derived != want {
            return Err(Error::WrongPeer { want, got: derived });
        }
    }
    Ok(Session { peer: derived, key: peer_key, secret })
}

/// Send INIT, read RESP, derive the key.
fn initiate<L: Read + Write>(
    link: &mut L,
    cfg: &Config,
    me: &Identity,
) -> Result<(NodeId, Vec<u8>, [u8; AEAD_KEY_LEN]), Error> {
    // A fresh KEM key per link. Reusing one would make every session with this
    // node openable by whoever once opened any of them.
    let (secret, kem_key) = Ephemeral::generate(cfg.kem)?;

    let mut init = Init {
        version: PROTOCOL_VERSION,
        profile: cfg.profile,
        chain: cfg.chain,
        kem: cfg.kem,
        node: me.node(),
        identity_key: me.public().to_vec(),
        kem_key,
        signature: Vec::new(),
    };
    init.signature = me.sign(&init.prefix(), CONTEXT_INITIATOR)?;
    write_frame(link, &init.canonical())?;

    let resp = Resp::decode(&read_frame(link)?, cfg.kem)?;
    check_resp(cfg, &init, &resp)?;
    verify(&resp.identity_key, &resp.prefix(&init), CONTEXT_RESPONDER, &resp.signature)?;

    let shared = secret.decapsulate(&resp.kem_ciphertext)?;
    let hash = hash_transcript(&bind_aead_transcript(&init, &resp));
    Ok((resp.node, resp.identity_key, derive_aead_key(cfg.kem, &shared, &hash)))
}

/// Read INIT, send RESP, derive the key.
fn respond<L: Read + Write>(
    link: &mut L,
    cfg: &Config,
    me: &Identity,
) -> Result<(NodeId, Vec<u8>, [u8; AEAD_KEY_LEN]), Error> {
    let init = Init::decode(&read_frame(link)?, cfg.kem)?;
    // Every cheap check runs before the signature does, so a peer cannot spend
    // this node's cycles on lattice arithmetic over a message it already lost.
    check_init(cfg, &init)?;
    verify(&init.identity_key, &init.prefix(), CONTEXT_INITIATOR, &init.signature)?;

    let (kem_ciphertext, shared) = encapsulate(cfg.kem, &init.kem_key)?;
    let mut resp = Resp {
        version: PROTOCOL_VERSION,
        profile: cfg.profile,
        chain: cfg.chain,
        kem: cfg.kem,
        node: me.node(),
        identity_key: me.public().to_vec(),
        kem_ciphertext,
        signature: Vec::new(),
    };
    resp.signature = me.sign(&resp.prefix(&init), CONTEXT_RESPONDER)?;
    write_frame(link, &resp.canonical())?;

    let hash = hash_transcript(&bind_aead_transcript(&init, &resp));
    Ok((init.node, init.identity_key, derive_aead_key(cfg.kem, &shared, &hash)))
}

/// Every cross-field rule an INIT has to satisfy.
fn check_init(cfg: &Config, init: &Init) -> Result<(), Error> {
    if init.version != PROTOCOL_VERSION {
        return Err(Error::BadVersion(init.version));
    }
    if init.profile != cfg.profile {
        return Err(Error::ProfileMismatch { want: cfg.profile, got: init.profile });
    }
    if init.chain != cfg.chain {
        return Err(Error::ChainMismatch);
    }
    if cfg.forbid_classical_kem && init.kem.is_forbidden_in_pq_mode() {
        return Err(Error::ClassicalKem(init.kem));
    }
    if init.kem != cfg.kem {
        return Err(Error::KemScheme { want: cfg.kem, got: init.kem });
    }
    if init.node == [0u8; NODE_ID_LEN] {
        return Err(Error::NodeIdZero);
    }
    if init.identity_key.len() != IDENTITY_KEY_LEN {
        return Err(Error::BadIdentity("the identity key is not ML-DSA-65 wide"));
    }
    if Some(init.kem_key.len()) != init.kem.key_len() {
        return Err(Error::KemScheme { want: cfg.kem, got: init.kem });
    }
    if init.signature.len() != IDENTITY_SIGNATURE_LEN {
        return Err(Error::BadIdentity("the signature is not ML-DSA-65 wide"));
    }
    Ok(())
}

/// Every cross-field rule a RESP has to satisfy, including the one that stops
/// a responder answering with a weaker scheme than was offered.
fn check_resp(cfg: &Config, init: &Init, resp: &Resp) -> Result<(), Error> {
    if resp.version != PROTOCOL_VERSION {
        return Err(Error::BadVersion(resp.version));
    }
    if resp.profile != cfg.profile {
        return Err(Error::ProfileMismatch { want: cfg.profile, got: resp.profile });
    }
    if resp.chain != cfg.chain {
        return Err(Error::ChainMismatch);
    }
    if cfg.forbid_classical_kem && resp.kem.is_forbidden_in_pq_mode() {
        return Err(Error::ClassicalKem(resp.kem));
    }
    if resp.kem != init.kem {
        return Err(Error::KemScheme { want: init.kem, got: resp.kem });
    }
    if resp.node == [0u8; NODE_ID_LEN] {
        return Err(Error::NodeIdZero);
    }
    if resp.identity_key.len() != IDENTITY_KEY_LEN {
        return Err(Error::BadIdentity("the identity key is not ML-DSA-65 wide"));
    }
    if Some(resp.kem_ciphertext.len()) != resp.kem.ciphertext_len() {
        return Err(Error::KemScheme { want: init.kem, got: resp.kem });
    }
    if resp.signature.len() != IDENTITY_SIGNATURE_LEN {
        return Err(Error::BadIdentity("the signature is not ML-DSA-65 wide"));
    }
    Ok(())
}

/// Check one ML-DSA-65 signature under its context.
pub fn verify(
    public_key: &[u8],
    message: &[u8],
    context: &[u8],
    signature: &[u8],
) -> Result<(), Error> {
    let encoded: &EncodedVerifyingKey<MlDsa65> = public_key
        .try_into()
        .map_err(|_| Error::BadIdentity("the identity key is not ML-DSA-65 wide"))?;
    let signature: &EncodedSignature<MlDsa65> = signature
        .try_into()
        .map_err(|_| Error::BadIdentity("the signature is not ML-DSA-65 wide"))?;
    let Some(signature) = Signature::<MlDsa65>::decode(signature) else {
        return Err(Error::Unproven);
    };
    if VerifyingKey::<MlDsa65>::decode(encoded).verify_with_context(message, context, &signature) {
        Ok(())
    } else {
        Err(Error::Unproven)
    }
}

/// The decapsulation half of a per-link KEM key.
enum Ephemeral {
    K768(ml_kem_768::DecapsulationKey),
    K1024(ml_kem_1024::DecapsulationKey),
}

impl Ephemeral {
    /// A fresh keypair, returning the key to send and the half to keep.
    fn generate(kem: Kem) -> Result<(Self, Vec<u8>), Error> {
        match kem {
            Kem::MLKEM768 => {
                let (dk, ek) = MlKem768::generate_keypair();
                Ok((Ephemeral::K768(dk), ek.to_bytes().to_vec()))
            }
            Kem::MLKEM1024 => {
                let (dk, ek) = MlKem1024::generate_keypair();
                Ok((Ephemeral::K1024(dk), ek.to_bytes().to_vec()))
            }
            other => Err(Error::KemScheme { want: other, got: other }),
        }
    }

    /// Recover the shared secret the responder encapsulated.
    fn decapsulate(&self, ciphertext: &[u8]) -> Result<[u8; 32], Error> {
        let shared = match self {
            Ephemeral::K768(dk) => dk
                .decapsulate_slice(ciphertext)
                .map_err(|_| Error::BadIdentity("the ciphertext is not the scheme's width"))?,
            Ephemeral::K1024(dk) => dk
                .decapsulate_slice(ciphertext)
                .map_err(|_| Error::BadIdentity("the ciphertext is not the scheme's width"))?,
        };
        Ok(shared.into())
    }
}

/// Encapsulate to the initiator's key.
fn encapsulate(kem: Kem, key: &[u8]) -> Result<(Vec<u8>, [u8; 32]), Error> {
    match kem {
        Kem::MLKEM768 => {
            let key: &Key<ml_kem_768::EncapsulationKey> = key
                .try_into()
                .map_err(|_| Error::BadIdentity("the session key is not the scheme's width"))?;
            let ek = ml_kem_768::EncapsulationKey::new(key)
                .map_err(|_| Error::BadIdentity("the session key does not decode"))?;
            let (ct, shared) = ek.encapsulate();
            Ok((ct.to_vec(), shared.into()))
        }
        Kem::MLKEM1024 => {
            let key: &Key<ml_kem_1024::EncapsulationKey> = key
                .try_into()
                .map_err(|_| Error::BadIdentity("the session key is not the scheme's width"))?;
            let ek = ml_kem_1024::EncapsulationKey::new(key)
                .map_err(|_| Error::BadIdentity("the session key does not decode"))?;
            let (ct, shared) = ek.encapsulate();
            Ok((ct.to_vec(), shared.into()))
        }
        other => Err(Error::KemScheme { want: other, got: other }),
    }
}

/// Write one handshake frame: a four-byte big-endian length, then the body.
pub fn write_frame<W: Write>(w: &mut W, payload: &[u8]) -> Result<(), Error> {
    if payload.len() > FRAME_MAX {
        return Err(Error::Oversize(payload.len()));
    }
    w.write_all(&(payload.len() as u32).to_be_bytes())?;
    w.write_all(payload)?;
    w.flush()?;
    Ok(())
}

/// Read one handshake frame. The cap is checked before the buffer exists, so
/// an announced size is never an allocation.
pub fn read_frame<R: Read>(r: &mut R) -> Result<Vec<u8>, Error> {
    let mut header = [0u8; 4];
    r.read_exact(&mut header)?;
    let size = u32::from_be_bytes(header) as usize;
    if size > FRAME_MAX {
        return Err(Error::Oversize(size));
    }
    let mut body = vec![0u8; size];
    r.read_exact(&mut body)?;
    Ok(body)
}

/// A bounds-checked walk over an untrusted message.
struct Cursor<'a> {
    buf: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, at: 0 }
    }

    fn remaining(&self) -> usize {
        self.buf.len() - self.at
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if self.remaining() < n {
            return Err(Error::Malformed("the message ends mid-field"));
        }
        let out = &self.buf[self.at..self.at + n];
        self.at += n;
        Ok(out)
    }

    /// The fixed head both messages share.
    fn header(&mut self) -> Result<(u8, Profile, [u8; CHAIN_ID_LEN], Kem, NodeId), Error> {
        let version = self.take(1)?[0];
        let profile = Profile(self.take(1)?[0]);
        let mut chain = [0u8; CHAIN_ID_LEN];
        chain.copy_from_slice(self.take(CHAIN_ID_LEN)?);
        let kem = Kem(self.take(1)?[0]);
        let mut node = [0u8; NODE_ID_LEN];
        node.copy_from_slice(self.take(NODE_ID_LEN)?);
        Ok((version, profile, chain, kem, node))
    }

    /// One length-prefixed field.
    fn bytes(&mut self) -> Result<&'a [u8], Error> {
        let len = u32::from_be_bytes(self.take(4)?.try_into().expect("four bytes are four bytes"))
            as usize;
        if len > self.remaining() {
            return Err(Error::Malformed("a field claims more than the message holds"));
        }
        self.take(len)
    }

    /// Nothing may follow the last field: a message with a tail is a message
    /// two readers could disagree about.
    fn end(&self) -> Result<(), Error> {
        if self.remaining() != 0 {
            return Err(Error::Malformed("trailing bytes after the last field"));
        }
        Ok(())
    }
}

/// A four-byte big-endian length, then the bytes — so two different
/// concatenations cannot collide at a field boundary.
fn append_length_prefixed(dst: &mut Vec<u8>, b: &[u8]) {
    dst.extend_from_slice(&(b.len() as u32).to_be_bytes());
    dst.extend_from_slice(b);
}

/// SP 800-185 §2.3.1 left_encode.
fn left_encode(x: u64) -> Vec<u8> {
    if x == 0 {
        return vec![0x01, 0x00];
    }
    let be = x.to_be_bytes();
    let lead = be.iter().take(7).take_while(|&&b| b == 0).count();
    let mut out = Vec::with_capacity(9 - lead);
    out.push((8 - lead) as u8);
    out.extend_from_slice(&be[lead..]);
    out
}

/// SP 800-185 §2.3.1 right_encode.
fn right_encode(x: u64) -> Vec<u8> {
    if x == 0 {
        return vec![0x00, 0x01];
    }
    let be = x.to_be_bytes();
    let lead = be.iter().take(7).take_while(|&&b| b == 0).count();
    let mut out = Vec::with_capacity(9 - lead);
    out.extend_from_slice(&be[lead..]);
    out.push((8 - lead) as u8);
    out
}

/// cSHAKE256 with a function name, per SP 800-185.
fn cshake256(function: &[u8], customization: &[u8], input: &[u8], out: &mut [u8]) {
    let mut h = CoreWrapper::from_core(sha3::CShake256Core::new_with_function_name(
        function,
        customization,
    ));
    h.update(input);
    XofReader::read(&mut h.finalize_xof(), out);
}

/// Write a key file readable by nobody else.
///
/// Shared with [`crate::keys`], so every secret this node writes is written
/// one way.
///
/// The mode is set as the file is CREATED, not after: a file that is briefly
/// world-readable has been readable, and the window is exactly when a fresh
/// secret is being written into it.
pub fn write_private(path: &std::path::Path, secret: &[u8]) -> Result<(), Error> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file =
        std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
    file.write_all(secret)?;
    file.sync_all()?;
    Ok(())
}

/// How long a handshake may take before the peer is judged absent.
pub const TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};

    /// Two identities, and the handshake run between them over a real socket.
    /// Returns each end's outcome and the names they should have learned.
    fn run() -> (Result<Session, Error>, Result<Session, Error>, NodeId, NodeId) {
        run_with(Config::strict_pq(), Config::strict_pq())
    }

    /// The same, with each end holding its own policy — which is how a
    /// disagreement about profile, chain or scheme is put on the wire.
    fn run_with(
        dialer_cfg: Config,
        accepter_cfg: Config,
    ) -> (Result<Session, Error>, Result<Session, Error>, NodeId, NodeId) {
        let a = Identity::generate().expect("identity");
        let b = Identity::generate().expect("identity");
        let (an, bn) = (a.node(), b.node());

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let accepter = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().expect("accept");
            greet(&mut s, &accepter_cfg, &b, Role::Responder, None)
        });
        let mut near = TcpStream::connect(addr).expect("dial");
        let dialer = greet(&mut near, &dialer_cfg, &a, Role::Initiator, Some(bn));
        (dialer, accepter.join().expect("accepter"), an, bn)
    }

    #[test]
    fn sp_800_185_encodes_lengths_the_way_the_standard_says() {
        assert_eq!(left_encode(0), vec![0x01, 0x00]);
        assert_eq!(left_encode(1), vec![0x01, 0x01]);
        assert_eq!(left_encode(255), vec![0x01, 0xff]);
        assert_eq!(left_encode(256), vec![0x02, 0x01, 0x00]);
        assert_eq!(left_encode(384), vec![0x02, 0x01, 0x80]);
        assert_eq!(right_encode(0), vec![0x00, 0x01]);
        assert_eq!(right_encode(1), vec![0x01, 0x01]);
        assert_eq!(right_encode(384), vec![0x01, 0x80, 0x02]);
    }

    #[test]
    fn both_ends_prove_who_they_are_and_agree_on_a_key() {
        let (mine, theirs, an, bn) = run();
        let mine = mine.expect("the dialer completes");
        let theirs = theirs.expect("the accepter completes");
        assert_eq!(mine.peer, bn, "the dialer learned the accepter's real name");
        assert_eq!(theirs.peer, an, "and the accepter learned the dialer's");
        assert_eq!(
            mine.secret, theirs.secret,
            "one session, one key — everything after this depends on it"
        );
        assert_ne!(mine.secret, [0u8; AEAD_KEY_LEN]);
    }

    #[test]
    fn a_name_is_the_digest_of_the_key_and_nothing_else() {
        let a = Identity::generate().expect("identity");
        assert_eq!(a.node(), name(a.public()));
        let b = Identity::generate().expect("identity");
        assert_ne!(a.node(), b.node(), "two keys are two validators");
    }

    #[test]
    fn every_session_has_its_own_key() {
        // A recorded handshake cannot be replayed into a new session: the KEM
        // key is fresh per link, so the transcript is too.
        let (one, _, _, _) = run();
        let (two, _, _, _) = run();
        assert_ne!(one.expect("first").secret, two.expect("second").secret, "two links, two keys");
    }

    #[test]
    fn dialing_one_validator_and_reaching_another_is_refused() {
        let a = Identity::generate().expect("identity");
        let b = Identity::generate().expect("identity");
        let elsewhere = Identity::generate().expect("identity").node();

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().expect("accept");
            let _ = greet(&mut s, &Config::strict_pq(), &b, Role::Responder, None);
        });
        let mut near = TcpStream::connect(addr).expect("dial");
        let refused = greet(&mut near, &Config::strict_pq(), &a, Role::Initiator, Some(elsewhere))
            .expect_err("not who we dialed");
        assert!(matches!(refused, Error::WrongPeer { .. }), "{refused}");
    }

    #[test]
    fn a_node_that_reaches_itself_is_not_a_peer() {
        // A link home completes honestly — same key on both ends, so every
        // signature verifies and both sides derive the key. Only the identity
        // check stops it counting as a validator.
        let me = Identity::generate().expect("identity");
        let twin = Identity::from_seed(&me.seed);

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let far = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().expect("accept");
            greet(&mut s, &Config::strict_pq(), &twin, Role::Responder, None)
        });
        let mut near = TcpStream::connect(addr).expect("dial");
        let refused = greet(&mut near, &Config::strict_pq(), &me, Role::Initiator, None)
            .expect_err("a node is not its own peer");
        assert!(matches!(refused, Error::Reflected), "{refused}");
        assert!(matches!(far.join().expect("far"), Err(Error::Reflected)));
    }

    #[test]
    fn a_peer_that_does_not_hold_its_key_is_refused() {
        // The impostor sends a real validator's identity key and signs with a
        // key of its own. Every field is the right width, the transcript is
        // the right transcript, and the signature does not verify.
        let victim = Identity::generate().expect("identity");
        let impostor = Identity::generate().expect("identity");
        let honest = Identity::generate().expect("identity");

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let victim_key = victim.public().to_vec();
        let victim_node = victim.node();

        std::thread::spawn(move || {
            let mut s = TcpStream::connect(addr).expect("dial");
            let (_secret, kem_key) = Ephemeral::generate(Kem::MLKEM768).expect("kem");
            let cfg = Config::strict_pq();
            let mut init = Init {
                version: PROTOCOL_VERSION,
                profile: cfg.profile,
                chain: cfg.chain,
                kem: cfg.kem,
                node: victim_node,
                identity_key: victim_key,
                kem_key,
                signature: Vec::new(),
            };
            init.signature = impostor
                .sign(&init.prefix(), CONTEXT_INITIATOR)
                .expect("the impostor can sign, just not as the victim");
            let _ = write_frame(&mut s, &init.canonical());
            // Held open until the responder lets go. Hanging up after a fixed
            // wait races the refusal under test: a responder slower than the
            // wait writes into a closed socket and reports a broken pipe.
            let _ = std::io::copy(&mut s, &mut std::io::sink());
        });

        let (mut s, _) = listener.accept().expect("accept");
        let refused = greet(&mut s, &Config::strict_pq(), &honest, Role::Responder, None)
            .expect_err("a forged identity must not pass");
        assert!(matches!(refused, Error::Unproven), "{refused}");
    }

    #[test]
    fn a_key_that_does_not_derive_the_claimed_name_is_refused() {
        // This peer holds the key it presents — the signature verifies — and
        // it calls itself somebody else. Nothing before the identity binding
        // catches that.
        let liar = Identity::generate().expect("identity");
        let honest = Identity::generate().expect("identity");
        let borrowed = Identity::generate().expect("identity").node();

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            let mut s = TcpStream::connect(addr).expect("dial");
            let (_secret, kem_key) = Ephemeral::generate(Kem::MLKEM768).expect("kem");
            let cfg = Config::strict_pq();
            let mut init = Init {
                version: PROTOCOL_VERSION,
                profile: cfg.profile,
                chain: cfg.chain,
                kem: cfg.kem,
                node: borrowed,
                identity_key: liar.public().to_vec(),
                kem_key,
                signature: Vec::new(),
            };
            init.signature = liar.sign(&init.prefix(), CONTEXT_INITIATOR).expect("sign");
            let _ = write_frame(&mut s, &init.canonical());
            // Held open until the responder lets go. Hanging up after a fixed
            // wait races the refusal under test: a responder slower than the
            // wait writes into a closed socket and reports a broken pipe.
            let _ = std::io::copy(&mut s, &mut std::io::sink());
        });

        let (mut s, _) = listener.accept().expect("accept");
        let refused = greet(&mut s, &Config::strict_pq(), &honest, Role::Responder, None)
            .expect_err("a borrowed name must not pass");
        assert!(matches!(refused, Error::Impersonation { .. }), "{refused}");
    }

    #[test]
    fn a_peer_on_another_profile_or_chain_is_refused() {
        let mut other_profile = Config::strict_pq();
        other_profile.profile = Profile::PERMISSIVE;
        other_profile.forbid_classical_kem = false;
        let (_, accepter, _, _) = run_with(other_profile, Config::strict_pq());
        assert!(matches!(accepter, Err(Error::ProfileMismatch { .. })), "{accepter:?}");

        let mut other_chain = Config::strict_pq();
        other_chain.chain = [7u8; CHAIN_ID_LEN];
        let (_, accepter, _, _) = run_with(other_chain, Config::strict_pq());
        assert!(matches!(accepter, Err(Error::ChainMismatch)), "{accepter:?}");
    }

    #[test]
    fn a_peer_offering_another_scheme_is_refused_before_the_signature() {
        let mut wider = Config::strict_pq();
        wider.kem = Kem::MLKEM1024;
        let (_, accepter, _, _) = run_with(wider, Config::strict_pq());
        assert!(matches!(accepter, Err(Error::KemScheme { .. })), "{accepter:?}");
    }

    #[test]
    fn ml_kem_1024_is_a_handshake_this_node_can_run() {
        let mut wide = Config::strict_pq();
        wide.kem = Kem::MLKEM1024;
        let (mine, theirs, an, bn) = run_with(wide, wide);
        let mine = mine.expect("the dialer completes");
        let theirs = theirs.expect("the accepter completes");
        assert_eq!(mine.peer, bn);
        assert_eq!(theirs.peer, an);
        assert_eq!(mine.secret, theirs.secret);
    }

    #[test]
    fn a_responder_may_not_answer_with_a_weaker_scheme() {
        // The downgrade the wire allows and the rule forbids: an initiator
        // offers category 5 and is answered in category 3.
        let mut wide = Config::strict_pq();
        wide.kem = Kem::MLKEM1024;
        let init = Init {
            version: PROTOCOL_VERSION,
            profile: wide.profile,
            chain: wide.chain,
            kem: Kem::MLKEM1024,
            node: [1u8; NODE_ID_LEN],
            identity_key: vec![0u8; IDENTITY_KEY_LEN],
            kem_key: vec![0u8; 1568],
            signature: vec![0u8; IDENTITY_SIGNATURE_LEN],
        };
        let resp = Resp {
            version: PROTOCOL_VERSION,
            profile: wide.profile,
            chain: wide.chain,
            kem: Kem::MLKEM768,
            node: [2u8; NODE_ID_LEN],
            identity_key: vec![0u8; IDENTITY_KEY_LEN],
            kem_ciphertext: vec![0u8; 1088],
            signature: vec![0u8; IDENTITY_SIGNATURE_LEN],
        };
        assert!(matches!(check_resp(&wide, &init, &resp), Err(Error::KemScheme { .. })));
    }

    #[test]
    fn classical_key_exchange_is_refused_on_both_sides_of_the_link() {
        let mut classical = Config::strict_pq();
        classical.kem = Kem::X25519_UNSAFE;
        assert!(matches!(classical.check(), Err(Error::ClassicalKem(_))));

        let mut none = Config::strict_pq();
        none.profile = Profile::NONE;
        assert!(matches!(none.check(), Err(Error::ProfileMismatch { .. })));

        // And a peer that puts the marker on the wire, against a strict
        // config, is refused for the marker rather than for the width.
        let cfg = Config::strict_pq();
        let init = Init {
            version: PROTOCOL_VERSION,
            profile: cfg.profile,
            chain: cfg.chain,
            kem: Kem::X25519_UNSAFE,
            node: [1u8; NODE_ID_LEN],
            identity_key: vec![0u8; IDENTITY_KEY_LEN],
            kem_key: vec![0u8; 32],
            signature: vec![0u8; IDENTITY_SIGNATURE_LEN],
        };
        assert!(matches!(check_init(&cfg, &init), Err(Error::ClassicalKem(_))));
    }

    #[test]
    fn a_protocol_version_this_node_does_not_speak_stops_the_handshake() {
        let cfg = Config::strict_pq();
        let init = Init {
            version: 0x02,
            profile: cfg.profile,
            chain: cfg.chain,
            kem: cfg.kem,
            node: [1u8; NODE_ID_LEN],
            identity_key: vec![0u8; IDENTITY_KEY_LEN],
            kem_key: vec![0u8; 1184],
            signature: vec![0u8; IDENTITY_SIGNATURE_LEN],
        };
        assert!(matches!(check_init(&cfg, &init), Err(Error::BadVersion(2))));
    }

    #[test]
    fn the_zero_name_is_nobody() {
        let cfg = Config::strict_pq();
        let init = Init {
            version: PROTOCOL_VERSION,
            profile: cfg.profile,
            chain: cfg.chain,
            kem: cfg.kem,
            node: [0u8; NODE_ID_LEN],
            identity_key: vec![0u8; IDENTITY_KEY_LEN],
            kem_key: vec![0u8; 1184],
            signature: vec![0u8; IDENTITY_SIGNATURE_LEN],
        };
        assert!(matches!(check_init(&cfg, &init), Err(Error::NodeIdZero)));
    }

    #[test]
    fn a_message_that_parses_two_ways_parses_no_ways() {
        let cfg = Config::strict_pq();
        let init = Init {
            version: PROTOCOL_VERSION,
            profile: cfg.profile,
            chain: cfg.chain,
            kem: cfg.kem,
            node: [3u8; NODE_ID_LEN],
            identity_key: vec![9u8; IDENTITY_KEY_LEN],
            kem_key: vec![8u8; 1184],
            signature: vec![7u8; IDENTITY_SIGNATURE_LEN],
        };
        let good = init.canonical();
        assert_eq!(Init::decode(&good, cfg.kem).expect("decodes"), init);

        let mut trailing = good.clone();
        trailing.push(0);
        assert!(matches!(Init::decode(&trailing, cfg.kem), Err(Error::Malformed(_))));

        assert!(matches!(Init::decode(&good[..good.len() - 1], cfg.kem), Err(Error::Malformed(_))));

        // A field that claims more than the message holds is refused before
        // the claim is used to size anything.
        let mut greedy = good.clone();
        greedy[55..59].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(matches!(Init::decode(&greedy, cfg.kem), Err(Error::Malformed(_))));
    }

    #[test]
    fn an_announced_size_is_never_an_allocation() {
        let announced = (FRAME_MAX as u32 + 1).to_be_bytes();
        let mut wire: &[u8] = &announced;
        assert!(matches!(read_frame(&mut wire), Err(Error::Oversize(_))));
        assert!(matches!(
            write_frame(&mut Vec::new(), &vec![0u8; FRAME_MAX + 1]),
            Err(Error::Oversize(_))
        ));
    }

    #[test]
    fn a_frame_round_trips() {
        let mut wire = Vec::new();
        write_frame(&mut wire, b"payload").expect("write");
        assert_eq!(wire, b"\x00\x00\x00\x07payload");
        assert_eq!(read_frame(&mut &wire[..]).expect("read"), b"payload");
    }

    #[test]
    fn the_session_key_moves_when_anything_it_is_bound_to_moves() {
        let hash = hash_transcript(b"one");
        assert_ne!(hash, hash_transcript(b"two"));
        let key = derive_aead_key(Kem::MLKEM768, &[1u8; 32], &hash);
        assert_ne!(key, derive_aead_key(Kem::MLKEM768, &[2u8; 32], &hash));
        assert_ne!(key, derive_aead_key(Kem::MLKEM768, &[1u8; 32], &hash_transcript(b"two")));
        assert_ne!(key, derive_aead_key(Kem::MLKEM1024, &[1u8; 32], &hash));
    }

    #[test]
    fn a_transcript_separates_fields_that_could_run_together() {
        // Without the length prefix, moving a byte from the identity key to
        // the session key would leave the signed bytes unchanged.
        let cfg = Config::strict_pq();
        let mut a = Init {
            version: PROTOCOL_VERSION,
            profile: cfg.profile,
            chain: cfg.chain,
            kem: cfg.kem,
            node: [1u8; NODE_ID_LEN],
            identity_key: b"ab".to_vec(),
            kem_key: b"c".to_vec(),
            signature: Vec::new(),
        };
        let one = a.prefix();
        a.identity_key = b"a".to_vec();
        a.kem_key = b"bc".to_vec();
        assert_ne!(one, a.prefix());
    }

    #[test]
    fn a_signature_made_under_one_role_does_not_verify_as_the_other() {
        let me = Identity::generate().expect("identity");
        let sig = me.sign(b"transcript", CONTEXT_INITIATOR).expect("sign");
        assert!(verify(me.public(), b"transcript", CONTEXT_INITIATOR, &sig).is_ok());
        assert!(matches!(
            verify(me.public(), b"transcript", CONTEXT_RESPONDER, &sig),
            Err(Error::Unproven)
        ));
    }

    #[test]
    fn an_identity_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("lux-pq-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        let path = dir.join("identity.key");
        let first = Identity::open(&path).expect("made");
        let second = Identity::open(&path).expect("loaded");
        assert_eq!(
            first.node(),
            second.node(),
            "a validator that renamed itself on restart is a new validator"
        );
        assert_eq!(first.public(), second.public());
        assert_eq!(std::fs::read(&path).expect("seed").len(), 32);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
