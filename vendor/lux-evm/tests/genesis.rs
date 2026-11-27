// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! The C-chain genesis, against the Go corpus.
//!
//! `luxfi/conformance` records what a live Go node reports and what the Go
//! libraries compute. This builds the genesis header out of those field values
//! in Rust and checks two things the network depends on: that the encoding is
//! byte-identical to Go's, and that it is named the same block.
//!
//! Byte-identity is the stronger claim and the one worth making. Two encoders
//! can hash the same only by hashing the same bytes, but a test that compares
//! only hashes cannot say *where* they diverged when they do.

mod corpus;

use alloy_consensus::Header;
use alloy_primitives::{hex, Address, Bloom, Bytes, B64, U256};
use corpus::{bytes, get as corpus, hash, number};
use lux_evm::{header, root};

/// The genesis header, read out of what the node reported.
fn genesis() -> Header {
    let v = corpus("evm_genesis_block.json");
    Header {
        parent_hash: hash(&v, "parentHash"),
        ommers_hash: hash(&v, "sha3Uncles"),
        beneficiary: Address::from_slice(&bytes(&v, "miner")),
        state_root: hash(&v, "stateRoot"),
        transactions_root: hash(&v, "transactionsRoot"),
        receipts_root: hash(&v, "receiptsRoot"),
        logs_bloom: Bloom::from_slice(&bytes(&v, "logsBloom")),
        difficulty: U256::from(number(&v, "difficulty")),
        number: number(&v, "number"),
        gas_limit: number(&v, "gasLimit"),
        gas_used: number(&v, "gasUsed"),
        timestamp: number(&v, "timestamp"),
        extra_data: Bytes::from(bytes(&v, "extraData")),
        mix_hash: hash(&v, "mixHash"),
        nonce: B64::from_slice(&bytes(&v, "nonce")),
        base_fee_per_gas: Some(number(&v, "baseFeePerGas")),
        // Everything after London is absent from the C-chain header. The
        // corpus says so itself: `evm_header.json` records `fields: 16`.
        ..Default::default()
    }
}

#[test]
fn rust_encodes_the_genesis_header_to_the_bytes_go_encodes() {
    let want = bytes(&corpus("evm_header.json"), "rlp");
    let got = alloy_rlp::encode(genesis());
    assert_eq!(
        hex::encode(&got),
        hex::encode(&want),
        "the header Rust builds does not encode to Go's bytes"
    );
}

#[test]
fn rust_names_the_genesis_block_what_the_network_calls_it() {
    let block = corpus("evm_genesis_block.json");
    let want = hash(&block, "hash");
    assert_eq!(
        header::seal(&genesis()),
        want,
        "keccak256(rlp(header)) is not the hash the node reports"
    );
    assert_eq!(
        header::seal(&genesis()),
        hash(&corpus("evm_header.json"), "hash"),
        "the two vectors disagree; re-capture the corpus"
    );
}

#[test]
fn the_genesis_header_has_the_shape_the_c_chain_seals() {
    let h = genesis();
    assert!(
        header::is_london(&h),
        "sixteen fields through baseFeePerGas, and nothing after it"
    );
    assert_eq!(
        corpus("evm_header.json")["fields"].as_u64(),
        Some(16),
        "the corpus and the predicate must be talking about the same shape"
    );
}

#[test]
fn an_empty_block_commits_to_the_empty_roots() {
    let v = corpus("evm_genesis_block.json");
    assert!(
        v["transactions"].as_array().is_some_and(|t| t.is_empty()),
        "the genesis carries no transactions"
    );
    let none: [&[u8]; 0] = [];
    assert_eq!(root::transactions(&none), hash(&v, "transactionsRoot"));
    assert_eq!(root::receipts(&none), hash(&v, "receiptsRoot"));
}

#[test]
fn the_reported_size_is_the_encoding_plus_its_two_empty_lists() {
    // A block's `size` is the RLP of the whole block, not of the header: the
    // header list, then an empty transaction list and an empty uncle list, all
    // inside one outer list. Checking it catches an implementation that
    // reported the header length and called it the block size.
    let v = corpus("evm_genesis_block.json");
    let header = alloy_rlp::encode(genesis()).len();
    let inner = header + 1 + 1; // 0xc0 for each empty list
    let outer = if inner <= 55 { 1 } else { 1 + 2 }; // f9 + two length bytes
    assert_eq!(inner + outer, number(&v, "size") as usize);
}
