// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! A running node, from a [`Spec`] and a command line.
//!
//! Binds a validator mesh, brings up the chain, serves JSON-RPC, and decides
//! blocks ([`crate::engine`]): the proposer whose window is open builds, every
//! validator executes before it says anything and signs once a height, and a
//! block is accepted only on a certificate — because a block this node has not
//! executed is one it must not sign for, and a block without a certificate is
//! one it must not accept.
//!
//! ONE PATH FOR EVERY BRAND. Lux's binary and a brand's are this module with a
//! different [`Spec`]; nothing below knows which. What differs between them is
//! the name the chain answers to, the client's name, and which networks exist.
//!
//! WHO THE VALIDATORS ARE, TWO WAYS. As an L1, a network names its validators
//! in a file of what each one published (`--committee`). As an L2, the Lux
//! P-chain's registration for the network names them (`--pchain`), and that is
//! the default for a brand: its validators do not co-validate the Lux primary
//! network. The L2 source is refused, precisely, until the P-chain serves what
//! a committee is built from — see [`L2`].
//!
//! SHUTDOWN IS A SIGNAL, AND IT IS CLEAN. `SIGTERM` and `SIGINT` set a flag;
//! the loop sees it, the server stops, the journal is already on disk. Nothing
//! here is killed mid-write, because a validator that dies between recording a
//! vote and sending it has to be able to come back and send it.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use alloy_primitives::B256;

use crate::engine::{self, Committee, Engine, Event, Peers};
use crate::evm::Evm;
use crate::gate::Gate;
use crate::held::Held;
use crate::host::{Config, Host};
use crate::journal::Journal;
use crate::keys::Keys;
use crate::ledger::Ledger;
use crate::relay::Relay;
use crate::rpc::{Rpc, Serving};
use crate::spec::{self, Network, Spec};
use crate::vm::{Id, Vm};

/// Where JSON-RPC listens when `--rpc` does not say.
pub const RPC: &str = "127.0.0.1:9650";
/// Where the validator mesh listens when `--mesh` does not say.
pub const MESH: &str = "127.0.0.1:9651";
/// `--produce` when it is not given, in milliseconds.
pub const PRODUCE: u64 = 200;

/// A proposer's window, in `--produce` periods: a slot lasts this many before
/// the next validator's turn. Every validator of a network runs the same one.
/// A height settles for one period (within Go's 150 ms to 2 s) before its
/// validators sign.
pub const WINDOWS: u32 = 5;

/// Why an L2 committee cannot be read from the P-chain yet.
///
/// Each clause is a thing the P-chain does not serve or this node does not
/// hold, measured against `luxfi/node` v1.37.3. Building a committee around any
/// one of them would be a committee this node cannot check.
pub const L2: &str = "\
the Lux P-chain cannot name this network's validators to this node yet:
  - GET /v1/chain/P/ops/validators/at?height=<h>&netID=<net> serves each
    validator's node id, weight and BLS key, and NOT the proof of possession
    the key was registered with; an L1 validator's record does not keep it
  - lux-consensus admits no signing validator without a proof it can check,
    and the proof exists only inside the RegisterL1ValidatorTx or
    ConvertNetworkToL1Tx bytes (GET .../ops/tx), which this node cannot decode
  - the answer is plain HTTP from one luxd, with no P-chain certificate over
    it, so a set read that way is as honest as the host that sent it
  - a set changes with P-chain height and the engine holds one set for its life
Run as an L1 until then: --committee <file>, one line from each validator's
--publish";

/// Set by the signal handler, read by the loop. The only thing a handler is
/// allowed to touch.
static RUNNING: AtomicBool = AtomicBool::new(true);

extern "C" fn on_signal(_: libc::c_int) {
    RUNNING.store(false, Ordering::SeqCst);
}

/// Run the node `spec` describes, from this process's arguments, until
/// `SIGTERM` or `SIGINT`. This is a brand's whole `main`.
pub fn run(spec: &Spec) -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match command(spec, &args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("{}: {why}", spec.client());
            ExitCode::FAILURE
        }
    }
}

fn command(spec: &Spec, args: &[String]) -> Result<(), String> {
    spec.check()?;
    match Command::parse(args)? {
        Command::Help => print!("{}", usage(spec)),
        Command::Version => println!("{} {}", spec.client(), spec.version()),
        Command::Publish(flags) => println!("{}", publish(spec, &flags)?),
        Command::Run(flags) => {
            // Signals before anything long-running, so a Ctrl-C during startup
            // is still a clean stop.
            unsafe {
                libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
                libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
            }
            Node::start(spec, flags)?.run(&RUNNING)?;
        }
    }
    Ok(())
}

/// What `--help` prints.
pub fn usage(spec: &Spec) -> String {
    let client = spec.client();
    let default = spec.default_network().map(|n| n.name).unwrap_or_default();
    format!(
        "\
{client} — a validator of the {name} network

  --network <name>     {networks}   (default {default})
  --data <dir>         where this validator's keys and journal live  (default ~/.{client})
  --committee <file>   L1: the validators, one published line each
  --pchain <addr>      L2: read the validators from the Lux P-chain at this luxd
  --peers <a,b,...>    every validator's mesh address, in committee order
  --rpc <addr>         JSON-RPC address                (default {RPC})
  --mesh <addr>        validator mesh address, also --stake  (default {MESH})
  --genesis <file>     this network's genesis from a file, checked against it
  --upgrade <file>     this network's upgrade.json; an entry this node does not
                       run refuses the start
  --chain <hex32>      the chain's 32-byte id, as the P-chain issued it
  --plugin <addr>      run the chain in its own process, at this address —
                       a host:port, or the path of a unix socket
  --plugin-id <hex>    the identity that chain must prove (from its startup line)
  --produce <ms>       a proposer's window is {WINDOWS} of these; every validator
                       of a network runs the same one  (default {PRODUCE})
  --archive <url>      ask this http:// node for what this one does not hold
  --import-chain-data <file>
                       import an RLP export at startup, before consensus
  --publish            print this validator's committee line, and exit
  --version            print the version, and exit
  --help               print this text, and exit

The chain is served at /v1/chain/{chain} and /v1/chain/{chain}/rpc.

A validator is NAMED by the ML-DSA-65 key it proves on every link, so there is
no name to claim without the key. An L1 committee is the list of what its
validators published, so bringing one up takes two steps:

  1. {client} --data n0 --publish >> committee.txt   (once per validator)
  2. {client} --data n0 --committee committee.txt --peers <every mesh address>

A line is published for one chain and admits nowhere else. A certificate needs
a committee of at least four: below that one compromised key forges it, and
the rule refuses rather than pretend.

An import is history, not agreement: an export carries blocks and no
certificates, so this node serves the chain it read and will NOT build or sign
until a peer's certificate anchors the imported tip. The data directory keeps
that (<data>/uncertified), so a later start without the flag is held to it too.
",
        name = spec.name(),
        networks = spec.names(),
        chain = spec.chain(),
    )
}

/// What the command line asked for.
#[derive(Debug)]
pub enum Command {
    Help,
    Version,
    Publish(Flags),
    Run(Flags),
}

/// The flags a node runs under, read and checked before anything opens.
#[derive(Clone, Debug, Default)]
pub struct Flags {
    pub network: Option<String>,
    pub data: Option<PathBuf>,
    pub committee: Option<PathBuf>,
    pub pchain: Option<SocketAddr>,
    pub peers: Vec<SocketAddr>,
    pub rpc: Option<SocketAddr>,
    pub mesh: Option<SocketAddr>,
    pub genesis: Option<PathBuf>,
    pub upgrade: Option<PathBuf>,
    pub chain: Option<Id>,
    pub plugin: Option<String>,
    pub plugin_id: Option<[u8; 20]>,
    pub produce: Option<Duration>,
    pub archive: Option<String>,
    pub import: Option<PathBuf>,
}

impl Command {
    /// Read `args`.
    ///
    /// Strict, because a flag that is quietly ignored is a node running in a
    /// mode nobody asked for: an unknown flag, a flag with no value, a flag
    /// given twice and a word that is not a flag are each an error.
    pub fn parse(args: &[String]) -> Result<Self, String> {
        if args.iter().any(|a| a == "--help" || a == "-h") {
            return Ok(Command::Help);
        }
        if args.iter().any(|a| a == "--version") {
            return Ok(Command::Version);
        }
        let mut flags = Flags::default();
        let mut publish = false;
        let mut seen: Vec<&str> = Vec::new();
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            // `--stake` is what Lux has always called the mesh port.
            let name = if arg == "--stake" { "--mesh" } else { arg.as_str() };
            if seen.contains(&name) {
                return Err(format!("{arg} is given twice"));
            }
            seen.push(name);
            if name == "--publish" {
                publish = true;
                continue;
            }
            let value = || it.clone().next().ok_or_else(|| format!("{arg} needs a value"));
            let v = match name {
                "--network"
                | "--data"
                | "--committee"
                | "--pchain"
                | "--peers"
                | "--rpc"
                | "--mesh"
                | "--genesis"
                | "--upgrade"
                | "--chain"
                | "--plugin"
                | "--plugin-id"
                | "--produce"
                | "--archive"
                | "--import-chain-data" => value()?.clone(),
                other => return Err(format!("{other} is not a flag; --help lists them")),
            };
            it.next();
            match name {
                "--network" => flags.network = Some(v),
                "--data" => flags.data = Some(v.into()),
                "--committee" => flags.committee = Some(v.into()),
                "--pchain" => flags.pchain = Some(address(name, &v)?),
                "--peers" => {
                    flags.peers = v
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(|a| address(name, a))
                        .collect::<Result<_, _>>()?
                }
                "--rpc" => flags.rpc = Some(address(name, &v)?),
                "--mesh" => flags.mesh = Some(address(arg, &v)?),
                "--genesis" => flags.genesis = Some(v.into()),
                "--upgrade" => flags.upgrade = Some(v.into()),
                "--chain" => flags.chain = Some(hex(name, &v)?),
                "--plugin" => flags.plugin = Some(v),
                "--plugin-id" => flags.plugin_id = Some(hex(name, &v)?),
                "--produce" => {
                    let ms: u64 =
                        v.parse().map_err(|_| format!("--produce: {v} is not a number"))?;
                    if ms == 0 {
                        return Err("--produce is at least 1 ms".into());
                    }
                    flags.produce = Some(Duration::from_millis(ms));
                }
                "--archive" => flags.archive = Some(v),
                _ => flags.import = Some(v.into()),
            }
        }
        Ok(if publish { Command::Publish(flags) } else { Command::Run(flags) })
    }
}

/// An address, parsed at startup rather than at bind, when the node is half up.
fn address(flag: &str, text: &str) -> Result<SocketAddr, String> {
    text.parse().map_err(|e| format!("{flag} {text}: {e}"))
}

/// Exactly `N` bytes of hexadecimal.
fn hex<const N: usize>(flag: &str, text: &str) -> Result<[u8; N], String> {
    let raw = alloy_primitives::hex::decode(text).map_err(|e| format!("{flag}: {e}"))?;
    raw.try_into().map_err(|raw: Vec<u8>| format!("{flag} is {N} bytes, not {}", raw.len()))
}

/// The network, its genesis, and the chain it runs: what a node is on before
/// it is anything else.
struct On {
    net: &'static Network,
    /// Where the genesis came from, for the startup line.
    source: String,
    raw: String,
    genesis: lux_evm::genesis::Genesis,
    /// The chain's 32 bytes, and whether a P-chain issued them.
    chain: Id,
    issued: bool,
}

impl On {
    fn read(spec: &Spec, flags: &Flags) -> Result<Self, String> {
        let net = match &flags.network {
            Some(name) => spec.network(name).ok_or_else(|| {
                format!("{name} is not a {} network; try {}", spec.name(), spec.names())
            })?,
            None => spec.default_network().ok_or("this spec runs no network")?,
        };
        // A file replaces the compiled-in document, and is held to this network
        // before it is used.
        let (source, raw) = match &flags.genesis {
            Some(path) => {
                let at = path.display().to_string();
                let raw =
                    std::fs::read_to_string(path).map_err(|e| format!("cannot read {at}: {e}"))?;
                (at, raw)
            }
            None => (format!("{}'s own", net.name), net.genesis.to_string()),
        };
        net.check(&raw).map_err(|e| format!("{source}: {e}"))?;
        let mut genesis = crate::genesis::parse(&raw).map_err(|e| format!("{source}: {e}"))?;
        if let Some(path) = &flags.upgrade {
            let at = path.display().to_string();
            let doc =
                std::fs::read_to_string(path).map_err(|e| format!("cannot read {at}: {e}"))?;
            crate::genesis::upgrade(&mut genesis, &doc).map_err(|e| format!("{at}: {e}"))?;
        }
        // What luxd refuses to start on, this node refuses too.
        genesis.verify().map_err(|e| format!("{source}: {e}"))?;
        // The chain's identity. Go computes it as sha256 of the P-chain's
        // CreateChainTx; with none issued, the genesis stand-in is only good for
        // a set of nodes talking to each other.
        let (chain, issued) = match flags.chain {
            Some(id) => (id, true),
            None => (engine::stand_in(genesis.hash().0), false),
        };
        Ok(Self { net, source, raw, genesis, chain, issued })
    }
}

/// Where this validator's keys and journal live.
fn data(spec: &Spec, flags: &Flags) -> Result<PathBuf, String> {
    match &flags.data {
        Some(dir) => Ok(dir.clone()),
        None => std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(|h| PathBuf::from(h).join(format!(".{}", spec.client())))
            .ok_or_else(|| "HOME is not set, so --data has no default; pass it".into()),
    }
}

/// The line this validator publishes for the chain `flags` name.
///
/// A line entitles a validator ON A CHAIN, so the chain has to be known before
/// there is a line to print: publishing for the wrong one produces a line that
/// parses everywhere and admits nowhere.
pub fn publish(spec: &Spec, flags: &Flags) -> Result<String, String> {
    let keys = Keys::open(data(spec, flags)?)?;
    let on = On::read(spec, flags)?;
    Ok(keys.publish(&on.chain))
}

/// The validators, from the one source the flags name.
fn committee(flags: &Flags, on: &On) -> Result<(Committee, String), String> {
    match (&flags.committee, flags.pchain) {
        (Some(_), Some(_)) => {
            Err("--committee and --pchain are two sources for one set; give one".into())
        }
        (Some(path), None) => {
            let at = path.display().to_string();
            let text =
                std::fs::read_to_string(path).map_err(|e| format!("cannot read {at}: {e}"))?;
            let c = Committee::read(&text, on.chain).map_err(|e| format!("{at}: {e}"))?;
            Ok((c, at))
        }
        (None, Some(at)) => Err(format!("--pchain {at}: {L2}")),
        (None, None) => Err(format!(
            "no validators: --committee <file> names them (L1), or --pchain <addr> reads \
             {}'s from the Lux P-chain (L2). {L2}",
            on.net.name
        )),
    }
}

/// A validator that is up: chain, engine, mesh, journal and server.
pub struct Node {
    chain: Arc<dyn Vm>,
    engine: Engine,
    host: Host,
    votes: Journal,
    serving: Serving,
    peers: Peers,
    window: Duration,
    met: Instant,
}

impl Node {
    /// Bring the node `flags` describe up, and say what it is.
    pub fn start(spec: &Spec, flags: Flags) -> Result<Node, String> {
        spec.check()?;
        // The keys first: everything else is named by them. And in a frame of
        // their own, because deriving an ML-DSA key is deep, and it should not
        // sit on top of everything else startup holds.
        let data = data(spec, &flags)?;
        let keys = Keys::open(&data)?;
        Self::up(spec, flags, data, keys)
    }

    fn up(spec: &Spec, flags: Flags, data: PathBuf, keys: Keys) -> Result<Node, String> {
        let rpc_at = flags.rpc.unwrap_or_else(|| RPC.parse().expect("a literal address"));
        let mesh_at = flags.mesh.unwrap_or_else(|| MESH.parse().expect("a literal address"));
        if rpc_at == mesh_at && rpc_at.port() != 0 {
            return Err("--rpc and --mesh cannot be one address".into());
        }
        let window = flags.produce.unwrap_or(Duration::from_millis(PRODUCE)) * WINDOWS;

        let on = On::read(spec, &flags)?;
        let genesis = on.genesis.clone();
        let genesis_hash = genesis.hash();

        let (committee, source) = committee(&flags, &on)?;
        let me = keys.identity.node();
        let seat = committee.seat(&me).ok_or_else(|| {
            format!(
                "this validator ({}) is not in {source}; add the line --publish prints",
                alloy_primitives::hex::encode(me)
            )
        })?;
        let peers = peers(&flags.peers, &committee)?;

        // The chain runs here, or in its own process. Same trait either way, so
        // nothing below this line knows which — that is the point of the seam.
        let chain: Arc<dyn Vm> = match &flags.plugin {
            Some(at) => Arc::new(
                crate::plugin::Client::connect(
                    "C",
                    at,
                    &keys.identity,
                    flags.plugin_id,
                    genesis_hash.0,
                )
                .map_err(|e| format!("cannot reach the chain's plugin at {at}: {e}"))?,
            ),
            // Every accepted block reaches `<data>/blocks` before it is
            // accepted, and opening replays them, so a restart resumes here.
            None => Arc::new(Evm::open(genesis, &data)?),
        };
        // The certificate each block was accepted on, kept so a peer that fell
        // behind can be answered with proof and not only a block.
        let ledger = Arc::new(
            Ledger::open(data.join("certs"))
                .map_err(|e| format!("cannot open the certificate ledger: {e}"))?,
        );
        // What an earlier import left: read from the data directory whether
        // or not this run imports, because the history it covers is on disk
        // either way and a flag dropped from the command line agrees nothing.
        let tip = chain.get(&chain.last_accepted()).map(|b| b.height()).unwrap_or(0);
        let gate = Arc::new(
            Gate::open(data.join("uncertified"), tip)
                .map_err(|e| format!("cannot read the import gate: {e}"))?,
        );
        // The block this validator signed at a height still open, so a restart
        // offers it again with the signature.
        let held = Held::open(data.join("held"))
            .map_err(|e| format!("cannot read the block this validator signed: {e}"))?;
        if let Some(aside) = held.damaged() {
            eprintln!(
                "{}: not a signed proposal; set aside at {}. A restart cannot offer the block \
                 this validator last signed; the journal still holds it to that signature.",
                data.join("held").display(),
                aside.display()
            );
        }
        let engine = Engine::new(me, keys.consensus.clone(), &committee, 1, on.chain)?
            .with_ledger(ledger.clone())
            .with_gate(gate.clone())
            .with_held(held)
            .with_window(window);

        // The export goes in before consensus starts, through the same seam a
        // peer's block goes through — parsed, executed, and refused if what it
        // claims is not what running it produces. Go does this at the same
        // point, from the same flag: the chain is whole before it is served.
        // The gate is closed before its first block is accepted.
        let imported = match &flags.import {
            Some(path) => Some(crate::import::from_file(chain.as_ref(), path, &gate)?),
            None => None,
        };

        let host = Host::bind(Config { identity: Arc::new(keys.identity), bind: mesh_at })
            .map_err(|e| format!("cannot bind the validator mesh at {mesh_at}: {e}"))?;
        // One signature per height, on the device before the key is used: the
        // file v0.2.x wrote, read the same way.
        let votes = Journal::open(data.join("votes"))
            .map_err(|e| format!("cannot open the signing journal: {e}"))?;
        // Peers are answered from this chain and its certificates, and what RPC
        // admits is relayed to them.
        let chain: Arc<dyn Vm> = Relay::attach(chain, ledger, &host);

        let mut rpc =
            Rpc::new(u64::from(on.net.id), spec.agent()).with(spec.chain(), chain.clone());
        if let Some(url) = &flags.archive {
            rpc = rpc.with_archive(url).map_err(|e| format!("--archive {e}"))?;
        }
        let serving =
            rpc.listen(rpc_at).map_err(|e| format!("cannot serve JSON-RPC at {rpc_at}: {e}"))?;

        let n = committee.len();
        let net = on.net;
        println!("{} {}", spec.client(), spec.version());
        println!("  network       {}, network id {}, chain id {}", net.name, net.id, net.chain);
        println!(
            "  genesis       {}, {} accounts, {genesis_hash}",
            on.source,
            spec::accounts(&on.raw)
        );
        println!(
            "  chain         {}{}",
            B256::from(on.chain),
            if on.issued { "" } else { "  (derived from the genesis: no P-chain issued it)" }
        );
        println!("  data          {}", data.display());
        let refused = crate::genesis::refused(&on.genesis);
        if refused.is_empty() {
            println!("  precompiles   every one this chain turns on runs here");
        } else {
            println!("  refuses       calls to {}", refused.join("; "));
        }
        println!("  validator     {seat} of {n}, id {}", alloy_primitives::hex::encode(me));
        println!("  identity      ML-DSA-65, proved on every link");
        println!("  committee     {source}, weight {}", engine.set().carried());
        println!("  quorum        {} of {n}", engine.quorum());
        println!("  window        {} ms a proposer", window.as_millis());
        println!("  mesh          {}", host.addr());
        println!("  rpc           http://{}/v1/chain/{}", serving.addr(), spec.chain());
        println!(
            "  runs          {}",
            match &flags.plugin {
                Some(at) => format!("a plugin at {at}"),
                None => "in this process".into(),
            }
        );
        if let Some(url) = &flags.archive {
            println!("  archive       {url}");
        }

        // Reach the other validators. The mesh is not the quorum: `connect`
        // reports who it found and the rule decides whether that is enough.
        if !peers.is_empty() {
            let reached = host.connect(&peers, Duration::from_secs(10));
            println!("  peers         {reached} of {}", peers.len().saturating_sub(1));
        }
        if let Some(done) = imported {
            println!(
                "  imported      {} blocks, tip height {} at {}",
                done.blocks,
                done.height,
                B256::from(done.tip)
            );
            println!("  state root    {}", B256::from(done.state_root));
        }
        if let Some((from, to)) = gate.held() {
            println!(
                "  standing      NOT a caught-up validator — heights {from} to {to} were \
                 imported and carry no certificate; this node will not build or sign until a \
                 peer's certificate anchors them"
            );
        }
        let quorum = engine.quorum();
        if quorum > 1 && host.peer_count() + 1 < quorum {
            println!(
                "  waiting       {} of {quorum} validators reachable; heights advance when the \
                 rest join",
                host.peer_count() + 1
            );
        }

        Ok(Node { chain, engine, host, votes, serving, peers, window, met: Instant::now() })
    }

    /// Where JSON-RPC is served.
    pub fn rpc(&self) -> SocketAddr {
        self.serving.addr()
    }

    /// Where the validator mesh listens.
    pub fn mesh(&self) -> SocketAddr {
        self.host.addr()
    }

    /// Decide blocks until `running` is cleared, then stop cleanly.
    /// Returns an error, for a non-zero exit, when the signing journal can
    /// no longer write: a validator that cannot sign sits out silently
    /// otherwise, and a restart is what reopens the journal whole.
    pub fn run(mut self, running: &AtomicBool) -> Result<(), String> {
        // Whatever the peers certified while this node was away, before it
        // builds on a tip they have left behind: a pass at a time, until one
        // finds nothing new.
        while running.load(Ordering::SeqCst) {
            match self.engine.catch_up(self.chain.as_ref(), &self.host, self.window * 2) {
                Ok(c) if c.accepted > 0 => say(&Event::Caught(c), self.chain.as_ref()),
                Ok(c) => {
                    say(&Event::Caught(c), self.chain.as_ref());
                    break;
                }
                Err(e) => {
                    eprintln!("catching up: {e}");
                    break;
                }
            }
        }
        let mut why = None;
        while running.load(Ordering::SeqCst) {
            self.step();
            if self.votes.broken() {
                why =
                    Some("the signing journal could not undo a failed write; restart to reopen it");
                break;
            }
        }
        println!("stopping");
        self.serving.stop();
        self.host.mesh().close();
        why.map_or(Ok(()), |w| Err(w.into()))
    }

    /// One pass: meet the validators, then one step of the engine.
    fn step(&mut self) {
        // The mesh is not formed once. A validator that restarts comes back to
        // peers still holding its dead socket, so without this it dials into a
        // backlog nobody reads and never rejoins. Every other window, because
        // a dial to a peer that is down costs its timeout and this loop's job
        // is blocks.
        if !self.peers.is_empty() && self.met.elapsed() >= self.window * 2 {
            self.host.sweep(&self.peers);
            self.met = Instant::now();
        }
        let step = self.engine.step(self.chain.as_ref(), &self.host, &mut self.votes);
        for e in &step.events {
            say(e, self.chain.as_ref());
        }
        if step.quiet && step.events.is_empty() {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// One line per thing worth saying; one per decided block, carrying the
/// number that matters.
fn say(event: &Event, vm: &dyn Vm) {
    match event {
        Event::Decided { height, slot, cert, built } => {
            let root = vm
                .get(&vm.last_accepted())
                .map(|b| B256::from(b.state_root()).to_string())
                .unwrap_or_default();
            let what = if *built { "produced" } else { "accepted" };
            let slot = slot.map(|s| format!("  slot {s}")).unwrap_or_default();
            println!("{what}  height {height}  state {root}  {} votes{slot}", cert.votes.len());
        }
        Event::Waiting { height, slot, most, need, blocks } => eprintln!(
            "height {height} slot {slot}: {most} of {need} votes on {blocks} blocks, asking again"
        ),
        Event::Caught(c) => {
            if c.accepted > 0 {
                println!("caught up  {} blocks  peers report height {}", c.accepted, c.reported);
            }
            for (peer, why) in &c.refused {
                eprintln!(
                    "catching up: {} not believed: {why}",
                    alloy_primitives::hex::encode(peer)
                );
            }
        }
        Event::Refused(why) => eprintln!("{why}"),
    }
}

/// Every validator's mesh address, in committee order.
///
/// A peer list is positions, not names: the names are already fixed by the
/// committee, and the handshake proves them, so an address that turns out to
/// belong to someone else is refused rather than believed.
fn peers(list: &[SocketAddr], committee: &Committee) -> Result<Peers, String> {
    if list.len() > committee.len() {
        return Err("--peers has more addresses than the committee has validators".into());
    }
    Ok(committee.members().iter().zip(list).map(|(m, a)| (m.node, *a)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lux-run-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    fn flags(args: &[&str]) -> Result<Flags, String> {
        match Command::parse(&argv(args))? {
            Command::Run(f) | Command::Publish(f) => Ok(f),
            other => Err(format!("{other:?}")),
        }
    }

    #[test]
    fn a_flag_takes_the_value_after_it() {
        let f = flags(&[
            "--network",
            "testnet",
            "--produce",
            "700",
            "--peers",
            "127.0.0.1:1, 127.0.0.1:2",
        ])
        .expect("flags");
        assert_eq!(f.network.as_deref(), Some("testnet"));
        assert_eq!(f.produce, Some(Duration::from_millis(700)));
        assert_eq!(f.peers.len(), 2);
        assert!(matches!(Command::parse(&argv(&["--publish"])), Ok(Command::Publish(_))));
        assert!(matches!(Command::parse(&argv(&["--data", "d", "--help"])), Ok(Command::Help)));
        assert!(matches!(Command::parse(&argv(&["--version"])), Ok(Command::Version)));
    }

    #[test]
    fn nothing_on_the_line_is_ignored() {
        for bad in [
            &["--network"][..],
            &["--netwrk", "mainnet"],
            &["mainnet"],
            &["--network", "a", "--network", "b"],
            &["--mesh", "127.0.0.1:1", "--stake", "127.0.0.1:2"],
            &["--light"],
        ] {
            assert!(Command::parse(&argv(bad)).is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn stake_is_the_mesh() {
        let f = flags(&["--stake", "127.0.0.1:9651"]).expect("flags");
        assert_eq!(f.mesh, Some("127.0.0.1:9651".parse().expect("addr")));
    }

    #[test]
    fn a_bad_value_is_refused_before_anything_opens() {
        for bad in [
            &["--rpc", "127.0.0.1"][..],
            &["--rpc", "9650"],
            &["--pchain", "luxd"],
            &["--peers", "127.0.0.1:1,nowhere"],
            &["--produce", "often"],
            &["--produce", "0"],
            &["--chain", &"00".repeat(31)],
            &["--chain", &"zz".repeat(32)],
            &["--plugin-id", &"00".repeat(32)],
        ] {
            assert!(Command::parse(&argv(bad)).is_err(), "{bad:?} was accepted");
        }
        let f = flags(&["--chain", &format!("0x{}", "ff".repeat(32))]).expect("0x is allowed");
        assert_eq!(f.chain, Some([0xff; 32]));
    }

    #[test]
    fn the_usage_names_every_network_and_both_sources() {
        let text = usage(&spec::LUX);
        for n in spec::LUX.networks() {
            assert!(text.contains(n.name), "usage does not offer {}", n.name);
        }
        for f in ["--committee", "--pchain", "/v1/chain/c/rpc", "lux-node --data n0 --publish"] {
            assert!(text.contains(f), "usage does not say {f}");
        }
    }

    #[test]
    fn a_genesis_from_a_file_is_held_to_the_network() {
        let dir = scratch("held");
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("genesis.json");
        std::fs::write(&path, spec::LUX.network("testnet").expect("testnet").genesis).expect("w");
        let mut f = Flags { genesis: Some(path), ..Flags::default() };
        let err = On::read(&spec::LUX, &f).err().expect("testnet's genesis is not localnet's");
        assert!(err.contains("chain 96368, not localnet's chain 31337"), "{err}");
        f.network = Some("testnet".into());
        assert!(On::read(&spec::LUX, &f).is_ok(), "and it is testnet's");
        f.network = Some("mainet".into());
        assert!(On::read(&spec::LUX, &f).is_err(), "a typo is not a network");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_validators_come_from_one_source() {
        let on = On::read(&spec::LUX, &Flags::default()).expect("localnet");
        let both = Flags {
            committee: Some("c.txt".into()),
            pchain: Some("127.0.0.1:9650".parse().expect("addr")),
            ..Flags::default()
        };
        assert!(committee(&both, &on).expect_err("two sources").contains("give one"));

        // L2 names what the P-chain does not serve, rather than pretending.
        let l2 =
            Flags { pchain: Some("127.0.0.1:9650".parse().expect("addr")), ..Flags::default() };
        let err = committee(&l2, &on).expect_err("L2 is refused");
        assert!(err.contains("proof of possession") && err.contains("--committee"), "{err}");
        let neither = committee(&Flags::default(), &on).expect_err("no source");
        assert!(neither.contains("--committee") && neither.contains("--pchain"), "{neither}");
    }
}
