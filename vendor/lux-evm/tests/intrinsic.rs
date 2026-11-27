// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! Intrinsic gas and deployment addresses, against the Go corpus.
//!
//! Both are pure functions of their input, so they need no chain, no funded
//! account and no block to check — and both are load-bearing. Charge the wrong
//! intrinsic gas and two nodes disagree about which transactions fit in a
//! block; derive the wrong address and a deployment lands somewhere nobody
//! else looks.

mod corpus;

use alloy_primitives::Address;
use corpus::{get, text, unhex};
use lux_evm::{gas, Chain};
use revm::primitives::hardfork::SpecId;
use serde_json::Value;

/// The corpus records geth's schedule with EIP-3860 live, which is what a Lux
/// chain charges once Durango is active — and Durango is active from genesis
/// on every Lux network.
const SPEC: SpecId = SpecId::SHANGHAI;

fn address(v: &Value, field: &str) -> Address {
    Address::from_slice(&unhex(text(v, field), field))
}

#[test]
fn rust_charges_the_intrinsic_gas_go_charges() {
    let rules = Chain::lux().at(0);
    let cases = get("evm_transaction.json")["intrinsic_gas"]
        .as_array()
        .expect("the corpus lists intrinsic gas cases")
        .clone();
    assert!(!cases.is_empty(), "an empty case list proves nothing");

    for case in cases {
        let data = unhex(text(&case, "data"), "data");
        let create = case["is_creation"].as_bool().expect("is_creation");
        let want = case["gas"].as_u64().expect("gas");
        assert_eq!(
            gas::intrinsic(SPEC, rules, &data, create, 0, 0),
            want,
            "{}: {} bytes, creation {create}",
            text(&case, "note"),
            data.len()
        );
    }
}

#[test]
fn rust_derives_the_deployment_addresses_go_derives() {
    let cases = get("evm_transaction.json")["contract_address"]
        .as_array()
        .expect("the corpus lists the two address rules")
        .clone();
    assert_eq!(cases.len(), 2, "CREATE and CREATE2");

    for case in cases {
        let sender = address(&case, "sender");
        let want = address(&case, "address");
        let got = match case.get("salt") {
            // CREATE2: sender, salt and the hash of the initcode.
            Some(salt) => {
                let salt: [u8; 32] = unhex(salt.as_str().expect("salt"), "salt")
                    .try_into()
                    .expect("a salt is 32 bytes");
                sender.create2_from_code(salt, unhex(text(&case, "init_code"), "init_code"))
            }
            // CREATE: sender and nonce alone.
            None => sender.create(case["nonce"].as_u64().expect("nonce")),
        };
        assert_eq!(got, want, "{}", text(&case, "note"));
    }
}

#[test]
fn the_corpus_and_the_signing_key_agree_on_the_sender() {
    // The address every case is signed by. Reading it here keeps the two
    // address rules anchored to the same account the transactions come from,
    // so a corpus re-capture with a different key cannot silently pass.
    let v = get("evm_transaction.json");
    let signer = address(&v, "signing_address");
    for case in v["contract_address"].as_array().unwrap() {
        assert_eq!(address(case, "sender"), signer);
    }
}
