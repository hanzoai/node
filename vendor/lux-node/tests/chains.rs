// Copyright (C) 2026, Lux Industries, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! The chains plane of github.com/luxfi/conformance: an imported history,
//! answered the way a Go node answers it.
//!
//! Each `chain_<name>_source.json` names a genesis document, an RLP export in
//! github.com/luxfi/state — its parts in order, and the sha256 of the whole
//! they concatenate to — and what a live Go node served for
//! that chain: every block, every receipt, and the tip balance and nonce of
//! every address a transaction touched. This node imports the same export onto
//! the same genesis and must give the same answers through its own RPC — equal
//! strings, not equivalent values.
//!
//! LUX_CONFORMANCE is the corpus's `vectors/` directory and LUX_STATE the root
//! of a luxfi/state checkout. Without both the test says so and fails: a
//! conformance claim that skips itself is not one.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::Instant;

use lux_node::evm::Evm;
use lux_node::vm::Vm as _;
use serde_json::{json, Value};

/// A directory that removes itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("lux-chains-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Where each field disagreed, and the first few examples of it.
#[derive(Default)]
struct Diff(BTreeMap<String, (usize, Vec<String>)>);

impl Diff {
    fn check(&mut self, what: &str, field: &str, want: &Value, got: &Value) {
        if norm(want) != norm(got) {
            let e = self.0.entry(field.to_string()).or_default();
            e.0 += 1;
            if e.1.len() < 3 {
                e.1.push(format!("{what}: want {want} got {got}"));
            }
        }
    }
}

/// Addresses and hashes compare case-blind; everything else exactly.
fn norm(v: &Value) -> Value {
    match v {
        Value::String(s) if s.starts_with("0x") => Value::String(s.to_lowercase()),
        Value::Array(a) => Value::Array(a.iter().map(norm).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), norm(v))).collect()),
        other => other.clone(),
    }
}

fn lines(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("a JSON line"))
        .collect()
}

/// The export as one stream: its parts, in order, read end to end. A mainnet
/// export is gigabytes, so nothing here holds more than a buffer of it.
fn export(parts: &[PathBuf]) -> Result<impl Read, String> {
    let mut whole: Box<dyn Read> = Box::new(std::io::empty());
    for part in parts {
        let file = File::open(part).map_err(|e| format!("{}: {e}", part.display()))?;
        whole = Box::new(whole.chain(file));
    }
    Ok(BufReader::with_capacity(1 << 20, whole))
}

fn sha256(parts: &[PathBuf]) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    std::io::copy(&mut export(parts)?, &mut hasher)
        .map_err(|e| format!("reading the export: {e}"))?;
    Ok(alloy_primitives::hex::encode(hasher.finalize()))
}

/// The most memory this process has held, from the kernel's own count.
fn peak_rss() -> String {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines().find(|l| l.starts_with("VmHWM:")).map(|l| l[6..].trim().to_string())
        })
        .unwrap_or_else(|| "unknown".into())
}

/// Import one chain and compare everything the Go node recorded for it.
fn conform(vectors: &Path, state: &Path, name: &str) -> Result<String, String> {
    let source: Value = serde_json::from_str(
        &std::fs::read_to_string(vectors.join(format!("chain_{name}_source.json")))
            .expect("source"),
    )
    .expect("source JSON");
    let parts: Vec<PathBuf> = source["rlp"]["parts"]
        .as_array()
        .expect("the export's parts")
        .iter()
        .map(|p| state.join(p.as_str().expect("a part's path")))
        .collect();
    assert!(!parts.is_empty(), "{name}: an export of no parts");
    let pinned = source["rlp"]["sha256"].as_str().expect("a pinned sha256");
    let got = sha256(&parts)?;
    if got != pinned {
        return Err(format!("{name}: the export's sha256 is {got}, the corpus pins {pinned}"));
    }
    let genesis =
        std::fs::read_to_string(vectors.join(source["genesis"].as_str().expect("a genesis")))
            .expect("genesis");

    let dir = Scratch::new(name);
    let chain = Evm::open(lux_node::genesis::parse(&genesis).expect("a genesis document"), &dir.0)?;
    let started = Instant::now();
    let done = lux_node::import::from_reader(&chain, export(&parts)?, &lux_node::Gate::memory())
        .map_err(|e| format!("{name}: {e}"))?;
    let took = started.elapsed();

    let mut diff = Diff::default();
    let blocks = lines(&vectors.join(format!("chain_{name}.ndjson")));
    for want in &blocks {
        let number = want["number"].clone();
        let got = chain
            .call("eth_getBlockByNumber", &json!([number, false]))
            .map_err(|e| format!("block {number}: {e}"))?;
        for (field, w) in want.as_object().expect("a block") {
            diff.check(&format!("block {number}"), &format!("block.{field}"), w, &got[field]);
        }
    }
    let receipts = lines(&vectors.join(format!("chain_{name}_receipts.ndjson")));
    for want in &receipts {
        let tx = want["transactionHash"].clone();
        let got = chain
            .call("eth_getTransactionReceipt", &json!([tx]))
            .map_err(|e| format!("receipt {tx}: {e}"))?;
        for (field, w) in want.as_object().expect("a receipt") {
            diff.check(&format!("receipt {tx}"), &format!("receipt.{field}"), w, &got[field]);
        }
    }
    let tip: Value = serde_json::from_str(
        &std::fs::read_to_string(vectors.join(format!("chain_{name}_state.json"))).expect("state"),
    )
    .expect("state JSON");
    let at = json!(format!("{:#x}", tip["tip"].as_u64().expect("a tip")));
    for (address, want) in tip["accounts"].as_object().expect("accounts") {
        for (field, method) in [("balance", "eth_getBalance"), ("nonce", "eth_getTransactionCount")]
        {
            let got = chain
                .call(method, &json!([address, at]))
                .map_err(|e| format!("{method} {address}: {e}"))?;
            diff.check(address, &format!("state.{field}"), &want[field], &got);
        }
    }

    if diff.0.is_empty() {
        Ok(format!(
            "{name}: {} blocks imported in {:.1?} (peak RSS {}); {} blocks, {} receipts, {} \
             accounts equal to the Go node",
            done.blocks,
            took,
            peak_rss(),
            blocks.len(),
            receipts.len(),
            tip["accounts"].as_object().map_or(0, |a| a.len())
        ))
    } else {
        let mut out = format!("{name}: differs from the Go node\n");
        for (field, (n, examples)) in &diff.0 {
            out.push_str(&format!("  {field}: {n} differ, e.g.\n"));
            for e in examples {
                out.push_str(&format!("    {e}\n"));
            }
        }
        Err(out)
    }
}

#[test]
fn every_imported_history_answers_as_the_go_node_does() {
    let (Some(vectors), Some(state)) =
        (std::env::var_os("LUX_CONFORMANCE"), std::env::var_os("LUX_STATE"))
    else {
        panic!(
            "LUX_CONFORMANCE (luxfi/conformance vectors/) and LUX_STATE (a luxfi/state checkout) \
             must both be set; this test compares against recorded Go answers and has nothing to \
             compare without them"
        );
    };
    let (vectors, state) = (PathBuf::from(vectors), PathBuf::from(state));
    let mut names: Vec<String> = std::fs::read_dir(&vectors)
        .expect("the vectors directory")
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|f| f.strip_prefix("chain_")?.strip_suffix("_source.json").map(str::to_string))
        .collect();
    names.sort();

    let mut failed = Vec::new();
    for name in &names {
        match conform(&vectors, &state, name) {
            Ok(line) => println!("{line}"),
            Err(why) => failed.push(why),
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}
