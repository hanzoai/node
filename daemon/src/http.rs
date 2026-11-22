//! The loopback view on `127.0.0.1:3690`: the same routes for callers that
//! cannot speak ZAP. It is reachable by any local process and by a web page, so
//! it checks `Host` (against DNS rebinding) and `Origin` (against cross-site
//! requests) before anything else, and `daemon` demands a verified bearer.

use crate::daemon::{Daemon, Via};
use crate::table::Reply;
use crate::zap::{self, Request, Response};
use std::io;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const MAX_HEAD: usize = 64 * 1024;
const MAX_BODY: usize = 32 << 20;

pub async fn serve(l: TcpListener, d: Arc<Daemon>) {
    loop {
        let Ok((conn, peer)) = l.accept().await else { continue };
        if !peer.ip().is_loopback() {
            continue;
        }
        let d = d.clone();
        tokio::spawn(async move {
            let _ = connection(conn, d).await;
        });
    }
}

struct Head {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    /// Bytes read past the head.
    rest: Vec<u8>,
}

async fn read_head(c: &mut TcpStream) -> io::Result<Option<Head>> {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 4096];
    let end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p;
        }
        if buf.len() > MAX_HEAD {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "head too large"));
        }
        let n = c.read(&mut chunk).await?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let text = std::str::from_utf8(&buf[..end]).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "head"))?;
    let mut lines = text.split("\r\n");
    let mut first = lines.next().unwrap_or("").split(' ');
    let (method, target) = (first.next().unwrap_or("").to_string(), first.next().unwrap_or("").to_string());
    let headers = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
    Ok(Some(Head { method, target, headers, rest: buf[end + 4..].to_vec() }))
}

fn header<'a>(h: &'a [(String, String)], name: &str) -> Option<&'a str> {
    h.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

/// A host that is this machine's loopback, with or without the port.
pub fn loopback_host(h: &str) -> bool {
    let host = h.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit())).map(|(a, _)| a).unwrap_or(h);
    matches!(host, "127.0.0.1" | "localhost" | "[::1]")
}

/// An origin that is a local page or one of the apps' own webviews.
pub fn local_origin(o: &str) -> bool {
    let rest = o.strip_prefix("http://").or_else(|| o.strip_prefix("https://")).or_else(|| o.strip_prefix("tauri://"));
    rest.is_some_and(loopback_host)
}

fn status_line(s: u16) -> &'static str {
    match s {
        200 => "200 OK",
        201 => "201 Created",
        202 => "202 Accepted",
        204 => "204 No Content",
        304 => "304 Not Modified",
        400 => "400 Bad Request",
        401 => "401 Unauthorized",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        405 => "405 Method Not Allowed",
        408 => "408 Request Timeout",
        409 => "409 Conflict",
        410 => "410 Gone",
        413 => "413 Payload Too Large",
        421 => "421 Misdirected Request",
        422 => "422 Unprocessable Entity",
        429 => "429 Too Many Requests",
        501 => "501 Not Implemented",
        502 => "502 Bad Gateway",
        503 => "503 Service Unavailable",
        504 => "504 Gateway Timeout",
        _ => "500 Internal Server Error",
    }
}

async fn respond(c: &mut TcpStream, r: &Response) -> io::Result<()> {
    let mut out = format!("HTTP/1.1 {}\r\nConnection: close\r\nContent-Length: {}\r\n", status_line(r.status), r.body.len());
    for (k, v) in &r.headers {
        if !k.eq_ignore_ascii_case("content-length") && !k.eq_ignore_ascii_case("connection") && !k.contains(['\r', '\n']) && !v.contains(['\r', '\n']) {
            out.push_str(&format!("{k}: {v}\r\n"));
        }
    }
    out.push_str("\r\n");
    c.write_all(out.as_bytes()).await?;
    c.write_all(&r.body).await?;
    c.flush().await
}

async fn connection(mut c: TcpStream, d: Arc<Daemon>) -> io::Result<()> {
    let Some(head) = read_head(&mut c).await? else { return Ok(()) };
    if !header(&head.headers, "host").is_some_and(loopback_host) {
        return respond(&mut c, &Response::error(421, "this daemon answers on loopback only")).await;
    }
    if header(&head.headers, "origin").is_some_and(|o| !local_origin(o)) {
        return respond(&mut c, &Response::error(403, "cross-origin requests are refused")).await;
    }
    if header(&head.headers, "transfer-encoding").is_some() {
        return respond(&mut c, &Response::error(400, "send a Content-Length body")).await;
    }
    let want: usize = header(&head.headers, "content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    if want > MAX_BODY {
        return respond(&mut c, &Response::error(400, "body too large")).await;
    }

    // A streamed upstream (the engine) is proxied byte for byte, once the same
    // checks have passed.
    let path = head.target.split('?').next().unwrap_or("");
    if crate::engine::proxied(path) {
        // The probe only admits the caller and starts the engine; the caller's own request is the
        // one proxied below, so the probe is a harmless GET, never a bodiless copy of a POST.
        let probe = Request { method: "GET".into(), target: "/v1/engine/health".into(), headers: vec![], body: vec![] };
        let (reply, busy) = d.handle(Via::Http, probe).await;
        // handle() has started the engine; route again for its port.
        let _hold = busy;
        if let Reply::Whole(r) = &reply {
            if r.status >= 400 && r.status != 404 {
                return respond(&mut c, r).await;
            }
        }
        if let Ok((sub, _b)) = d.table.route(path).await {
            if let Some(port) = sub.http_port() {
                return proxy(c, head, port).await;
            }
        }
        return match reply {
            Reply::Whole(r) => respond(&mut c, &r).await,
            Reply::Frames(_) => respond(&mut c, &Response::error(502, "no stream")).await,
        };
    }

    let mut body = head.rest;
    while body.len() < want {
        let mut chunk = vec![0u8; (want - body.len()).min(64 * 1024)];
        let n = c.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(want);
    let req = Request { method: head.method, target: head.target, headers: head.headers, body };
    let (reply, _busy) = d.handle(Via::Http, req).await;
    let r = match reply {
        Reply::Whole(r) => r,
        Reply::Frames(mut rx) => collect(&mut rx).await,
    };
    respond(&mut c, &r).await
}

/// Gather a streamed ZAP-HTTP reply (head, data, end) into one response.
async fn collect(rx: &mut tokio::sync::mpsc::Receiver<Vec<u8>>) -> Response {
    let mut out = Response { status: 502, headers: vec![], body: vec![] };
    while let Some(f) = rx.recv().await {
        match zap::frame_type(&f) {
            Some(zap::FRAME_RESPONSE) => return zap::decode_response(&f).unwrap_or(out),
            Some(zap::FRAME_HEAD) => {
                if let Ok(h) = zap::decode_head(&f) {
                    out = h;
                }
            }
            Some(0x04) => {
                if let Ok(b) = zap::data_chunk(&f) {
                    out.body.extend_from_slice(b);
                }
            }
            Some(zap::FRAME_END) => break,
            _ => {}
        }
    }
    out
}

/// Forward the request to an upstream HTTP server and relay both directions.
/// `Connection: close` is forced so a second request on the socket cannot skip
/// the checks made on the first.
async fn proxy(mut c: TcpStream, head: Head, port: u16) -> io::Result<()> {
    let mut up = TcpStream::connect(("127.0.0.1", port)).await?;
    let mut out = format!("{} {} HTTP/1.1\r\nConnection: close\r\n", head.method, crate::engine::upstream(&head.target));
    for (k, v) in &head.headers {
        if !k.eq_ignore_ascii_case("connection") && !k.eq_ignore_ascii_case("authorization") && !k.eq_ignore_ascii_case("origin") {
            out.push_str(&format!("{k}: {v}\r\n"));
        }
    }
    out.push_str("\r\n");
    up.write_all(out.as_bytes()).await?;
    up.write_all(&head.rest).await?;
    tokio::io::copy_bidirectional(&mut c, &mut up).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_hosts_and_local_origins_pass() {
        assert!(loopback_host("127.0.0.1:3690"));
        assert!(loopback_host("localhost"));
        assert!(!loopback_host("evil.example:3690"));
        assert!(!loopback_host("127.0.0.1.evil.example"));
        assert!(local_origin("http://localhost:1420"));
        assert!(local_origin("tauri://localhost"));
        assert!(!local_origin("https://evil.example"));
        assert!(!local_origin("http://localhost.evil.example"));
    }

    #[test]
    fn status_line_preserves_created_and_conflict() {
        assert_eq!(status_line(200), "200 OK");
        assert_eq!(status_line(201), "201 Created");
        assert_eq!(status_line(202), "202 Accepted");
        assert_eq!(status_line(204), "204 No Content");
        assert_eq!(status_line(409), "409 Conflict");
        assert_eq!(status_line(410), "410 Gone");
        assert_eq!(status_line(422), "422 Unprocessable Entity");
        assert_eq!(status_line(429), "429 Too Many Requests");
        assert_eq!(status_line(501), "501 Not Implemented");
        assert_eq!(status_line(503), "503 Service Unavailable");
        assert_eq!(status_line(504), "504 Gateway Timeout");
        assert_eq!(status_line(599), "500 Internal Server Error");
    }

    #[tokio::test]
    async fn respond_writes_201_created_and_409_conflict_wire_lines() {
        for (code, want_prefix) in [(201u16, "HTTP/1.1 201 Created\r\n"), (409u16, "HTTP/1.1 409 Conflict\r\n")] {
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let a = l.local_addr().unwrap();
            let srv = tokio::spawn(async move {
                let (mut s, _) = l.accept().await.unwrap();
                respond(&mut s, &Response::json(code, &serde_json::json!({ "ok": code < 400 }))).await.unwrap();
            });
            let mut client = TcpStream::connect(a).await.unwrap();
            let mut buf = Vec::new();
            client.read_to_end(&mut buf).await.unwrap();
            srv.await.unwrap();
            let raw = String::from_utf8(buf).unwrap();
            assert!(raw.starts_with(want_prefix), "got {raw:?}, want prefix {want_prefix:?}");
        }
    }
}
