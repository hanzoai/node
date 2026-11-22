// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! `hanzo-chain` — the Hanzo network validator library, embedded directly into
//! the unified `hanzod` binary (and wrapped by the standalone `hanzo-chain` bin).
//!
//! Powered by the Lux node SDK (`lux-node`), which holds the keys, validator
//! mesh, EVM, chain journal, catch-up, and JSON-RPC at `/v1/chain/hanzo/rpc`.

use lux_node::spec::{Network, Spec};

/// Hanzo's three networks. The network id is what validators greet each other
/// under; the chain id is what a transaction is signed against. The genesis
/// each starts from is compiled in.
pub const HANZO: Spec = Spec::new(
    "hanzo",
    "hanzod",
    env!("CARGO_PKG_VERSION"),
    &[
        Network {
            name: "mainnet",
            id: 1,
            chain: 36963,
            genesis: include_str!("../genesis/mainnet.json"),
        },
        Network {
            name: "testnet",
            id: 2,
            chain: 36962,
            genesis: include_str!("../genesis/testnet.json"),
        },
        Network {
            name: "devnet",
            id: 3,
            chain: 36964,
            genesis: include_str!("../genesis/devnet.json"),
        },
    ],
);

/// Run the Hanzo validator using process arguments (`std::env::args`).
pub fn run() -> std::process::ExitCode {
    lux_node::run(&HANZO)
}
