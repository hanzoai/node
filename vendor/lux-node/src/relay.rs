// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! A chain as its peers see it.
//!
//! Two things cross the mesh that are neither votes nor proposals, and both are
//! a chain's business rather than consensus's:
//!
//! - A TRANSACTION submitted to one validator has to reach the others: only
//!   the round's proposer builds ([`crate::proposer`]), so a transaction held
//!   by one validator alone waits for that validator's window. [`Relay`] wraps
//!   the chain the RPC server is handed; a transaction the chain admits is
//!   relayed to every peer — the proposer among them, whoever's window is open
//!   — and one a peer relays is put into this chain's pool, through the chain,
//!   never around it, so it is admitted by the same check.
//! - A PEER THAT IS BEHIND asks for the blocks it missed. It is answered from
//!   the chain and the [`Ledger`], joined on height and block id: a block is
//!   served only where the chain's id at that height is the id the ledger
//!   recorded a certificate for. What is served proves itself — the asker
//!   checks every certificate — so serving decides nothing.
//!
//! The wrapper answers every other call by passing it through, so a chain in
//! this process and a chain in a plugin are relayed the same way.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::host::Host;
use crate::ledger::Ledger;
use crate::mesh::{Outlet, Record, Service};
use crate::vm::{Block, Error, Id, Vm};

/// The JSON-RPC method a transaction arrives by. The one call the wrapper
/// looks at.
const SEND: &str = "eth_sendRawTransaction";

/// A chain, put on a mesh.
pub struct Relay {
    chain: Arc<dyn Vm>,
    ledger: Arc<Ledger>,
    outlet: Outlet,
}

impl Relay {
    /// Put `chain` on `host`'s mesh.
    ///
    /// Returns the chain to serve RPC from and to drive: the same chain, with
    /// the transactions it admits relayed. Peers are answered from it and from
    /// `ledger` from here on.
    pub fn attach(chain: Arc<dyn Vm>, ledger: Arc<Ledger>, host: &Host) -> Arc<Relay> {
        let relay = Arc::new(Relay { chain, ledger, outlet: host.mesh().outlet() });
        host.mesh().serve(relay.clone());
        relay
    }
}

impl Service for Relay {
    fn tip(&self) -> u64 {
        self.chain.get(&self.chain.last_accepted()).map(|b| b.height()).unwrap_or(0)
    }

    fn record(&self, height: u64) -> Option<Record> {
        let (id, cert) = self.ledger.get(height)?;
        // The join. A certificate names a block; the chain says which block it
        // accepted there. Only where they are the same block is there anything
        // to serve.
        if self.chain.block_id_at(height).ok()? != id {
            return None;
        }
        Some(Record { block: self.chain.get(&id).ok()?.bytes(), cert })
    }

    fn transaction(&self, raw: &[u8]) {
        // Admitted by the chain's own check, the way a transaction off RPC is.
        // A refusal is the chain's answer and there is no one to tell it to.
        let _ =
            self.chain.call(SEND, &json!([format!("0x{}", alloy_primitives::hex::encode(raw))]));
    }
}

impl Vm for Relay {
    fn name(&self) -> &'static str {
        self.chain.name()
    }

    fn version(&self) -> String {
        self.chain.version()
    }

    fn build(&self) -> Result<Box<dyn Block>, Error> {
        self.chain.build()
    }

    fn parse(&self, raw: &[u8]) -> Result<Box<dyn Block>, Error> {
        self.chain.parse(raw)
    }

    fn get(&self, id: &Id) -> Result<Box<dyn Block>, Error> {
        self.chain.get(id)
    }

    fn verify(&self, id: &Id) -> Result<(), Error> {
        self.chain.verify(id)
    }

    fn accept(&self, id: &Id) -> Result<(), Error> {
        self.chain.accept(id)
    }

    fn reject(&self, id: &Id) -> Result<(), Error> {
        self.chain.reject(id)
    }

    fn set_preference(&self, id: &Id) -> Result<(), Error> {
        self.chain.set_preference(id)
    }

    fn last_accepted(&self) -> Id {
        self.chain.last_accepted()
    }

    fn block_id_at(&self, height: u64) -> Result<Id, Error> {
        self.chain.block_id_at(height)
    }

    fn health(&self) -> Result<(), Error> {
        self.chain.health()
    }

    /// Answer one call. A transaction the chain ADMITS is relayed; one it
    /// refuses goes nowhere, so a peer is never sent what this node would not
    /// take itself.
    fn call(&self, method: &str, params: &Value) -> Result<Value, Error> {
        let out = self.chain.call(method, params)?;
        if method == SEND {
            if let Some(raw) = params
                .get(0)
                .and_then(Value::as_str)
                .and_then(|h| alloy_primitives::hex::decode(h).ok())
            {
                self.outlet.relay(&raw);
            }
        }
        Ok(out)
    }
}
