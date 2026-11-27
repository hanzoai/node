// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! The weighted validator set, read from the chain that decides it.
//!
//! `validator_set_root` is the one field of a vote a validator cannot derive
//! from the block it is voting on: it commits to the set in force at the
//! block's P-chain epoch, and that set is the P-chain's answer, not the
//! genesis's. The two differ — a genesis that DECLARES `weight: 1000000000000`
//! can produce an effective weight of `7500000000`, because allocations
//! contribute stake as well. Signing over the declared number produces a
//! signature every other node refuses.
//!
//! So the set is asked for rather than assumed. luxd serves it at
//!
//! ```text
//! GET /v1/chain/<P-chain id>/ops/validators/at?height=<epoch>
//! ```
//!
//! ROUTE SHAPE, because it is not the one the discovery document advertises:
//! every chain's handlers live under `/v1/chain/<alias-or-id>/<endpoint>`, and
//! the P-chain is reachable only by its ID — `/v1/chain/P` is not routed. The
//! platformvm's JSON-RPC surface (`platform.getCurrentValidators`) is GONE in
//! luxd 1.36.x; `/ops` is what remains, and it is a plain REST surface with an
//! OpenAPI document at `/ops/.well-known/openapi.json`.
//!
//! THE KEY IS UNCOMPRESSED. The endpoint returns 96 bytes — blst's
//! serialisation, not the 48-byte compressed form — and that is what
//! `hashValidatorSet` hashes. Both the length prefix and the bytes are in the
//! preimage, so hashing the compressed key gives a different root for the same
//! set.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use lux_consensus::cert::NodeId;
use sha2::{Digest, Sha256};

/// The P-chain, as this build routes it.
pub const P_CHAIN: &str = "11111111111111111111111111111111P";

/// How long to wait on luxd's HTTP port.
const TIMEOUT: Duration = Duration::from_secs(10);

/// The most bytes to read back. A validator set of any plausible size is far
/// under this; a larger answer is a peer that is not luxd.
const BODY_MAX: usize = 4 * 1024 * 1024;

/// One validator, as the set commits to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub node: NodeId,
    pub weight: u64,
    /// The BLS public key, UNCOMPRESSED — 96 bytes.
    pub key: Vec<u8>,
}

#[derive(Debug)]
pub enum Error {
    Io(String),
    Malformed(String),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Io(w) => write!(f, "{w}"),
            Error::Malformed(w) => write!(f, "malformed: {w}"),
        }
    }
}

impl std::error::Error for Error {}

/// The set in force at `height`, as luxd computes it.
pub fn at(http: SocketAddr, height: u64) -> Result<Vec<Member>, Error> {
    let path = format!("/v1/chain/{P_CHAIN}/ops/validators/at?height={height}");
    let body = get(http, &path)?;
    parse(&body)
}

/// Read a validator set out of what the endpoint answered.
pub fn parse(body: &[u8]) -> Result<Vec<Member>, Error> {
    let doc: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| Error::Malformed(format!("not JSON: {e}")))?;
    let entries = doc
        .as_object()
        .ok_or_else(|| Error::Malformed("the set is an object keyed by node id".into()))?;

    let mut set = Vec::with_capacity(entries.len());
    for (name, v) in entries {
        let node = node_id(name)?;
        // A weight is a STRING in this document, because it does not fit a
        // JSON number safely. Reading it as one silently loses validators.
        let weight = v["weight"]
            .as_str()
            .ok_or_else(|| Error::Malformed(format!("{name}: weight is a string")))?
            .parse::<u64>()
            .map_err(|e| Error::Malformed(format!("{name}: weight: {e}")))?;
        let key = v["publicKey"]
            .as_str()
            .ok_or_else(|| Error::Malformed(format!("{name}: no publicKey")))?;
        let key = alloy_primitives::hex::decode(key.trim_start_matches("0x"))
            .map_err(|e| Error::Malformed(format!("{name}: publicKey: {e}")))?;
        // 96 and not 48: a compressed key here would mean the endpoint changed
        // shape, and hashing it would produce a root nobody else has.
        if key.len() != 96 {
            return Err(Error::Malformed(format!(
                "{name}: publicKey is {} bytes, expected the uncompressed 96",
                key.len()
            )));
        }
        set.push(Member { node, weight, key });
    }
    Ok(set)
}

/// The commitment to a set — Go's `hashValidatorSet`, byte for byte.
///
/// SHA-256 over validators sorted by RAW node id, each written as
/// `nodeID || weight(u64 BE) || len(key)(u64 BE) || key`. An empty set commits
/// to zero, not to the hash of nothing.
pub fn root(set: &[Member]) -> [u8; 32] {
    if set.is_empty() {
        return [0u8; 32];
    }
    // Sorted, and de-duplicated by node: two entries for one validator would be
    // a set that counts it twice.
    let ordered: BTreeMap<NodeId, &Member> = set.iter().map(|m| (m.node, m)).collect();
    let mut h = Sha256::new();
    for m in ordered.values() {
        h.update(m.node);
        h.update(m.weight.to_be_bytes());
        h.update((m.key.len() as u64).to_be_bytes());
        h.update(&m.key);
    }
    h.finalize().into()
}

/// A node id out of its `NodeID-<cb58>` spelling.
fn node_id(name: &str) -> Result<NodeId, Error> {
    let raw = bs58::decode(name.trim_start_matches("NodeID-"))
        .into_vec()
        .map_err(|e| Error::Malformed(format!("{name}: not cb58: {e}")))?;
    // Twenty bytes and a four-byte checksum. The checksum is CHECKED: a
    // mistyped id that decoded anyway would put a validator in the set that
    // nobody can verify a signature for.
    if raw.len() != 24 {
        return Err(Error::Malformed(format!("{name}: not a node id")));
    }
    let (id, sum) = raw.split_at(20);
    if Sha256::digest(id)[28..] != *sum {
        return Err(Error::Malformed(format!("{name}: checksum")));
    }
    let mut out = [0u8; 20];
    out.copy_from_slice(id);
    Ok(out)
}

/// One HTTP GET, with no dependency and no keep-alive.
///
/// Enough to ask luxd one question. A client library here would be a second
/// HTTP implementation in a crate that already has one for serving.
fn get(addr: SocketAddr, path: &str) -> Result<Vec<u8>, Error> {
    let mut sock =
        TcpStream::connect_timeout(&addr, TIMEOUT).map_err(|e| Error::Io(e.to_string()))?;
    sock.set_read_timeout(Some(TIMEOUT)).map_err(|e| Error::Io(e.to_string()))?;
    sock.set_write_timeout(Some(TIMEOUT)).map_err(|e| Error::Io(e.to_string()))?;
    // `Host: localhost` because luxd's default `http-allowed-hosts` is exactly
    // that, and an address in the header is refused.
    write!(
        sock,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    )
    .map_err(|e| Error::Io(e.to_string()))?;
    sock.flush().map_err(|e| Error::Io(e.to_string()))?;

    let mut raw = Vec::new();
    sock.take(BODY_MAX as u64).read_to_end(&mut raw).map_err(|e| Error::Io(e.to_string()))?;

    let head_end =
        find(&raw, b"\r\n\r\n").ok_or_else(|| Error::Malformed("no end of headers".into()))?;
    let status = core::str::from_utf8(&raw[..raw.iter().position(|b| *b == b'\r').unwrap_or(0)])
        .unwrap_or("");
    if !status.contains(" 200") {
        return Err(Error::Io(format!("luxd answered: {status}")));
    }
    let body = &raw[head_end + 4..];
    // Chunked is the only transfer encoding luxd uses here when it uses one.
    if raw[..head_end].windows(24).any(|w| w.eq_ignore_ascii_case(b"transfer-encoding: chunk")) {
        return dechunk(body);
    }
    Ok(body.to_vec())
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Reassemble a chunked body.
fn dechunk(mut body: &[u8]) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    loop {
        let line = find(body, b"\r\n").ok_or_else(|| Error::Malformed("chunk header".into()))?;
        let size = core::str::from_utf8(&body[..line])
            .ok()
            .and_then(|s| usize::from_str_radix(s.split(';').next().unwrap_or(s).trim(), 16).ok())
            .ok_or_else(|| Error::Malformed("chunk size".into()))?;
        if size == 0 {
            return Ok(out);
        }
        let start = line + 2;
        let end = start + size;
        out.extend_from_slice(
            body.get(start..end).ok_or_else(|| Error::Malformed("short chunk".into()))?,
        );
        body = body.get(end + 2..).unwrap_or(&[]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What luxd's `/validators/at?height=0` answered on the four-validator
    /// network this was developed against, trimmed to two entries.
    const ANSWER: &str = r#"{
      "NodeID-9XacvrcF3HTzCqb8gY4zctu614jQaDDyY": {
        "publicKey": "0x0fb4c478e08c78b575e2b7bf977226d08ab85c8b5394d7e3adcdf42312df4232cd6cc32e45f46a022644ac6bbc1b3b2d157f946f3c1d71b22e72acd2d3c92c836b5eccb78083f540ffe54841c1dc2fee2245cfbe9b4afffa150dd4532e77716a",
        "weight": "7500000000"
      },
      "NodeID-PEPLweTWhTvZbbMncxPEicgtWV7ydjFTU": {
        "publicKey": "0x04552ed0e1c451dcd8b1c2f1850edb359b931dab48f07b949975e9ce55fa694d73b504f7fac8779ccdd6c9a457f58c240282082c5735f0e1a1ae8a2c483b65f6e4b90136d8ba8423c38a18c923a0eea0e9d4f61f66f845e3962c579c17b89b11",
        "weight": "7500000000"
      }
    }"#;

    #[test]
    fn the_set_is_read_the_way_luxd_writes_it() {
        let set = parse(ANSWER.as_bytes()).expect("parses");
        assert_eq!(set.len(), 2);
        for m in &set {
            assert_eq!(m.key.len(), 96, "the key is uncompressed");
            assert_eq!(m.weight, 7_500_000_000, "the EFFECTIVE weight, not a declared one");
        }
    }

    #[test]
    fn the_root_is_the_one_that_closed_the_signature() {
        // This exact value is what luxd's own vote signature verified over on
        // the two-validator network — the number that closed the last field of
        // the position. It is not recomputed here from the same code that
        // produced it; it is the answer a live node agreed with.
        let set = parse(ANSWER.as_bytes()).expect("parses");
        assert_eq!(
            alloy_primitives::hex::encode(root(&set)),
            "897c3646944fd5c88236ac197c5e34544c84d91e8ce71b1d0562c09e5742c79f"
        );
    }

    #[test]
    fn an_empty_set_commits_to_zero() {
        assert_eq!(root(&[]), [0u8; 32]);
    }

    #[test]
    fn the_order_the_answer_arrived_in_cannot_change_the_root() {
        let mut set = parse(ANSWER.as_bytes()).expect("parses");
        let forward = root(&set);
        set.reverse();
        assert_eq!(root(&set), forward, "sorted by node id, not by arrival");
    }

    #[test]
    fn a_compressed_key_is_refused_rather_than_hashed() {
        // Hashing 48 bytes where Go hashes 96 is a root nobody else has, and it
        // would fail as a silent signature mismatch rather than as an error.
        let shrunk = ANSWER.replace(
            "0x0fb4c478e08c78b575e2b7bf977226d08ab85c8b5394d7e3adcdf42312df4232cd6cc32e45f46a022644ac6bbc1b3b2d157f946f3c1d71b22e72acd2d3c92c836b5eccb78083f540ffe54841c1dc2fee2245cfbe9b4afffa150dd4532e77716a",
            "0xafb4c478e08c78b575e2b7bf977226d08ab85c8b5394d7e3adcdf42312df4232cd6cc32e45f46a022644ac6bbc1b3b2d",
        );
        assert!(parse(shrunk.as_bytes()).is_err());
    }

    #[test]
    fn a_weight_that_is_a_number_is_refused() {
        // The document writes weights as strings because they do not fit a JSON
        // number safely. Reading one as a number would drop that validator.
        let bad = ANSWER.replace("\"7500000000\"", "7500000000");
        assert!(parse(bad.as_bytes()).is_err());
    }

    #[test]
    fn a_node_id_whose_checksum_fails_is_not_a_validator() {
        let bad = ANSWER.replace(
            "NodeID-9XacvrcF3HTzCqb8gY4zctu614jQaDDyY",
            "NodeID-9XacvrcF3HTzCqb8gY4zctu614jQaDDyZ",
        );
        assert!(parse(bad.as_bytes()).is_err());
    }
}
