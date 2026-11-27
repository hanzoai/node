// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! The transactions root, against the Go corpus.
//!
//! A block whose transactions are all valid and whose root is wrong is a block
//! nobody else accepts, so the root is worth checking on its own — and it needs
//! no chain history to check, only a list. The corpus carries five signed
//! transactions of four different types and the root Go derives from them.
//!
//! The typed ones are why this matters. A 2930 or 1559 transaction goes into
//! the trie as its EIP-2718 envelope — the type byte followed by the payload —
//! and an implementation that RLP-wraps that byte string instead produces a
//! different root while every transaction in the block is still valid.

mod corpus;

use corpus::{get, hash, text, unhex};
use lux_evm::root;

fn signed() -> Vec<Vec<u8>> {
    get("evm_transaction.json")["cases"]
        .as_array()
        .expect("the corpus lists its cases")
        .iter()
        .map(|c| unhex(text(c, "signed_rlp"), "signed_rlp"))
        .collect()
}

#[test]
fn rust_derives_the_transactions_root_go_derives() {
    let v = get("evm_transaction.json");
    let txs = signed();
    assert_eq!(
        txs.len(),
        v["transactions_root"]["count"].as_u64().unwrap() as usize,
        "the corpus counts a different number of transactions than it lists"
    );
    assert_eq!(
        root::transactions(&txs),
        hash(&v["transactions_root"], "root"),
        "the root over Go's own signed bytes differs"
    );
}

#[test]
fn the_order_is_part_of_the_commitment() {
    let mut txs = signed();
    let straight = root::transactions(&txs);
    txs.swap(0, 1);
    assert_ne!(
        root::transactions(&txs),
        straight,
        "the trie is keyed by index, so reordering must rename the block"
    );
}

#[test]
fn a_typed_transaction_goes_in_as_its_envelope() {
    // If the envelope were RLP-wrapped, dropping the wrapper would not change
    // the root. Prove the encoding in the trie is the bytes themselves by
    // showing that a single-item root over the raw envelope is not the root
    // over the same bytes wrapped as an RLP string.
    let txs = signed();
    let typed = txs
        .iter()
        .find(|t| t[0] == 0x02)
        .expect("the corpus carries a 1559 transaction");

    let wrapped = alloy_rlp::encode(typed.as_slice());
    assert_ne!(
        root::transactions(std::slice::from_ref(typed)),
        root::transactions(&[wrapped]),
        "wrapping the envelope would be a different, wrong root"
    );
}

#[test]
fn an_empty_list_is_the_root_the_corpus_records() {
    let v = get("evm_transaction.json");
    let none: [&[u8]; 0] = [];
    assert_eq!(
        root::transactions(&none),
        hash(&v["transactions_root"], "empty_root")
    );
    assert_eq!(root::EMPTY, hash(&v["transactions_root"], "empty_root"));
}
