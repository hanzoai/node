//! # hanzo-brain
//!
//! Pure-CPU algorithm primitives that the Hanzo Brain shares with every
//! other Hanzo runtime (TypeScript, Python, Go, Rust standalone, C++).
//!
//! This crate is the **node-side canonical home** for the Rust port.
//! The Hanzo Node ([`hanzo-libs/hanzo-runtime`](../hanzo-runtime)) uses
//! it to power `~/.hanzo/brain/brain.db` access through the node's RPC
//! surface, so any agent talking to a Hanzo Node gets brain.recall /
//! brain.search / brain.ingest "for free" without spawning a sidecar.
//!
//! Sibling Hanzo crates the brain integrates with:
//!
//! - [`hanzo-libs/hanzo-consensus`](../hanzo-consensus) — metastable
//!   consensus (Quasar). Storage quorum for multi-node brain replicas.
//! - [`hanzo-libs/hanzo-zap`](../hanzo-zap) — ZAP transport. The wire
//!   format brain operations ride on between nodes.
//! - [`hanzo-libs/hanzo-pqc`](../hanzo-pqc) — post-quantum signatures
//!   the brain uses for wallet-style address-bound recipient blocks.
//! - [`hanzo-libs/hanzo-machine`](../hanzo-machine) — threshold-crypto
//!   primitives. The brain's MMPKE01 multi-recipient envelope can have
//!   per-recipient DEK wraps signed by a threshold quorum.
//! - [`hanzo-libs/hanzo-db-sqlite`](../hanzo-db-sqlite) — SQLite +
//!   FTS5 default storage. The brain's `pages / edges / facts` schema
//!   lives here in solo mode.
//!
//! The same algorithm surface (rrf_fuse, mmr_rerank, dedup_hits, …) is
//! mirrored in `@hanzo/bot-memory` (TS), `hanzo_memory.algorithms`
//! (Python), `bot-go/pkg/brain` (Go), and `hanzo/brain/algorithms.hpp`
//! (C++). A `~/.hanzo/brain/brain.db` written by any runtime is read
//! by every other without translation.

pub mod algorithms;

pub use algorithms::*;
