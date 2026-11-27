// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! A brand is a spec and one call.
//!
//! These bring whole nodes up — keys, committee, mesh, journal and server —
//! from a command line, the way a brand's `main` does, and ask them over a real
//! socket what they are called. The last ones run the binary itself.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use lux_node::engine::{self, Committee};
use lux_node::keys::Keys;
use lux_node::run::{self, Command, Flags};
use lux_node::spec::{Network, Spec, LUX};
use lux_node::Node;
use serde_json::{json, Value};

/// A brand this crate knows nothing about.
const ZETA: Spec = Spec::new(
    "zeta",
    "zetad",
    "9.9.9",
    &[
        Network { name: "mainnet", id: 7, chain: 7777, genesis: ZETA_MAINNET },
        Network { name: "testnet", id: 8, chain: 7778, genesis: ZETA_TESTNET },
    ],
);
const ZETA_MAINNET: &str = r#"{"config":{"chainId":7777},"gasLimit":"0xb71b00",
    "alloc":{"0x9011E888251AB053B7bD1cdB598Db4f9DEd94714":{"balance":"0x1"}}}"#;
const ZETA_TESTNET: &str = r#"{"config":{"chainId":7778},"gasLimit":"0xb71b00",
    "alloc":{"0x9011E888251AB053B7bD1cdB598Db4f9DEd94714":{"balance":"0x2"}}}"#;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lux-node-run-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// Run `f` on a stack the size of a process's main thread, which is where a
/// node starts. A test thread's is 2 MiB, and generating an ML-DSA key
/// unoptimised needs nearly all of it.
fn on_main_stack<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(8 << 20)
            .spawn_scoped(s, f)
            .expect("a thread")
            .join()
            .expect("it finished")
    })
}

fn flags(args: &[&str]) -> Flags {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    match Command::parse(&args).expect("flags") {
        Command::Run(f) | Command::Publish(f) => f,
        other => panic!("{other:?}"),
    }
}

/// A one-validator network of `spec`, published and brought up the way an
/// operator does it: publish a line, put it in a committee, start.
fn up(spec: &Spec, dir: &Path, extra: &[&str]) -> Node {
    let data = dir.to_str().expect("utf-8");
    let mut args = vec!["--data", data];
    args.extend_from_slice(extra);
    let line = run::publish(spec, &flags(&args)).expect("a published line");
    let file = dir.join("committee.txt");
    std::fs::write(&file, format!("{line}\n")).expect("committee");
    let file = file.to_str().expect("utf-8").to_string();
    args.extend_from_slice(&[
        "--committee",
        &file,
        "--rpc",
        "127.0.0.1:0",
        "--mesh",
        "127.0.0.1:0",
    ]);
    Node::start(spec, flags(&args)).expect("the node comes up")
}

/// One HTTP request, over a real socket.
fn http(at: SocketAddr, method: &str, path: &str, body: &str) -> (u16, Value) {
    let mut c = TcpStream::connect(at).expect("connect");
    write!(
        c,
        "{method} {path} HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .expect("write");
    let mut r = BufReader::new(c);
    let mut status = String::new();
    r.read_line(&mut status).expect("status");
    let code = status.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let mut rest = String::new();
    r.read_to_string(&mut rest).expect("body");
    let body = rest.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    (code, serde_json::from_str(body).unwrap_or(Value::Null))
}

fn ask(at: SocketAddr, path: &str, method: &str) -> (u16, Value) {
    http(at, "POST", path, &json!({"jsonrpc": "2.0", "id": 1, "method": method}).to_string())
}

#[test]
fn a_brand_answers_under_its_own_name_and_not_at_c() {
    let dir = scratch("brand");
    let node = up(&ZETA, &dir, &[]);
    let at = node.rpc();

    for path in ["/v1/chain/zeta", "/v1/chain/zeta/rpc"] {
        let (code, a) = ask(at, path, "web3_clientVersion");
        assert_eq!(code, 200, "{path}");
        assert_eq!(a["result"], "zetad/v9.9.9", "the brand, by name: {path}");
        assert_eq!(ask(at, path, "eth_chainId").1["result"], "0x1e61", "chain 7777: {path}");
    }
    // The C-chain's name is Lux's.
    for path in ["/v1/chain/c", "/v1/chain/c/rpc", "/v1/chain/C/rpc"] {
        assert_eq!(ask(at, path, "eth_chainId").0, 404, "{path}");
    }

    let (_, dir_doc) = http(at, "GET", "/", "");
    assert_eq!(dir_doc["chains"], json!({"zeta": "/v1/chain/zeta"}));
    assert_eq!(dir_doc["version"], "zetad/v9.9.9");
    assert_eq!(dir_doc["networkId"], 7, "the network id, not the chain id");
    assert_eq!(ask(at, "/v1/info", "info.getNetworkID").1["result"]["networkID"], "7");

    // A clean stop: the loop sees the flag, the server stops, the port closes.
    let running = Arc::new(AtomicBool::new(true));
    let flag = running.clone();
    let (done, stopped) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        node.run(&flag).expect("a clean stop");
        let _ = done.send(());
    });
    std::thread::sleep(Duration::from_millis(100));
    running.store(false, Ordering::SeqCst);
    stopped.recv_timeout(Duration::from_secs(10)).expect("the loop stops when told");
    assert!(TcpStream::connect(at).is_err(), "the server stopped with the node");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn lux_answers_at_c() {
    let dir = scratch("lux");
    let node = up(&LUX, &dir, &[]);
    let at = node.rpc();
    let (code, a) = ask(at, "/v1/chain/c/rpc", "eth_chainId");
    assert_eq!(code, 200);
    assert_eq!(a["result"], "0x7a69", "localnet, the default");
    assert_eq!(
        ask(at, "/v1/chain/c", "web3_clientVersion").1["result"],
        format!("lux-node/v{}", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(ask(at, "/v1/chain/lux", "eth_chainId").0, 404, "Lux's chain is c");
    let (_, doc) = http(at, "GET", "/", "");
    assert_eq!(doc["chains"], json!({"c": "/v1/chain/c"}));
    assert_eq!(doc["networkId"], 1337);
    drop(node);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_published_line_admits_its_validator_on_its_chain_and_nowhere_else() {
    // Publishing and starting, the way an operator does both: on a stack the
    // size of a process's main thread.
    on_main_stack(|| {
        let dir = scratch("line");
        let data = dir.to_str().expect("utf-8");
        let line = run::publish(&ZETA, &flags(&["--data", data])).expect("a line");
        assert_eq!(line.lines().count(), 1, "one line, appended to a committee file");
        assert_eq!(line.split_whitespace().count(), 3, "identity, key, proof");

        let genesis = lux_node::genesis::parse(ZETA_MAINNET).expect("genesis");
        let chain = engine::stand_in(genesis.hash().0);
        let me = Keys::open(&dir).expect("keys").identity.node();
        let c = Committee::read(&line, chain).expect("a committee line");
        assert_eq!(c.seat(&me), Some(0), "under its own name");
        c.set().expect("the proof verifies on this chain");

        // The same validator, published for testnet, is not entitled on mainnet.
        let other =
            run::publish(&ZETA, &flags(&["--data", data, "--network", "testnet"])).expect("a line");
        assert_ne!(other, line);
        assert!(Committee::read(&other, chain).expect("parses").set().is_err());
        // And a node whose committee does not hold it says so.
        let stranger = scratch("line-stranger");
        let file = dir.join("committee.txt");
        std::fs::write(&file, &line).expect("committee");
        let args = [
            "--data",
            stranger.to_str().expect("utf-8"),
            "--committee",
            file.to_str().expect("utf-8"),
            "--rpc",
            "127.0.0.1:0",
            "--mesh",
            "127.0.0.1:0",
        ];
        let err = Node::start(&ZETA, flags(&args)).err().expect("not a member");
        assert!(err.contains("is not in"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&stranger);
    });
}

#[test]
fn a_genesis_from_a_file_is_held_to_the_network_it_claims() {
    let dir = scratch("genesis");
    let file = dir.join("testnet.json");
    std::fs::write(&file, ZETA_TESTNET).expect("genesis");
    let path = file.to_str().expect("utf-8");
    let data = dir.to_str().expect("utf-8");
    let err = run::publish(&ZETA, &flags(&["--data", data, "--genesis", path]))
        .expect_err("testnet's genesis on mainnet");
    assert!(err.contains("chain 7778, not mainnet's chain 7777"), "{err}");
    assert!(run::publish(
        &ZETA,
        &flags(&["--data", data, "--network", "testnet", "--genesis", path])
    )
    .is_ok());
    let err = run::publish(&ZETA, &flags(&["--data", data, "--network", "devnet"]))
        .expect_err("zeta runs no devnet");
    assert!(err.contains("mainnet | testnet"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The binary, with its output.
fn luxd(args: &[&str]) -> (bool, String, String) {
    let out = Process::new(env!("CARGO_BIN_EXE_luxd")).args(args).output().expect("luxd runs");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn the_binary_says_what_it_is() {
    let (ok, out, _) = luxd(&["--version"]);
    assert!(ok);
    assert_eq!(out, format!("lux-node {}\n", env!("CARGO_PKG_VERSION")));

    let (ok, out, _) = luxd(&["--help"]);
    assert!(ok);
    for want in ["localnet | mainnet | testnet", "--committee", "--pchain", "/v1/chain/c/rpc"] {
        assert!(out.contains(want), "--help does not say {want}");
    }

    let (ok, _, err) = luxd(&["--light"]);
    assert!(!ok, "an unknown flag is refused");
    assert!(err.starts_with("lux-node: --light is not a flag"), "{err}");
}

#[test]
fn the_binary_publishes_then_runs_then_stops_on_sigterm() {
    let dir = scratch("bin");
    let data = dir.to_str().expect("utf-8");

    // With no source of validators it names both, and what L2 lacks.
    let (ok, _, err) = luxd(&["--data", data]);
    assert!(!ok);
    assert!(err.contains("--committee") && err.contains("--pchain"), "{err}");

    let (ok, line, _) = luxd(&["--data", data, "--publish"]);
    assert!(ok);
    let file = dir.join("committee.txt");
    std::fs::write(&file, &line).expect("committee");

    let mut child = Process::new(env!("CARGO_BIN_EXE_luxd"))
        .args(["--data", data, "--committee", file.to_str().expect("utf-8")])
        .args(["--rpc", "127.0.0.1:0", "--mesh", "127.0.0.1:0", "--produce", "20"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("luxd starts");
    let mut lines = BufReader::new(child.stdout.take().expect("stdout")).lines();
    let rpc = lines
        .by_ref()
        .map_while(Result::ok)
        .find(|l| l.trim_start().starts_with("rpc "))
        .expect("the node says where it serves");
    assert!(rpc.ends_with("/v1/chain/c"), "{rpc}");

    let pid = libc::pid_t::try_from(child.id()).expect("a pid");
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
    let status = child.wait().expect("it exits");
    assert!(status.success(), "SIGTERM is a clean stop, not a kill: {status}");
    let rest: Vec<String> = lines.map_while(Result::ok).collect();
    assert_eq!(rest.last().map(String::as_str), Some("stopping"), "{rest:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

// -- the import gate, end to end --------------------------------------------

/// The key a funded brand's one account is held by.
fn payer() -> k256::ecdsa::SigningKey {
    k256::ecdsa::SigningKey::from_bytes(&[7u8; 32].into()).expect("key")
}

/// A brand whose genesis funds [`payer`], so a solo validator has something
/// to build. Leaked, because a spec is a `'static` constant in every brand.
/// Its schedule moves neither the base fee nor the block gas cost: the history
/// here is several blocks a second, and these tests are about the gate, not
/// fees.
fn funded() -> &'static Spec {
    let point = payer().verifying_key().to_encoded_point(false);
    let from = alloy_primitives::Address::from_slice(
        &alloy_primitives::keccak256(&point.as_bytes()[1..])[12..],
    );
    let genesis = format!(
        r#"{{"config":{{"chainId":7779,"feeConfig":{{"gasLimit":12000000,"targetBlockRate":2,
            "minBaseFee":25000000000,"targetGas":1000000000000,"baseFeeChangeDenominator":36,
            "minBlockGasCost":0,"maxBlockGasCost":1000000,"blockGasCostStep":0}}}},
            "gasLimit":"0xb71b00",
            "alloc":{{"{from:#x}":{{"balance":"0x3635c9adc5dea00000"}}}}}}"#
    );
    let networks: &'static [Network] = Box::leak(Box::new([Network {
        name: "mainnet",
        id: 9,
        chain: 7779,
        genesis: Box::leak(genesis.into_boxed_str()),
    }]));
    Box::leak(Box::new(Spec::new("omega", "omegad", "1.0.0", networks)))
}

/// A signed transfer from [`payer`] on `spec`'s chain.
fn transfer(spec: &Spec, nonce: u64) -> Vec<u8> {
    use alloy_consensus::{SignableTransaction, TxEip1559};
    use alloy_eips::eip2718::Encodable2718 as _;
    use alloy_primitives::{Address, Signature, U256};

    let t = TxEip1559 {
        chain_id: spec.networks()[0].chain,
        nonce,
        gas_limit: 100_000,
        max_fee_per_gas: 30_000_000_000,
        max_priority_fee_per_gas: 1_000_000_000,
        to: revm::primitives::TxKind::Call(Address::repeat_byte(0xaa)),
        value: U256::from(1u64),
        ..Default::default()
    };
    let (sig, recid) =
        payer().sign_prehash_recoverable(t.signature_hash().as_slice()).expect("sign");
    alloy_consensus::TxEnvelope::Eip1559(t.into_signed(Signature::new(
        U256::from_be_slice(&sig.r().to_bytes()),
        U256::from_be_slice(&sig.s().to_bytes()),
        recid.is_y_odd(),
    )))
    .encoded_2718()
}

/// `n` blocks of `spec`'s chain, built by someone else: genesis first, then
/// each block, as an export holds them.
fn history(spec: &Spec, n: u64) -> Vec<Vec<u8>> {
    use lux_node::vm::Vm as _;
    let chain = lux_node::evm::Evm::new(
        lux_node::genesis::parse(spec.networks()[0].genesis).expect("genesis"),
    );
    let mut out = vec![chain.get(&chain.last_accepted()).expect("genesis").bytes()];
    for nonce in 0..n {
        chain.submit(&transfer(spec, nonce)).expect("submit");
        let b = chain.build().expect("build");
        chain.verify(&b.id()).expect("verify");
        chain.accept(&b.id()).expect("accept");
        out.push(b.bytes());
    }
    out
}

/// Run `node`'s loop with `raw` sent to it over RPC until `done` or `wait`
/// runs out, and stop it. Returns its height, and whether `done` came true.
fn live(
    node: Node,
    spec: &Spec,
    raw: &[u8],
    wait: Duration,
    done: impl Fn() -> bool,
) -> (u64, bool) {
    let at = node.rpc();
    let path = format!("/v1/chain/{}", spec.chain());
    let running = Arc::new(AtomicBool::new(true));
    let flag = running.clone();
    let driving = std::thread::spawn(move || node.run(&flag));
    let send = json!({"jsonrpc": "2.0", "id": 1, "method": "eth_sendRawTransaction",
        "params": [format!("0x{}", alloy_primitives::hex::encode(raw))]});
    let (_, sent) = http(at, "POST", &path, &send.to_string());
    assert!(sent["result"].is_string(), "the transaction is admitted: {sent}");
    let end = std::time::Instant::now() + wait;
    while !done() && std::time::Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
    let (_, h) = ask(at, &path, "eth_blockNumber");
    let hex = h["result"].as_str().expect("a height").trim_start_matches("0x").to_string();
    running.store(false, Ordering::SeqCst);
    driving.join().expect("the loop stops").expect("a clean stop");
    // The last RPC connection lets go of the chain as it closes; give it that.
    std::thread::sleep(Duration::from_millis(100));
    (u64::from_str_radix(&hex, 16).expect("hex"), done())
}

/// Bytes in the signing journal: what this validator has signed.
fn signed(dir: &Path) -> u64 {
    std::fs::metadata(dir.join("votes")).map(|m| m.len()).unwrap_or(0)
}

// A solo validator builds and signs, and cannot certify: a Quasar certificate
// needs a committee of four. So what these watch is the signing journal — the
// record a validator writes before its key is used — not the height.

#[test]
fn an_import_holds_the_node_across_a_restart_without_the_flag() {
    let spec = funded();
    let dir = scratch("gated");
    let export = dir.join("export.rlp");
    std::fs::write(&export, history(spec, 2).concat()).expect("export");
    let export = export.to_str().expect("utf-8");
    let wait = Duration::from_millis(1500);

    // With the flag: it imports two blocks, and builds and signs nothing.
    let node = up(spec, &dir, &["--import-chain-data", export, "--produce", "20"]);
    let (height, spoke) = live(node, spec, &transfer(spec, 2), wait, || signed(&dir) > 0);
    assert_eq!(height, 2, "the export's two blocks");
    assert!(!spoke, "an imported node signed");
    assert_eq!(std::fs::read_to_string(dir.join("uncertified")).expect("the gate"), "1\n");

    // Without it: the data directory remembers.
    let node = up(spec, &dir, &["--produce", "20"]);
    let (height, spoke) = live(node, spec, &transfer(spec, 2), wait, || signed(&dir) > 0);
    assert_eq!(height, 2);
    assert!(!spoke, "SAFETY: signed on an uncertified tip after a restart without the flag");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_chain_with_blocks_and_no_certificates_and_no_gate_is_an_ordinary_validator() {
    // What hanzod 0.2.2 left on every seat: `<data>/blocks`, in its format,
    // with no certificate under any block and no gate beside them.
    let spec = funded();
    let dir = scratch("seat");
    let mut blocks = Vec::new();
    for raw in &history(spec, 2)[1..] {
        blocks.extend_from_slice(&(raw.len() as u32).to_be_bytes());
        blocks.extend_from_slice(raw);
    }
    std::fs::write(dir.join("blocks"), blocks).expect("blocks");

    let node = up(spec, &dir, &["--produce", "20"]);
    let wait = Duration::from_secs(20);
    let (height, spoke) = live(node, spec, &transfer(spec, 2), wait, || signed(&dir) > 0);
    assert_eq!(height, 2, "the chain it held, replayed");
    assert!(spoke, "it built on what it holds, and signed");
    assert!(!dir.join("uncertified").exists());
    let _ = std::fs::remove_dir_all(&dir);
}
