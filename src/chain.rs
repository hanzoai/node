// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! The chain this node serves, under the name it answers to.
//!
//! Execution is the host's and is not touched: a block is built, verified,
//! accepted and rejected by exactly the interpreter every other node on this
//! network runs, because a second interpreter is a second answer to what a
//! transaction did.
//!
//! What is added is the NAME. Every version string a client can read —
//! `web3_clientVersion`, and what the node reports for the chains it carries —
//! names this node, not the package it was compiled from. A wallet that asked
//! what it was talking to and got the name of a library would have been told
//! about the wrong thing.

use serde_json::{json, Value};

use lux_node::evm::Evm;
use lux_node::vm::{Block, Error, Id, Vm};

/// The chain, and the name it answers under.
pub struct Chain {
    inner: Evm,
    version: String,
}

impl Chain {
    pub fn new(inner: Evm, version: impl Into<String>) -> Self {
        Self { inner, version: version.into() }
    }
}

impl Vm for Chain {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    fn version(&self) -> String {
        self.version.clone()
    }

    /// One method is answered here, and it is the one that asks who is
    /// answering. Everything else is the chain's.
    fn call(&self, method: &str, params: &Value) -> Result<Value, Error> {
        match method {
            "web3_clientVersion" => Ok(json!(self.version)),
            other => self.inner.call(other, params),
        }
    }

    fn build(&self) -> Result<Box<dyn Block>, Error> {
        self.inner.build()
    }

    fn parse(&self, raw: &[u8]) -> Result<Box<dyn Block>, Error> {
        self.inner.parse(raw)
    }

    fn get(&self, id: &Id) -> Result<Box<dyn Block>, Error> {
        self.inner.get(id)
    }

    fn verify(&self, id: &Id) -> Result<(), Error> {
        self.inner.verify(id)
    }

    fn accept(&self, id: &Id) -> Result<(), Error> {
        self.inner.accept(id)
    }

    fn reject(&self, id: &Id) -> Result<(), Error> {
        self.inner.reject(id)
    }

    fn set_preference(&self, id: &Id) -> Result<(), Error> {
        self.inner.set_preference(id)
    }

    fn last_accepted(&self) -> Id {
        self.inner.last_accepted()
    }

    fn block_id_at(&self, height: u64) -> Result<Id, Error> {
        self.inner.block_id_at(height)
    }

    fn health(&self) -> Result<(), Error> {
        self.inner.health()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{genesis, network::MAINNET};

    fn chain() -> Chain {
        let g = lux_node::genesis::parse(genesis::document(&MAINNET)).expect("a genesis");
        Chain::new(Evm::new(g), "hanzod/v9.9.9")
    }

    #[test]
    fn the_chain_answers_under_this_nodes_name() {
        let c = chain();
        assert_eq!(c.version(), "hanzod/v9.9.9");
        assert_eq!(
            c.call("web3_clientVersion", &json!([])).expect("answered"),
            json!("hanzod/v9.9.9")
        );
    }

    #[test]
    fn nothing_else_is_intercepted() {
        // The chain's own answers come through unchanged, including the chain
        // id every transaction on it is signed against.
        let c = chain();
        assert_eq!(
            c.call("eth_chainId", &json!([])).expect("answered"),
            json!(format!("0x{:x}", MAINNET.chain))
        );
        assert!(c.call("eth_blockNumber", &json!([])).is_ok());
        assert!(c.call("not_a_method", &json!([])).is_err());
    }

    #[test]
    fn the_genesis_block_is_the_one_the_chain_computed() {
        // The wrapper forwards state, not just strings: the block the chain
        // starts from is reachable through it.
        let c = chain();
        let head = c.last_accepted();
        assert_eq!(c.get(&head).expect("the genesis block").height(), 0);
        assert_eq!(c.block_id_at(0).expect("height zero"), head);
    }
}
