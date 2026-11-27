// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! The Rust handshake against the Go node's own bytes.
//!
//! `tests/vectors/pq_handshake.json` was written by `mesh/peer/handshake.go`,
//! through the generator in `tests/vectors/gen/` — sixteen complete handshakes
//! keyed by seed, so every value in the file is a function of a few bytes and
//! nothing in it can drift without the seeds changing. Each one is replayed
//! here from the seeds up: the two identities, the session keypair, both
//! transcript prefixes, both signatures, both wire messages, the transcript
//! commitment and the session key.
//!
//! Reading two implementations side by side does not establish that they agree.
//! This does, or fails.
//!
//! The refusals are proved the same way, from the same file: a signature made
//! under the context string the Rust node used BEFORE it spoke this protocol
//! is in every vector, and it must not verify. Neither may an initiator's
//! signature presented as a responder's, nor any of them once a byte moves.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

use alloy_primitives::hex;
use lux_node::pq::{
    self, bind_aead_transcript, derive_aead_key, hash_transcript, name, verify, Config, Identity,
    Init, Kem, Profile, Resp, Role, CONTEXT_INITIATOR, CONTEXT_RESPONDER,
};
use ml_kem::{ml_kem_1024, ml_kem_768, Decapsulate, KeyExport};
use serde_json::Value;

/// One handshake, decoded out of the file.
struct Vector {
    name: String,
    version: u8,
    profile: Profile,
    chain: [u8; 32],
    kem: Kem,
    init_seed: [u8; 32],
    init_key: Vec<u8>,
    init_node: [u8; 20],
    kem_seed: Vec<u8>,
    kem_key: Vec<u8>,
    init_prefix: Vec<u8>,
    init_sig: Vec<u8>,
    init_canonical: Vec<u8>,
    resp_seed: [u8; 32],
    resp_key: Vec<u8>,
    resp_node: [u8; 20],
    encap_seed: [u8; 32],
    kem_ciphertext: Vec<u8>,
    resp_prefix: Vec<u8>,
    resp_sig: Vec<u8>,
    resp_canonical: Vec<u8>,
    shared: [u8; 32],
    transcript_hash: [u8; 48],
    aead_key: [u8; 32],
    legacy_sig: Vec<u8>,
}

fn bytes(v: &Value, path: &[&str]) -> Vec<u8> {
    let mut at = v;
    for step in path {
        at = at.get(step).unwrap_or_else(|| panic!("{path:?} is not in the vector file"));
    }
    hex::decode(at.as_str().unwrap_or_else(|| panic!("{path:?} is not hex")))
        .unwrap_or_else(|_| panic!("{path:?} does not decode"))
}

fn fixed<const N: usize>(v: &Value, path: &[&str]) -> [u8; N] {
    let raw = bytes(v, path);
    raw.as_slice()
        .try_into()
        .unwrap_or_else(|_| panic!("{path:?} is {} bytes, want {N}", raw.len()))
}

fn byte(v: &Value, key: &str) -> u8 {
    v.get(key).and_then(Value::as_u64).unwrap_or_else(|| panic!("{key} is not a number")) as u8
}

/// The file, and the context it says the retired signatures were made under.
fn load() -> (Vec<Vector>, Vec<u8>) {
    let raw = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/vectors/pq_handshake.json"
    ))
    .expect("the conformance vectors are part of the tree");
    let file: Value = serde_json::from_str(&raw).expect("the vector file is JSON");

    // The file names the contexts it signed under, and this build has its own
    // idea of them. If those two ever part, everything below would still pass
    // while agreeing about nothing.
    assert_eq!(
        file["sigContextInitiator"].as_str().expect("initiator context"),
        std::str::from_utf8(CONTEXT_INITIATOR).expect("utf8")
    );
    assert_eq!(
        file["sigContextResponder"].as_str().expect("responder context"),
        std::str::from_utf8(CONTEXT_RESPONDER).expect("utf8")
    );
    let legacy = file["legacyContext"].as_str().expect("legacy context").as_bytes().to_vec();

    let vectors = file["vectors"]
        .as_array()
        .expect("the file holds an array of vectors")
        .iter()
        .map(|v| Vector {
            name: v["name"].as_str().expect("name").to_string(),
            version: byte(v, "protocolVersion"),
            profile: Profile(byte(v, "profile")),
            chain: fixed(v, &["chainId"]),
            kem: Kem(byte(v, "kemScheme")),
            init_seed: fixed(v, &["init", "mldsaSeed"]),
            init_key: bytes(v, &["init", "mldsaPub"]),
            init_node: fixed(v, &["init", "nodeId"]),
            kem_seed: bytes(v, &["init", "kemSeed"]),
            kem_key: bytes(v, &["init", "kemPub"]),
            init_prefix: bytes(v, &["init", "transcriptPrefix"]),
            init_sig: bytes(v, &["init", "sig"]),
            init_canonical: bytes(v, &["init", "canonical"]),
            resp_seed: fixed(v, &["resp", "mldsaSeed"]),
            resp_key: bytes(v, &["resp", "mldsaPub"]),
            resp_node: fixed(v, &["resp", "nodeId"]),
            encap_seed: fixed(v, &["resp", "encapSeed"]),
            kem_ciphertext: bytes(v, &["resp", "kemCiphertext"]),
            resp_prefix: bytes(v, &["resp", "transcriptPrefix"]),
            resp_sig: bytes(v, &["resp", "sig"]),
            resp_canonical: bytes(v, &["resp", "canonical"]),
            shared: fixed(v, &["sharedSecret"]),
            transcript_hash: fixed(v, &["transcriptHash"]),
            aead_key: fixed(v, &["aeadKey"]),
            legacy_sig: bytes(v, &["legacyContextSig"]),
        })
        .collect::<Vec<_>>();

    assert!(
        vectors.len() >= 12,
        "the file carries {} handshakes; twelve is the floor",
        vectors.len()
    );
    (vectors, legacy)
}

/// The INIT and RESP a vector describes, rebuilt from its parts.
fn messages(v: &Vector) -> (Init, Resp) {
    (
        Init {
            version: v.version,
            profile: v.profile,
            chain: v.chain,
            kem: v.kem,
            node: v.init_node,
            identity_key: v.init_key.clone(),
            kem_key: v.kem_key.clone(),
            signature: v.init_sig.clone(),
        },
        Resp {
            version: v.version,
            profile: v.profile,
            chain: v.chain,
            kem: v.kem,
            node: v.resp_node,
            identity_key: v.resp_key.clone(),
            kem_ciphertext: v.kem_ciphertext.clone(),
            signature: v.resp_sig.clone(),
        },
    )
}

/// Derive the session keypair a vector's seed names, and hand back the
/// encapsulation key as it goes on the wire.
fn session_keypair(v: &Vector) -> (Ephemeral, Vec<u8>) {
    match v.kem {
        Kem::MLKEM768 => {
            let seed = v.kem_seed.as_slice().try_into().expect("64-byte ML-KEM seed");
            let dk = ml_kem_768::DecapsulationKey::from_seed(seed);
            let ek = dk.encapsulation_key().to_bytes().to_vec();
            (Ephemeral::K768(dk), ek)
        }
        Kem::MLKEM1024 => {
            let seed = v.kem_seed.as_slice().try_into().expect("64-byte ML-KEM seed");
            let dk = ml_kem_1024::DecapsulationKey::from_seed(seed);
            let ek = dk.encapsulation_key().to_bytes().to_vec();
            (Ephemeral::K1024(dk), ek)
        }
        other => panic!("the vector names {other:?}, which this node does not run"),
    }
}

enum Ephemeral {
    K768(ml_kem_768::DecapsulationKey),
    K1024(ml_kem_1024::DecapsulationKey),
}

impl Ephemeral {
    fn decapsulate(&self, ciphertext: &[u8]) -> [u8; 32] {
        match self {
            Ephemeral::K768(dk) => dk.decapsulate_slice(ciphertext).expect("width").into(),
            Ephemeral::K1024(dk) => dk.decapsulate_slice(ciphertext).expect("width").into(),
        }
    }

    /// Encapsulate under the vector's own randomness, which is the only way
    /// the ciphertext can be compared to a fixed one.
    fn encapsulate_deterministic(&self, m: &[u8; 32]) -> (Vec<u8>, [u8; 32]) {
        match self {
            Ephemeral::K768(dk) => {
                let (ct, ss) = dk.encapsulation_key().encapsulate_deterministic(&(*m).into());
                (ct.to_vec(), ss.into())
            }
            Ephemeral::K1024(dk) => {
                let (ct, ss) = dk.encapsulation_key().encapsulate_deterministic(&(*m).into());
                (ct.to_vec(), ss.into())
            }
        }
    }
}

#[test]
fn every_byte_the_go_node_produced_is_reproduced_here() {
    let (vectors, _) = load();
    for v in &vectors {
        let at = &v.name;

        // 1. The identities. A seed names a key, a key names a validator, and
        //    the two implementations have to agree about both or nothing else
        //    matters.
        let initiator = Identity::from_seed(&v.init_seed.into());
        assert_eq!(initiator.public(), v.init_key, "{at}: initiator key");
        assert_eq!(initiator.node(), v.init_node, "{at}: initiator name");
        assert_eq!(name(&v.init_key), v.init_node, "{at}: name is the key");

        let responder = Identity::from_seed(&v.resp_seed.into());
        assert_eq!(responder.public(), v.resp_key, "{at}: responder key");
        assert_eq!(responder.node(), v.resp_node, "{at}: responder name");

        // 2. The session keypair, from the same FIPS 203 seed.
        let (ephemeral, kem_key) = session_keypair(v);
        assert_eq!(kem_key, v.kem_key, "{at}: session key");

        let (init, resp) = messages(v);

        // 3. What the initiator signs, what it signed, and what it sends.
        assert_eq!(init.prefix(), v.init_prefix, "{at}: INIT transcript prefix");
        assert_eq!(
            initiator.sign(&init.prefix(), CONTEXT_INITIATOR).expect("sign"),
            v.init_sig,
            "{at}: INIT signature"
        );
        assert_eq!(init.canonical(), v.init_canonical, "{at}: INIT on the wire");
        assert_eq!(
            Init::decode(&v.init_canonical, v.kem).expect("INIT decodes"),
            init,
            "{at}: INIT reads back to what was written"
        );
        verify(&v.init_key, &v.init_prefix, CONTEXT_INITIATOR, &v.init_sig)
            .unwrap_or_else(|e| panic!("{at}: INIT signature does not verify: {e}"));

        // 4. The encapsulation, both ways: the same ciphertext out of the same
        //    randomness, and the same secret back out of the ciphertext.
        let (ct, ss) = ephemeral.encapsulate_deterministic(&v.encap_seed);
        assert_eq!(ct, v.kem_ciphertext, "{at}: ciphertext");
        assert_eq!(ss, v.shared, "{at}: encapsulated secret");
        assert_eq!(ephemeral.decapsulate(&v.kem_ciphertext), v.shared, "{at}: decapsulated secret");

        // 5. The responder's half, which commits to the initiator's whole
        //    message before it commits to its own fields.
        assert_eq!(resp.prefix(&init), v.resp_prefix, "{at}: RESP transcript prefix");
        assert_eq!(
            responder.sign(&resp.prefix(&init), CONTEXT_RESPONDER).expect("sign"),
            v.resp_sig,
            "{at}: RESP signature"
        );
        assert_eq!(resp.canonical(), v.resp_canonical, "{at}: RESP on the wire");
        assert_eq!(
            Resp::decode(&v.resp_canonical, v.kem).expect("RESP decodes"),
            resp,
            "{at}: RESP reads back to what was written"
        );
        verify(&v.resp_key, &v.resp_prefix, CONTEXT_RESPONDER, &v.resp_sig)
            .unwrap_or_else(|e| panic!("{at}: RESP signature does not verify: {e}"));

        // 6. And the key the whole exchange exists to produce.
        let hash = hash_transcript(&bind_aead_transcript(&init, &resp));
        assert_eq!(hash, v.transcript_hash, "{at}: transcript commitment");
        assert_eq!(derive_aead_key(v.kem, &v.shared, &hash), v.aead_key, "{at}: session key");
    }
    eprintln!("{} handshakes reproduced byte for byte", vectors.len());
}

#[test]
fn a_signature_under_the_context_this_node_used_to_sign_with_is_refused() {
    // The Rust node signed its transcript under "lux-validator-link-v1" before
    // it spoke this protocol. The context is part of the message FIPS 204
    // signs, so that signature is over different bytes — and every vector
    // carries one, made by the same key over the same prefix, to prove it is
    // refused rather than to assert it.
    let (vectors, legacy) = load();
    assert_eq!(legacy, b"lux-validator-link-v1");
    for v in &vectors {
        let at = &v.name;
        assert!(
            matches!(
                verify(&v.init_key, &v.init_prefix, CONTEXT_INITIATOR, &v.legacy_sig),
                Err(pq::Error::Unproven)
            ),
            "{at}: a signature under the old context was accepted"
        );
        // It is a real signature — under the context it was made for.
        verify(&v.init_key, &v.init_prefix, &legacy, &v.legacy_sig)
            .unwrap_or_else(|e| panic!("{at}: the legacy signature is not even valid: {e}"));
    }
}

#[test]
fn an_initiators_signature_is_not_a_responders() {
    let (vectors, _) = load();
    for v in &vectors {
        let at = &v.name;
        assert!(
            matches!(
                verify(&v.init_key, &v.init_prefix, CONTEXT_RESPONDER, &v.init_sig),
                Err(pq::Error::Unproven)
            ),
            "{at}: an initiator's signature passed as a responder's"
        );
        assert!(
            matches!(
                verify(&v.resp_key, &v.resp_prefix, CONTEXT_INITIATOR, &v.resp_sig),
                Err(pq::Error::Unproven)
            ),
            "{at}: a responder's signature passed as an initiator's"
        );
    }
}

#[test]
fn one_byte_out_of_place_and_nothing_verifies() {
    let (vectors, _) = load();
    for v in &vectors {
        let at = &v.name;

        // A byte moved anywhere in what was signed: the fixed head, the
        // identity key, the session key.
        for at_byte in [0usize, 1, 2, 40, 55, 60, 1000, 2000, 3100, 3198] {
            let mut tampered = v.init_prefix.clone();
            tampered[at_byte] ^= 0x01;
            assert!(
                matches!(
                    verify(&v.init_key, &tampered, CONTEXT_INITIATOR, &v.init_sig),
                    Err(pq::Error::Unproven)
                ),
                "{at}: byte {at_byte} of the INIT prefix moved and the signature still verified"
            );
        }

        // A byte moved in the signature itself.
        for at_byte in [0usize, 32, 1000, 3308] {
            let mut tampered = v.init_sig.clone();
            tampered[at_byte] ^= 0x01;
            assert!(
                verify(&v.init_key, &v.init_prefix, CONTEXT_INITIATOR, &tampered).is_err(),
                "{at}: byte {at_byte} of the signature moved and it still verified"
            );
        }

        // A byte moved in the bound transcript: a different commitment, and a
        // different session key. This is the one that makes a downgrade fail
        // closed instead of quietly succeeding.
        let (init, resp) = messages(v);
        let bound = bind_aead_transcript(&init, &resp);
        for at_byte in [0usize, 1, 6512, 12_000, bound.len() - 1] {
            let mut tampered = bound.clone();
            tampered[at_byte] ^= 0x01;
            let hash = hash_transcript(&tampered);
            assert_ne!(hash, v.transcript_hash, "{at}: byte {at_byte} left the commitment unmoved");
            assert_ne!(
                derive_aead_key(v.kem, &v.shared, &hash),
                v.aead_key,
                "{at}: byte {at_byte} left the session key unmoved"
            );
        }

        // And a KEM secret that is off by one bit is a different key, even
        // over the identical transcript.
        let mut other = v.shared;
        other[0] ^= 0x01;
        assert_ne!(
            derive_aead_key(v.kem, &other, &v.transcript_hash),
            v.aead_key,
            "{at}: the session key does not depend on the KEM secret"
        );
    }
}

#[test]
fn a_message_the_go_node_did_not_write_does_not_decode_as_one() {
    let (vectors, _) = load();
    for v in &vectors {
        let at = &v.name;

        // A scheme byte the reader did not offer stops the decode before any
        // length in the message is trusted.
        let other = if v.kem == Kem::MLKEM768 { Kem::MLKEM1024 } else { Kem::MLKEM768 };
        assert!(
            matches!(Init::decode(&v.init_canonical, other), Err(pq::Error::KemScheme { .. })),
            "{at}: an INIT decoded under a scheme it does not name"
        );

        // A tail nobody wrote.
        let mut trailing = v.init_canonical.clone();
        trailing.push(0);
        assert!(Init::decode(&trailing, v.kem).is_err(), "{at}: trailing bytes accepted");

        // A message cut short.
        assert!(
            Init::decode(&v.init_canonical[..v.init_canonical.len() - 1], v.kem).is_err(),
            "{at}: a truncated INIT decoded"
        );

        // A RESP is not an INIT, whatever the widths happen to be.
        let decoded = Init::decode(&v.resp_canonical, v.kem);
        assert!(
            decoded.is_err() || decoded.expect("checked").kem_key != v.kem_key,
            "{at}: a RESP read as the INIT it answers"
        );
    }
}

#[test]
fn this_node_answers_the_go_node_over_a_socket() {
    // The whole responder path, driven by bytes the Go node wrote: its INIT
    // goes on a real socket, this node's RESP comes back, and the far end
    // decapsulates with the vector's own session secret. Both ends have to
    // arrive at one key without ever having exchanged one.
    let (vectors, _) = load();
    for v in &vectors {
        let at = &v.name;
        let cfg = Config {
            profile: v.profile,
            chain: v.chain,
            kem: v.kem,
            forbid_classical_kem: v.profile.is_strict(),
        };
        let responder = Identity::from_seed(&v.resp_seed.into());

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let served = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().expect("accept");
            pq::greet(&mut s, &cfg, &responder, Role::Responder, None)
        });

        let mut wire = TcpStream::connect(addr).expect("dial");
        wire.write_all(&(v.init_canonical.len() as u32).to_be_bytes()).expect("length");
        wire.write_all(&v.init_canonical).expect("INIT");

        let mut header = [0u8; 4];
        wire.read_exact(&mut header).expect("RESP length");
        let mut body = vec![0u8; u32::from_be_bytes(header) as usize];
        wire.read_exact(&mut body).expect("RESP body");

        let session = served
            .join()
            .expect("responder")
            .unwrap_or_else(|e| panic!("{at}: this node refused the Go node's INIT: {e}"));
        assert_eq!(session.peer, v.init_node, "{at}: the responder learned the wrong name");
        assert_eq!(session.key, v.init_key, "{at}: the responder kept the wrong key");

        // Now finish the initiator's side against that RESP, exactly as the
        // initiator would: verify what it signed, decapsulate, bind, derive.
        let (init, _) = messages(v);
        let resp = Resp::decode(&body, v.kem).unwrap_or_else(|e| panic!("{at}: RESP: {e}"));
        assert_eq!(resp.node, v.resp_node, "{at}: RESP names the wrong node");
        assert_eq!(resp.kem, v.kem, "{at}: RESP answered with another scheme");
        verify(&resp.identity_key, &resp.prefix(&init), CONTEXT_RESPONDER, &resp.signature)
            .unwrap_or_else(|e| panic!("{at}: this node's own RESP does not verify: {e}"));

        let (ephemeral, _) = session_keypair(v);
        let shared = ephemeral.decapsulate(&resp.kem_ciphertext);
        let hash = hash_transcript(&bind_aead_transcript(&init, &resp));
        assert_eq!(
            derive_aead_key(v.kem, &shared, &hash),
            session.secret,
            "{at}: the two ends derived different keys"
        );
    }
}

#[test]
fn this_node_refuses_a_go_init_that_was_tampered_with_in_flight() {
    // Same socket, one byte moved. The responder has to stop, and stop for the
    // reason the byte broke rather than for some accident of parsing.
    let (vectors, _) = load();
    let v = &vectors[0];
    let cfg = Config { profile: v.profile, chain: v.chain, kem: v.kem, forbid_classical_kem: true };

    // A byte inside the signed prefix: the signature no longer covers it.
    for at_byte in [2000usize, 3000] {
        let responder = Identity::from_seed(&v.resp_seed.into());
        let mut wire_bytes = v.init_canonical.clone();
        wire_bytes[at_byte] ^= 0x01;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let served = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().expect("accept");
            pq::greet(&mut s, &cfg, &responder, Role::Responder, None)
        });
        let mut wire = TcpStream::connect(addr).expect("dial");
        let _ = wire.write_all(&(wire_bytes.len() as u32).to_be_bytes());
        let _ = wire.write_all(&wire_bytes);
        let outcome = served.join().expect("responder");
        assert!(
            matches!(outcome, Err(pq::Error::Unproven)),
            "byte {at_byte} moved and the handshake did not stop at the signature: {outcome:?}"
        );
    }

    // The chain commitment: refused before any lattice arithmetic runs.
    let responder = Identity::from_seed(&v.resp_seed.into());
    let mut wire_bytes = v.init_canonical.clone();
    wire_bytes[2] ^= 0x01;
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let served = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().expect("accept");
        pq::greet(&mut s, &cfg, &responder, Role::Responder, None)
    });
    let mut wire = TcpStream::connect(addr).expect("dial");
    let _ = wire.write_all(&(wire_bytes.len() as u32).to_be_bytes());
    let _ = wire.write_all(&wire_bytes);
    assert!(
        matches!(served.join().expect("responder"), Err(pq::Error::ChainMismatch)),
        "a peer on another chain was admitted"
    );
}
