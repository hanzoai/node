// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! The `/v1/chain` route of `hanzod` (HIP-1330 phase 6).
//!
//!   GET    /v1/chain                 status
//!   PUT    /v1/chain {"network": n}  fetch, verify and run `hanzo-chain` on n
//!   DELETE /v1/chain                 stop it and release its ports
//!
//! Until PUT, nothing exists: no listener, no child, and no `~/.hanzod`. A
//! disabled chain keeps its keys, because they are the validator's identity; only
//! a never-enabled one leaves no state.
//!
//! `hanzo-chain` is fetched as `hanzo-chain-<target>.tar.gz` from the release the
//! pins name. The tarball and the binary inside it are each checked against a
//! pinned sha256, and the binary is checked again at every exec. A network whose
//! `lux-node` tag differs from the one the pinned binary was built with is
//! refused, because two nodes on different tags are two networks.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// What `daemon/pins.json` pins for the chain.
///
/// ```json
/// { "version": "0.2.5", "lux_node": "v0.2.2",
///   "targets":  { "darwin-arm64": { "tarball": "<sha256>", "binary": "<sha256>" } },
///   "networks": { "devnet": { "lux_node": "v0.2.2", "pchain": "host:port" } } }
/// ```
pub struct Pins(Value);

impl Pins {
    pub fn parse(json: &str) -> Result<Pins, String> {
        serde_json::from_str(json).map(Pins).map_err(|e| format!("pins: {e}"))
    }
    fn target(&self, t: &str) -> Option<(&str, &str)> {
        let e = self.0.get("targets")?.get(t)?;
        Some((e.get("tarball")?.as_str()?, e.get("binary")?.as_str()?))
    }
    fn version(&self) -> Option<&str> {
        self.0.get("version")?.as_str()
    }
    fn network(&self, n: &str) -> Option<&Value> {
        self.0.get("networks")?.get(n)
    }
}

pub struct Steward {
    /// `~/.hanzod`: the validator's keys and chain. Created on first enable.
    pub data: PathBuf,
    /// Where the verified binary lives: `~/.hanzo/bin`.
    pub bin: PathBuf,
    /// Release download root; `<base>/v<version>/hanzo-chain-<target>.tar.gz`.
    pub base: String,
    pub pins: Pins,
    pub rpc: SocketAddr,
    pub mesh: SocketAddr,
    child: Mutex<Option<Child>>,
}

pub struct Reply {
    pub status: u16,
    pub body: Value,
}

fn refuse(status: u16, why: impl ToString) -> Reply {
    Reply { status, body: json!({ "error": why.to_string() }) }
}

pub fn target() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        o => o,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        a => a,
    };
    format!("{os}-{arch}")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

impl Steward {
    pub fn new(data: PathBuf, bin: PathBuf, base: String, pins: Pins) -> Steward {
        Steward {
            data,
            bin,
            base,
            pins,
            rpc: ([127, 0, 0, 1], 9630).into(),
            mesh: ([127, 0, 0, 1], 9631).into(),
            child: Mutex::new(None),
        }
    }

    fn binary(&self) -> PathBuf {
        self.bin.join("hanzo-chain")
    }

    fn marker(&self) -> PathBuf {
        self.data.join("network")
    }

    /// The route. `hanzod` calls this with the verb and the body of a frame whose
    /// bearer it has already verified.
    pub fn handle(&self, method: &str, body: &[u8]) -> Reply {
        match method {
            "GET" => Reply { status: 200, body: self.status() },
            "PUT" => {
                let network = serde_json::from_slice::<Value>(body)
                    .ok()
                    .and_then(|v| v.get("network")?.as_str().map(str::to_owned));
                match network {
                    Some(n) => self.enable(&n),
                    None => refuse(400, "body must be {\"network\": \"mainnet|testnet|devnet\"}"),
                }
            }
            "DELETE" => self.disable(),
            _ => refuse(405, "GET, PUT or DELETE"),
        }
    }

    /// Start again after `hanzod` restarts, if the user had it on.
    pub fn resume(&self) {
        if let Ok(n) = std::fs::read_to_string(self.marker()) {
            let _ = self.enable(n.trim());
        }
    }

    pub fn status(&self) -> Value {
        let network = std::fs::read_to_string(self.marker()).ok().map(|s| s.trim().to_owned());
        let running = self.alive();
        let mut s = json!({
            "enabled": network.is_some(),
            "running": running,
            "network": network,
            "version": self.pins.version(),
        });
        if running {
            s["height"] = self.height().map_or(Value::Null, Value::from);
        }
        s
    }

    fn alive(&self) -> bool {
        let mut g = self.child.lock().unwrap();
        match g.as_mut().map(|c| c.try_wait()) {
            Some(Ok(None)) => true,
            Some(_) => {
                *g = None;
                false
            }
            None => false,
        }
    }

    fn height(&self) -> Option<u64> {
        let url = format!("http://{}/v1/chain/hanzo/rpc", self.rpc);
        let r: Value = ureq::post(&url)
            .timeout(Duration::from_secs(2))
            .send_json(json!({"jsonrpc":"2.0","id":1,"method":"eth_blockNumber","params":[]}))
            .ok()?
            .into_json()
            .ok()?;
        u64::from_str_radix(r["result"].as_str()?.trim_start_matches("0x"), 16).ok()
    }

    pub fn enable(&self, network: &str) -> Reply {
        if self.alive() {
            return refuse(409, "the chain is already running; disable it first");
        }
        if !["mainnet", "testnet", "devnet"].contains(&network) {
            return refuse(400, format!("unknown network {network:?}"));
        }
        let (tarball, binary) = match self.pins.target(&target()) {
            Some(t) => t,
            None => return refuse(422, format!("no pinned digest for {}", target())),
        };
        let version = self.pins.version().unwrap_or_default();
        let net = match self.pins.network(network) {
            Some(n) => n,
            None => return refuse(422, format!("no pinned network {network}")),
        };
        // Two nodes on different lux-node tags are two networks.
        let built = self.pins.0.get("lux_node").and_then(Value::as_str);
        if built.is_none() || net.get("lux_node").and_then(Value::as_str) != built {
            return refuse(
                422,
                format!(
                    "lux-node mismatch: the pinned chain is built on {}, {network} runs {}",
                    built.unwrap_or("nothing"),
                    net.get("lux_node").and_then(Value::as_str).unwrap_or("nothing")
                ),
            );
        }
        let Some(pchain) = net.get("pchain").and_then(Value::as_str) else {
            return refuse(422, format!("no committee source pinned for {network}"));
        };
        if let Err(e) = self.fetch(version, tarball, binary) {
            return refuse(e.0, e.1);
        }
        if let Err(e) = self.private_dir() {
            return refuse(500, e);
        }
        let mut cmd = Command::new(self.binary());
        cmd.args(["--network", network, "--pchain", pchain])
            .arg("--data")
            .arg(&self.data)
            .args(["--rpc", &self.rpc.to_string(), "--mesh", &self.mesh.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Keys the child makes on first use are the user's alone.
            unsafe {
                cmd.pre_exec(|| {
                    libc::umask(0o077);
                    Ok(())
                })
            };
        }
        // The binary on disk is the one that was verified, at this exec too.
        match std::fs::read(self.binary()) {
            Ok(b) if sha256(&b) == binary => {}
            _ => return refuse(422, "the installed hanzo-chain does not match its pinned digest"),
        }
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return refuse(500, format!("starting hanzo-chain: {e}")),
        };
        // Up means the RPC port answers; an exit before that is the refusal.
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            if let Ok(Some(st)) = child.try_wait() {
                return refuse(502, format!("hanzo-chain exited at start: {st}"));
            }
            if TcpStream::connect_timeout(&self.rpc, Duration::from_millis(200)).is_ok() {
                break;
            }
            if Instant::now() > until {
                let _ = child.kill();
                let _ = child.wait();
                return refuse(502, "hanzo-chain did not open its RPC port in 20 s");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if let Err(e) = std::fs::write(self.marker(), network) {
            let _ = child.kill();
            let _ = child.wait();
            return refuse(500, format!("recording the network: {e}"));
        }
        *self.child.lock().unwrap() = Some(child);
        Reply { status: 200, body: self.status() }
    }

    pub fn disable(&self) -> Reply {
        if let Some(mut c) = self.child.lock().unwrap().take() {
            #[cfg(unix)]
            unsafe {
                libc::kill(c.id() as i32, libc::SIGTERM);
            }
            let until = Instant::now() + Duration::from_secs(10);
            while !matches!(c.try_wait(), Ok(Some(_))) {
                if Instant::now() > until {
                    let _ = c.kill();
                    let _ = c.wait();
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        let _ = std::fs::remove_file(self.marker());
        Reply { status: 200, body: self.status() }
    }

    /// `~/.hanzod` at 0700, holding nothing another user can read.
    fn private_dir(&self) -> Result<(), String> {
        std::fs::create_dir_all(&self.data).map_err(|e| format!("{}: {e}", self.data.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.data, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
            for f in std::fs::read_dir(&self.data).map_err(|e| e.to_string())?.flatten() {
                let m = f.metadata().map_err(|e| e.to_string())?.permissions().mode();
                if f.file_type().map_or(false, |t| t.is_file()) && m & 0o077 != 0 {
                    return Err(format!("{} is readable by others (mode {:o}); refusing to run", f.path().display(), m & 0o777));
                }
            }
        }
        Ok(())
    }

    /// Download, verify the tarball, extract, verify the binary, install it. A
    /// binary already installed that matches the pin is not fetched again.
    fn fetch(&self, version: &str, tarball: &str, binary: &str) -> Result<(), (u16, String)> {
        if matches!(std::fs::read(self.binary()), Ok(b) if sha256(&b) == binary) {
            return Ok(());
        }
        let url = format!("{}/v{version}/hanzo-chain-{}.tar.gz", self.base, target());
        let mut gz = Vec::new();
        ureq::get(&url)
            .timeout(Duration::from_secs(120))
            .call()
            .map_err(|e| (502, format!("fetching {url}: {e}")))?
            .into_reader()
            .take(256 << 20)
            .read_to_end(&mut gz)
            .map_err(|e| (502, format!("reading {url}: {e}")))?;
        if sha256(&gz) != tarball {
            return Err((422, format!("{url} does not match its pinned digest")));
        }
        let mut found = None;
        let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(&gz[..]));
        for e in ar.entries().map_err(|e| (422, e.to_string()))? {
            let e = e.map_err(|e| (422, e.to_string()))?;
            if e.path().map_err(|e| (422, e.to_string()))?.as_ref() == Path::new("hanzo-chain") {
                let mut b = Vec::new();
                e.take(256 << 20).read_to_end(&mut b).map_err(|e| (422, e.to_string()))?;
                found = Some(b);
            }
        }
        let bytes = found.ok_or((422, "the archive holds no hanzo-chain".to_string()))?;
        if sha256(&bytes) != binary {
            return Err((422, "the extracted hanzo-chain does not match its pinned digest".into()));
        }
        std::fs::create_dir_all(&self.bin).map_err(|e| (500, e.to_string()))?;
        let tmp = self.bin.join("hanzo-chain.part");
        let mut f = std::fs::File::create(&tmp).map_err(|e| (500, e.to_string()))?;
        f.write_all(&bytes).map_err(|e| (500, e.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).map_err(|e| (500, e.to_string()))?;
        }
        std::fs::rename(&tmp, self.binary()).map_err(|e| (500, e.to_string()))
    }
}

impl Drop for Steward {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}
