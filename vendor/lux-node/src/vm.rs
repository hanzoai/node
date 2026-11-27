// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! The seam a chain plugs into.
//!
//! Go states this as `block.ChainVM` and `block.Block`
//! (`luxfi/consensus/engine/chain/block/block.go`), and the engine there
//! depends on a five-method subset of it — `BuildBlock`, `GetBlock`,
//! `ParseBlock`, `LastAccepted`, `SetPreference`. This is that interface, with
//! one deliberate difference of shape and no difference of meaning.
//!
//! THE DIFFERENCE: Go puts `Verify`/`Accept`/`Reject` on the block. Here they
//! are on the VM, keyed by block id. That is not a redesign — it is what the
//! wire already does. luxd's C-chain is an out-of-process plugin and those
//! three calls travel as `MsgBlockVerify`, `MsgBlockAccept`, `MsgBlockReject`
//! carrying an id, because a block handle cannot cross a process boundary. A
//! trait whose methods match the messages that implement them has one shape
//! instead of two.
//!
//! WHY OBJECT-SAFE. A node runs several chains at once — P, X, C, and whatever
//! else is registered — and holds them in one place. That means `dyn Vm`, which
//! means no associated types and `&self` with the lock inside. Go arrives at
//! the same arrangement from the other side: its VMs are interfaces with
//! pointer receivers and their own internal mutexes.

use std::fmt;

pub use lux_consensus::finality::Id;

/// Where a block stands. The values are Go's `choices` package, so a status
/// crossing the wire means the same thing at both ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    Unknown = 0,
    Processing = 1,
    Rejected = 2,
    Accepted = 3,
}

/// What a chain can refuse for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// No block by that id.
    NotFound,
    /// The bytes are not a block of this chain.
    Malformed(String),
    /// The block is a block, and it is wrong.
    Invalid(String),
    /// There is nothing to build: no transactions, nothing to say.
    Empty,
    /// The call is not one this chain answers.
    NoMethod(String),
    /// The caller said something this chain cannot use.
    BadRequest(String),
    /// The chain could not be asked: the link to it failed or timed out. Says
    /// nothing about the block; the same question may be asked again.
    Unavailable(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound => write!(f, "not found"),
            Error::Malformed(why) => write!(f, "malformed: {why}"),
            Error::Invalid(why) => write!(f, "invalid: {why}"),
            Error::Empty => write!(f, "nothing to build"),
            Error::NoMethod(m) => write!(f, "the method {m} does not exist"),
            Error::BadRequest(why) => write!(f, "{why}"),
            Error::Unavailable(why) => write!(f, "unavailable: {why}"),
        }
    }
}

impl std::error::Error for Error {}

/// One block, as consensus sees it.
///
/// Consensus needs an identity, a parent, a height, a time and the bytes. What
/// a block *means* is the chain's business and is deliberately not here.
pub trait Block: Send + Sync {
    fn id(&self) -> Id;
    fn parent(&self) -> Id;
    fn height(&self) -> u64;
    /// Seconds since the epoch.
    fn timestamp(&self) -> u64;
    /// The block's canonical encoding — what a peer is sent and what
    /// [`Vm::parse`] must accept back.
    fn bytes(&self) -> Vec<u8>;
    /// The root of the state this block leaves behind.
    ///
    /// This is the field consensus signs over. Go's engine reads it through an
    /// unexported `canonicalCommitter` interface and its proposervm returns
    /// `ids.Empty` for it today; a chain here answers it for real, because a
    /// certificate over a root nobody computed certifies nothing.
    fn state_root(&self) -> Id;

    /// The root of what this block carries — its transactions.
    ///
    /// The other half of `canonicalCommitter`: a certificate that named a state
    /// but not the payload that produced it would be a certificate two
    /// different blocks could satisfy.
    fn payload_root(&self) -> Id;
}

/// A chain.
///
/// One method per thing the node does to a chain, and no method that is two
/// things. `call` is the exception worth naming: Go returns an `http.Handler`
/// from `NewHTTPHandler` and lets the chain route inside it. Here a chain
/// answers one JSON-RPC method at a time and the node owns the server, so
/// there is one HTTP implementation in the node rather than one per chain.
pub trait Vm: Send + Sync {
    /// The chain's letter — `C`, `P`, `X`. What it is called on the wire and in
    /// a URL.
    fn name(&self) -> &'static str;

    /// The version string this chain reports. Go's C-chain answers
    /// `web3_clientVersion` and `VM.Version()` with the same string.
    fn version(&self) -> String;

    /// Build a block from whatever is pending. [`Error::Empty`] when there is
    /// nothing — Go refuses an empty block on the build path too.
    fn build(&self) -> Result<Box<dyn Block>, Error>;

    /// Read a block off the wire. Must not need the parent: a bootstrapping
    /// node parses blocks whose parents it does not have yet.
    fn parse(&self, raw: &[u8]) -> Result<Box<dyn Block>, Error>;

    fn get(&self, id: &Id) -> Result<Box<dyn Block>, Error>;

    /// Execute the block and check what it claims against what happened.
    ///
    /// Idempotent: Go's engine calls this more than once for one block.
    fn verify(&self, id: &Id) -> Result<(), Error>;

    /// Commit it. Called only after a certificate exists, and never before the
    /// parent was accepted.
    fn accept(&self, id: &Id) -> Result<(), Error>;

    /// Commit a block no certificate stands behind — history, read from an
    /// export — leaving the wait for the device to [`Vm::sync`], once for
    /// every block committed so since the last. The caller builds, signs and
    /// serves nothing on it before `sync` returns, so a crash before then loses
    /// only history nobody outside this process saw. A chain that does not
    /// tell the two apart commits it as [`Vm::accept`] does.
    fn accept_history(&self, id: &Id) -> Result<(), Error> {
        self.accept(id)
    }

    /// Wait for the device to hold every block [`Vm::accept_history`]
    /// committed.
    fn sync(&self) -> Result<(), Error> {
        Ok(())
    }

    /// Drop a block that lost.
    fn reject(&self, id: &Id) -> Result<(), Error>;

    fn set_preference(&self, id: &Id) -> Result<(), Error>;

    fn last_accepted(&self) -> Id;

    fn block_id_at(&self, height: u64) -> Result<Id, Error>;

    fn health(&self) -> Result<(), Error> {
        Ok(())
    }

    /// Answer one JSON-RPC call.
    fn call(&self, method: &str, params: &serde_json::Value) -> Result<serde_json::Value, Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_is_the_number_go_uses() {
        // The wire carries the number, so these cannot drift.
        assert_eq!(Status::Unknown as u8, 0);
        assert_eq!(Status::Processing as u8, 1);
        assert_eq!(Status::Rejected as u8, 2);
        assert_eq!(Status::Accepted as u8, 3);
    }

    #[test]
    fn the_seam_is_object_safe() {
        // The whole reason for the shape: a node holds several chains at once.
        fn takes(_: &[&dyn Vm]) {}
        takes(&[]);
        fn holds(_: Box<dyn Block>) {}
        let _ = holds as fn(Box<dyn Block>);
    }
}
