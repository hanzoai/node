//! A Lux node, in Rust — and the node every brand runs.
//!
//! It executes real EVM blocks through revm, certifies the state root those
//! blocks actually produce, and serves JSON-RPC. Two binaries, because luxd is
//! two: `luxd` is the validator and `lux-evm-plugin` is the C-chain in its own
//! process, reached over ZAP.
//!
//! A BRAND IS A SPEC AND ONE CALL. [`run`] is the whole of a node's `main`;
//! a [`Spec`] says what differs — the brand's name, its client, its networks.
//! Lux's own binary is [`spec::LUX`] and the same call:
//!
//! ```no_run
//! fn main() -> std::process::ExitCode {
//!     lux_node::run(&lux_node::spec::LUX)
//! }
//! ```
//!
//! - [`spec`] is what a brand states; [`run`] is flags, keys, committee, mesh,
//!   journal, RPC and the loop that steps the engine, once.
//! - [`vm`] is the seam a chain plugs into — Go's `block.ChainVM`.
//! - [`evm`] is the C-chain on `lux-evm`; [`genesis`] reads what it starts from.
//! - [`import`] reads a chain back out of its RLP export, and says plainly that
//!   what it read carries no certificate; [`gate`] keeps that on disk, so a
//!   node that imported does not build or sign until a certificate anchors it.
//! - [`engine`] carries a block from built to accepted, and holds the committee;
//!   [`proposer`] is who may propose at each height and slot, and how a
//!   proposal proves it.
//! - [`plugin`] is that seam across a socket, sealed and on Go's own numbers.
//! - [`pq`] is ML-DSA-65 identity and ML-KEM-768 sessions; [`keys`] is what a
//!   validator holds and publishes.
//! - [`host`], [`mesh`] and [`journal`] are the validator link: who is reached,
//!   what crosses it, and what this node has already signed.
//! - [`ledger`] keeps the certificate every accepted block was accepted on;
//!   [`relay`] serves those blocks to a peer that is behind and carries
//!   transactions between pools.
//! - [`rpc`] is one bounded HTTP server for every registered chain.
//!
//! The finality rule stays in `lux-consensus`. A node holding its own copy of
//! the rule would be a second implementation of it.

pub mod engine;
pub mod evm;
pub mod gate;
pub mod genesis;
pub mod held;
pub mod host;
pub mod import;
pub mod journal;
pub mod keys;
pub mod ledger;
pub mod luxd;
pub mod mesh;
pub mod plugin;
pub mod pq;
pub mod precompile;
pub mod proposer;
pub mod relay;
pub mod rpc;
pub mod run;
pub mod spec;
pub mod validators;
pub mod vm;
pub mod wire;

pub use engine::{Engine, Event, Step};
pub use gate::Gate;
pub use held::Held;
pub use host::{Config, Host};
pub use journal::{Journal, Refused};
pub use keys::Keys;
pub use ledger::Ledger;
pub use mesh::{Mesh, Proposal, LINK_MAX};
pub use pq::Identity;
pub use relay::Relay;
pub use rpc::Rpc;
pub use run::{run, Node};
pub use spec::{Network, Spec};
pub use vm::{Block, Vm};
