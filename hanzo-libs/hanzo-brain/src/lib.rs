//! # hanzo-brain
//!
//! Pure-CPU algorithm primitives that the Hanzo Brain shares with every
//! other Hanzo runtime (TypeScript, Python, Go, Rust standalone, C++).
//!
//! This crate is the **node-side canonical home** for the Rust port. It
//! lives at `hanzo-libs/hanzo-brain/` inside the `hanzoai/node` Cargo
//! workspace and is consumed by the node's runtime to power
//! `~/.hanzo/brain/brain.db` access through the node's RPC surface, so
//! any agent talking to a Hanzo Node gets `brain.recall` / `brain.search`
//! / `brain.ingest` without spawning a sidecar.
//!
//! Sibling workspace crates the brain integrates with:
//!
//! - `hanzo-runtime` — workspace crate. Hosts the node's RPC surface and
//!   wires the brain algorithms into `brain.*` RPC methods.
//! - `hanzo-consensus` — workspace crate. Metastable consensus (Quasar).
//!   Storage quorum for multi-node brain replicas.
//! - `hanzo-zap` — workspace crate. ZAP transport. The wire format brain
//!   operations ride on between nodes.
//! - `hanzo-pqc` — workspace crate. Post-quantum signatures the brain
//!   uses for wallet-style address-bound recipient blocks.
//! - `hanzo-machine` — workspace crate. Threshold-crypto primitives. The
//!   brain's MMPKE01 multi-recipient envelope can have per-recipient DEK
//!   wraps signed by a threshold quorum.
//! - `hanzo-db-sqlite` — workspace crate. SQLite + FTS5 default storage.
//!   The brain's `pages / edges / facts` schema lives here in solo mode.
//!
//! The same algorithm surface (`rrf_fuse`, `mmr_rerank`, `dedup_hits`, …)
//! is mirrored in `@hanzo/bot-memory` (TS), `hanzo_memory.algorithms`
//! (Python), `bot-go/pkg/brain` (Go), and `hanzo/brain/algorithms.hpp`
//! (C++). A `~/.hanzo/brain/brain.db` written by any runtime is read by
//! every other without translation.

pub mod algorithms;

pub use algorithms::*;
