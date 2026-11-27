// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! Reading the Go corpus.
//!
//! There is one corpus, `luxfi/conformance`, produced only by a Go node or the
//! Go libraries. Nothing here writes a vector or keeps a copy of one: a second
//! copy is a second truth, and the point of the corpus is that there is one.
//!
//! Found at `$LUX_VECTORS`, or at `../../lux/conformance/vectors` beside this
//! crate. A missing corpus is a failure, never a skip — a harness that quietly
//! passes with nothing to check is worse than one that fails.
#![allow(dead_code)]

use std::path::PathBuf;

use alloy_primitives::{hex, B256};
use serde_json::Value;

/// One vector.
pub fn get(name: &str) -> Value {
    let dir = match std::env::var_os("LUX_VECTORS") {
        Some(p) => PathBuf::from(p),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../lux/conformance/vectors"),
    };
    let path = dir.join(name);
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "no corpus at {}: {e}. Set LUX_VECTORS to a luxfi/conformance vectors directory",
            path.display()
        )
    });
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

/// A byte string. Always an even number of digits, because it has a width.
pub fn bytes(v: &Value, field: &str) -> Vec<u8> {
    unhex(text(v, field), field)
}

pub fn hash(v: &Value, field: &str) -> B256 {
    B256::from_slice(&bytes(v, field))
}

/// A quantity. JSON-RPC writes these minimally, so `0x5d21dba00` has an odd
/// number of digits and is not a byte string — reading one as bytes is a
/// classic way to mis-parse a header.
pub fn number(v: &Value, field: &str) -> u64 {
    let s = text(v, field);
    u64::from_str_radix(s.strip_prefix("0x").unwrap_or(s), 16)
        .unwrap_or_else(|e| panic!("{field} is not a quantity: {e}"))
}

pub fn text<'a>(v: &'a Value, field: &str) -> &'a str {
    v[field]
        .as_str()
        .unwrap_or_else(|| panic!("{field} is not a string in the corpus"))
}

pub fn unhex(s: &str, what: &str) -> Vec<u8> {
    hex::decode(s).unwrap_or_else(|e| panic!("{what} is not hex: {e}"))
}
