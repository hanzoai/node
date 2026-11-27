// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! The C-chain, as its own program.
//!
//! luxd runs its EVM this way and not as a linked library, which is what lets
//! the chain be replaced, upgraded or crash without taking the validator with
//! it. This is that program: a `lux-evm` chain behind [`plugin::Server`],
//! answering the ZAP messages a node sends.
//!
//! It holds no keys, joins no mesh and votes on nothing. It knows what a block
//! MEANS; which block wins is the node's business, and the socket between them
//! is where that separation is enforced rather than remembered.

use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use lux_node::evm::Evm;
use lux_node::plugin::Server;
use lux_node::pq::Identity;
use lux_node::vm::Vm;

static RUNNING: AtomicBool = AtomicBool::new(true);

extern "C" fn on_signal(_: libc::c_int) {
    RUNNING.store(false, Ordering::SeqCst);
}

const USAGE: &str = "\
lux-evm-plugin — the Lux C-chain, as a plugin

  --bind <addr>      where the node reaches this chain (default 127.0.0.1:9652)
  --data <dir>       this chain's blocks and link identity   (default .lux-c)
  --network <name>   mainnet | localnet               (default localnet)
  --genesis <file>   a C-chain genesis document, instead of --network
  --upgrade <file>   the network's upgrade.json; an entry not run here refuses

The node is given the SAME genesis and the address above; a node running any
other genesis is refused at the door. The link is proved and sealed, so give
the node the identity printed at startup to pin.

This program decides what a block MEANS. Which block wins is the node's, and
the socket between them is where that separation is kept.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("lux-evm-plugin: {why}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let bind = flag(args, "--bind").unwrap_or_else(|| "127.0.0.1:9652".into());

    let mut genesis = match flag(args, "--genesis") {
        Some(path) => {
            let raw =
                std::fs::read_to_string(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
            lux_node::genesis::parse(&raw).map_err(|e| format!("{path}: {e}"))?
        }
        None => match flag(args, "--network").unwrap_or_else(|| "localnet".into()).as_str() {
            "mainnet" => lux_evm::genesis::mainnet(),
            "localnet" => lux_evm::genesis::localnet(),
            other => {
                return Err(format!(
                    "unknown network {other}; try mainnet, localnet, or --genesis <file>"
                ))
            }
        },
    };

    if let Some(path) = flag(args, "--upgrade") {
        let raw = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
        lux_node::genesis::upgrade(&mut genesis, &raw).map_err(|e| format!("{path}: {e}"))?;
    }
    // What luxd refuses to start on, this refuses too.
    genesis.verify()?;
    let refused = lux_node::genesis::refused(&genesis);

    let chain_id = genesis.chain_id;
    let hash = genesis.hash();
    let data = flag(args, "--data").unwrap_or_else(|| ".lux-c".into());
    std::fs::create_dir_all(&data).map_err(|e| format!("cannot make {data}: {e}"))?;
    let identity = Arc::new(
        Identity::open(std::path::Path::new(&data).join("identity.key"))
            .map_err(|e| format!("identity: {e}"))?,
    );
    // The chain as it was left: every block it accepted, executed again.
    let chain = Arc::new(Evm::open(genesis, &data)?);
    let server = Server::new(chain.clone() as Arc<dyn Vm>, identity.clone(), hash.0);
    let stop = server.stop();
    let addr = server.listen(&bind).map_err(|e| format!("cannot serve at {bind}: {e}"))?;

    unsafe {
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
    }

    println!("lux-evm-plugin {}", chain.version());
    println!("  chain id      {chain_id}");
    println!("  height        {}", chain.height());
    println!("  genesis       {hash}");
    if refused.is_empty() {
        println!("  precompiles   every one this chain turns on runs here");
    } else {
        println!("  refuses       calls to {}", refused.join("; "));
    }
    println!("  serving       {addr}");
    println!(
        "  identity      {}  (give the node --plugin-id)",
        alloy_primitives::hex::encode(identity.node())
    );

    while RUNNING.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(100));
    }
    println!("stopping");
    stop.store(false, Ordering::Relaxed);
    Ok(())
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}
