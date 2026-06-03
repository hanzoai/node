// Sidecar fallback: talk to `zoo-machine machined` over a Unix socket using
// hand-rolled HTTP/1.1. Used when libluxmachine isn't linked. We deliberately
// avoid pulling hyper/reqwest into this crate's dep tree — every byte of dep
// surface gets compiled into hanzod.
//
// Protocol mirrors the existing daemon (see hanzo-desktop/src-tauri client):
//   GET    /v1/health
//   GET    /v1/machines
//   GET    /v1/machines/{name}
//   POST   /v1/machines           body: spec
//   POST   /v1/machines/{name}/start
//   POST   /v1/machines/{name}/stop
//   DELETE /v1/machines/{name}

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::types::{Info, Spec, State};

#[derive(Debug, Clone)]
pub struct SidecarClient {
    socket: PathBuf,
    timeout: Duration,
}

impl SidecarClient {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self { socket: socket.into(), timeout: Duration::from_secs(15) }
    }

    pub fn with_timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket
    }

    pub fn health(&self) -> Result<Health> {
        self.get_json("/v1/health")
    }

    pub fn list(&self) -> Result<Vec<Info>> {
        let raw: Vec<WireMachine> = self.get_json("/v1/machines")?;
        Ok(raw.into_iter().map(Into::into).collect())
    }

    pub fn get(&self, name: &str) -> Result<Info> {
        let raw: WireMachine = self.get_json(&format!("/v1/machines/{}", name))?;
        Ok(raw.into())
    }

    pub fn create(&self, spec: &Spec) -> Result<Info> {
        let raw: WireMachine = self.send_json("POST", "/v1/machines", Some(spec))?;
        Ok(raw.into())
    }

    pub fn start(&self, name: &str) -> Result<()> {
        self.send_empty("POST", &format!("/v1/machines/{}/start", name))
    }

    pub fn stop(&self, name: &str) -> Result<()> {
        self.send_empty("POST", &format!("/v1/machines/{}/stop", name))
    }

    pub fn delete(&self, name: &str) -> Result<()> {
        self.send_empty("DELETE", &format!("/v1/machines/{}", name))
    }

    // ---- internals ---------------------------------------------------------

    fn get_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T> {
        let body = self.request("GET", path, None)?;
        serde_json::from_slice(&body).map_err(|e| Error::Sidecar(format!("decode: {e}")))
    }

    fn send_json<B: Serialize, T: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        path: &str,
        body: Option<&B>,
    ) -> Result<T> {
        let payload = match body {
            Some(b) => Some(
                serde_json::to_vec(b).map_err(|e| Error::Sidecar(format!("encode: {e}")))?,
            ),
            None => None,
        };
        let resp = self.request(method, path, payload.as_deref())?;
        serde_json::from_slice(&resp).map_err(|e| Error::Sidecar(format!("decode: {e}")))
    }

    fn send_empty(&self, method: &str, path: &str) -> Result<()> {
        let _ = self.request(method, path, None)?;
        Ok(())
    }

    fn request(&self, method: &str, path: &str, body: Option<&[u8]>) -> Result<Vec<u8>> {
        let mut sock = UnixStream::connect(&self.socket).map_err(|e| {
            Error::Sidecar(format!("connect {}: {e}", self.socket.display()))
        })?;
        sock.set_read_timeout(Some(self.timeout))
            .map_err(|e| Error::Sidecar(format!("read-timeout: {e}")))?;
        sock.set_write_timeout(Some(self.timeout))
            .map_err(|e| Error::Sidecar(format!("write-timeout: {e}")))?;

        let body_bytes = body.unwrap_or(&[]);
        let mut req = format!(
            "{method} {path} HTTP/1.1\r\n\
             Host: machined.local\r\n\
             Accept: application/json\r\n\
             Connection: close\r\n\
             Content-Length: {}\r\n",
            body_bytes.len()
        );
        if !body_bytes.is_empty() {
            req.push_str("Content-Type: application/json\r\n");
        }
        req.push_str("\r\n");
        sock.write_all(req.as_bytes())
            .map_err(|e| Error::Sidecar(format!("write head: {e}")))?;
        if !body_bytes.is_empty() {
            sock.write_all(body_bytes)
                .map_err(|e| Error::Sidecar(format!("write body: {e}")))?;
        }
        sock.flush().ok();

        let mut raw = Vec::with_capacity(2048);
        sock.read_to_end(&mut raw)
            .map_err(|e| Error::Sidecar(format!("read: {e}")))?;
        parse_response(&raw)
    }
}

fn parse_response(raw: &[u8]) -> Result<Vec<u8>> {
    let split = find_double_crlf(raw)
        .ok_or_else(|| Error::Sidecar("no header terminator".into()))?;
    let head = std::str::from_utf8(&raw[..split])
        .map_err(|_| Error::Sidecar("non-utf8 headers".into()))?;
    let mut lines = head.lines();
    let status_line = lines.next().unwrap_or("");
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| Error::Sidecar(format!("bad status line: {status_line}")))?;

    let mut chunked = false;
    let mut content_length: Option<usize> = None;
    for line in lines {
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            content_length = v.trim().parse().ok();
        }
        if let Some(v) = lower.strip_prefix("transfer-encoding:") {
            if v.trim() == "chunked" {
                chunked = true;
            }
        }
    }

    let body_raw = &raw[split + 4..];
    let body = if chunked {
        decode_chunked(body_raw)?
    } else if let Some(n) = content_length {
        body_raw[..n.min(body_raw.len())].to_vec()
    } else {
        body_raw.to_vec()
    };

    if status == 404 {
        return Err(Error::NotFound);
    }
    if status == 204 {
        return Ok(Vec::new());
    }
    if !(200..300).contains(&status) {
        let msg = String::from_utf8_lossy(&body).into_owned();
        return Err(Error::Sidecar(format!("daemon {status}: {msg}")));
    }
    Ok(body)
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn decode_chunked(buf: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(buf.len());
    let mut i = 0usize;
    while i < buf.len() {
        let line_end = match buf[i..].windows(2).position(|w| w == b"\r\n") {
            Some(p) => i + p,
            None => return Err(Error::Sidecar("chunk header truncated".into())),
        };
        let size_str = std::str::from_utf8(&buf[i..line_end])
            .map_err(|_| Error::Sidecar("chunk size not utf-8".into()))?;
        let size = usize::from_str_radix(size_str.trim().split(';').next().unwrap_or(""), 16)
            .map_err(|_| Error::Sidecar(format!("bad chunk size: {size_str}")))?;
        i = line_end + 2;
        if size == 0 {
            break;
        }
        if i + size > buf.len() {
            return Err(Error::Sidecar("chunk truncated".into()));
        }
        out.extend_from_slice(&buf[i..i + size]);
        i += size;
        if i + 2 > buf.len() || &buf[i..i + 2] != b"\r\n" {
            return Err(Error::Sidecar("missing chunk trailer".into()));
        }
        i += 2;
    }
    Ok(out)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Health {
    pub ok: bool,
    pub version: String,
}

// Wire shape used by the existing zoo-machine daemon. Distinct from `Info`
// because the daemon emits ISO-8601 timestamps (string) while the C ABI emits
// epoch seconds (int64); we normalise on the way in.
#[derive(Debug, Deserialize)]
struct WireMachine {
    name: String,
    distro: String,
    cpus: u32,
    memory_mb: u64,
    disk_gb: u64,
    state: State,
    #[serde(default)]
    ip: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    started_at: Option<String>,
}

impl From<WireMachine> for Info {
    fn from(w: WireMachine) -> Info {
        Info {
            name: w.name,
            distro: w.distro,
            cpus: w.cpus,
            memory_mb: w.memory_mb,
            disk_gb: w.disk_gb,
            state: w.state,
            ip: w.ip,
            created_at: parse_ts(w.created_at.as_deref()),
            started_at: parse_ts(w.started_at.as_deref()),
        }
    }
}

fn parse_ts(_s: Option<&str>) -> i64 {
    // We don't pull chrono into this crate. Callers that need precise
    // timestamps should use the native Manager (which yields epoch seconds
    // directly) or parse the raw JSON themselves.
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_response() {
        let raw = b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 9\r\n\r\n{\"ok\":1}\n";
        let body = parse_response(raw).unwrap();
        assert_eq!(body, b"{\"ok\":1}\n");
    }

    #[test]
    fn parses_chunked_response() {
        let raw =
            b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n5\r\nhello\r\n5\r\nworld\r\n0\r\n\r\n";
        let body = parse_response(raw).unwrap();
        assert_eq!(body, b"helloworld");
    }

    #[test]
    fn maps_404() {
        let raw = b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n";
        match parse_response(raw) {
            Err(Error::NotFound) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn maps_5xx() {
        let raw = b"HTTP/1.1 500 oops\r\ncontent-length: 4\r\n\r\nboom";
        match parse_response(raw) {
            Err(Error::Sidecar(s)) => assert!(s.contains("500"), "got: {s}"),
            other => panic!("unexpected: {other:?}"),
        }
    }
}
