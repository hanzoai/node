// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! JSON-RPC over HTTP, for whatever chains are registered.
//!
//! One HTTP implementation in the node rather than one per chain. Go hands each
//! VM an `http.Handler` and lets it route inside; here a chain answers one
//! method at a time through [`Vm::call`] and the routing is done once, which
//! means a new chain gets a working RPC surface by existing.
//!
//! THE ROUTES ARE THE RUNNING NODE'S. A chain is served under the name the
//! node gives it — `c` for Lux's C-chain, the brand's own name for a brand's —
//! at `/v1/chain/<name>` and `/v1/chain/<name>/rpc`, one parser that does not
//! care which spelling was used, because two spellings of one route is not two
//! routes. A name the node does not serve is a 404, or, when an archive is
//! configured, the archive's to answer under the same name.
//!
//! WHAT A HOSTILE CLIENT CANNOT DO. Every limit here exists because the
//! opposite is a way to take the node down with a socket:
//!
//! - a request body is capped and the cap is checked against `Content-Length`
//!   BEFORE anything is allocated;
//! - a connection that goes quiet is dropped on a timeout, so it cannot hold a
//!   thread forever;
//! - the number of connections being served at once is capped, so a flood
//!   costs a bounded number of threads rather than one per socket;
//! - a batch is capped, because one request that unrolls into a million is the
//!   same attack wearing a different hat.
//!
//! An error NEVER carries anything the caller did not already send. A chain's
//! refusal reaches the client as a JSON-RPC error object and the node's own
//! failures reach it as a status code, because a client that can read this
//! node's internals through its error strings is a client that has been handed
//! a map.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::vm::{Error, Vm};

/// The largest request body this server will read.
///
/// A signed transaction is a few kilobytes; a megabyte is generous for a batch
/// of them and small enough that a thousand concurrent liars cannot exhaust
/// memory.
pub const BODY_MAX: usize = 1024 * 1024;

/// The most requests one batch may carry.
pub const BATCH_MAX: usize = 64;

/// How long a connection may be silent before it is dropped.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// How many connections are served at once.
pub const CONNECTIONS_MAX: usize = 128;

/// The header block a client may send before it is judged hostile.
const HEAD_MAX: usize = 16 * 1024;

/// The JSON-RPC codes this server uses. They are the specification's, so a
/// client's own error handling works without knowing anything about Lux.
mod code {
    pub const PARSE: i64 = -32700;
    pub const REQUEST: i64 = -32600;
    pub const NO_METHOD: i64 = -32601;
    pub const PARAMS: i64 = -32602;
    pub const INTERNAL: i64 = -32603;
}

/// The largest answer read back from an archive. Larger than a request,
/// because a log query can be; bounded, because an archive that claimed a
/// terabyte would otherwise be believed.
const ARCHIVE_MAX: usize = 16 * 1024 * 1024;

/// The longest chain name a path may carry. A CB58 blockchain id is 50.
const ALIAS_MAX: usize = 64;

/// The chains this node serves, and the answers that are the node's own rather
/// than any chain's.
pub struct Rpc {
    network: u64,
    version: String,
    chains: BTreeMap<String, Arc<dyn Vm>>,
    /// The chain served first, which `/` advertises as this node's RPC.
    primary: Option<String>,
    archive: Option<Archive>,
}

/// Where calls this node cannot answer are sent: `http://host:port`.
#[derive(Clone, Debug)]
struct Archive {
    url: String,
    host: String,
    port: u16,
}

impl Archive {
    fn parse(url: &str) -> Result<Self, String> {
        let bad = |why: &str| format!("{url}: {why}");
        let rest =
            url.strip_prefix("http://").ok_or_else(|| bad("an archive is http://host:port"))?;
        let rest = rest.trim_end_matches('/');
        if rest.contains('/') {
            return Err(bad("an archive is a host and a port, with no path"));
        }
        // Everything here reaches a request line, so nothing that could end one.
        if !rest.bytes().all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b)) {
            return Err(bad("not a host"));
        }
        let (host, port) = match rest.rsplit_once(':').filter(|(h, _)| !h.ends_with(':')) {
            Some((h, p)) if !h.is_empty() => {
                (h, p.parse::<u16>().map_err(|e| bad(&format!("port: {e}")))?)
            }
            _ => (rest, 80),
        };
        if host.is_empty() {
            return Err(bad("no host"));
        }
        Ok(Self { url: url.to_string(), host: host.to_string(), port })
    }
}

impl Rpc {
    /// A server for network `network`, reporting itself as `version` — the
    /// client, as `web3_clientVersion` names it.
    pub fn new(network: u64, version: impl Into<String>) -> Self {
        Self {
            network,
            version: version.into(),
            chains: BTreeMap::new(),
            primary: None,
            archive: None,
        }
    }

    /// Send what this node cannot answer to the archive at `url`, under the
    /// chain name it was asked by.
    pub fn with_archive(mut self, url: &str) -> Result<Self, String> {
        self.archive = Some(Archive::parse(url)?);
        Ok(self)
    }

    /// Serve `vm` at `/v1/chain/<name>`.
    ///
    /// The name is the node's to give, not the chain's: one EVM is Lux's
    /// C-chain on one node and a brand's chain on another.
    pub fn with(mut self, name: &str, vm: Arc<dyn Vm>) -> Self {
        let name = name.to_ascii_lowercase();
        self.primary.get_or_insert_with(|| name.clone());
        self.chains.insert(name, vm);
        self
    }

    pub fn chain(&self, alias: &str) -> Option<&Arc<dyn Vm>> {
        self.chains.get(&alias.to_ascii_lowercase())
    }

    /// Bind and serve until the returned handle is stopped.
    pub fn listen(self, bind: SocketAddr) -> std::io::Result<Serving> {
        let listener = TcpListener::bind(bind)?;
        let addr = listener.local_addr()?;
        listener.set_nonblocking(true)?;

        let running = Arc::new(AtomicBool::new(true));
        let live = Arc::new(AtomicUsize::new(0));
        let node = Arc::new(self);

        let stop = running.clone();
        let thread = std::thread::spawn(move || {
            while stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        // The listener polls, so it is non-blocking, and on BSD
                        // and macOS a socket accepted from it inherits that;
                        // on Linux it does not. Inherited, the first read of
                        // the request returns EAGAIN, which `read_request`
                        // cannot tell from a request it could not parse — so a
                        // valid call whose bytes had not landed yet was
                        // answered "400 malformed request", on one platform,
                        // depending on timing.
                        let _ = stream.set_nonblocking(false);
                        // A flood costs a bounded number of threads. Past the
                        // cap the socket is closed at once rather than queued,
                        // because a queue is just a slower way to run out.
                        if live.load(Ordering::Relaxed) >= CONNECTIONS_MAX {
                            drop(stream);
                            continue;
                        }
                        live.fetch_add(1, Ordering::Relaxed);
                        let node = node.clone();
                        let live = live.clone();
                        std::thread::spawn(move || {
                            let _ = serve(&node, stream);
                            live.fetch_sub(1, Ordering::Relaxed);
                        });
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
        });

        Ok(Serving { addr, running, thread: Some(thread) })
    }

    /// Answer one JSON-RPC request. `alias` is the chain the URL named, or
    /// `None` for a call to the node itself.
    fn answer(&self, alias: Option<&str>, request: &Value) -> Option<Value> {
        let id = request.get("id").cloned();
        // A request without an id is a notification: it is answered by doing
        // the work and saying nothing.
        let quiet = id.is_none();

        let Some(method) = request.get("method").and_then(|m| m.as_str()) else {
            return (!quiet).then(|| fail(id, code::REQUEST, "no method"));
        };
        let params = request.get("params").cloned().unwrap_or(json!([]));

        let outcome = match alias {
            None => self.node_call(method, &params),
            // The client is the node, whichever chain was asked.
            Some(name) if method == "web3_clientVersion" && self.chain(name).is_some() => {
                Ok(json!(self.version))
            }
            Some(name) => {
                let answered = match self.chain(name) {
                    Some(vm) => vm.call(method, &params),
                    None => Err(Error::NotFound),
                };
                match answered {
                    // What this node does not hold, an archive may.
                    Err(Error::BadRequest(_) | Error::NoMethod(_) | Error::NotFound) => {
                        if let Some(archived) = self.forward(name, request) {
                            return Some(archived);
                        }
                        answered
                    }
                    other => other,
                }
            }
        };

        if quiet {
            return None;
        }
        Some(match outcome {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(e) => {
                let (c, message) = match &e {
                    Error::NoMethod(m) => {
                        (code::NO_METHOD, format!("the method {m} does not exist"))
                    }
                    Error::BadRequest(w) => (code::PARAMS, w.clone()),
                    Error::Malformed(w) => (code::PARAMS, w.clone()),
                    Error::NotFound => (code::INTERNAL, "not found".into()),
                    Error::Empty => (code::INTERNAL, "nothing to build".into()),
                    Error::Invalid(w) | Error::Unavailable(w) => (code::INTERNAL, w.clone()),
                };
                fail(id, c, &message)
            }
        })
    }

    /// The methods that belong to the node rather than to a chain.
    fn node_call(&self, method: &str, _params: &Value) -> Result<Value, Error> {
        match method {
            "info.getNodeVersion" => Ok(json!({
                "version": self.version,
                "vmVersions": self.chains.iter()
                    .map(|(k, v)| (k.to_ascii_uppercase(), Value::String(v.version())))
                    .collect::<serde_json::Map<_, _>>(),
            })),
            "info.getNetworkID" => Ok(json!({"networkID": self.network.to_string()})),
            "info.getBlockchainID" => Ok(json!({
                "blockchains": self.chains.keys().map(|k| k.to_ascii_uppercase()).collect::<Vec<_>>(),
            })),
            other => Err(Error::NoMethod(other.to_string())),
        }
    }

    /// Ask the archive, under the name this node was asked by.
    fn forward(&self, name: &str, request: &Value) -> Option<Value> {
        self.archive.as_ref().and_then(|a| proxy(a, name, request))
    }

    /// What `/` advertises: the chains this node serves, each under the name it
    /// answers to, and nothing it does not.
    fn directory(&self) -> Value {
        let chains: serde_json::Map<String, Value> =
            self.chains.keys().map(|k| (k.clone(), json!(format!("/v1/chain/{k}")))).collect();
        let mut endpoints = json!({"info": "/v1/info", "health": "/v1/health"});
        if let Some(primary) = &self.primary {
            endpoints["rpc"] = json!(format!("/v1/chain/{primary}"));
        }
        json!({
            "networkId": self.network,
            "ready": true,
            "mode": "light",
            "version": self.version,
            "archiveRpc": self.archive.as_ref().map(|a| a.url.clone()),
            "chains": chains,
            "endpoints": endpoints,
        })
    }

    fn health(&self) -> (u16, Value) {
        let sick: Vec<&str> = self
            .chains
            .iter()
            .filter(|(_, v)| v.health().is_err())
            .map(|(k, _)| k.as_str())
            .collect();
        if sick.is_empty() {
            (200, json!({"healthy": true}))
        } else {
            // Fail closed: an unhealthy chain answers with a status a load
            // balancer acts on, not a 200 carrying bad news in its body.
            (503, json!({"healthy": false, "failing": sick}))
        }
    }
}

fn fail(id: Option<Value>, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// A running server. Dropping it stops the listener and joins its thread, so a
/// server cannot outlive the thing that started it.
pub struct Serving {
    addr: SocketAddr,
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Serving {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

impl Drop for Serving {
    fn drop(&mut self) {
        self.stop();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// What a client asked for.
struct Request {
    method: String,
    path: String,
    body: Vec<u8>,
}

/// Read one request, answer it, and close.
///
/// One request per connection: keep-alive is a state machine whose failure mode
/// is a desynchronised stream, and a node's RPC is not the place to earn that.
/// The client is told `Connection: close` so it knows.
fn serve(node: &Rpc, stream: TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(IDLE_TIMEOUT))?;
    stream.set_write_timeout(Some(IDLE_TIMEOUT))?;

    let request = match read_request(&stream) {
        Ok(r) => r,
        Err(status) => return respond(&stream, status, &json!({"error": reason(status)})),
    };

    let target = route(&request.path);
    match target {
        Route::Directory => respond(&stream, 200, &node.directory()),
        Route::Health => {
            let (status, body) = node.health();
            respond(&stream, status, &body)
        }
        Route::ChainHealth(ref alias) => {
            if let Some(vm) = node.chain(alias) {
                let healthy = vm.health().is_ok();
                let status = if healthy { 200 } else { 503 };
                respond(
                    &stream,
                    status,
                    &json!({
                        "chain": alias,
                        "healthy": healthy,
                    }),
                )
            } else {
                respond(&stream, 404, &json!({"error": format!("no such chain: {alias}")}))
            }
        }
        Route::Node | Route::Chain(_) if request.method != "POST" => {
            respond(&stream, 405, &json!({"error": "use POST"}))
        }
        // A chain this node does not serve and no archive to ask is not a
        // chain here — and a brand's node is not Lux's C-chain.
        Route::Chain(ref alias) if node.chain(alias).is_none() && node.archive.is_none() => {
            respond(&stream, 404, &json!({"error": format!("no such chain: {alias}")}))
        }
        Route::Node | Route::Chain(_) => {
            let alias = match &target {
                Route::Chain(a) => Some(a.clone()),
                _ => None,
            };
            let Ok(parsed) = serde_json::from_slice::<Value>(&request.body) else {
                return respond(&stream, 200, &fail(None, code::PARSE, "not JSON"));
            };
            let answer = match &parsed {
                Value::Array(batch) if batch.len() > BATCH_MAX => {
                    fail(None, code::REQUEST, "batch too large")
                }
                Value::Array(batch) => {
                    let answers: Vec<Value> =
                        batch.iter().filter_map(|one| node.answer(alias.as_deref(), one)).collect();
                    // An all-notification batch is answered with nothing, per
                    // the specification.
                    if answers.is_empty() {
                        return respond_empty(&stream);
                    }
                    Value::Array(answers)
                }
                Value::Object(_) => match node.answer(alias.as_deref(), &parsed) {
                    Some(a) => a,
                    None => return respond_empty(&stream),
                },
                _ => fail(None, code::REQUEST, "a request is an object or an array"),
            };
            respond(&stream, 200, &answer)
        }
        Route::Unknown => respond(&stream, 404, &json!({"error": "no such endpoint"})),
    }
}

/// Where a path points.
#[derive(Debug, PartialEq, Eq)]
enum Route {
    Directory,
    Health,
    ChainHealth(String),
    Node,
    Chain(String),
    Unknown,
}

/// Resolve a path.
///
/// `/v1/chain/c` and `/v1/chain/C/rpc` are the same route: the running node
/// serves the first and the Go source spells it the second way, and a parser
/// that does not care which word is in the middle is one route rather than two.
fn route(path: &str) -> Route {
    let path = path.split('?').next().unwrap_or(path);
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    // A chain is named in letters, digits, `-` and `_`. Anything else — `..`,
    // an encoded slash — is refused here, because a name this node does not
    // serve is forwarded to the archive in a path of its own.
    if let ["v1", "bc" | "chain", alias, ..] = parts.as_slice() {
        let named = alias.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if !named || alias.len() > ALIAS_MAX {
            return Route::Unknown;
        }
    }
    match parts.as_slice() {
        [] => Route::Directory,
        ["v1", "health"] | ["health"] | ["healthz"] => Route::Health,
        // A chain is keyed by its ALIAS, not by the spelling someone typed:
        // /v1/chain/c and /v1/chain/C name one chain, so the alias is folded
        // to upper case here and every caller downstream compares one form.
        ["v1", "bc" | "chain", alias, "health"] => Route::ChainHealth(alias.to_ascii_uppercase()),
        ["v1", "info"] => Route::Node,
        ["v1", "bc" | "chain", alias] => Route::Chain(alias.to_ascii_uppercase()),
        ["v1", "bc" | "chain", alias, "rpc"] => Route::Chain(alias.to_ascii_uppercase()),
        _ => Route::Unknown,
    }
}

/// Read the head and the body, refusing anything oversized before allocating
/// for it.
fn read_request(stream: &TcpStream) -> Result<Request, u16> {
    let mut reader = BufReader::new(stream);
    let mut head = String::new();
    let mut line = String::new();

    // The request line.
    reader.read_line(&mut line).map_err(|_| 400u16)?;
    let mut start = line.split_whitespace();
    let method = start.next().unwrap_or_default().to_string();
    let path = start.next().unwrap_or_default().to_string();
    if method.is_empty() || path.is_empty() {
        return Err(400);
    }

    // The headers. Only the length matters here, but they are all read so the
    // body starts where it should.
    let mut length = 0usize;
    loop {
        line.clear();
        let n = reader.read_line(&mut line).map_err(|_| 400u16)?;
        if n == 0 || line.trim().is_empty() {
            break;
        }
        head.push_str(&line);
        if head.len() > HEAD_MAX {
            return Err(431);
        }
        if let Some(v) = line.split_once(':') {
            if v.0.trim().eq_ignore_ascii_case("content-length") {
                length = v.1.trim().parse().map_err(|_| 400u16)?;
            }
        }
    }

    // The cap is applied to the CLAIM, before a byte is reserved for it.
    if length > BODY_MAX {
        return Err(413);
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).map_err(|_| 400u16)?;

    Ok(Request { method, path, body })
}

fn reason(status: u16) -> &'static str {
    match status {
        400 => "malformed request",
        405 => "use POST",
        413 => "request too large",
        431 => "headers too large",
        503 => "unhealthy",
        _ => "error",
    }
}

fn respond(mut stream: &TcpStream, status: u16, body: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(body).unwrap_or_else(|_| b"{}".to_vec());
    write!(
        stream,
        "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reason(status),
        body.len()
    )?;
    stream.write_all(&body)?;
    stream.flush()
}

/// Ask the archive the same question, at `/v1/chain/<name>`.
///
/// `None` is every way this can fail — the archive is down, slow, or answered
/// something that is not JSON — and the caller answers with its own refusal.
fn proxy(archive: &Archive, name: &str, request: &Value) -> Option<Value> {
    let path = format!("/v1/chain/{}", name.to_ascii_lowercase());
    let body = serde_json::to_vec(request).ok()?;
    let host = archive.host.trim_start_matches('[').trim_end_matches(']');
    let mut stream = (host, archive.port)
        .to_socket_addrs()
        .ok()?
        .find_map(|a| TcpStream::connect_timeout(&a, Duration::from_secs(4)).ok())?;
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok()?;
    stream.set_write_timeout(Some(Duration::from_secs(10))).ok()?;

    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        archive.host,
        archive.port,
        body.len()
    )
    .ok()?;
    stream.write_all(&body).ok()?;
    stream.flush().ok()?;

    let mut reader = BufReader::new(stream.take(ARCHIVE_MAX as u64));
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut length = None;
    let mut chunked = false;
    loop {
        line.clear();
        if reader.read_line(&mut line).ok()? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                length = Some(v.trim().parse::<usize>().ok()?);
            } else if k.trim().eq_ignore_ascii_case("transfer-encoding")
                && v.trim().to_ascii_lowercase().contains("chunked")
            {
                chunked = true;
            }
        }
    }
    let mut body = Vec::new();
    if chunked {
        loop {
            line.clear();
            if reader.read_line(&mut line).ok()? == 0 {
                break;
            }
            let size = line.trim();
            if size.is_empty() {
                continue;
            }
            let size = usize::from_str_radix(size.split(';').next()?.trim(), 16).ok()?;
            if size == 0 {
                break;
            }
            // The cap is on the claim, before a byte is reserved for it.
            if body.len() + size > ARCHIVE_MAX {
                return None;
            }
            let start = body.len();
            body.resize(start + size, 0);
            reader.read_exact(&mut body[start..]).ok()?;
            line.clear();
            reader.read_line(&mut line).ok()?;
        }
    } else if let Some(n) = length {
        if n > ARCHIVE_MAX {
            return None;
        }
        body.resize(n, 0);
        reader.read_exact(&mut body).ok()?;
    } else {
        reader.read_to_end(&mut body).ok()?;
    }
    serde_json::from_slice(&body).ok()
}

fn respond_empty(mut stream: &TcpStream) -> std::io::Result<()> {
    write!(stream, "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_spellings_of_a_chain_route_are_one_route() {
        assert_eq!(route("/v1/chain/c"), Route::Chain("C".into()));
        assert_eq!(route("/v1/chain/C/rpc"), Route::Chain("C".into()));
        assert_eq!(route("/v1/chain/p"), Route::Chain("P".into()));
        assert_eq!(route("/"), Route::Directory);
        assert_eq!(route(""), Route::Directory);
        assert_eq!(route("/v1/health"), Route::Health);
        assert_eq!(route("/v1/info"), Route::Node);
    }

    /// The server is only a server if something can talk to it.
    ///
    /// Every other test here reads a path and decides a route, which is the
    /// half of this file that never touches a socket. The half that does was
    /// unexercised, and it was broken on this platform: the listener polls, so
    /// it is non-blocking, and on BSD and macOS what it accepts inherits that.
    /// The first read of the request then returned EAGAIN and the connection
    /// was answered with nothing at all.
    #[test]
    fn a_request_over_a_real_socket_is_answered() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpStream;

        let serving =
            Rpc::new(1337, "test").listen("127.0.0.1:0".parse().expect("addr")).expect("listen");
        let mut c = TcpStream::connect(serving.addr()).expect("connect");
        // Connect, then wait before sending. A client whose bytes are already
        // in the buffer when the server first reads is answered either way, so
        // writing immediately tests nothing: it is the read that arrives
        // BEFORE the request that tells a blocking socket from one that
        // returns EAGAIN and gives up.
        std::thread::sleep(Duration::from_millis(50));
        c.write_all(b"GET /v1/health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .expect("write");
        let mut status = String::new();
        BufReader::new(&c).read_line(&mut status).expect("read");
        // The status, not merely a status line. Every read error in
        // `read_request` becomes a 400, so a server that could not read the
        // request at all still answers something that starts with HTTP/1.1 —
        // and a test that accepted any status line would pass against a server
        // that never read a byte.
        let code = status.split_whitespace().nth(1).unwrap_or_default();
        assert_eq!(code, "200", "answered {status:?}");
    }

    #[test]
    fn a_query_string_does_not_change_the_route() {
        assert_eq!(route("/v1/chain/c?trace=1"), Route::Chain("C".into()));
    }

    #[test]
    fn a_path_that_is_not_a_route_is_not_a_route() {
        // Including the ones a prober reaches for.
        for p in ["/v1/chain/c/../../admin", "/admin", "/v1", "/v1/bc", "/v1/chain/c/extra"] {
            assert_eq!(route(p), Route::Unknown, "{p}");
        }
    }

    #[test]
    fn an_empty_segment_does_not_smuggle_a_chain() {
        // `//v1//bc//C//rpc` collapses to the same parts, which is fine — what
        // must not happen is an empty alias becoming a chain name.
        assert_eq!(route("/v1/chain//rpc"), Route::Chain("RPC".into()));
        assert_ne!(route("/v1/chain///"), Route::Chain(String::new()));
    }

    #[test]
    fn a_chain_name_is_a_name_and_not_a_path() {
        // A name this node does not serve is forwarded in a path of its own, so
        // nothing that walks a path, or spells one, is a name.
        for p in ["/v1/chain/../rpc", "/v1/chain/%2e%2e", "/v1/chain/c%2Frpc", "/v1/chain/a.b"] {
            assert_eq!(route(p), Route::Unknown, "{p}");
        }
        assert_eq!(route(&format!("/v1/chain/{}", "a".repeat(65))), Route::Unknown);
        assert_eq!(route("/v1/chain/hanzo/rpc"), Route::Chain("HANZO".into()));
        let id = "2q9e4r6Mu3U68nU1fYjgbR6JvwrRx36CohpAX5UQxse55x1Q5";
        assert_eq!(route(&format!("/v1/chain/{id}")), Route::Chain(id.to_ascii_uppercase()));
    }

    #[test]
    fn an_archive_is_an_http_host_and_nothing_else() {
        for ok in
            ["http://127.0.0.1:9630", "http://archive.lux:9650/", "http://[::1]:9650", "http://h"]
        {
            assert!(Archive::parse(ok).is_ok(), "{ok}");
        }
        for bad in [
            "https://archive:9650",
            "archive:9650",
            "http://archive:9650/v1/chain/c",
            "http://archive:port",
            "http://",
            "http://a\r\nX-Evil: 1:80",
        ] {
            assert!(Archive::parse(bad).is_err(), "{bad}");
        }
    }

    /// One request, answered with a fixed JSON body, over a real socket.
    fn call(at: SocketAddr, method: &str, path: &str, body: &str) -> (u16, Value) {
        use std::io::{BufRead, BufReader, Read, Write};
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

    #[test]
    fn a_chain_is_served_under_the_name_the_node_gives_it() {
        let serving = Rpc::new(7, "zetad/v9.9.9")
            .with("zeta", Arc::new(crate::evm::Evm::new(lux_evm::genesis::localnet())))
            .listen("127.0.0.1:0".parse().expect("addr"))
            .expect("listen");
        let ask = r#"{"jsonrpc":"2.0","id":1,"method":"web3_clientVersion"}"#;
        for path in ["/v1/chain/zeta", "/v1/chain/zeta/rpc", "/v1/chain/ZETA/rpc"] {
            let (code, answer) = call(serving.addr(), "POST", path, ask);
            assert_eq!(code, 200, "{path}");
            assert_eq!(answer["result"], "zetad/v9.9.9", "the client, not the library: {path}");
        }
        // The chain the EVM is on Lux is not this node's name for it.
        for path in ["/v1/chain/c", "/v1/chain/c/rpc", "/v1/chain/p"] {
            let (code, _) = call(serving.addr(), "POST", path, ask);
            assert_eq!(code, 404, "{path}");
        }
        let (_, dir) = call(serving.addr(), "GET", "/", "");
        assert_eq!(dir["chains"], json!({"zeta": "/v1/chain/zeta"}), "only what is served");
        assert_eq!(dir["endpoints"]["rpc"], "/v1/chain/zeta");
        assert_eq!(dir["networkId"], 7);
        assert_eq!(dir["mode"], "light");
        assert_eq!(dir["archiveRpc"], Value::Null);
        let (_, health) = call(serving.addr(), "GET", "/v1/chain/zeta/health", "");
        assert_eq!(health["healthy"], true);
    }

    #[test]
    fn what_this_node_does_not_serve_is_asked_of_the_archive_by_the_same_name() {
        use std::io::{BufRead, BufReader, Read, Write};
        // An archive that records the path it was asked at and answers once.
        let archive = TcpListener::bind("127.0.0.1:0").expect("archive");
        let at = archive.local_addr().expect("addr");
        let asked = std::thread::spawn(move || {
            let (s, _) = archive.accept().expect("asked");
            let mut r = BufReader::new(&s);
            let mut first = String::new();
            r.read_line(&mut first).expect("request line");
            let mut length = 0;
            loop {
                let mut h = String::new();
                r.read_line(&mut h).expect("header");
                if h.trim().is_empty() {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().expect("length");
                }
            }
            let mut body = vec![0u8; length];
            r.read_exact(&mut body).expect("body");
            let answer = br#"{"jsonrpc":"2.0","id":1,"result":"0x1e61"}"#;
            let mut w = &s;
            write!(w, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", answer.len()).expect("w");
            w.write_all(answer).expect("w");
            first
        });

        let serving = Rpc::new(7, "zetad/v9.9.9")
            .with_archive(&format!("http://{at}"))
            .expect("an archive")
            .listen("127.0.0.1:0".parse().expect("addr"))
            .expect("listen");
        let (code, answer) = call(
            serving.addr(),
            "POST",
            "/v1/chain/Zeta/rpc",
            r#"{"jsonrpc":"2.0","id":1,"method":"eth_chainId"}"#,
        );
        assert_eq!(code, 200);
        assert_eq!(answer["result"], "0x1e61");
        let first = asked.join().expect("the archive was asked");
        assert!(first.starts_with("POST /v1/chain/zeta "), "asked at {first:?}");
        let (_, dir) = call(serving.addr(), "GET", "/", "");
        assert_eq!(dir["archiveRpc"], format!("http://{at}"));
        assert_eq!(dir["chains"], json!({}), "an archive's chains are not advertised as ours");
    }
}
