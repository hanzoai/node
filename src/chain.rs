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

use std::path::Path;
use std::sync::Mutex;

use serde_json::{json, Value};

use lux_node::evm::Evm;
use lux_node::vm::{Block, Error, Id, Vm};

use crate::blocks::Blocks;

/// The chain, the name it answers under, and the blocks it has accepted.
pub struct Chain {
    inner: Evm,
    version: String,
    blocks: Mutex<Blocks>,
}

impl Chain {
    /// The chain as this validator left it: `inner` at genesis, then every
    /// block in the log at `path` verified and accepted again, in order.
    pub fn open(inner: Evm, version: impl Into<String>, path: &Path) -> Result<Self, String> {
        let at = path.display();
        let (blocks, records) = Blocks::open(path).map_err(|e| format!("{at}: {e}"))?;
        for (i, raw) in records.iter().enumerate() {
            let block = inner
                .parse(raw)
                .map_err(|e| format!("{at}, block {i}: {e}"))?;
            if block.parent() != inner.last_accepted() {
                return Err(format!(
                    "{at}, block {i}: does not extend the block before it"
                ));
            }
            let id = block.id();
            inner
                .verify(&id)
                .and_then(|()| inner.accept(&id))
                .map_err(|e| format!("{at}, block {i}: {e}"))?;
        }
        Ok(Self {
            inner,
            version: version.into(),
            blocks: Mutex::new(blocks),
        })
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

    /// The block reaches the log before it is accepted, so a restart finds
    /// every block this validator accepted. One already accepted is not
    /// written twice.
    fn accept(&self, id: &Id) -> Result<(), Error> {
        let block = self.inner.get(id)?;
        if self.inner.block_id_at(block.height()).ok() != Some(*id) {
            self.blocks
                .lock()
                .map_err(|_| Error::Invalid("the block log is poisoned".into()))?
                .append(&block.bytes())
                .map_err(|e| Error::Invalid(format!("the block did not reach the disk: {e}")))?;
        }
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
    use crate::{
        genesis,
        network::{DEVNET, MAINNET},
    };

    fn chain() -> Chain {
        let g = lux_node::genesis::parse(genesis::document(&MAINNET)).expect("a genesis");
        Chain::open(
            Evm::new(g),
            "hanzod/v9.9.9",
            &crate::blocks::scratch("name"),
        )
        .expect("open")
    }

    /// Devnet's genesis with one more account funded: anvil's public test key
    /// #0, which signed TRANSFER.
    fn funded() -> String {
        let mut doc: Value = serde_json::from_str(genesis::document(&DEVNET)).expect("json");
        doc["alloc"]["f39fd6e51aad88f6f4ce6ab8827279cfffb92266"] =
            json!({"balance": "0xde0b6b3a7640000"});
        doc.to_string()
    }

    /// 1 wei from 0xf39F…2266 to 0x…0001 on chain 36964, nonce 0.
    const TRANSFER: &str = "0x02f86d82906480843b9aca0085174876e8008252089400000000000000000000000000000000000000010180c001a052504fa6f69694143abe94f34e35e6cc1aa4115da5747c09210b64982d8873e4a05a599807e68b86caf7837aab90f478227ef5db946d4fc5a9976ad6bc36d24ed0";

    #[test]
    fn a_restart_resumes_at_the_last_accepted_block() {
        let path = crate::blocks::scratch("resume");
        let doc = funded();
        let open = || {
            let g = lux_node::genesis::parse(&doc).expect("a genesis");
            Chain::open(Evm::new(g), "hanzod/v9.9.9", &path).expect("open")
        };

        let c = open();
        let genesis = c.last_accepted();
        c.call("eth_sendRawTransaction", &json!([TRANSFER]))
            .expect("accepted into the pool");
        let block = c.build().expect("a block");
        let id = block.id();
        c.verify(&id).expect("verify");
        c.accept(&id).expect("accept");
        c.accept(&id).expect("accepting again is a no-op");
        assert_ne!(c.last_accepted(), genesis);
        drop(c);

        let again = open();
        assert_eq!(
            again.last_accepted(),
            id,
            "the chain came back where it was"
        );
        assert_eq!(again.get(&id).expect("held").height(), 1);
        assert_eq!(again.block_id_at(1).expect("height 1"), id);
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
