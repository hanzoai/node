// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! `hanzod` — the Hanzo network node: Hanzo's name and networks, run by the
//! Lux node SDK (github.com/lux-rs/node), which holds the keys, the validator
//! mesh, the EVM, the chain on disk, catch-up and JSON-RPC.
//!
//! The chain answers at `/v1/chain/hanzo` and `/v1/chain/hanzo/rpc`.

use lux_node::spec::{Network, Spec};

/// Hanzo's three networks. The network id is what validators greet each other
/// under; the chain id is what a transaction is signed against. The genesis
/// each starts from is compiled in.
const HANZO: Spec = Spec::new(
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

fn main() -> std::process::ExitCode {
    lux_node::run(&HANZO)
}
