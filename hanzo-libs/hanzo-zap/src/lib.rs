//! ZAP (Zero-copy Agent Protocol) wire protocol.
//!
//! Compatible with the Go `github.com/luxfi/zap` v0.2.0 binary format and
//! the Rust implementation in `hanzo-dev/core/src/zap_wire.rs`.
//!
//! Every Hanzo product embeds this crate natively — no sidecars.
//!
//! Wire format:
//!   Frame: [4-byte LE length][message bytes]
//!   Message header (16 bytes): magic(4) + version(2) + flags(2) + root_offset(4) + size(4)
//!   Object fields: inline primitives, (relOffset:i32 + length:u32) for text/bytes

mod wire;
mod server;

pub use wire::*;
pub use server::*;
