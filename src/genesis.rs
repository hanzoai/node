// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! Reading a genesis document, and holding it to the network it claims to be.
//!
//! A genesis is data, and this is the one place it becomes a value this node
//! will act on. What is refused matters more than what is read: a field that is
//! present and unreadable is an error, never a default. A document that fell
//! back to zero for a number it could not parse would agree with nothing and
//! look like it had been checked.

use serde_json::Value;

use crate::network::Network;

/// How deep a `cChainGenesis` chain is followed before it is called a loop.
/// A document that wraps itself would otherwise recurse until the stack ends.
const MAX_NESTING: u8 = 8;

/// What a genesis document says about which network it is for.
pub struct Genesis {
    /// The number the document states — its EVM chain id, and for a sovereign
    /// network its network id too.
    pub id: u64,
    /// How many accounts it allocates.
    pub accounts: usize,
}

impl Genesis {
    /// Refuse this document if it is not this network's.
    pub fn agrees_with(&self, net: &Network) -> Result<(), String> {
        match self.id == net.id {
            true => Ok(()),
            false => Err(format!(
                "this genesis is network {}, not {} ({})",
                self.id, net.name, net.id
            )),
        }
    }
}

/// Read a genesis document.
///
/// Both shapes on disk are accepted, because both are what is written: the
/// chain's own document, or a whole-network document carrying it under
/// `cChainGenesis` — as an object, or as a string holding the document.
pub fn read(raw: &str) -> Result<Genesis, String> {
    let doc: Value = serde_json::from_str(raw).map_err(|e| format!("not JSON: {e}"))?;
    if !doc.is_object() {
        return Err("not a genesis document".into());
    }

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

    // A whole-network document states the number twice. Both must be the same
    // number: a document whose two halves name different networks is one half
    // of two genesis files, and whichever this node believed would be wrong
    // somewhere else.
    if doc.get("networkID").is_some() {
        let network = number(&doc, "networkID")?;
        if network != id {
            return Err(format!("it is network {network} carrying chain {id}"));
        }
    }

    let accounts = match chain.get("alloc") {
        None => 0,
        Some(Value::Object(entries)) => entries.len(),
        Some(_) => return Err("alloc is not a set of accounts".into()),
    };
    Ok(Genesis { id, accounts })
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
    use crate::network::{DEVNET, MAINNET, TESTNET};

    fn accepted(raw: &str, net: &Network) -> bool {
        read(raw).and_then(|g| g.agrees_with(net)).is_ok()
    }

    #[test]
    fn a_genesis_stating_this_network_is_accepted() {
        let raw = r#"{"config":{"chainId":36963},
            "alloc":{"0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266":{"balance":"0x1"}}}"#;
        assert!(accepted(raw, &MAINNET));
        assert_eq!(read(raw).unwrap().accounts, 1);
    }

    #[test]
    fn a_genesis_for_another_network_is_refused() {
        let raw = r#"{"config":{"chainId":36963}}"#;
        assert!(!accepted(raw, &TESTNET));
        assert!(!accepted(r#"{"config":{"chainId":96369}}"#, &MAINNET));
    }

    #[test]
    fn a_whole_network_document_is_read_in_both_shapes() {
        assert!(accepted(
            r#"{"networkID":36962,"cChainGenesis":{"config":{"chainId":36962}}}"#,
            &TESTNET
        ));
        assert!(accepted(
            r#"{"networkID":36964,"cChainGenesis":"{\"config\":{\"chainId\":36964}}"}"#,
            &DEVNET
        ));
    }

    #[test]
    fn a_document_naming_two_networks_is_refused() {
        assert!(read(r#"{"networkID":36963,"cChainGenesis":{"config":{"chainId":36962}}}"#).is_err());
    }

    #[test]
    fn nothing_unreadable_becomes_a_default() {
        assert!(read("").is_err(), "an empty file");
        assert!(read(r#"{"config":{"chainId":36963"#).is_err(), "truncated JSON");
        assert!(read("[1,2,3]").is_err(), "not an object");
        assert!(read(r#"{"alloc":{}}"#).is_err(), "no config");
        assert!(read(r#"{"config":{}}"#).is_err(), "no chain id");
        assert!(read(r#"{"config":{"chainId":"36963"}}"#).is_err(), "a quoted chain id");
        assert!(read(r#"{"config":{"chainId":-1}}"#).is_err(), "a negative chain id");
        assert!(
            read(r#"{"config":{"chainId":36963},"alloc":[]}"#).is_err(),
            "an allocation that is not a set of accounts"
        );
    }

    #[test]
    fn a_document_nested_past_the_limit_is_refused() {
        let mut loop_ = r#"{"config":{"chainId":36963}}"#.to_string();
        for _ in 0..12 {
            loop_ = format!(r#"{{"cChainGenesis":{loop_}}}"#);
        }
        assert!(read(&loop_).is_err());
    }
}
