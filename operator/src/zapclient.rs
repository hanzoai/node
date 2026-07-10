//! Rust ZAP client for the canonical Lux KMS server.
//!
//! Wire-compatible with [`luxfi/kms/pkg/zapserver`] — only `OpSecretGet`
//! (`0x0040`) is implemented in this foundation drop. Put / List / Delete
//! land alongside the operator's first KMSSecret cutover.
//!
//! # Wire format (verbatim with luxfi/zap)
//!
//! ## Outer transport frame
//!
//! `[u32 length LE] || [ZAP message]`
//!
//! ## ZAP message
//!
//! ```text
//! ┌─ 16-byte header ──────────────────────────────┐
//! │ "ZAP\0"  | u16 ver=1 | u16 flags | u32 root | u32 size │
//! └─ data segment (variable) ─────────────────────┘
//! ```
//!
//! ## Handshake (exchanged once per TCP connection, both directions)
//!
//! A ZAP message whose root Object has data size 64. NodeID bytes live at
//! offsets `0..60` and the NodeID length lives at offset `60` as a `u32 LE`.
//! See `~/work/lux/zap/node.go::handleConn`.
//!
//! ## Call request (after handshake)
//!
//! `[u32 reqID LE] || [u32 reqFlag LE = 1 (request)] || [ZAP message]`
//!
//! The inner ZAP message wraps the opcode + JSON body inside the root
//! Object at field offset 0 as a `Bytes` field:
//!
//! ```text
//! payload = [u16 opcode LE] || [JSON body]
//! root.Bytes(0) = payload
//! flags = opcode << 8
//! ```
//!
//! ## Call response
//!
//! `[u32 reqID LE] || [u32 reqFlag LE = 2 (response)] || [ZAP message]`
//!
//! The response message's root carries a single `Bytes(0)` field containing
//! `[u8 status] || [JSON body]`. Status byte values:
//!
//! | byte | meaning   | sentinel       |
//! |------|-----------|----------------|
//! | 0x00 | OK        | (return body)  |
//! | 0x01 | NotFound  | `ZapNotFound`  |
//! | 0x02 | Error     | `ZapError`     |
//! | 0x03 | Forbidden | `ZapForbidden` |

use base64::Engine;
use byteorder::{ByteOrder, LittleEndian};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{debug, info, warn};

// ── Wire constants ──────────────────────────────────────────────────────────

/// `b"ZAP\0"` magic bytes at the start of every ZAP message.
const MAGIC: &[u8; 4] = b"ZAP\0";
const VERSION: u16 = 1;
const HEADER_SIZE: usize = 16;
const ALIGNMENT: usize = 8;

const REQ_FLAG_REQUEST: u32 = 1;
const REQ_FLAG_RESPONSE: u32 = 2;

const HANDSHAKE_OBJECT_SIZE: usize = 64;
const HANDSHAKE_ID_OFFSET: usize = 0;
const HANDSHAKE_ID_LEN_OFFSET: usize = 60;
const HANDSHAKE_ID_MAX: usize = 60;

/// 10 MiB ceiling on a single inbound message — matches `luxfi/zap/node.go`.
const MAX_FRAME_SIZE: u32 = 10 * 1024 * 1024;

/// Status bytes inside the response payload.
const STATUS_OK: u8 = 0x00;
const STATUS_NOT_FOUND: u8 = 0x01;
const STATUS_ERROR: u8 = 0x02;
const STATUS_FORBIDDEN: u8 = 0x03;

// ── Opcodes (canonical luxfi/kms wire) ──────────────────────────────────────

/// `OpSecretGet` — `{path,name,env}` → `{value: base64}`.
pub const OP_SECRET_GET: u16 = 0x0040;
#[allow(dead_code)]
pub const OP_SECRET_PUT: u16 = 0x0041;
#[allow(dead_code)]
pub const OP_SECRET_LIST: u16 = 0x0042;
#[allow(dead_code)]
pub const OP_SECRET_DELETE: u16 = 0x0043;

// ── Dial-backoff tuning (mirrors the canonical KMS client) ───────────────────

/// First retry delay when dialing the ZAP peer.
const DIAL_INITIAL_DELAY: Duration = Duration::from_millis(100);
/// Cap on the exponential backoff between retries.
const DIAL_MAX_DELAY: Duration = Duration::from_secs(5);
/// Total dial attempts before giving up so the caller can fall back to HTTP.
const DIAL_MAX_ATTEMPTS: u32 = 6;

/// Default per-call timeout. Server-side handlers are sub-millisecond; we
/// budget for the round-trip plus generous slack.
const CALL_TIMEOUT: Duration = Duration::from_secs(2);

// ── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum ZapError {
    #[error("zap: connect to {addr} failed: {source}")]
    Connect {
        addr: String,
        #[source]
        source: io::Error,
    },

    #[error("zap: dial {addr} exhausted ({attempts} attempts): {last}")]
    DialExhausted {
        addr: String,
        attempts: u32,
        last: Box<ZapError>,
    },

    #[error("zap: i/o: {0}")]
    Io(#[from] io::Error),

    #[error("zap: invalid wire format: {0}")]
    InvalidWire(&'static str),

    #[error("zap: invalid magic")]
    InvalidMagic,

    #[error("zap: unsupported version {0}")]
    UnsupportedVersion(u16),

    #[error("zap: response too short ({0} bytes)")]
    ShortResponse(usize),

    #[error("zap: frame size {0} exceeds 10MiB")]
    FrameTooLarge(u32),

    #[error("zap: server reported error: {0}")]
    ZapServerError(String),

    #[error("zap: secret not found")]
    ZapNotFound,

    #[error("zap: forbidden (admin role required)")]
    ZapForbidden,

    #[error("zap: timed out waiting for response")]
    Timeout,

    #[error("zap: peer mismatch (expected NodeID exchange) — server returned no peer ID")]
    HandshakePeerEmpty,

    #[error("zap: decode response: {0}")]
    DecodeResponse(String),
}

pub type Result<T> = std::result::Result<T, ZapError>;

// ── Builder / Parser primitives ─────────────────────────────────────────────
//
// These mirror the Go luxfi/zap.Builder and zap.Message just deeply enough to
// build the messages we send and parse the messages we receive. We don't try
// to be a general-purpose ZAP codec — only what `OpSecretGet` needs.

/// Build a ZAP message whose root Object has the given dataSize and a single
/// `Bytes(0)` field carrying `payload`. `flags` is written into the header
/// (used to encode opcode << 8 — matches Go `Builder.FinishWithFlags`).
fn build_object_with_bytes_field(payload: &[u8], object_size: usize, flags: u16) -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::with_capacity(HEADER_SIZE + object_size + payload.len() + 16);

    // 16-byte header — magic, version, flags. Root offset and size are
    // back-patched at the end.
    buf.extend_from_slice(MAGIC);
    buf.extend_from_slice(&VERSION.to_le_bytes());
    buf.extend_from_slice(&flags.to_le_bytes());
    // Root offset placeholder (4 bytes).
    buf.extend_from_slice(&[0u8; 4]);
    // Size placeholder (4 bytes).
    buf.extend_from_slice(&[0u8; 4]);
    debug_assert_eq!(buf.len(), HEADER_SIZE);

    // Align Object start to 8 bytes (Go Builder does the same).
    align_to(&mut buf, ALIGNMENT);
    let object_start = buf.len();

    // Reserve fixed-size object section (zero-filled). The Bytes-field at
    // offset 0 occupies the first 8 bytes (relOffset u32 LE + length u32 LE).
    let fixed_section_size = object_size.max(8);
    buf.resize(object_start + fixed_section_size, 0);

    // Write length at offset+4 (Bytes field layout: relOffset || length).
    let len_pos = object_start + 4;
    LittleEndian::write_u32(&mut buf[len_pos..len_pos + 4], payload.len() as u32);

    // Append payload after the fixed section. Patch the relOffset to point
    // at it (relative to the field-position itself, which is `object_start`
    // because field is at offset 0 within the object).
    let data_pos = buf.len();
    let rel_offset: i32 = (data_pos as i32) - (object_start as i32);
    LittleEndian::write_u32(&mut buf[object_start..object_start + 4], rel_offset as u32);
    buf.extend_from_slice(payload);

    // Patch root offset + total size in the header.
    let total_size = buf.len() as u32;
    LittleEndian::write_u32(&mut buf[8..12], object_start as u32);
    LittleEndian::write_u32(&mut buf[12..16], total_size);

    buf
}

/// Build a ZAP message whose root Object has data size 64, with NodeID bytes
/// laid out at `[0..60]` and the NodeID length at `[60..64]`. Used for the
/// initial handshake exchange — see `~/work/lux/zap/node.go::handleConn`.
fn build_handshake(node_id: &[u8]) -> Vec<u8> {
    let id_len = node_id.len().min(HANDSHAKE_ID_MAX);
    let mut buf: Vec<u8> = Vec::with_capacity(HEADER_SIZE + HANDSHAKE_OBJECT_SIZE + 16);

    buf.extend_from_slice(MAGIC);
    buf.extend_from_slice(&VERSION.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes()); // flags = 0
    buf.extend_from_slice(&[0u8; 4]); // root offset placeholder
    buf.extend_from_slice(&[0u8; 4]); // size placeholder
    debug_assert_eq!(buf.len(), HEADER_SIZE);

    align_to(&mut buf, ALIGNMENT);
    let object_start = buf.len();

    // Reserve the 64-byte fixed object section (zero-initialised).
    buf.resize(object_start + HANDSHAKE_OBJECT_SIZE, 0);

    // NodeID bytes at offsets [0..id_len].
    buf[object_start + HANDSHAKE_ID_OFFSET..object_start + HANDSHAKE_ID_OFFSET + id_len]
        .copy_from_slice(&node_id[..id_len]);
    // NodeID length at offset 60 (u32 LE).
    let len_pos = object_start + HANDSHAKE_ID_LEN_OFFSET;
    LittleEndian::write_u32(&mut buf[len_pos..len_pos + 4], id_len as u32);

    // Patch root offset + total size.
    let total_size = buf.len() as u32;
    LittleEndian::write_u32(&mut buf[8..12], object_start as u32);
    LittleEndian::write_u32(&mut buf[12..16], total_size);

    buf
}

fn align_to(buf: &mut Vec<u8>, alignment: usize) {
    let pad = (alignment - (buf.len() % alignment)) % alignment;
    if pad > 0 {
        buf.resize(buf.len() + pad, 0);
    }
}

/// Validate a ZAP message header (4B magic, 2B version, 4B size). Returns the
/// declared `(rootOffset, size)` so the caller can read the root Object.
fn parse_header(data: &[u8]) -> Result<(u32, u32)> {
    if data.len() < HEADER_SIZE {
        return Err(ZapError::InvalidWire("buffer < 16 bytes"));
    }
    if &data[0..4] != MAGIC {
        return Err(ZapError::InvalidMagic);
    }
    let version = LittleEndian::read_u16(&data[4..6]);
    if version != VERSION {
        return Err(ZapError::UnsupportedVersion(version));
    }
    let root_offset = LittleEndian::read_u32(&data[8..12]);
    let size = LittleEndian::read_u32(&data[12..16]);
    if (size as usize) > data.len() {
        return Err(ZapError::InvalidWire("declared size > buffer"));
    }
    Ok((root_offset, size))
}

/// Read the `Bytes(0)` field from a ZAP message's root Object. Used to
/// extract the response payload (status || JSON) from a server reply.
fn read_root_bytes_field(data: &[u8]) -> Result<&[u8]> {
    let (root_offset, _size) = parse_header(data)?;
    let root = root_offset as usize;
    if root + 8 > data.len() {
        return Err(ZapError::InvalidWire("root + bytes-field header OOB"));
    }
    let rel_offset = LittleEndian::read_i32(&data[root..root + 4]);
    let length = LittleEndian::read_u32(&data[root + 4..root + 8]) as usize;
    if rel_offset == 0 || length == 0 {
        return Ok(&[]);
    }
    let abs_pos = (root as i64) + (rel_offset as i64);
    if abs_pos < 0 {
        return Err(ZapError::InvalidWire("negative absolute offset"));
    }
    let abs_pos = abs_pos as usize;
    if abs_pos + length > data.len() {
        return Err(ZapError::InvalidWire("bytes payload OOB"));
    }
    Ok(&data[abs_pos..abs_pos + length])
}

/// Read NodeID bytes from a handshake-shaped root Object (size 64).
fn read_handshake_node_id(data: &[u8]) -> Result<Vec<u8>> {
    let (root_offset, _size) = parse_header(data)?;
    let root = root_offset as usize;
    if root + HANDSHAKE_OBJECT_SIZE > data.len() {
        return Err(ZapError::InvalidWire("handshake object OOB"));
    }
    let len_pos = root + HANDSHAKE_ID_LEN_OFFSET;
    let id_len = LittleEndian::read_u32(&data[len_pos..len_pos + 4]) as usize;
    if id_len == 0 || id_len > HANDSHAKE_ID_MAX {
        return Err(ZapError::HandshakePeerEmpty);
    }
    Ok(data[root + HANDSHAKE_ID_OFFSET..root + HANDSHAKE_ID_OFFSET + id_len].to_vec())
}

// ── Frame I/O over a TCP stream ─────────────────────────────────────────────

async fn write_frame(stream: &mut TcpStream, data: &[u8]) -> Result<()> {
    let mut len_buf = [0u8; 4];
    LittleEndian::write_u32(&mut len_buf, data.len() as u32);
    stream.write_all(&len_buf).await?;
    stream.write_all(data).await?;
    stream.flush().await?;
    Ok(())
}

async fn read_frame(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let length = LittleEndian::read_u32(&len_buf);
    if length > MAX_FRAME_SIZE {
        return Err(ZapError::FrameTooLarge(length));
    }
    let mut buf = vec![0u8; length as usize];
    stream.read_exact(&mut buf).await?;
    Ok(buf)
}

// ── Public client ───────────────────────────────────────────────────────────

/// A ZAP client connection to a single KMS server.
///
/// One client owns one TCP connection. Reconnection is on-demand: if a
/// `get_secret` call fails the caller drops the client and creates a new one.
/// The kms_controller layer caches one client per host:port.
///
/// **Concurrency**: this struct is `!Sync` — concurrent calls on the same
/// client are not supported. The `Call` correlator is single-flight per
/// instance: each `get_secret` issues one request and awaits one response
/// before returning. Wrap in `Mutex<ZapClient>` if you need shared use.
pub struct ZapClient {
    addr: String,
    node_id: Vec<u8>,
    stream: TcpStream,
    /// Monotonic request ID — wraps after 2^32 calls. The server echoes it
    /// back so we know which response is ours; with single-flight callers
    /// the wrap is a non-issue.
    next_req_id: AtomicU32,
}

impl ZapClient {
    /// Dial `addr` (host:port) and complete the ZAP handshake.
    ///
    /// Uses the same bounded-backoff schedule as `the canonical KMS client`:
    /// 100ms → 200ms → 400ms → 800ms → 1.6s → 3.2s (capped at 5s), 6 attempts
    /// total. After exhaustion the caller should fall back to HTTP.
    ///
    /// `cluster_name` is hashed into the ephemeral NodeID so the server-side
    /// ACL can recognise this operator instance. Pass an empty string and we
    /// fall back to a deterministic default.
    pub async fn connect(addr: &str, cluster_name: &str) -> Result<Self> {
        let node_id = derive_node_id(cluster_name);
        let mut delay = DIAL_INITIAL_DELAY;
        let mut last_err: Option<ZapError> = None;
        for attempt in 1..=DIAL_MAX_ATTEMPTS {
            match Self::connect_once(addr, &node_id).await {
                Ok(client) => {
                    if attempt > 1 {
                        info!(
                            target: "operator::zap",
                            addr = addr,
                            attempt = attempt,
                            "ZAP peer reconnected"
                        );
                    }
                    return Ok(client);
                }
                Err(e) => {
                    debug!(
                        target: "operator::zap",
                        addr = addr,
                        attempt = attempt,
                        max = DIAL_MAX_ATTEMPTS,
                        err = %e,
                        "ZAP dial failed"
                    );
                    last_err = Some(e);
                    if attempt == DIAL_MAX_ATTEMPTS {
                        break;
                    }
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(DIAL_MAX_DELAY);
                }
            }
        }
        Err(ZapError::DialExhausted {
            addr: addr.to_string(),
            attempts: DIAL_MAX_ATTEMPTS,
            last: Box::new(last_err.unwrap_or(ZapError::HandshakePeerEmpty)),
        })
    }

    async fn connect_once(addr: &str, node_id: &[u8]) -> Result<Self> {
        let mut stream = TcpStream::connect(addr)
            .await
            .map_err(|e| ZapError::Connect {
                addr: addr.to_string(),
                source: e,
            })?;
        // Cork small writes — the handshake is one frame, the call is another.
        let _ = stream.set_nodelay(true);

        // Send our handshake.
        let our_handshake = build_handshake(node_id);
        write_frame(&mut stream, &our_handshake).await?;

        // Read the server's handshake.
        let server_handshake = read_frame(&mut stream).await?;
        let peer_id = read_handshake_node_id(&server_handshake)?;
        debug!(
            target: "operator::zap",
            addr = addr,
            peer_id = %String::from_utf8_lossy(&peer_id),
            "ZAP handshake complete"
        );

        Ok(ZapClient {
            addr: addr.to_string(),
            node_id: node_id.to_vec(),
            stream,
            next_req_id: AtomicU32::new(0),
        })
    }

    /// Fetch a single secret value via `OpSecretGet`. Returns the decoded
    /// plaintext (server side base64-encodes the value).
    pub async fn get_secret(&mut self, path: &str, name: &str, env: &str) -> Result<String> {
        let body = serde_json::json!({
            "path": path,
            "name": name,
            "env": env,
        });
        let body_bytes = serde_json::to_vec(&body)
            .map_err(|e| ZapError::DecodeResponse(format!("encode get body: {e}")))?;
        let payload = self.call(OP_SECRET_GET, &body_bytes).await?;

        #[derive(Deserialize)]
        struct GetResp {
            #[serde(default)]
            value: String,
        }
        let resp: GetResp = serde_json::from_slice(&payload)
            .map_err(|e| ZapError::DecodeResponse(format!("decode get response: {e}")))?;
        let pt = base64::engine::general_purpose::STANDARD
            .decode(resp.value.as_bytes())
            .map_err(|e| ZapError::DecodeResponse(format!("base64 decode: {e}")))?;
        let s =
            String::from_utf8(pt).map_err(|e| ZapError::DecodeResponse(format!("utf-8: {e}")))?;
        Ok(s)
    }

    /// Send `[opcode LE || body]` wrapped in a Call request, await the
    /// matching response, return the JSON body (status byte already stripped
    /// + classified into `ZapNotFound` / `ZapForbidden` / `ZapServerError`).
    async fn call(&mut self, op: u16, body: &[u8]) -> Result<Vec<u8>> {
        // Build the inner ZAP message: root Object with a Bytes(0) field
        // containing `[opcode LE || body]`. flags = opcode << 8 (matches Go
        // `Builder.FinishWithFlags`).
        let mut payload = Vec::with_capacity(2 + body.len());
        payload.extend_from_slice(&op.to_le_bytes());
        payload.extend_from_slice(body);
        let inner = build_object_with_bytes_field(&payload, 8, op << 8);

        // Wrap in the Call correlation header [reqID || reqFlag] and send.
        let req_id = self
            .next_req_id
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        let mut wrapped = Vec::with_capacity(8 + inner.len());
        wrapped.extend_from_slice(&req_id.to_le_bytes());
        wrapped.extend_from_slice(&REQ_FLAG_REQUEST.to_le_bytes());
        wrapped.extend_from_slice(&inner);
        write_frame(&mut self.stream, &wrapped).await?;

        // Await the response with a per-call deadline.
        let response = match tokio::time::timeout(CALL_TIMEOUT, self.read_response(req_id)).await {
            Ok(r) => r?,
            Err(_) => return Err(ZapError::Timeout),
        };
        Ok(response)
    }

    /// Read inbound frames until we see a Call response with a matching
    /// `reqID`. Frames carrying other `reqIDs` are unexpected on this
    /// single-flight client and are dropped with a warn-level log.
    async fn read_response(&mut self, expected_req_id: u32) -> Result<Vec<u8>> {
        loop {
            let frame = read_frame(&mut self.stream).await?;
            if frame.len() < 8 {
                return Err(ZapError::ShortResponse(frame.len()));
            }
            let resp_id = LittleEndian::read_u32(&frame[0..4]);
            let flag = LittleEndian::read_u32(&frame[4..8]);
            if flag != REQ_FLAG_RESPONSE {
                // Server should never send us a request — log and skip.
                warn!(
                    target: "operator::zap",
                    flag = flag,
                    "unexpected frame flag (not a response)"
                );
                continue;
            }
            if resp_id != expected_req_id {
                warn!(
                    target: "operator::zap",
                    expected = expected_req_id,
                    actual = resp_id,
                    "stale response — dropping"
                );
                continue;
            }

            // Parse the inner ZAP message and extract the {status||json} body.
            let inner = &frame[8..];
            let payload = read_root_bytes_field(inner)?;
            return classify_response(payload);
        }
    }

    /// Address this client is connected to. Used by the kms_controller
    /// connection cache.
    #[allow(dead_code)]
    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// NodeID this client presented to the server. Useful for log output
    /// when correlating server-side audit lines.
    #[allow(dead_code)]
    pub fn node_id(&self) -> &[u8] {
        &self.node_id
    }
}

/// Apply the `{status || json}` framing classification per `the canonical KMS client`.
fn classify_response(payload: &[u8]) -> Result<Vec<u8>> {
    if payload.is_empty() {
        return Err(ZapError::ShortResponse(0));
    }
    let status = payload[0];
    let body = &payload[1..];
    match status {
        STATUS_OK => Ok(body.to_vec()),
        STATUS_NOT_FOUND => Err(ZapError::ZapNotFound),
        STATUS_FORBIDDEN => Err(ZapError::ZapForbidden),
        STATUS_ERROR => Err(ZapError::ZapServerError(extract_error(body))),
        other => Err(ZapError::ZapServerError(format!(
            "unknown status byte 0x{other:02x}: {}",
            extract_error(body)
        ))),
    }
}

/// Best-effort decode of the server's `{"error":"..."}` JSON body.
fn extract_error(body: &[u8]) -> String {
    #[derive(Deserialize)]
    struct Err {
        #[serde(default)]
        error: String,
    }
    if let Ok(e) = serde_json::from_slice::<Err>(body) {
        if !e.error.is_empty() {
            return e.error;
        }
    }
    String::from_utf8_lossy(body).into_owned()
}

/// Derive a stable 32-byte ASCII NodeID from `"operator-kms-client"` plus an
/// optional cluster discriminator. Server-side ACLs can recognise this exact
/// prefix to grant operator-namespace read access without enabling the
/// universal-auth flow.
///
/// The output is a hex-encoded SHA-256 digest truncated to 32 bytes — printable
/// ASCII suitable for logs and audit trails.
pub fn derive_node_id(cluster_name: &str) -> Vec<u8> {
    let mut h = Sha256::new();
    h.update(b"operator-kms-client");
    h.update(b"|");
    h.update(cluster_name.as_bytes());
    let digest = h.finalize();
    // 32 bytes hex = 64 chars; we want exactly 32 bytes total to leave room
    // for the 60-byte handshake field. Keep the first 16 bytes hex-encoded
    // (= 32 ASCII chars).
    let hex = hex_encode(&digest[..16]);
    hex.into_bytes()
}

fn hex_encode(b: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(b.len() * 2);
    for &c in b {
        out.push(HEX[(c >> 4) as usize] as char);
        out.push(HEX[(c & 0x0f) as usize] as char);
    }
    out
}

// ────────────────────────────────────────────────────────────────────────────
// Unit tests — pure-function paths only. Mock-server integration tests live
// in `tests/zap_secret_get.rs`.
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_object_with_bytes_field_round_trips_via_parser() {
        let payload = b"\x40\x00{\"path\":\"x\"}";
        let msg = build_object_with_bytes_field(payload, 8, OP_SECRET_GET << 8);
        let got = read_root_bytes_field(&msg).expect("parse");
        assert_eq!(got, payload);
    }

    #[test]
    fn build_handshake_round_trips_node_id() {
        let id = b"operator-kms-client-test-42";
        let msg = build_handshake(id);
        let parsed = read_handshake_node_id(&msg).unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn build_handshake_truncates_oversize_node_id() {
        let id: Vec<u8> = (0..70).map(|i| b'a' + (i % 26) as u8).collect();
        let msg = build_handshake(&id);
        let parsed = read_handshake_node_id(&msg).unwrap();
        assert_eq!(parsed.len(), HANDSHAKE_ID_MAX);
        assert_eq!(parsed, &id[..HANDSHAKE_ID_MAX]);
    }

    #[test]
    fn parse_header_rejects_bad_magic() {
        let mut buf = vec![0u8; HEADER_SIZE];
        buf[..4].copy_from_slice(b"NOPE");
        assert!(matches!(parse_header(&buf), Err(ZapError::InvalidMagic)));
    }

    #[test]
    fn parse_header_rejects_bad_version() {
        let mut buf = vec![0u8; HEADER_SIZE];
        buf[..4].copy_from_slice(MAGIC);
        LittleEndian::write_u16(&mut buf[4..6], 99);
        LittleEndian::write_u32(&mut buf[12..16], HEADER_SIZE as u32);
        assert!(matches!(
            parse_header(&buf),
            Err(ZapError::UnsupportedVersion(99))
        ));
    }

    #[test]
    fn classify_response_ok_returns_body() {
        let payload = b"\x00{\"value\":\"aGVsbG8=\"}";
        let body = classify_response(payload).unwrap();
        assert_eq!(body, b"{\"value\":\"aGVsbG8=\"}");
    }

    #[test]
    fn classify_response_not_found() {
        let payload = b"\x01{\"error\":\"not found\"}";
        match classify_response(payload) {
            Err(ZapError::ZapNotFound) => {}
            other => panic!("expected ZapNotFound, got {other:?}"),
        }
    }

    #[test]
    fn classify_response_forbidden() {
        let payload = b"\x03{\"error\":\"forbidden\"}";
        match classify_response(payload) {
            Err(ZapError::ZapForbidden) => {}
            other => panic!("expected ZapForbidden, got {other:?}"),
        }
    }

    #[test]
    fn classify_response_error_extracts_message() {
        let payload = b"\x02{\"error\":\"boom\"}";
        match classify_response(payload) {
            Err(ZapError::ZapServerError(s)) => assert_eq!(s, "boom"),
            other => panic!("expected ZapServerError, got {other:?}"),
        }
    }

    #[test]
    fn classify_response_unknown_status_byte() {
        let payload = b"\x77irrelevant";
        match classify_response(payload) {
            Err(ZapError::ZapServerError(s)) => assert!(s.contains("0x77")),
            other => panic!("expected ZapServerError, got {other:?}"),
        }
    }

    #[test]
    fn classify_response_empty_is_short() {
        match classify_response(&[]) {
            Err(ZapError::ShortResponse(0)) => {}
            other => panic!("expected ShortResponse(0), got {other:?}"),
        }
    }

    #[test]
    fn opcodes_match_canonical_lux_kms() {
        // Canonical wire format pinned by
        // `~/work/hanzo/kms/cmd/kmsd/wire_compat_test.go`.
        assert_eq!(OP_SECRET_GET, 0x0040);
        assert_eq!(OP_SECRET_PUT, 0x0041);
        assert_eq!(OP_SECRET_LIST, 0x0042);
        assert_eq!(OP_SECRET_DELETE, 0x0043);
    }

    #[test]
    fn derive_node_id_is_deterministic_per_cluster() {
        let a = derive_node_id("hanzo-devnet");
        let b = derive_node_id("hanzo-devnet");
        assert_eq!(a, b);
        assert_ne!(a, derive_node_id("hanzo-mainnet"));
    }

    #[test]
    fn derive_node_id_fits_handshake_field() {
        let id = derive_node_id("anything");
        assert!(
            id.len() <= HANDSHAKE_ID_MAX,
            "derived NodeID must fit handshake"
        );
    }

    #[test]
    fn align_to_pads_correctly() {
        let mut v = vec![1u8, 2, 3];
        align_to(&mut v, 8);
        assert_eq!(v.len(), 8);
        assert_eq!(&v[..3], &[1, 2, 3]);
        assert_eq!(&v[3..], &[0u8; 5]);
    }

    #[test]
    fn align_to_no_op_when_aligned() {
        let mut v = vec![0u8; 16];
        align_to(&mut v, 8);
        assert_eq!(v.len(), 16);
    }
}
