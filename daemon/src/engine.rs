//! `/v1/engine/*`: Hanzo Engine, the default local inference engine, with an
//! OpenAI-compatible surface (`/v1/engine/models`, `/chat/completions`,
//! `/embeddings`; the engine's own paths are `/v1/...`). It is a child process on
//! a private loopback port that hanzod picks, started on the first request and
//! stopped when idle, so there is no `:36900` listener. The engine's own program,
//! `hanzo-engine`, is pinned by digest.
//!
//! The engine runs Metal on a Mac and nothing leaves the machine: it binds
//! loopback only, does not advertise itself on the network, serves no web UI,
//! and runs with the Hugging Face hub offline, so it loads only weights already
//! in the cache (`catalog` puts them there).
//!
//! Other engines (Ollama, LM Studio, any OpenAI-compatible server) are not
//! routed here: callers use them as endpoints.
//!
//! Which model it serves is `engine.model` in `<data>/hanzo/hanzod.json`, a
//! catalog id; unset, it is the best installed model the memory suits.

use crate::catalog::{self, Entry};
use crate::paths;
use crate::pins::Pins;
use crate::proc;
use crate::table::{Fut, Reply, Subsystem};
use crate::zap::{Request, Response};
use std::io;
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

pub struct Engine {
    pins: Arc<Pins>,
    child: Mutex<Option<Child>>,
    port: StdMutex<Option<u16>>,
}

impl Engine {
    pub fn new(pins: Arc<Pins>) -> Engine {
        Engine { pins, child: Mutex::new(None), port: StdMutex::new(None) }
    }

    /// The private port the engine listens on, while it runs.
    pub fn port(&self) -> Option<u16> {
        *self.port.lock().unwrap()
    }
}

fn config() -> serde_json::Value {
    std::fs::read(paths::data().join("hanzod.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// The `hanzo-engine` arguments that serve `e` from the cache on `port`.
fn args(e: &Entry, port: u16) -> Vec<String> {
    let mut a: Vec<String> = ["--token-source", "none", "serve", "--host", "127.0.0.1", "--no-ui", "--no-advertise", "--port"].map(String::from).into();
    a.push(port.to_string());
    for s in ["--served-model-name", e.id, "--format", "gguf", "-m", e.repo, "-f", e.file] {
        a.push(s.to_string());
    }
    a
}

/// A request to `/v1/engine/<rest>` is the engine's `/v1/<rest>`.
pub fn upstream(target: &str) -> String {
    match target.strip_prefix("/v1/engine") {
        Some(rest) => format!("/v1{rest}"),
        None => target.to_string(),
    }
}

/// Whether `path` is the engine itself, as opposed to hanzod's own catalog.
pub fn proxied(path: &str) -> bool {
    (path == "/v1/engine" || path.starts_with("/v1/engine/")) && !(path == "/v1/engine/catalog" || path.starts_with("/v1/engine/catalog/"))
}

impl Engine {
    /// The model to serve: the one chosen, else the best installed that the
    /// memory suits, else the smallest installed.
    pub fn model(&self) -> Option<&'static Entry> {
        let hub = catalog::hub();
        let on_disk = |e: &&Entry| catalog::installed(&hub, e).is_some();
        config()
            .get("engine")
            .and_then(|c| c.get("model")?.as_str().and_then(catalog::find))
            .filter(on_disk)
            .or_else(|| {
                let mem = catalog::memory();
                catalog::MODELS.iter().rev().filter(on_disk).find(|m| mem >= m.memory << 30).or_else(|| catalog::MODELS.iter().find(on_disk))
            })
    }

    /// Choose `id`, persist it, and move a running engine onto it.
    pub async fn select(&self, id: &str) -> io::Result<()> {
        let path = paths::data().join("hanzod.json");
        let mut cfg = config();
        if !cfg.is_object() {
            cfg = serde_json::json!({});
        }
        cfg["engine"] = serde_json::json!({ "model": id });
        let tmp = path.with_extension("json.tmp");
        std::fs::create_dir_all(paths::data())?;
        std::fs::write(&tmp, serde_json::to_vec_pretty(&cfg).map_err(io::Error::other)?)?;
        std::fs::rename(&tmp, &path)?;
        if self.port().is_some() {
            self.start().await?;
        }
        Ok(())
    }
}

impl Subsystem for Engine {
    fn name(&self) -> &'static str {
        "engine"
    }
    fn prefix(&self) -> &'static str {
        "/v1/engine"
    }
    fn idle(&self) -> Duration {
        Duration::from_secs(900)
    }
    fn start(&self) -> Fut<'_, io::Result<()>> {
        Box::pin(async move {
            let mut slot = self.child.lock().await;
            if let Some(mut old) = slot.take() {
                proc::terminate(&mut old, Duration::from_secs(5)).await;
            }
            let bin = proc::find("hanzo-engine").ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "hanzo-engine (Hanzo Engine) is not installed"))?;
            proc::owned_by_us_alone(&bin)?;
            self.pins.verify("hanzo-engine", &bin).map_err(proc::refuse)?;
            let model = self.model().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no local model is installed; POST /v1/engine/catalog/pull {\"id\"} first"))?;
            let port = proc::free_port()?;
            *self.port.lock().unwrap() = None;
            let child = Command::new(&bin)
                .args(args(model, port))
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("HOME", dirs::home_dir().unwrap_or_default())
                .env("HF_HUB_OFFLINE", "1")
                .envs(["HF_HOME", "HF_HUB_CACHE"].iter().filter_map(|k| std::env::var_os(k).map(|v| (*k, v))))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()?;
            *slot = Some(child);
            // Up means the listener answers; the model loads in the background.
            let up = proc::until(Duration::from_secs(180), || async {
                matches!(fetch(([127, 0, 0, 1], port).into(), &Request { method: "GET".into(), target: "/health".into(), headers: vec![], body: vec![] }).await, Ok(r) if r.status == 200)
            })
            .await;
            if !up {
                if let Some(mut c) = slot.take() {
                    proc::terminate(&mut c, Duration::from_secs(2)).await;
                }
                return Err(io::Error::other("the engine did not come up"));
            }
            *self.port.lock().unwrap() = Some(port);
            Ok(())
        })
    }
    fn frame<'a>(&'a self, _who: &'a str, req: Request) -> Fut<'a, io::Result<Reply>> {
        Box::pin(async move {
            let port = self.port().ok_or_else(|| io::Error::other("the engine is not running"))?;
            let req = Request { target: upstream(&req.target), ..req };
            Ok(Reply::Whole(fetch(([127, 0, 0, 1], port).into(), &req).await?))
        })
    }
    fn stop(&self) -> Fut<'_, ()> {
        Box::pin(async move {
            *self.port.lock().unwrap() = None;
            if let Some(mut c) = self.child.lock().await.take() {
                proc::terminate(&mut c, Duration::from_secs(10)).await;
            }
        })
    }
    fn http_port(&self) -> Option<u16> {
        self.port()
    }
}

/// One HTTP/1.1 request to a loopback upstream, answered whole (a streamed reply is
/// gathered; callers that need the stream use the loopback view, which is
/// proxied byte for byte).
pub async fn fetch(addr: std::net::SocketAddr, req: &Request) -> io::Result<Response> {
    let port = addr.port();
    let mut s = TcpStream::connect(addr).await?;
    let mut head = format!("{} {} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Length: {}\r\n", req.method, req.target, req.body.len());
    for (k, v) in &req.headers {
        if !matches!(k.to_ascii_lowercase().as_str(), "host" | "content-length" | "connection" | "transfer-encoding" | "authorization") {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).await?;
    s.write_all(&req.body).await?;
    let mut raw = Vec::new();
    s.take(64 << 20).read_to_end(&mut raw).await?;
    parse_http(&raw)
}

fn parse_http(raw: &[u8]) -> io::Result<Response> {
    let bad = || io::Error::new(io::ErrorKind::InvalidData, "malformed engine response");
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or_else(bad)?;
    let head = std::str::from_utf8(&raw[..end]).map_err(|_| bad())?;
    let mut lines = head.split("\r\n");
    let status = lines.next().and_then(|l| l.split(' ').nth(1)).and_then(|s| s.parse().ok()).ok_or_else(bad)?;
    let mut headers = Vec::new();
    let mut chunked = false;
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            let (k, v) = (k.trim(), v.trim());
            if k.eq_ignore_ascii_case("transfer-encoding") && v.eq_ignore_ascii_case("chunked") {
                chunked = true;
            } else if !k.eq_ignore_ascii_case("content-length") && !k.eq_ignore_ascii_case("connection") {
                headers.push((k.to_string(), v.to_string()));
            }
        }
    }
    let body = &raw[end + 4..];
    let body = if chunked { dechunk(body).ok_or_else(bad)? } else { body.to_vec() };
    Ok(Response { status, headers, body })
}

fn dechunk(mut b: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let eol = b.windows(2).position(|w| w == b"\r\n")?;
        let n = usize::from_str_radix(std::str::from_utf8(&b[..eol]).ok()?.split(';').next()?.trim(), 16).ok()?;
        b = &b[eol + 2..];
        if n == 0 {
            return Some(out);
        }
        out.extend_from_slice(b.get(..n)?);
        b = b.get(n + 2..)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chunked_reply_is_gathered() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let r = parse_http(raw).unwrap();
        assert_eq!((r.status, r.body.as_slice()), (200, b"hello world".as_slice()));
        assert!(parse_http(b"garbage").is_err());
    }
}
