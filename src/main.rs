// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! hanzod — the Hanzo network node.
//!
//! It answers the questions a node is asked before it can do anything: which
//! network this is and what number that network is, where this validator's key
//! lives, what it is called, what it binds, and — given the lines the other
//! validators published — the rule the committee decides under.
//!
//! EVERY ANSWER IS COMPILED IN OR REFUSED. The networks are three constants,
//! not a file: a node that read its own identity from somewhere editable is a
//! node that can be pointed at a fork by editing it. An unknown network name is
//! an error and never a default, because a default would join the wrong network
//! on a typo and look like it had started correctly.

mod committee;
mod genesis;
mod network;
mod validator;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use network::Network;
use validator::Validator;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "\
hanzod — a Hanzo validator

  --network <name>     mainnet | testnet | devnet          (default mainnet)
  --data <dir>         where this validator's key lives    (default ~/.hanzod)
  --rpc <addr>         JSON-RPC address                    (default 127.0.0.1:9630)
  --mesh <addr>        validator mesh address              (default 127.0.0.1:9631)
  --weight <n>         the stake published behind this validator   (default 1)
  --genesis <file>     a genesis document to check against this network
  --committee <file>   the validators of this network, one published line each
  --publish            print this validator's committee line, and exit
  --version            print the version, and exit
  --help               print this text, and exit

A genesis is checked, not trusted: the document must state the number of the
network it was named with. A genesis that disagrees is refused here, where the
refusal is one line, rather than at the first block nobody else accepts.

A validator is NAMED by the key it signs with — the name is that key's digest,
so there is no name to claim without the key and none to publish separately.

Bringing a network up takes two steps, because a committee is the list of what
its validators published and nothing can publish before it exists:

  1. hanzod --data n0 --publish >> committee.txt   (once per validator)
  2. hanzod --data n0 --committee committee.txt

A certificate needs a committee of at least four: below that one compromised
key forges it, and the rule refuses rather than pretend.
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
    // The key first: everything this node is called is named by it.
    let data = match flag(args, "--data") {
        Some(dir) => PathBuf::from(dir),
        None => home()?.join(".hanzod"),
    };
    let me = Validator::open(&data)?;
    let weight = number(args, "--weight", 1)?;
    if weight == 0 {
        return Err("--weight 0 is a validator that cannot carry a vote".into());
    }

    // A published line is the whole answer, so it goes out alone: this is the
    // output that gets appended to a committee file.
    if args.iter().any(|a| a == "--publish") {
        println!("{}", me.line(weight));
        return Ok(());
    }

    let name = flag(args, "--network").unwrap_or_else(|| "mainnet".into());
    let net = Network::named(&name).ok_or_else(|| {
        format!("{name} is not a network; try {}", Network::names())
    })?;
    let rpc = address(args, "--rpc", "127.0.0.1:9630")?;
    let mesh = address(args, "--mesh", "127.0.0.1:9631")?;
    if rpc == mesh {
        return Err("--rpc and --mesh cannot be one address".into());
    }

    println!("hanzod {VERSION}");
    println!("network    {}", net.name);
    println!("id         {}", net.id);
    println!("data       {}", data.display());
    println!("rpc        {rpc}");
    println!("mesh       {mesh}");
    println!("validator  {}", validator::hex(&me.name()));
    println!("key        {}", validator::hex(&me.key()));

    if let Some(path) = flag(args, "--genesis") {
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {path}: {e}"))?;
        let doc = genesis::read(&raw).map_err(|e| format!("{path}: {e}"))?;
        doc.agrees_with(&net).map_err(|e| format!("{path}: {e}"))?;
        println!("genesis    {path}");
        println!("states     {}", doc.id);
        println!("allocates  {}", plural(doc.accounts, "account"));
    }

    let Some(path) = flag(args, "--committee") else {
        return Ok(());
    };
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {path}: {e}"))?;
    let set = committee::read(&text).map_err(|e| format!("{path}: {e}"))?;
    let rule = committee::Rule::of(&set);

    println!("committee  {} carrying {}", plural(rule.members, "validator"), rule.weight);
    println!("quorum     {} of {}", rule.quorum, rule.members);
    println!("signers    {} distinct", rule.signers);
    println!("depth      {}", plural(rule.depth as usize, "round"));
    println!("crash      {} survived", plural(rule.crash as usize, "fault"));
    println!("export     {} weight", rule.export);
    match set.contains(&me.name()) {
        true => println!("seat       held"),
        // Said plainly, because it is the difference between a validator and a
        // spectator, and a node that shrugged at it would look like it had
        // joined.
        false => println!("seat       none — this validator is not in {path}"),
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_flag_takes_the_value_after_it() {
        let a = argv(&["--network", "devnet", "--weight", "7"]);
        assert_eq!(flag(&a, "--network").as_deref(), Some("devnet"));
        assert_eq!(number(&a, "--weight", 1), Ok(7));
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
        assert!(number(&argv(&["--weight", "many"]), "--weight", 1).is_err());
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
    fn the_usage_names_every_network() {
        for n in network::ALL {
            assert!(USAGE.contains(n.name), "usage does not offer {}", n.name);
        }
    }

    #[test]
    fn plurals_read_like_english() {
        assert_eq!(plural(1, "validator"), "1 validator");
        assert_eq!(plural(4, "validator"), "4 validators");
        assert_eq!(plural(0, "fault"), "0 faults");
    }
}
