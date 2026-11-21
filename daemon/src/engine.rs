//! `/v1/engine/*`: Hanzo Engine, the default local inference engine, with an
//! OpenAI-compatible surface (`/v1/engine/models`, `/chat/completions`,
//! `/embeddings`). It is a child process on a private loopback port that hanzod
//! picks, started on the first request and stopped when idle, so there is no
//! `:36900` listener. The engine's own program is pinned by digest.
//!
//! Other engines (Ollama, LM Studio, any OpenAI-compatible server) are not
//! routed here: callers use them as endpoints.
//!
//! Which model it serves is `engine.args` in `<data>/hanzo/hanzod.json`.

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

const DEFAULT_ARGS: [&str; 5] = ["gguf", "-m", "zenlm/zen-eco-4b-agent-gguf", "-f", "zen-eco-3b-agent-q4_k_m.gguf"];

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

fn args() -> Vec<String> {
    let cfg = std::fs::read(paths::data().join("hanzod.json")).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    cfg.and_then(|c| {
        c.get("engine")?.get("args")?.as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect::<Vec<_>>())
    })
    .filter(|a| !a.is_empty())
    .unwrap_or_else(|| DEFAULT_ARGS.iter().map(|s| s.to_string()).collect())
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
            let bin = proc::find("hanzo-server").ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "hanzo-server (Hanzo Engine) is not installed"))?;
            proc::owned_by_us_alone(&bin)?;
            self.pins.verify("hanzo-engine", &bin).map_err(proc::refuse)?;
            let port = proc::free_port()?;
            let child = Command::new(&bin)
                .arg("--port")
                .arg(port.to_string())
                .args(args())
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("HOME", dirs::home_dir().unwrap_or_default())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()?;
            *slot = Some(child);
            // Up means the listener answers; the model loads in the background.
            let up = proc::until(Duration::from_secs(60), || async {
                matches!(fetch(port, &Request { method: "GET".into(), target: "/health".into(), headers: vec![], body: vec![] }).await, Ok(r) if r.status == 200)
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
            Ok(Reply::Whole(fetch(port, &req).await?))
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

/// One HTTP/1.1 request to the engine, answered whole (a streamed reply is
/// gathered; callers that need the stream use the loopback view, which is
/// proxied byte for byte).
pub async fn fetch(port: u16, req: &Request) -> io::Result<Response> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await?;
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
