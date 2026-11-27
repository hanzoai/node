// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! The Lux EVM in Rust.
//!
//! Go is the network. `luxfi/geth` and `luxfi/evm` decide what a block means,
//! and this crate exists to arrive at the same answer from Rust — same gas,
//! same balances, same roots, same block hashes. Where the two could differ,
//! the Go source is cited in the module that reproduces it.
//!
//! It is built out of four things, and deliberately no more:
//!
//! - [`rules`] — the rules that make Lux not Ethereum, as a value.
//! - [`handler`] — those rules in the two places revm asks about them.
//! - [`gas`] — what a transaction owes before it runs.
//! - [`fee`] — what a block owes, and the header fields Go derives from its
//!   parent; [`block::rule`] holds a header to them.
//! - [`precompile`] — which precompile answers where, and at what price.
//! - [`header`] and [`root`] — how a block is named and what it commits to.
//! - [`state`] and [`trie`] — the world, kept as the tries its root is over.
//!
//! revm is a dependency, not a fork. Three implementations of one protocol are
//! already two more than the protocol needs; a fourth interpreter, maintained
//! here, would be a fifth thing to keep in step. Everything Lux changes is
//! reachable through revm's own seams.
//!
//! ```
//! use lux_evm::{Chain, Precompiles, Profile};
//! use revm::primitives::hardfork::SpecId;
//!
//! // The C-chain: Lux gas accounting from genesis, classical precompiles
//! // admitted, London header shape.
//! let rules = Chain::lux().at(0);
//! assert!(!rules.refunds());
//! let _ = Precompiles::new(SpecId::SHANGHAI, Profile::default());
//! ```

pub mod block;
pub mod fee;
pub mod gas;
pub mod genesis;
pub mod handler;
pub mod header;
pub mod precompile;
pub mod root;
pub mod rules;
pub mod state;
pub mod trie;

pub use block::{Built, Env, Invalid};
pub use genesis::{Alloc, Genesis};
pub use handler::Handler;
pub use precompile::{Module, Op, Precompiles, Profile};
pub use rules::{Chain, Rules};
pub use state::{Account, World};
