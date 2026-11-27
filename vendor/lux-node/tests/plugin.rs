// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! This node driving a chain luxd wrote.
//!
//! The point of the plugin boundary is that twelve Go chains do not have to be
//! ported to be run. That claim is only worth something against a real plugin
//! binary, so this test takes one — `LUX_VM_PLUGIN` names it — starts it the
//! way luxd starts one, and drives it: initialize on a real genesis, read the
//! block it accepted, parse those bytes back, ask for the id at height zero,
//! prefer it, and ask it to build.
//!
//! A binary is not something to commit: it is fifty megabytes out of
//! `ghcr.io/luxfi/node`'s `/luxd/build/plugins/`. Naming it in the environment
//! is the same shape `tests/import.rs` uses for the export it compares
//! against, and for the same reason — the artifact is the Go node's, not this
//! crate's.

use lux_node::plugin::{Child, Start};
use lux_node::vm::Block;

fn plugin() -> std::path::PathBuf {
    let Some(path) = std::env::var_os("LUX_VM_PLUGIN") else {
        panic!(
            "LUX_VM_PLUGIN is not set to a Go VM plugin binary. This test drives a real one and \
             cannot be run without it; take one from /luxd/build/plugins/ in the \
             ghcr.io/luxfi/node image."
        );
    };
    std::path::PathBuf::from(path)
}

/// The genesis a localnet C-chain starts from, and an upgrade config the
/// shipped plugin accepts.
///
/// `tests/vectors/luxd_localnet_upgrade.json` is NOT that config: it carries a
/// `_comment` key, and the plugin decodes upgrade json with unknown fields
/// disallowed — and once that is removed it refuses the precompile list for
/// re-enabling at timestamp zero what the genesis already enabled. The genesis
/// alone is what a plugin needs, so the upgrade sent here is empty.
fn genesis() -> Vec<u8> {
    std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/vectors/luxd_localnet_cchain.json"))
        .expect("the localnet genesis is part of the tree")
}

fn start_in(dir: &std::path::Path) -> Start {
    Start {
        network: 1337,
        chain: [7u8; 32],
        node: [0u8; 20],
        genesis: genesis(),
        upgrade: b"{}".to_vec(),
        config: Vec::new(),
        data_dir: dir.to_string_lossy().into_owned(),
    }
}

#[test]
fn this_node_starts_a_go_chain_and_drives_it() {
    let dir = std::env::temp_dir().join(format!("lux-vm-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("chain data dir");

    let chain = Child::start(&plugin(), &start_in(&dir)).expect("the chain starts");

    // It says what it is.
    assert!(
        chain.version_line().starts_with('v'),
        "the chain named itself {:?}",
        chain.version_line()
    );
    let health = chain.health().expect("health");
    assert!(health.contains("\"healthy\":true"), "the chain says {health}");

    // It accepted a block before this node ever spoke to it: its genesis.
    let accepted = chain.accepted().clone();
    assert_eq!(accepted.height(), 0, "the accepted block is not the genesis");
    assert_ne!(accepted.id(), [0u8; 32]);
    assert!(!accepted.bytes().is_empty());
    assert_eq!(accepted.parent(), [0u8; 32], "the genesis has no parent");

    // The state root is not on Go's wire — it is the fourth field of the RLP
    // header inside the block bytes, and reading it is what lets a block this
    // node did not execute go into a certificate.
    assert_ne!(accepted.state_root(), [0u8; 32], "no state root came out of the block bytes");

    // The same block, asked for three ways, is the same block.
    let fetched = chain.get(&accepted.id()).expect("get");
    assert_eq!(fetched.id(), accepted.id());
    assert_eq!(fetched.state_root(), accepted.state_root());
    let parsed = chain.parse(&accepted.bytes()).expect("parse");
    assert_eq!(parsed.id(), accepted.id(), "parsing its own bytes named another block");
    assert_eq!(chain.at_height(0).expect("at height"), accepted.id());

    // And it will answer a build. A chain with no transactions has nothing to
    // put in a block and says so with an empty answer rather than an error —
    // which is a block that does not exist, not a block that is empty.
    match chain.build().expect("build is answered") {
        None => {}
        Some(block) => {
            assert_eq!(block.height(), 1, "a built block follows the accepted one");
            assert_eq!(block.parent(), accepted.id());
            chain.verify(&block.bytes()).expect("the chain verifies its own block");
            chain.accept(&block.id()).expect("accept");
            let back = chain.get(&block.id()).expect("read it back");
            assert_eq!(back.id(), block.id());
        }
    }

    eprintln!(
        "drove {} — accepted {} at height {}, state root {}",
        chain.version_line(),
        alloy_primitives::hex::encode(accepted.id()),
        accepted.height(),
        alloy_primitives::hex::encode(accepted.state_root()),
    );
    let _ = std::fs::remove_dir_all(&dir);
}
