//! ZAP (Zero-copy Agent Protocol) — the arena wire format.
//!
//! Conformant to `github.com/luxfi/zap` v1.2.6, the version `luxfi/node`
//! pins; its codec is byte-identical through v1.2.9. Go is the network, and
//! `tests/conformance.rs` holds Go's own bytes for the whole codec surface
//! along with its answers on malformed frames.
//!
//! Every Hanzo product embeds this crate natively — no sidecars.
//!
//! Wire format:
//!   Frame:   [4-byte LE length][message bytes]
//!   Header:  magic(4) + version(2) + flags(2) + root offset(4) + size(4)
//!   Fields:  scalars inline, little-endian, objects aligned to 8
//!            bytes/text as (relative offset u32, length u32) — forward only
//!            objects/lists as a SIGNED relative offset, so a child written
//!            before its parent is reachable backwards
//!   Flags:   message type in the high byte, header flags in the low
//!
//! Not to be confused with the framed multiplexed RPC in `luxfi/api/zap`,
//! which shares the name and nothing else: that one is big-endian, has a
//! 5-byte frame header, and is what `lux::zap` implements in C++. The two
//! cannot read each other, and neither is a version of the other.

mod client;
mod server;
mod wire;

pub use client::*;
pub use server::*;
pub use wire::*;
