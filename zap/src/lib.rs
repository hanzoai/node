//! ZAP-HTTP framing (github.com/zap-proto/http v0.3, the wire the CLI and the
//! cloud host speak) and the router envelope Dev sessions use.
//!
//! On the socket each ZAP-HTTP message is `[u32 BE length][frame]`; router
//! envelopes are `[u32 LE length][frame]`. Both frames start with the same
//! 16-byte header (`ZAP\0`, version, flags, root offset, size); the high byte of
//! `flags` is the frame type: 0x01..0x05 are ZAP-HTTP, 200 is a router envelope.

use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAGIC: &[u8; 4] = b"ZAP\0";
pub const HEADER: usize = 16;
const SLOTS: usize = 48;
pub const MAX_FRAME: u32 = 64 << 20;

pub const FRAME_REQUEST: u16 = 0x01;
pub const FRAME_RESPONSE: u16 = 0x02;
pub const FRAME_HEAD: u16 = 0x03;
pub const FRAME_END: u16 = 0x05;
pub const FRAME_ROUTER: u16 = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    /// The path without its query.
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or("")
    }
    pub fn bearer(&self) -> Option<&str> {
        self.header("authorization")?.strip_prefix("Bearer ")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json(status: u16, v: &serde_json::Value) -> Response {
        Response {
            status,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: serde_json::to_vec(v).unwrap_or_default(),
        }
    }
    pub fn error(status: u16, why: &str) -> Response {
        Response::json(status, &serde_json::json!({ "error": why }))
    }
}

pub fn frame_type(f: &[u8]) -> Option<u16> {
    if f.len() < HEADER || &f[0..4] != MAGIC {
        return None;
    }
    Some(u16::from_le_bytes([f[6], f[7]]) >> 8)
}

pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut p = [0u8; 4];
    match r.read_exact(&mut p).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let n = u32::from_be_bytes(p);
    if n == 0 || n > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("frame of {n} bytes")));
    }
    let mut buf = vec![0u8; n as usize];
    r.read_exact(&mut buf).await?;
    Ok(Some(buf))
}

pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, f: &[u8]) -> io::Result<()> {
    w.write_all(&(f.len() as u32).to_be_bytes()).await?;
    w.write_all(f).await?;
    w.flush().await
}

fn bad(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why.to_string())
}

fn valid(f: &[u8]) -> io::Result<usize> {
    if f.len() < HEADER || &f[0..4] != MAGIC {
        return Err(bad("not a ZAP frame"));
    }
    let version = u16::from_le_bytes([f[4], f[5]]);
    if version != 1 && version != 2 {
        return Err(bad("unknown ZAP version"));
    }
    let root = u32::from_le_bytes([f[8], f[9], f[10], f[11]]) as usize;
    let size = u32::from_le_bytes([f[12], f[13], f[14], f[15]]) as usize;
    if size < HEADER || size > f.len() || root < HEADER || root + SLOTS > size {
        return Err(bad("frame geometry"));
    }
    Ok(size)
}

fn read_var(f: &[u8], size: usize, slot: usize) -> io::Result<&[u8]> {
    if slot + 8 > size {
        return Err(bad("slot past frame"));
    }
    let rel = u32::from_le_bytes(f[slot..slot + 4].try_into().unwrap()) as usize;
    if rel == 0 {
        return Ok(&[]);
    }
    let len = u32::from_le_bytes(f[slot + 4..slot + 8].try_into().unwrap()) as usize;
    let start = slot.checked_add(rel).ok_or_else(|| bad("overflow"))?;
    let end = start.checked_add(len).ok_or_else(|| bad("overflow"))?;
    if start < HEADER || end > size {
        return Err(bad("field past frame"));
    }
    Ok(&f[start..end])
}

fn put_var(f: &mut Vec<u8>, slot: usize, data: &[u8]) {
    if data.is_empty() {
        return;
    }
    let start = f.len();
    f.extend_from_slice(data);
    patch(f, slot, start);
}

fn patch(f: &mut [u8], slot: usize, start: usize) {
    let rel = (start - slot) as u32;
    let len = (f.len() - start) as u32;
    f[slot..slot + 4].copy_from_slice(&rel.to_le_bytes());
    f[slot + 4..slot + 8].copy_from_slice(&len.to_le_bytes());
}

fn headers_in(raw: &[u8]) -> io::Result<Vec<(String, String)>> {
    if raw.is_empty() {
        return Ok(vec![]);
    }
    let u32at = |at: usize| -> io::Result<usize> {
        raw.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize).ok_or_else(|| bad("headers"))
    };
    let count = u32at(0)?;
    let mut at = 4;
    let mut out = Vec::new();
    for _ in 0..count.min(1024) {
        let kl = u32at(at)?;
        let k = raw.get(at + 4..at + 4 + kl).ok_or_else(|| bad("headers"))?;
        at += 4 + kl;
        let vl = u32at(at)?;
        let v = raw.get(at + 4..at + 4 + vl).ok_or_else(|| bad("headers"))?;
        at += 4 + vl;
        out.push((String::from_utf8_lossy(k).into_owned(), String::from_utf8_lossy(v).into_owned()));
    }
    Ok(out)
}

fn headers_out(f: &mut Vec<u8>, slot: usize, headers: &[(String, String)]) {
    if headers.is_empty() {
        return;
    }
    let start = f.len();
    f.extend_from_slice(&(headers.len() as u32).to_le_bytes());
    for (k, v) in headers {
        f.extend_from_slice(&(k.len() as u32).to_le_bytes());
        f.extend_from_slice(k.as_bytes());
        f.extend_from_slice(&(v.len() as u32).to_le_bytes());
        f.extend_from_slice(v.as_bytes());
    }
    patch(f, slot, start);
}

fn begin(kind: u16) -> Vec<u8> {
    let mut f = Vec::with_capacity(256);
    f.extend_from_slice(MAGIC);
    f.extend_from_slice(&1u16.to_le_bytes());
    f.extend_from_slice(&(kind << 8).to_le_bytes());
    f.extend_from_slice(&(HEADER as u32).to_le_bytes());
    f.extend_from_slice(&0u32.to_le_bytes());
    f.resize(HEADER + SLOTS, 0);
    f
}

fn finish(mut f: Vec<u8>) -> Vec<u8> {
    let size = f.len() as u32;
    f[12..16].copy_from_slice(&size.to_le_bytes());
    f
}

pub fn encode_request(r: &Request) -> Vec<u8> {
    let mut f = begin(FRAME_REQUEST);
    put_var(&mut f, HEADER, r.method.as_bytes());
    put_var(&mut f, HEADER + 8, r.target.as_bytes());
    put_var(&mut f, HEADER + 16, b"HTTP/1.1");
    headers_out(&mut f, HEADER + 24, &r.headers);
    put_var(&mut f, HEADER + 32, &r.body);
    finish(f)
}

pub fn decode_request(f: &[u8]) -> io::Result<Request> {
    let size = valid(f)?;
    if frame_type(f) != Some(FRAME_REQUEST) {
        return Err(bad("not a request frame"));
    }
    let root = HEADER;
    let text = |slot| read_var(f, size, root + slot).map(|b| String::from_utf8_lossy(b).into_owned());
    Ok(Request {
        method: text(0)?,
        target: text(8)?,
        headers: headers_in(read_var(f, size, root + 24)?)?,
        body: read_var(f, size, root + 32)?.to_vec(),
    })
}

pub fn encode_response(r: &Response) -> Vec<u8> {
    let mut f = begin(FRAME_RESPONSE);
    f[HEADER..HEADER + 2].copy_from_slice(&r.status.to_le_bytes());
    put_var(&mut f, HEADER + 16, b"ZAP-HTTP/1.0");
    headers_out(&mut f, HEADER + 24, &r.headers);
    put_var(&mut f, HEADER + 32, &r.body);
    finish(f)
}

pub fn decode_response(f: &[u8]) -> io::Result<Response> {
    let size = valid(f)?;
    if frame_type(f) != Some(FRAME_RESPONSE) {
        return Err(bad("not a single-frame response"));
    }
    let root = HEADER;
    Ok(Response {
        status: u16::from_le_bytes([f[root], f[root + 1]]),
        headers: headers_in(read_var(f, size, root + 24)?)?,
        body: read_var(f, size, root + 32)?.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_round_trips() {
        let r = Request {
            method: "POST".into(),
            target: "/v1/dev/session".into(),
            headers: vec![("Authorization".into(), "Bearer x".into())],
            body: b"{}".to_vec(),
        };
        let f = encode_request(&r);
        assert_eq!(frame_type(&f), Some(FRAME_REQUEST));
        assert_eq!(decode_request(&f).unwrap(), r);
    }

    #[test]
    fn a_response_round_trips_and_empty_fields_stay_null() {
        let r = Response { status: 204, headers: vec![], body: vec![] };
        assert_eq!(decode_response(&encode_response(&r)).unwrap(), r);
        let r = Response::json(200, &serde_json::json!({"a": 1}));
        assert_eq!(decode_response(&encode_response(&r)).unwrap(), r);
    }

    #[test]
    fn a_truncated_or_forged_frame_is_refused() {
        let f = encode_request(&Request { method: "GET".into(), target: "/".into(), headers: vec![], body: vec![] });
        assert!(decode_request(&f[..20]).is_err());
        let mut g = f.clone();
        g[0] = b'X';
        assert!(decode_request(&g).is_err());
        let mut h = f.clone();
        h[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode_request(&h).is_err());
    }
}

// ---- the router envelope (Dev sessions) --------------------------------------

/// The `cap` of a router request envelope: for a Dev session, the caller's IAM
/// bearer. Slots are `{relOffset, length}` measured from the slot itself.
pub fn router_cap(f: &[u8]) -> io::Result<&[u8]> {
    let size = valid_router(f)?;
    let root = u32::from_le_bytes(f[8..12].try_into().unwrap()) as usize;
    read_var(f, size, root + 12)
}

fn valid_router(f: &[u8]) -> io::Result<usize> {
    valid_router_object(f, 28)
}

fn valid_router_object(f: &[u8], object: usize) -> io::Result<usize> {
    if f.len() < HEADER || &f[0..4] != MAGIC {
        return Err(bad("not a ZAP frame"));
    }
    if frame_type(f) != Some(FRAME_ROUTER) {
        return Err(bad("not a router envelope"));
    }
    let root = u32::from_le_bytes(f[8..12].try_into().unwrap()) as usize;
    let size = u32::from_le_bytes(f[12..16].try_into().unwrap()) as usize;
    if size < HEADER || size > f.len() || root < HEADER || root + object > size {
        return Err(bad("envelope geometry"));
    }
    Ok(size)
}

/// A router request envelope, byte for byte what `@zap-proto/zap` and the Go router
/// build: method, promise and target, then the `cap` and `payload` slots.
pub fn router_request(method: u32, promise: u32, cap: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(HEADER + 28 + cap.len() + payload.len());
    f.extend_from_slice(MAGIC);
    f.extend_from_slice(&2u16.to_le_bytes());
    f.extend_from_slice(&(FRAME_ROUTER << 8).to_le_bytes());
    f.extend_from_slice(&(HEADER as u32).to_le_bytes());
    f.extend_from_slice(&0u32.to_le_bytes());
    f.extend_from_slice(&method.to_le_bytes());
    f.extend_from_slice(&promise.to_le_bytes());
    f.extend_from_slice(&0u32.to_le_bytes());
    f.resize(HEADER + 28, 0);
    put_var(&mut f, HEADER + 12, cap);
    put_var(&mut f, HEADER + 20, payload);
    finish(f)
}

/// `(method, promise, cap, payload)` of a router request envelope.
pub fn router_request_parts(f: &[u8]) -> io::Result<(u32, u32, &[u8], &[u8])> {
    let size = valid_router(f)?;
    let root = u32::from_le_bytes(f[8..12].try_into().unwrap()) as usize;
    let word = |at: usize| u32::from_le_bytes(f[root + at..root + at + 4].try_into().unwrap());
    Ok((word(0), word(4), read_var(f, size, root + 12)?, read_var(f, size, root + 20)?))
}

/// A router response envelope: status, the promise it answers, and a body.
pub fn router_response(status: u32, promise: u32, body: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(HEADER + 20 + body.len());
    f.extend_from_slice(MAGIC);
    f.extend_from_slice(&2u16.to_le_bytes());
    f.extend_from_slice(&(FRAME_ROUTER << 8).to_le_bytes());
    f.extend_from_slice(&(HEADER as u32).to_le_bytes());
    f.extend_from_slice(&0u32.to_le_bytes());
    f.extend_from_slice(&status.to_le_bytes());
    f.extend_from_slice(&promise.to_le_bytes());
    f.resize(HEADER + 20, 0);
    put_var(&mut f, HEADER + 12, body);
    finish(f)
}

/// `(status, promise, body)` of a router response envelope.
pub fn router_response_parts(f: &[u8]) -> io::Result<(u32, u32, &[u8])> {
    let size = valid_router_object(f, 20)?;
    let root = u32::from_le_bytes(f[8..12].try_into().unwrap()) as usize;
    let word = |at: usize| u32::from_le_bytes(f[root + at..root + at + 4].try_into().unwrap());
    Ok((word(0), word(4), read_var(f, size, root + 12)?))
}

/// An unauthorized router response (`status` 401), for a session refused before
/// it reaches a worker. promiseID 0.
pub fn router_refusal(status: u32) -> Vec<u8> {
    let mut f = Vec::with_capacity(HEADER + 20);
    f.extend_from_slice(MAGIC);
    f.extend_from_slice(&2u16.to_le_bytes());
    f.extend_from_slice(&(FRAME_ROUTER << 8).to_le_bytes());
    f.extend_from_slice(&(HEADER as u32).to_le_bytes());
    f.extend_from_slice(&((HEADER + 20) as u32).to_le_bytes());
    f.extend_from_slice(&status.to_le_bytes());
    f.extend_from_slice(&[0u8; 16]);
    let mut out = ((f.len()) as u32).to_le_bytes().to_vec();
    out.extend(f);
    out
}

#[cfg(test)]
mod router_tests {
    use super::*;

    /// Built by `@zap-proto/zap` `buildRequest` with cap "tok.en.sig".
    const FROM_TS: &str = "5a415000020000c8100000003b000000000000000700000000000000100000000a0000001200000005000000746f6b2e656e2e73696768656c6c6f";

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    /// Built by the Go router (`rpc.BuildRequest` / `rpc.BuildResponse`).
    const REQ_FROM_GO: &str = "5a415000020000c810000000340000000100ffff090000000000000010000000060000000e00000002000000746f6b2e656e6869";
    const RESP_FROM_GO: &str = "5a415000020000c81000000029000000c800000009000000000000000800000005000000626f647921";

    #[test]
    fn envelopes_match_the_go_routers_bytes() {
        assert_eq!(router_request(0xFFFF0001, 9, b"tok.en", b"hi"), unhex(REQ_FROM_GO));
        assert_eq!(router_response(200, 9, b"body!"), unhex(RESP_FROM_GO));
        assert_eq!(router_request_parts(&unhex(REQ_FROM_GO)).unwrap(), (0xFFFF0001, 9, &b"tok.en"[..], &b"hi"[..]));
        assert_eq!(router_response_parts(&unhex(RESP_FROM_GO)).unwrap(), (200, 9, &b"body!"[..]));
        assert_eq!(router_response_parts(&router_response(409, 1, b"")).unwrap(), (409, 1, &b""[..]));
    }

    #[test]
    fn the_cap_of_a_typescript_envelope_is_read() {
        let f = unhex(FROM_TS);
        assert_eq!(frame_type(&f), Some(FRAME_ROUTER));
        assert_eq!(router_cap(&f).unwrap(), b"tok.en.sig");
        assert!(router_cap(&f[..30]).is_err());
    }
}

/// A streamed response's head frame: status and headers, no body.
pub fn decode_head(f: &[u8]) -> io::Result<Response> {
    let size = valid(f)?;
    if frame_type(f) != Some(FRAME_HEAD) {
        return Err(bad("not a response head"));
    }
    Ok(Response {
        status: u16::from_le_bytes([f[HEADER], f[HEADER + 1]]),
        headers: headers_in(read_var(f, size, HEADER + 24)?)?,
        body: vec![],
    })
}

/// A data frame's chunk: one bytes slot at the root.
pub fn data_chunk(f: &[u8]) -> io::Result<&[u8]> {
    let size = valid(f)?;
    read_var(f, size, HEADER)
}
