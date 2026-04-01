//! luxfi/zap wire protocol — builder, parser, frame I/O.
//!
//! Identical wire format to hanzo-dev/core/src/zap_wire.rs and Go luxfi/zap.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

// ── Constants ───────────────────────────────────────────────────────────

pub const ZAP_MAGIC: [u8; 4] = *b"ZAP\x00";
pub const HEADER_SIZE: usize = 16;
pub const VERSION: u16 = 1;
pub const ALIGNMENT: usize = 8;
pub const MAX_MESSAGE_SIZE: usize = 10 * 1024 * 1024; // 10 MB

/// Cloud service (native binary RPC).
pub const MSG_TYPE_CLOUD: u16 = 100;

// ── Cloud request field byte offsets ────────────────────────────────────
// Layout: method(0:Text) + auth(8:Text) + body(16:Bytes)

pub const CLOUD_REQ_METHOD: usize = 0;
pub const CLOUD_REQ_AUTH: usize = 8;
pub const CLOUD_REQ_BODY: usize = 16;
pub const CLOUD_REQ_FIXED_SIZE: usize = 24;

// ── Cloud response field byte offsets ───────────────────────────────────
// Layout: status(0:Uint32) + body(4:Bytes) + error(12:Text)

pub const CLOUD_RESP_STATUS: usize = 0;
pub const CLOUD_RESP_BODY: usize = 4;
pub const CLOUD_RESP_ERROR: usize = 12;

// ── Call correlation ────────────────────────────────────────────────────

pub const REQ_FLAG_REQ: u32 = 1;
pub const REQ_FLAG_RESP: u32 = 2;

// ── Handshake ───────────────────────────────────────────────────────────

pub const HANDSHAKE_OBJ_SIZE: usize = 64;
pub const HANDSHAKE_ID_MAX: usize = 60;
pub const HANDSHAKE_ID_LEN_OFFSET: usize = 60;

// ── Message ─────────────────────────────────────────────────────────────

/// A parsed ZAP message that owns its byte buffer.
pub struct Message {
    data: Vec<u8>,
}

impl Message {
    pub fn parse(data: Vec<u8>) -> Result<Self, &'static str> {
        if data.len() < HEADER_SIZE {
            return Err("buffer too small for ZAP header");
        }
        if data[0..4] != ZAP_MAGIC {
            return Err("invalid ZAP magic bytes");
        }
        let version = u16::from_le_bytes([data[4], data[5]]);
        if version != VERSION {
            return Err("unsupported ZAP version");
        }
        Ok(Self { data })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.data
    }

    pub fn flags(&self) -> u16 {
        u16::from_le_bytes([self.data[6], self.data[7]])
    }

    pub fn msg_type(&self) -> u16 {
        self.flags() >> 8
    }

    pub fn root(&self) -> Object<'_> {
        let offset = u32::from_le_bytes([
            self.data[8],
            self.data[9],
            self.data[10],
            self.data[11],
        ]) as usize;
        Object {
            data: &self.data,
            offset,
        }
    }
}

// ── Object (zero-copy reader) ───────────────────────────────────────────

pub struct Object<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Object<'a> {
    pub fn uint8(&self, field_offset: usize) -> u8 {
        let pos = self.offset + field_offset;
        if pos >= self.data.len() { return 0; }
        self.data[pos]
    }

    pub fn uint32(&self, field_offset: usize) -> u32 {
        let pos = self.offset + field_offset;
        if pos + 4 > self.data.len() { return 0; }
        u32::from_le_bytes([
            self.data[pos], self.data[pos + 1],
            self.data[pos + 2], self.data[pos + 3],
        ])
    }

    pub fn bytes_field(&self, field_offset: usize) -> &'a [u8] {
        let pos = self.offset + field_offset;
        if pos + 4 > self.data.len() { return &[]; }
        let rel_offset = i32::from_le_bytes([
            self.data[pos], self.data[pos + 1],
            self.data[pos + 2], self.data[pos + 3],
        ]);
        if rel_offset == 0 { return &[]; }
        let len_pos = pos + 4;
        if len_pos + 4 > self.data.len() { return &[]; }
        let length = u32::from_le_bytes([
            self.data[len_pos], self.data[len_pos + 1],
            self.data[len_pos + 2], self.data[len_pos + 3],
        ]) as usize;
        let abs_i64 = pos as i64 + rel_offset as i64;
        if abs_i64 < 0 || abs_i64 as usize > self.data.len() { return &[]; }
        let abs_pos = abs_i64 as usize;
        match abs_pos.checked_add(length) {
            Some(end) if end <= self.data.len() => &self.data[abs_pos..end],
            _ => &[],
        }
    }

    pub fn text(&self, field_offset: usize) -> &'a str {
        let b = self.bytes_field(field_offset);
        std::str::from_utf8(b).unwrap_or("")
    }
}

// ── Builder ─────────────────────────────────────────────────────────────

pub struct Builder {
    buf: Vec<u8>,
    pos: usize,
    root_offset: usize,
}

impl Builder {
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.max(256);
        let mut buf = vec![0u8; cap];
        buf[0..4].copy_from_slice(&ZAP_MAGIC);
        buf[4..6].copy_from_slice(&VERSION.to_le_bytes());
        Self { buf, pos: HEADER_SIZE, root_offset: 0 }
    }

    fn grow(&mut self, n: usize) {
        let needed = self.pos + n;
        if needed <= self.buf.len() { return; }
        let new_cap = (self.buf.len() * 2).max(needed);
        self.buf.resize(new_cap, 0);
    }

    fn align(&mut self, alignment: usize) {
        let padding = (alignment - (self.pos % alignment)) % alignment;
        self.grow(padding);
        for _ in 0..padding { self.buf[self.pos] = 0; self.pos += 1; }
    }

    pub fn start_object(&mut self, data_size: usize) -> ObjectBuilder<'_> {
        self.align(ALIGNMENT);
        ObjectBuilder {
            start_pos: self.pos,
            data_size,
            deferred: Vec::new(),
            builder: self,
        }
    }

    pub fn finish(mut self) -> Vec<u8> {
        self.buf[8..12].copy_from_slice(&(self.root_offset as u32).to_le_bytes());
        self.buf[12..16].copy_from_slice(&(self.pos as u32).to_le_bytes());
        self.buf.truncate(self.pos);
        self.buf
    }

    pub fn finish_with_flags(mut self, flags: u16) -> Vec<u8> {
        self.buf[6..8].copy_from_slice(&flags.to_le_bytes());
        self.finish()
    }
}

struct DeferredWrite {
    field_offset: usize,
    data: Vec<u8>,
}

pub struct ObjectBuilder<'a> {
    builder: &'a mut Builder,
    start_pos: usize,
    data_size: usize,
    deferred: Vec<DeferredWrite>,
}

impl<'a> ObjectBuilder<'a> {
    fn ensure_field(&mut self, end_offset: usize) {
        let needed = self.start_pos + end_offset;
        if needed > self.builder.pos {
            self.builder.grow(needed - self.builder.pos);
            for i in self.builder.pos..needed { self.builder.buf[i] = 0; }
            self.builder.pos = needed;
        }
    }

    pub fn set_uint32(&mut self, field_offset: usize, v: u32) {
        self.ensure_field(field_offset + 4);
        let pos = self.start_pos + field_offset;
        self.builder.buf[pos..pos + 4].copy_from_slice(&v.to_le_bytes());
    }

    pub fn set_uint8(&mut self, field_offset: usize, v: u8) {
        self.ensure_field(field_offset + 1);
        self.builder.buf[self.start_pos + field_offset] = v;
    }

    pub fn set_bytes(&mut self, field_offset: usize, data: &[u8]) {
        self.ensure_field(field_offset + 8);
        let pos = self.start_pos + field_offset;
        if data.is_empty() {
            self.builder.buf[pos..pos + 4].copy_from_slice(&0u32.to_le_bytes());
            self.builder.buf[pos + 4..pos + 8].copy_from_slice(&0u32.to_le_bytes());
            return;
        }
        self.builder.buf[pos + 4..pos + 8]
            .copy_from_slice(&(data.len() as u32).to_le_bytes());
        self.deferred.push(DeferredWrite { field_offset, data: data.to_vec() });
    }

    pub fn set_text(&mut self, field_offset: usize, text: &str) {
        self.set_bytes(field_offset, text.as_bytes());
    }

    fn do_finish(&mut self) {
        self.ensure_field(self.data_size);
        for entry in self.deferred.drain(..) {
            let data_pos = self.builder.pos;
            self.builder.grow(entry.data.len());
            let start = self.builder.pos;
            self.builder.buf[start..start + entry.data.len()].copy_from_slice(&entry.data);
            self.builder.pos += entry.data.len();
            let field_abs_pos = self.start_pos + entry.field_offset;
            let rel_offset = data_pos as i32 - field_abs_pos as i32;
            self.builder.buf[field_abs_pos..field_abs_pos + 4]
                .copy_from_slice(&(rel_offset as u32).to_le_bytes());
        }
    }

    pub fn finish_as_root(mut self) {
        self.do_finish();
        self.builder.root_offset = self.start_pos;
    }
}

// ── Frame I/O ───────────────────────────────────────────────────────────

pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, data: &[u8]) -> std::io::Result<()> {
    let len_buf = (data.len() as u32).to_le_bytes();
    w.write_all(&len_buf).await?;
    w.write_all(data).await?;
    w.flush().await?;
    Ok(())
}

pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let length = u32::from_le_bytes(len_buf) as usize;
    if length > MAX_MESSAGE_SIZE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("ZAP message too large: {} bytes", length),
        ));
    }
    let mut data = vec![0u8; length];
    r.read_exact(&mut data).await?;
    Ok(data)
}

// ── Handshake helpers ───────────────────────────────────────────────────

pub fn build_handshake(node_id: &str) -> Vec<u8> {
    let mut b = Builder::new(128);
    let mut obj = b.start_object(HANDSHAKE_OBJ_SIZE);
    let id_bytes = node_id.as_bytes();
    for (i, &byte) in id_bytes.iter().enumerate() {
        if i >= HANDSHAKE_ID_MAX { break; }
        obj.set_uint8(i, byte);
    }
    obj.set_uint32(HANDSHAKE_ID_LEN_OFFSET, id_bytes.len().min(HANDSHAKE_ID_MAX) as u32);
    obj.finish_as_root();
    b.finish()
}

pub fn parse_handshake(msg: &Message) -> String {
    let root = msg.root();
    let id_len = root.uint32(HANDSHAKE_ID_LEN_OFFSET) as usize;
    let id_len = id_len.min(HANDSHAKE_ID_MAX);
    let mut id = Vec::with_capacity(id_len);
    for i in 0..id_len { id.push(root.uint8(i)); }
    String::from_utf8_lossy(&id).into_owned()
}

// ── Cloud message builders ──────────────────────────────────────────────

pub fn build_cloud_request(method: &str, auth: &str, body: &[u8]) -> Vec<u8> {
    let mut b = Builder::new(body.len() + method.len() + auth.len() + 128);
    let mut obj = b.start_object(CLOUD_REQ_FIXED_SIZE);
    obj.set_text(CLOUD_REQ_METHOD, method);
    obj.set_text(CLOUD_REQ_AUTH, auth);
    obj.set_bytes(CLOUD_REQ_BODY, body);
    obj.finish_as_root();
    b.finish_with_flags(MSG_TYPE_CLOUD << 8)
}

pub fn build_cloud_response(status: u32, body: &[u8], error: &str) -> Vec<u8> {
    let mut b = Builder::new(body.len() + error.len() + 128);
    let mut obj = b.start_object(20);
    obj.set_uint32(CLOUD_RESP_STATUS, status);
    obj.set_bytes(CLOUD_RESP_BODY, body);
    obj.set_text(CLOUD_RESP_ERROR, error);
    obj.finish_as_root();
    b.finish_with_flags(MSG_TYPE_CLOUD << 8)
}

pub fn parse_cloud_request(msg: &Message) -> (&str, &str, &[u8]) {
    let root = msg.root();
    let method = root.text(CLOUD_REQ_METHOD);
    let auth = root.text(CLOUD_REQ_AUTH);
    let body = root.bytes_field(CLOUD_REQ_BODY);
    (method, auth, body)
}

pub fn parse_cloud_response(msg: &Message) -> (u32, Vec<u8>, String) {
    let root = msg.root();
    let status = root.uint32(CLOUD_RESP_STATUS);
    let body = root.bytes_field(CLOUD_RESP_BODY).to_vec();
    let error = root.text(CLOUD_RESP_ERROR).to_string();
    (status, body, error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_handshake() {
        let msg_bytes = build_handshake("hanzo-node");
        let msg = Message::parse(msg_bytes).unwrap();
        assert_eq!(parse_handshake(&msg), "hanzo-node");
    }

    #[test]
    fn roundtrip_cloud_request() {
        let body = br#"{"model":"zen4-mini","messages":[{"role":"user","content":"hi"}]}"#;
        let msg_bytes = build_cloud_request("chat.completions", "Bearer tok", body);
        let msg = Message::parse(msg_bytes).unwrap();
        assert_eq!(msg.msg_type(), MSG_TYPE_CLOUD);
        let (method, auth, req_body) = parse_cloud_request(&msg);
        assert_eq!(method, "chat.completions");
        assert_eq!(auth, "Bearer tok");
        assert_eq!(req_body, body.as_slice());
    }

    #[test]
    fn roundtrip_cloud_response() {
        let body = br#"{"id":"cmpl-1","choices":[{"message":{"content":"hello"}}]}"#;
        let msg_bytes = build_cloud_response(200, body, "");
        let msg = Message::parse(msg_bytes).unwrap();
        let (status, resp_body, error) = parse_cloud_response(&msg);
        assert_eq!(status, 200);
        assert_eq!(resp_body, body);
        assert!(error.is_empty());
    }

    #[test]
    fn cloud_response_error() {
        let msg_bytes = build_cloud_response(401, &[], "auth required");
        let msg = Message::parse(msg_bytes).unwrap();
        let (status, body, error) = parse_cloud_response(&msg);
        assert_eq!(status, 401);
        assert!(body.is_empty());
        assert_eq!(error, "auth required");
    }
}
