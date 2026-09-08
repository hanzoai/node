// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! hanzod — the Hanzo network node.
//!
//! It holds this validator's keys, joins the network's validator mesh, decides
//! blocks under the committee that network published, executes them, and serves
//! JSON-RPC.
//!
//! THE ORDER IS NOT NEGOTIABLE. A block is verified before this validator signs
//! for it, certified before it is accepted, and accepted only once a
//! certificate exists — so this node never signs for a state it has not
//! computed and never holds a state nobody agreed to.
//!
//! WHAT IS COMPILED IN IS COMPILED IN. The networks and the genesis each starts
//! from are constants, not files: a node that read its own identity from
//! somewhere editable is a node that can be pointed at a fork by editing it. An
//! unknown network name is an error and never a default, because a default
//! would join the wrong network on a typo and look like it had started
//! correctly.
//!
//! SHUTDOWN IS A SIGNAL, AND IT IS CLEAN. `SIGTERM` and `SIGINT` set a flag the
//! loops read; the server stops and the journal is already on disk. A validator
//! that died between recording a vote and sending it has to be able to come
//! back and send it.

mod chain;
mod committee;
mod genesis;
mod network;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use lux_node::engine::{self, Committee, Engine, Failed, Peers};
use lux_node::evm::Evm;
use lux_node::host::{Config, Host};
use lux_node::journal::Journal;
use lux_node::keys::Keys;
use lux_node::rpc::Rpc;
use lux_node::vm::Vm;

use crate::chain::Chain;
use crate::committee::Rule;
use crate::network::Network;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Set by the signal handler, read by every loop. The only thing a handler is
/// allowed to touch.
static RUNNING: AtomicBool = AtomicBool::new(true);

extern "C" fn on_signal(_: libc::c_int) {
    RUNNING.store(false, Ordering::SeqCst);
}

const USAGE: &str = "\
hanzod — a Hanzo validator

  --network <name>     mainnet | testnet | devnet          (default mainnet)
  --data <dir>         where this validator's keys live    (default ~/.hanzod)
  --committee <file>   the validators of this network, one published line each
  --peers <a,b,...>    every validator's mesh address, in committee order
  --rpc <addr>         JSON-RPC address                    (default 127.0.0.1:9630)
  --mesh <addr>        validator mesh address              (default 127.0.0.1:9631)
  --genesis <file>     a genesis document, instead of this network's own
  --chain <hex32>      the 32 bytes this chain was issued, in place of the ones
                       derived from its genesis
  --produce <ms>       how often to look for work                  (default 200)
  --archive <url>      serve state this node does not keep from an archive
  --publish            print this validator's committee line, and exit
  --version            print the version, and exit
  --help               print this text, and exit

A validator is NAMED by the key it proves on every link, so there is no name to
claim without the key and none to publish separately. A committee is the list
of what its validators published, so bringing a network up takes two steps:

  1. hanzod --data n0 --publish >> committee.txt   (once per validator)
  2. hanzod --data n0 --committee committee.txt --peers <every mesh address>

A certificate carries a supermajority of the seats: three of four, four of five.
Below four seats there is no supermajority that survives one compromised key,
and the rule refuses rather than pretend.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--version") {
        println!("hanzod {VERSION}");
        return ExitCode::SUCCESS;
    }
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("hanzod: {why}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    // The keys first: everything this node is called is named by them.
    let data = match flag(args, "--data") {
        Some(dir) => PathBuf::from(dir),
        None => home()?.join(".hanzod"),
    };
    let keys = Keys::open(&data)?;

    // A published line is the whole answer, so it goes out alone: this is the
    // output that gets appended to a committee file.
    if args.iter().any(|a| a == "--publish") {
        println!("{}", keys.publish());
        return Ok(());
    }

    let name = flag(args, "--network").unwrap_or_else(|| "mainnet".into());
    let net =
        Network::named(&name).ok_or_else(|| format!("{name} is not a network; try {}", Network::names()))?;
    let rpc_at = address(args, "--rpc", "127.0.0.1:9630")?;
    let mesh_at = address(args, "--mesh", "127.0.0.1:9631")?;
    if rpc_at == mesh_at {
        return Err("--rpc and --mesh cannot be one address".into());
    }
    let produce = Duration::from_millis(number(args, "--produce", 200)?);

    // A file wins over the compiled-in document, and it is checked against this
    // network before it is used.
    let (source, raw) = match flag(args, "--genesis") {
        Some(path) => {
            let raw = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
            (path, raw)
        }
        None => ("this network's own".to_string(), genesis::document(&net).to_string()),
    };
    genesis::check(&raw, &net).map_err(|e| format!("{source}: {e}"))?;
    let start = lux_node::genesis::parse(&raw).map_err(|e| format!("{source}: {e}"))?;

    // The chain's 32-byte name. A network issues it when it creates the chain;
    // with none issued here it is derived from the genesis, which is only good
    // for a set of nodes talking to each other.
    let (chain_name, issued) = match flag(args, "--chain") {
        Some(text) => (word(&text).map_err(|e| format!("--chain: {e}"))?, true),
        None => (engine::stand_in(start.hash().0), false),
    };

    let path = flag(args, "--committee")
        .ok_or("--committee names the validators of this network; --publish makes a line for it")?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let committee = Committee::read(&text).map_err(|e| format!("{path}: {e}"))?;
    let rule = Rule::of(&committee.set().map_err(|e| format!("{path}: {e}"))?);

    let me = keys.identity.node();
    let seat = committee.seat(&me).ok_or_else(|| {
        format!("this validator ({}) is not in {path}; add the line --publish prints", hex(&me))
    })?;

    let served: Arc<dyn Vm> = Arc::new(Chain::new(Evm::new(start), version()));
    let engine = Engine::new(me, keys.consensus.clone(), &committee, 1, chain_name)?;
    let host = Host::bind(Config { identity: Arc::new(keys.identity), bind: mesh_at })
        .map_err(|e| format!("cannot bind the validator mesh at {mesh_at}: {e}"))?;
    let mut journal = Journal::open(data.join("votes"))
        .map_err(|e| format!("cannot open the signing journal: {e}"))?;

    // Signals before anything long-running, so a Ctrl-C during startup is still
    // a clean stop.
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
    }

    let archive = flag(args, "--archive");
    let serving = Rpc::new(net.chain, version())
        .with(served.clone())
        .with_archive_rpc(archive.clone())
        .listen(rpc_at)
        .map_err(|e| format!("cannot serve JSON-RPC at {rpc_at}: {e}"))?;

    println!("hanzod {VERSION}");
    println!("network     {}", net.name);
    println!("network id  {}", net.id);
    println!("chain id    {}", net.chain);
    println!(
        "chain       {}{}",
        hex(&chain_name),
        if issued { "" } else { "  (derived from the genesis; none was issued)" }
    );
    println!("genesis     {source}, {}", plural(genesis::accounts(&raw), "account"));
    println!("data        {}", data.display());
    println!("validator   {}  seat {seat} of {}", hex(&me), rule.members);
    println!("committee   {} carrying {}", plural(rule.members, "validator"), rule.weight);
    println!("quorum      {} of {}", rule.signers, rule.members);
    println!("stake       more than {} of {}", rule.stake, rule.weight);
    println!("mesh        {}", host.addr());
    println!("rpc         http://{}/v1/chain/c", serving.addr());
    if let Some(ref at) = archive {
        println!("archive     {at}");
    }

    // Reach the other validators. The mesh is not the quorum: `connect` reports
    // who it found and the rule already says how many are enough.
    if let Some(list) = flag(args, "--peers") {
        let peers = parse_peers(&list, &committee)?;
        let reached = host.connect(&peers, Duration::from_secs(10));
        println!("peers       {reached} of {}", peers.len().saturating_sub(1));
    }
    let need = engine.quorum();
    if host.peer_count() + 1 < need {
        println!(
            "waiting     {} of {need} validators reachable; heights advance when the rest join",
            host.peer_count() + 1
        );
    }

    let round = produce * 10;
    while RUNNING.load(Ordering::SeqCst) {
        // A block a peer pushed comes first. Following is cheaper than
        // proposing, and a node that proposed while a round was already in
        // flight would be the second statement at one height.
        let arrived = host.mesh().arrived();
        let mut worked = false;
        for raw in arrived {
            worked = true;
            match engine.follow(served.as_ref(), &host, &mut journal, &raw, round) {
                Ok((height, cert)) => report("accepted", height, served.as_ref(), cert.votes.len()),
                Err(e) => eprintln!("following a block: {e}"),
            }
        }
        if worked {
            continue;
        }
        match engine.propose(served.as_ref(), &host, &mut journal, round) {
            Ok((height, cert)) => report("produced", height, served.as_ref(), cert.votes.len()),
            // Nothing to do. Not an error: a chain with no transactions is a
            // chain at rest.
            Err(Failed::Chain(lux_node::vm::Error::Empty)) => std::thread::sleep(produce),
            Err(Failed::NoQuorum { have, need }) => {
                eprintln!("{have} of {need} votes; dropping the round");
            }
            Err(e) => {
                eprintln!("proposing: {e}");
                std::thread::sleep(produce);
            }
        }
    }

    println!("stopping");
    serving.stop();
    host.mesh().close();
    Ok(())
}

/// One line per decided block, carrying the numbers that matter.
fn report(what: &str, height: u64, vm: &dyn Vm, votes: usize) {
    let state = vm.get(&vm.last_accepted()).map(|b| hex(&b.state_root())).unwrap_or_default();
    println!("{what}  height {height}  state {state}  {votes} votes");
}

fn version() -> String {
    format!("hanzod/v{VERSION}")
}

/// Render bytes the way a published line spells them.
fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Read 32 bytes of hexadecimal, refusing anything that is not exactly that.
fn word(text: &str) -> Result<[u8; 32], String> {
    let raw = text.strip_prefix("0x").unwrap_or(text);
    if raw.len() != 64 {
        return Err(format!("{} hexadecimal digits, not 64", raw.len()));
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&raw[i * 2..i * 2 + 2], 16)
            .map_err(|_| "not hexadecimal".to_string())?;
    }
    Ok(out)
}

/// The value after `flag`, if it is there.
fn flag(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

/// A number-valued flag, or its default.
fn number(args: &[String], name: &str, default: u64) -> Result<u64, String> {
    match flag(args, name) {
        None => Ok(default),
        Some(text) => text.parse().map_err(|_| format!("{name}: {text} is not a number")),
    }
}

/// An address-valued flag, or its default. Parsed here so a bad address is an
/// error at startup rather than at bind, when it is half running.
fn address(args: &[String], name: &str, default: &str) -> Result<SocketAddr, String> {
    flag(args, name)
        .unwrap_or_else(|| default.into())
        .parse()
        .map_err(|e| format!("{name}: {e}"))
}

/// Where a person's files live.
fn home() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|h| !h.as_os_str().is_empty())
        .ok_or_else(|| "HOME is not set, so --data has no default; pass it".into())
}

/// `1 validator`, `2 validators`.
fn plural(n: usize, thing: &str) -> String {
    match n {
        1 => format!("1 {thing}"),
        _ => format!("{n} {thing}s"),
    }
}

/// Every validator's mesh address, in committee order.
///
/// A peer list is positions, not names: the names are already fixed by the
/// committee and the handshake proves them, so an address that turns out to
/// belong to someone else is refused rather than believed.
fn parse_peers(list: &str, committee: &Committee) -> Result<Peers, String> {
    let mut peers = Peers::new();
    for (i, raw) in list.split(',').map(str::trim).filter(|s| !s.is_empty()).enumerate() {
        let who = committee
            .members()
            .get(i)
            .ok_or("--peers has more addresses than the committee has validators")?
            .node;
        let addr: SocketAddr = raw.parse().map_err(|e| format!("--peers {raw}: {e}"))?;
        peers.insert(who, addr);
    }
    Ok(peers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_flag_takes_the_value_after_it() {
        let a = argv(&["--network", "devnet", "--produce", "700"]);
        assert_eq!(flag(&a, "--network").as_deref(), Some("devnet"));
        assert_eq!(number(&a, "--produce", 200), Ok(700));
        assert_eq!(number(&a, "--absent", 3), Ok(3));
        assert_eq!(flag(&a, "--absent"), None);
    }

    #[test]
    fn a_flag_with_nothing_after_it_is_not_a_value() {
        // `--network` last on the line must not silently become a default.
        assert_eq!(flag(&argv(&["--network"]), "--network"), None);
    }

    #[test]
    fn a_bad_number_is_an_error_not_a_default() {
        assert!(number(&argv(&["--produce", "often"]), "--produce", 200).is_err());
    }

    #[test]
    fn a_bad_address_is_refused_before_anything_binds() {
        assert!(address(&argv(&["--rpc", "127.0.0.1"]), "--rpc", "127.0.0.1:9630").is_err());
        assert!(address(&argv(&["--rpc", "9630"]), "--rpc", "127.0.0.1:9630").is_err());
        assert_eq!(
            address(&argv(&[]), "--rpc", "127.0.0.1:9630").unwrap(),
            "127.0.0.1:9630".parse::<SocketAddr>().unwrap()
        );
    }

    #[test]
    fn a_chain_name_is_thirty_two_bytes_or_it_is_nothing() {
        assert_eq!(word(&"00".repeat(32)).unwrap(), [0u8; 32]);
        assert_eq!(word(&format!("0x{}", "ff".repeat(32))).unwrap(), [0xffu8; 32]);
        assert!(word(&"00".repeat(31)).is_err(), "too short");
        assert!(word(&"00".repeat(33)).is_err(), "too long");
        assert!(word(&"zz".repeat(32)).is_err(), "not hexadecimal");
        assert!(word("").is_err());
    }

    #[test]
    fn bytes_render_the_way_a_published_line_spells_them() {
        assert_eq!(hex(&[0x00, 0x0f, 0xa0, 0xff]), "000fa0ff");
    }

    #[test]
    fn the_usage_names_every_network() {
        for n in network::ALL {
            assert!(USAGE.contains(n.name), "usage does not offer {}", n.name);
        }
    }

    #[test]
    fn plurals_read_like_english() {
        assert_eq!(plural(1, "validator"), "1 validator");
        assert_eq!(plural(4, "validator"), "4 validators");
        assert_eq!(plural(0, "account"), "0 accounts");
    }
}
