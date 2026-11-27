// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! Every header shape the C-chain has accepted, against the headers it accepted.
//!
//! `luxfi/conformance` captured 21 heights from mainnet, each recording the RLP
//! whose keccak256 is the hash the network took. That is the strongest test
//! available for a naming function: not "does this agree with itself" but "does
//! this produce the name a live chain already gave the block".
//!
//! The corpus notes something worth repeating: of luxfi/geth's five header hash
//! functions, none reproduces every block, and block 1 is reproduced by none of
//! them. The accepted hash is keccak of the recorded RLP, and that is what is
//! checked here.

mod corpus;

use alloy_primitives::{hex, B256, U256};
use corpus::{bytes, get as corpus, hash};
use lux_evm::header::{self, Lux};
use serde_json::Value;

/// The header history, as the node served it. Each entry carries the RLP whose
/// keccak256 the network accepted, and nothing else is needed: the header is
/// read back out of those bytes.
fn history() -> Vec<Value> {
    corpus("evm_header_history.json")["blocks"]
        .as_array()
        .expect("the corpus records blocks")
        .clone()
}

fn at(b: &Value, field: &str) -> u64 {
    b[field]
        .as_u64()
        .unwrap_or_else(|| panic!("{field} is not a number in the corpus"))
}

fn rlp_of(b: &Value) -> Vec<u8> {
    bytes(b, "rlp")
}

#[test]
fn every_accepted_header_encodes_to_the_bytes_the_network_accepted() {
    // Byte-identity is the stronger claim: two encoders can hash the same only
    // by hashing the same bytes, and when they diverge this says where.
    let blocks = history();
    assert_eq!(blocks.len(), 21, "the corpus is 21 heights");
    for b in &blocks {
        let want = rlp_of(b);
        let got = Lux::read(&want)
            .unwrap_or_else(|e| panic!("block {} does not decode: {e:?}", at(b, "number")))
            .encoded();
        assert_eq!(
            hex::encode(&got),
            hex::encode(&want),
            "block {} ({}) does not survive its own encoding",
            at(b, "number"),
            b["shape"].as_str().unwrap_or("?")
        );
    }
}

#[test]
fn every_accepted_header_is_named_what_the_network_named_it() {
    for b in &history() {
        assert_eq!(
            Lux::read(&rlp_of(b)).expect("decodes").seal(),
            hash(b, "hash"),
            "block {} is named something else",
            at(b, "number")
        );
    }
}

#[test]
fn the_shape_is_read_off_the_header_not_declared_once() {
    // Two shapes on record, 17 and 20, and their seventeenth fields are
    // different things: an absent ExtDataHash in one, the withdrawals root in
    // the other. A node that declared one layout would read one of them wrongly.
    let mut seen = std::collections::BTreeSet::new();
    for b in &history() {
        let h = Lux::read(&rlp_of(b)).expect("decodes");
        assert_eq!(
            h.fields(),
            at(b, "fields") as usize,
            "block {} field count",
            at(b, "number")
        );
        seen.insert(h.layout.name);
    }
    assert_eq!(
        seen,
        ["16", "17", "20 eth"].into(),
        "the corpus carries three shapes"
    );
}

#[test]
fn the_genesis_is_the_only_sixteen_field_block() {
    for b in &history() {
        let n = at(b, "number");
        let fields = at(b, "fields");
        if n == 0 {
            assert_eq!(fields, 16);
        } else {
            assert!(fields > 16, "block {n} has {fields} fields");
        }
    }
}

#[test]
fn dropping_the_seventeenth_field_renames_an_ordinary_block() {
    // The reason this module exists. An ordinary block's seventeenth field is
    // there and empty, and a header hashed without it — the plain 16-field
    // header — is a different name: a node producing it has forked at block one.
    let blocks = history();
    let one = blocks
        .iter()
        .find(|b| at(b, "number") == 1)
        .expect("block 1");
    let lux = Lux::read(&rlp_of(one)).expect("decodes");
    assert_eq!(lux.fields(), 17);
    assert_eq!(lux.seal(), hash(one, "hash"));
    assert_ne!(
        header::seal(&lux.header),
        hash(one, "hash"),
        "the plain header hashes to a name the network never gave this block"
    );
}

#[test]
fn block_one_builds_on_the_genesis_this_crate_computes() {
    // Ties the header history to the genesis: the parent of block 1 is the
    // hash `lux_evm::genesis::mainnet()` produces from the allocation.
    let blocks = history();
    let one = blocks
        .iter()
        .find(|b| at(b, "number") == 1)
        .expect("block 1");
    assert_eq!(
        Lux::read(&rlp_of(one)).expect("decodes").header.parent_hash,
        lux_evm::genesis::mainnet().hash(),
        "the chain the corpus captured is the chain this crate describes"
    );
}

#[test]
fn a_header_that_is_only_a_genesis_shape_still_seals_the_old_way() {
    let g = lux_evm::genesis::mainnet();
    assert_eq!(Lux::genesis(g.header()).seal(), g.hash());
    assert_eq!(Lux::genesis(g.header()).fields(), 16);
    assert_ne!(
        Lux::block(g.header(), U256::ZERO).seal(),
        g.hash(),
        "adding the field renames the block"
    );
}

#[test]
fn the_seventeenth_field_is_an_absent_ext_data_hash_and_carries_no_block_gas_cost() {
    // The corpus labels the ordinary shape `blockGasCost`; Go does not read it
    // so. `DecodeHeader` takes a 17-field header's seventeenth as ExtDataHash,
    // and the empty string there as absent — so an ordinary block has neither.
    for b in &history() {
        let h = Lux::read(&rlp_of(b)).expect("decodes");
        match at(b, "fields") {
            16 => assert_eq!(h.layout.name, "16"),
            17 => {
                assert_eq!(h.layout.name, "17", "block {}", at(b, "number"));
                assert_eq!(h.ext_data_hash, None);
                assert_eq!(h.block_gas_cost, None);
            }
            20 => {
                assert_eq!(h.layout.name, "20 eth", "block {}", at(b, "number"));
                assert!(h.header.withdrawals_root.is_some());
                assert_eq!(h.block_gas_cost, None);
            }
            n => panic!("block {} has {n} fields", at(b, "number")),
        }
    }
}

/// A header as Go's oracle reports one: its name, its field count, and every
/// field `DecodeHeader` may leave nil, as `hexutil` writes it.
fn answer(h: &Lux) -> Value {
    let big = |b: &[u8]| {
        let s = hex::encode(b);
        let s = s.trim_start_matches('0');
        format!("0x{}", if s.is_empty() { "0" } else { s })
    };
    let hash = |v: Option<B256>| v.map(|v| format!("{v:#x}"));
    let u = &h.header;
    serde_json::json!({
        "hash": format!("{:#x}", h.seal()),
        "fields": h.fields(),
        "baseFeePerGas": u.base_fee_per_gas.map(|v| format!("{v:#x}")),
        "blockGasCost": h.block_gas_cost.map(|v| format!("{v:#x}")),
        "extDataHash": hash(h.ext_data_hash),
        "extDataGasUsed": h.ext_data_gas_used.as_ref().map(|b| big(b)),
        "blobGasUsed": u.blob_gas_used.map(|v| format!("{v:#x}")),
        "excessBlobGas": u.excess_blob_gas.map(|v| format!("{v:#x}")),
        "parentBeaconRoot": hash(u.parent_beacon_block_root),
        "withdrawalsHash": hash(u.withdrawals_root),
        "requestsHash": hash(u.requests_hash),
    })
}

#[test]
fn every_header_shape_go_reads_is_read_as_go_reads_it() {
    // luxfi/geth `DecodeHeader` on every shape, from Go's own oracle: the
    // fields each header decodes to and the name keccak256 of its bytes gives
    // it, or Go's refusal. The cases wrapped in a block are `DecodeBlock`'s
    // and belong to whoever reads blocks.
    let doc = corpus("evm_header_layouts.json");
    let mut n = 0;
    for c in doc["cases"].as_array().expect("cases") {
        if c.get("block_items").is_some() {
            continue;
        }
        n += 1;
        let name = c["name"].as_str().expect("a name");
        let raw = bytes(c, "header");
        let go = &c["go"];
        let read = Lux::read(&raw);
        if let Some(why) = go.get("error") {
            assert!(
                read.is_err(),
                "{name}: Go refuses it ({why}), and this read it"
            );
            continue;
        }
        // Go reads a base fee of any width; revm's block environment holds 64
        // bits, so a wider one is refused here while reading. Go refuses one
        // past 256 bits in SyntacticVerify, and no fee schedule these chains
        // run reaches 2^64.
        if go["baseFeePerGas"].as_str().is_some_and(|f| f.len() > 18) {
            assert!(read.is_err(), "{name}: a base fee past 64 bits is refused");
            continue;
        }
        let h = read.unwrap_or_else(|e| panic!("{name}: Go reads it, and this refused it: {e}"));
        assert_eq!(answer(&h), *go, "{name} ({})", h.layout.name);
        assert_eq!(
            h.encoded(),
            raw,
            "{name}: re-encodes to the bytes it arrived as"
        );
    }
    assert!(n >= 30, "the corpus holds {n} header shapes");
}
