// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! The genesis this node starts its chain from, and holding a supplied one to
//! the network it claims to be.
//!
//! Each network's document is compiled in, so a node pointed at a network by
//! name starts from the same state every other node on it does. A document read
//! off disk is checked against that network before it is used: a genesis is
//! data, and data from outside is checked, not trusted.
//!
//! WHAT IS CHECKED IS THE TWO NUMBERS. The chain id must be this network's
//! chain, because it is what every transaction is signed against. The network
//! id, when the document states one, must be this network, because a chain
//! document carried inside another network's genesis belongs to that network.
//! A field that is present and unreadable is an error, never a default: a
//! document that fell back to zero for a number it could not parse would agree
//! with nothing and look like it had been checked.

use serde_json::Value;

use crate::network::{Network, DEVNET, MAINNET, TESTNET};

/// How deep a `cChainGenesis` chain is followed before it is called a loop.
/// A document that wraps itself would otherwise recurse until the stack ends.
const MAX_NESTING: u8 = 8;

/// The document `net` starts from.
pub fn document(net: &Network) -> &'static str {
    match net.name {
        n if n == MAINNET.name => include_str!("../genesis/mainnet.json"),
        n if n == TESTNET.name => include_str!("../genesis/testnet.json"),
        n if n == DEVNET.name => include_str!("../genesis/devnet.json"),
        // Unreachable by construction: a `Network` value only exists for the
        // three above. Stated rather than unwrapped so adding a fourth without
        // its document is a compile-time-shaped failure and not a panic.
        other => panic!("no genesis is compiled in for {other}"),
    }
}

/// Refuse `raw` if it is not this network's genesis.
pub fn check(raw: &str, net: &Network) -> Result<(), String> {
    let doc: Value = serde_json::from_str(raw).map_err(|e| format!("not JSON: {e}"))?;
    if !doc.is_object() {
        return Err("not a genesis document".into());
    }

    // A whole-network document carries the chain's own inside it, and it is
    // written either as an object or as a string holding the document. Both
    // shapes are read, because both are what is on disk.
    let mut held;
    let mut chain = &doc;
    let mut depth = 0;
    while let Some(inner) = chain.get("cChainGenesis") {
        depth += 1;
        if depth > MAX_NESTING {
            return Err("cChainGenesis nests too deeply".into());
        }
        chain = match inner {
            Value::String(text) => {
                held = serde_json::from_str(text)
                    .map_err(|e| format!("cChainGenesis is not JSON: {e}"))?;
                &held
            }
            other => other,
        };
    }
    if !chain.is_object() {
        return Err("cChainGenesis is not a document".into());
    }

    let config = chain.get("config").filter(|c| c.is_object()).ok_or("no config")?;
    let id = number(config, "chainId")?;
    if id != net.chain {
        return Err(format!(
            "this genesis is chain {id}, not {}'s chain {}",
            net.name, net.chain
        ));
    }

    // The network the document names, when it names one. A chain document
    // carried inside another network's genesis is that network's, whatever its
    // chain id says.
    if doc.get("networkID").is_some() {
        let stated = number(&doc, "networkID")?;
        if stated != net.id as u64 {
            return Err(format!(
                "this genesis is network {stated}, not {} ({})",
                net.name, net.id
            ));
        }
    }
    Ok(())
}

/// How many accounts a document allocates.
pub fn accounts(raw: &str) -> usize {
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|doc| doc.get("alloc").and_then(|a| a.as_object()).map(|a| a.len()))
        .unwrap_or(0)
}

/// Read a number that must be a number. A quoted or absent value is an error
/// here rather than a zero three lines later.
fn number(at: &Value, field: &str) -> Result<u64, String> {
    at.get(field)
        .ok_or_else(|| format!("no {field}"))?
        .as_u64()
        .ok_or_else(|| format!("{field} is not a whole number"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::ALL;

    #[test]
    fn every_network_starts_from_its_own_chain() {
        for net in ALL {
            let raw = document(&net);
            check(raw, &net).unwrap_or_else(|e| panic!("{}: {e}", net.name));
            assert!(accounts(raw) > 0, "{} allocates nothing", net.name);
        }
    }

    #[test]
    fn one_networks_genesis_is_not_anothers() {
        for (i, a) in ALL.iter().enumerate() {
            for b in &ALL[i + 1..] {
                assert!(check(document(a), b).is_err(), "{}'s genesis passed for {}", a.name, b.name);
                assert!(check(document(b), a).is_err(), "{}'s genesis passed for {}", b.name, a.name);
            }
        }
    }

    #[test]
    fn a_document_naming_another_network_is_refused() {
        // The chain is right and the network is not: the document belongs to
        // whichever network's genesis carries it.
        let raw = r#"{"networkID":9,"cChainGenesis":{"config":{"chainId":36963}}}"#;
        let err = check(raw, &MAINNET).expect_err("network 9 is not mainnet");
        assert!(err.contains("network 9"), "{err}");

        // And the network is right, in both shapes it is written.
        assert!(check(r#"{"networkID":2,"cChainGenesis":{"config":{"chainId":36962}}}"#, &TESTNET).is_ok());
        assert!(check(r#"{"networkID":3,"cChainGenesis":"{\"config\":{\"chainId\":36964}}"}"#, &DEVNET).is_ok());
    }

    #[test]
    fn a_document_that_names_no_network_is_read_as_a_chain() {
        // The common shape: the chain's own document, with no network around it.
        assert!(check(r#"{"config":{"chainId":36963}}"#, &MAINNET).is_ok());
    }

    #[test]
    fn nothing_unreadable_becomes_a_default() {
        assert!(check("", &MAINNET).is_err(), "an empty file");
        assert!(check(r#"{"config":{"chainId":36963"#, &MAINNET).is_err(), "truncated JSON");
        assert!(check("[1,2,3]", &MAINNET).is_err(), "not an object");
        assert!(check(r#"{"alloc":{}}"#, &MAINNET).is_err(), "no config");
        assert!(check(r#"{"config":{}}"#, &MAINNET).is_err(), "no chain id");
        assert!(check(r#"{"config":{"chainId":"36963"}}"#, &MAINNET).is_err(), "a quoted chain id");
        assert!(check(r#"{"config":{"chainId":-1}}"#, &MAINNET).is_err(), "a negative chain id");
        assert!(
            check(r#"{"networkID":"1","cChainGenesis":{"config":{"chainId":36963}}}"#, &MAINNET).is_err(),
            "a quoted network id"
        );
    }

    #[test]
    fn a_document_nested_past_the_limit_is_refused() {
        let mut wrapped = r#"{"config":{"chainId":36963}}"#.to_string();
        for _ in 0..12 {
            wrapped = format!(r#"{{"cChainGenesis":{wrapped}}}"#);
        }
        assert!(check(&wrapped, &MAINNET).is_err());
    }
}
