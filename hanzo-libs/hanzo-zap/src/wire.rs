//! luxfi/zap wire protocol — builder, parser, frame I/O.
//!
//! Identical wire format to hanzo-dev/core/src/zap_wire.rs and Go luxfi/zap.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

// ── Constants ───────────────────────────────────────────────────────────

pub const ZAP_MAGIC: [u8; 4] = *b"ZAP\x00";
pub const HEADER_SIZE: usize = 16;

/// Wire versions, mirroring Go `luxfi/zap`.
///
/// Two schemas are defined. `VERSION_1` is the legacy platformvm schema;
/// `VERSION_2` carries the TxKind discriminator. The arena layout — magic,
/// header, fixed section, relative-offset slots — is identical under both, so
/// for every plane this crate speaks (handshake, cloud request/response) the
/// version byte is pure header.
///
/// `Message::parse` accepts both, byte for byte what Go's `zap.Parse` accepts:
/// versions 1 and 2 only, 0 and 3 refused. A reader that accepts a frame the
/// network refuses is a different protocol, so the bounds are exact, not
/// merely permissive. Callers needing v2 payload semantics gate on
/// [`Message::version`] after parsing, exactly as Go's contract directs.
pub const VERSION_1: u16 = 1;
pub const VERSION_2: u16 = 2;

/// The version this crate emits.
///
/// Go's `NewBuilder` emits `VERSION_2`; this emits `VERSION_1` deliberately.
/// Widening every reader must land and deploy before any emitter flips, and
/// one reader in the fleet — `hanzo/operator`'s `zapclient` — still refuses
/// anything but 1. Emitting 2 before that reader is widened would break it.
/// Every Go peer accepts 1, so emitting 1 interoperates today; the flip to
/// `VERSION_2` is the second phase, and it is what makes the handshake bytes
/// byte-identical to the Go oracle.
pub const VERSION: u16 = VERSION_1;
pub const ALIGNMENT: usize = 8;
pub const MAX_MESSAGE_SIZE: usize = 10 * 1024 * 1024; // 10 MB

/// The canonical TCP port for ZAP across the Lux ecosystem, as Go's
/// `DefaultPort`. 80 means HTTP, 443 means HTTPS, 9999 means ZAP; the DNS
/// name says which service answers.
pub const DEFAULT_PORT: u16 = 9999;

// ── Header flags (low byte; the high byte carries the message type) ─────

pub const FLAG_NONE: u16 = 0;
pub const FLAG_COMPRESSED: u16 = 1 << 0;
pub const FLAG_ENCRYPTED: u16 = 1 << 1;
pub const FLAG_SIGNED: u16 = 1 << 2;

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

/// The request/response preamble: request id then flag, both u32 LE.
pub const CORRELATED_HEADER_SIZE: usize = 8;

/// Prepend the correlation preamble to a message body.
///
/// One encoder, one decoder — the pair exists because this eight-byte header
/// was otherwise written out by hand wherever a call was made, and a header
/// written in three places is three chances to write it differently.
pub fn wrap_correlated(req_id: u32, flag: u32, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(CORRELATED_HEADER_SIZE + body.len());
    out.extend_from_slice(&req_id.to_le_bytes());
    out.extend_from_slice(&flag.to_le_bytes());
    out.extend_from_slice(body);
    out
}

/// Read the correlation preamble, or refuse the frame.
///
/// An unrecognised flag is a refusal, not a value to pass along: Go's
/// decoder rejects it, and a caller that skips eight bytes without looking
/// will happily parse the body of a frame that was never a call at all.
pub fn unwrap_correlated(data: &[u8]) -> Option<(u32, u32, &[u8])> {
    if data.len() < CORRELATED_HEADER_SIZE {
        return None;
    }
    let req_id = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let flag = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    if flag != REQ_FLAG_REQ && flag != REQ_FLAG_RESP {
        return None;
    }
    Some((req_id, flag, &data[CORRELATED_HEADER_SIZE..]))
}

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
        if version != VERSION_1 && version != VERSION_2 {
            return Err("unsupported ZAP version");
        }
        Ok(Self { data })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.data
    }

    /// The wire version of this message, `VERSION_1` or `VERSION_2`.
    ///
    /// Mirrors Go's `Message.Version()`. Callers that require v2 payload
    /// semantics gate on this after parsing.
    pub fn version(&self) -> u16 {
        u16::from_le_bytes([self.data[4], self.data[5]])
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

/// A zero-copy view into a ZAP struct.
///
/// Every accessor is total: an offset past the end of the buffer reads as the
/// zero value rather than panicking, exactly as Go's does. That is the
/// protocol, not laxity — a short frame is a peer that lied about its own
/// length, and the fields it did not send are absent, not fatal.
#[derive(Clone, Copy)]
pub struct Object<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Object<'a> {
    /// The null object. Every read on it yields the zero value.
    pub const fn null(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    /// True when this is the null object — an absent nested field.
    ///
    /// Offset zero can never name a real object: the wire header occupies
    /// bytes 0..HEADER_SIZE, and every accessor refuses a target below it.
    pub fn is_null(&self) -> bool {
        self.offset == 0
    }

    /// This object's absolute byte offset in the message, as Go's `Offset`.
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// The message bytes this object views.
    pub fn message_bytes(&self) -> &'a [u8] {
        self.data
    }

    /// The one bounds rule, in one place: a field is readable when this
    /// object is not null and the whole span lies inside the buffer.
    ///
    /// The null case matters. Go models a null object as one holding a nil
    /// message, so reading a field off it dereferences nil and crashes; and
    /// a message whose root offset is zero hands Go an object sitting on the
    /// wire header, so its "fields" read back as magic, version and flags.
    /// Neither is a decision about the wire — both are what happens when
    /// nobody decided. Here offset zero means the object has no fields, so
    /// every read yields the zero value: total, and unable to surface header
    /// bytes as data. No honest frame reaches either case.
    fn span(&self, field_offset: usize, n: usize) -> Option<&'a [u8]> {
        if self.offset == 0 {
            return None;
        }
        let pos = self.offset.checked_add(field_offset)?;
        let end = pos.checked_add(n)?;
        if end > self.data.len() {
            return None;
        }
        Some(&self.data[pos..end])
    }

    pub fn bool(&self, field_offset: usize) -> bool {
        self.uint8(field_offset) != 0
    }

    pub fn uint8(&self, field_offset: usize) -> u8 {
        self.span(field_offset, 1).map_or(0, |s| s[0])
    }

    pub fn uint16(&self, field_offset: usize) -> u16 {
        self.span(field_offset, 2)
            .map_or(0, |s| u16::from_le_bytes([s[0], s[1]]))
    }

    pub fn uint32(&self, field_offset: usize) -> u32 {
        self.span(field_offset, 4)
            .map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    pub fn uint64(&self, field_offset: usize) -> u64 {
        self.span(field_offset, 8).map_or(0, |s| {
            let mut w = [0u8; 8];
            w.copy_from_slice(s);
            u64::from_le_bytes(w)
        })
    }

    pub fn int8(&self, field_offset: usize) -> i8 {
        self.uint8(field_offset) as i8
    }

    pub fn int16(&self, field_offset: usize) -> i16 {
        self.uint16(field_offset) as i16
    }

    pub fn int32(&self, field_offset: usize) -> i32 {
        self.uint32(field_offset) as i32
    }

    pub fn int64(&self, field_offset: usize) -> i64 {
        self.uint64(field_offset) as i64
    }

    pub fn float32(&self, field_offset: usize) -> f32 {
        f32::from_bits(self.uint32(field_offset))
    }

    pub fn float64(&self, field_offset: usize) -> f64 {
        f64::from_bits(self.uint64(field_offset))
    }

    /// Read `n` inline bytes from the fixed payload — the reader for a
    /// fixed-width byte array (a 32-byte hash, a 20-byte address, a 96-byte
    /// witness), not a variable-length tail. Empty when the span runs past
    /// the buffer.
    pub fn bytes_fixed(&self, field_offset: usize, n: usize) -> &'a [u8] {
        if n == 0 { return &[]; }
        self.span(field_offset, n).unwrap_or(&[])
    }

    /// Read a variable-length byte field.
    ///
    /// The relative offset is an UNSIGNED forward pointer, and a target
    /// inside the wire header is refused. Both rules are load-bearing: a
    /// signed cast would let a crafted frame point a payload backwards into
    /// the fixed section or the header, so a reader that allows it accepts
    /// frames the network refuses — which is the same defect as refusing
    /// frames the network accepts, pointed the other way.
    pub fn bytes_field(&self, field_offset: usize) -> &'a [u8] {
        let Some(cell) = self.span(field_offset, 8) else { return &[] };
        let rel = u32::from_le_bytes([cell[0], cell[1], cell[2], cell[3]]) as usize;
        if rel == 0 { return &[]; }
        let length = u32::from_le_bytes([cell[4], cell[5], cell[6], cell[7]]) as usize;
        let abs_pos = self.offset + field_offset + rel;
        if abs_pos < HEADER_SIZE { return &[]; }
        match abs_pos.checked_add(length) {
            Some(end) if end <= self.data.len() => &self.data[abs_pos..end],
            _ => &[],
        }
    }

    /// Read a text field.
    ///
    /// Go hands back the raw bytes as a string, which may hold any byte;
    /// Rust strings are UTF-8, so a field that is not UTF-8 reads as empty
    /// rather than being repaired with replacement characters. Repairing
    /// would let two peers hold different values for the same field.
    pub fn text(&self, field_offset: usize) -> &'a str {
        let b = self.bytes_field(field_offset);
        std::str::from_utf8(b).unwrap_or("")
    }

    /// Read a nested object.
    ///
    /// Here the relative offset is SIGNED — a builder finalizes a child
    /// before its parent, so the child's payload legitimately lies earlier
    /// in the buffer. A target inside the wire header is still refused: the
    /// header carries magic, version, flags, root offset and size, none of
    /// which is an object payload.
    pub fn object(&self, field_offset: usize) -> Object<'a> {
        let Some(cell) = self.span(field_offset, 4) else { return Object::null(self.data) };
        let rel = i32::from_le_bytes([cell[0], cell[1], cell[2], cell[3]]);
        if rel == 0 { return Object::null(self.data); }
        let abs = (self.offset + field_offset) as i64 + rel as i64;
        if abs < HEADER_SIZE as i64 || abs >= self.data.len() as i64 {
            return Object::null(self.data);
        }
        Object { data: self.data, offset: abs as usize }
    }

    /// Read a list.
    ///
    /// The encoded length is clamped to the buffer size. Without that, a
    /// frame declaring 4 billion elements would make every `for i in
    /// 0..list.len()` loop spin for hours reading zeroes. The clamp here is
    /// the permissive one — the wire layer cannot know the element stride,
    /// so each element accessor re-checks its own bounds. When the caller
    /// does know the stride, [`Object::list_stride`] applies the tight clamp
    /// up front.
    pub fn list(&self, field_offset: usize) -> List<'a> {
        self.list_clamped(field_offset, 0)
    }

    /// [`Object::list`] with the per-element byte width supplied, so a
    /// declared length that could not possibly fit — `length * stride`
    /// beyond the end of the buffer — is refused before the first read
    /// rather than at each one.
    ///
    /// The wire format is unchanged; this is a tightened acceptance test.
    /// A stride of 0 means "unknown" and falls back to the baseline clamp.
    pub fn list_stride(&self, field_offset: usize, min_stride: u32) -> List<'a> {
        self.list_clamped(field_offset, min_stride)
    }

    fn list_clamped(&self, field_offset: usize, min_stride: u32) -> List<'a> {
        let Some(cell) = self.span(field_offset, 8) else { return List::null(self.data) };
        let rel = i32::from_le_bytes([cell[0], cell[1], cell[2], cell[3]]);
        if rel == 0 { return List::null(self.data); }
        let length = u32::from_le_bytes([cell[4], cell[5], cell[6], cell[7]]);
        let abs = (self.offset + field_offset) as i64 + rel as i64;
        if abs < HEADER_SIZE as i64 || abs >= self.data.len() as i64 {
            return List::null(self.data);
        }
        let abs = abs as usize;
        if min_stride > 0 {
            let remaining = (self.data.len() - abs) as u64;
            if length as u64 * min_stride as u64 > remaining {
                return List::null(self.data);
            }
        } else if length as usize > self.data.len() {
            return List::null(self.data);
        }
        List { data: self.data, offset: abs, length: length as usize }
    }
}

// ── List (zero-copy reader) ─────────────────────────────────────────────

/// A zero-copy view into a ZAP list.
#[derive(Clone, Copy)]
pub struct List<'a> {
    data: &'a [u8],
    offset: usize,
    length: usize,
}

impl<'a> List<'a> {
    pub const fn null(data: &'a [u8]) -> Self {
        Self { data, offset: 0, length: 0 }
    }

    /// The element count as encoded on the wire.
    ///
    /// Do not pre-allocate on this without an independent bound: the
    /// encoding only constrains it to the buffer size, so a 64 KiB frame can
    /// legitimately declare 65535 elements. Iterate and validate.
    pub fn len(&self) -> usize {
        self.length
    }

    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    pub fn is_null(&self) -> bool {
        self.offset == 0
    }

    pub fn uint8(&self, i: usize) -> u8 {
        if i >= self.length { return 0; }
        let pos = self.offset + i;
        if pos >= self.data.len() { return 0; }
        self.data[pos]
    }

    pub fn uint32(&self, i: usize) -> u32 {
        if i >= self.length { return 0; }
        let pos = self.offset + i * 4;
        if pos + 4 > self.data.len() { return 0; }
        u32::from_le_bytes([
            self.data[pos], self.data[pos + 1],
            self.data[pos + 2], self.data[pos + 3],
        ])
    }

    pub fn uint64(&self, i: usize) -> u64 {
        if i >= self.length { return 0; }
        let pos = self.offset + i * 8;
        if pos + 8 > self.data.len() { return 0; }
        let mut w = [0u8; 8];
        w.copy_from_slice(&self.data[pos..pos + 8]);
        u64::from_le_bytes(w)
    }

    /// Element `i` of an INLINE object list — a fixed-stride object living
    /// at `offset + i * elem_size`.
    pub fn object(&self, i: usize, elem_size: usize) -> Object<'a> {
        if i >= self.length { return Object::null(self.data); }
        Object { data: self.data, offset: self.offset + i * elem_size }
    }

    /// Element `i` of an OUT-OF-LINE object list — a 4-byte signed relative
    /// pointer, dereferenced the way [`Object::object`] dereferences one.
    /// This is how a repeated message field is encoded: a pointer array,
    /// with the objects themselves tailed elsewhere in the buffer.
    pub fn object_ptr(&self, i: usize) -> Object<'a> {
        if i >= self.length { return Object::null(self.data); }
        let pos = self.offset + i * 4;
        if pos + 4 > self.data.len() { return Object::null(self.data); }
        let rel = i32::from_le_bytes([
            self.data[pos], self.data[pos + 1],
            self.data[pos + 2], self.data[pos + 3],
        ]);
        if rel == 0 { return Object::null(self.data); }
        let abs = pos as i64 + rel as i64;
        if abs < HEADER_SIZE as i64 || abs >= self.data.len() as i64 {
            return Object::null(self.data);
        }
        Object { data: self.data, offset: abs as usize }
    }

    /// The raw bytes of a byte list.
    pub fn bytes(&self) -> &'a [u8] {
        if self.is_null() { return &[]; }
        match self.offset.checked_add(self.length) {
            Some(end) if end <= self.data.len() => &self.data[self.offset..end],
            _ => &[],
        }
    }
}

// ── Builder ─────────────────────────────────────────────────────────────

pub struct Builder {
    buf: Vec<u8>,
    pos: usize,
    root_offset: usize,
}

impl Builder {
    /// A builder emitting [`VERSION`].
    pub fn new(capacity: usize) -> Self {
        Self::with_version(capacity, VERSION)
    }

    /// A builder emitting an explicit wire version.
    ///
    /// The fleet flips emitters from 1 to 2 in a second phase, after every
    /// reader accepts both; this is the seam that flip turns, and it is the
    /// only place the emitted version is chosen.
    pub fn with_version(capacity: usize, version: u16) -> Self {
        let cap = if capacity < HEADER_SIZE { 256 } else { capacity };
        let mut buf = vec![0u8; cap];
        buf[0..4].copy_from_slice(&ZAP_MAGIC);
        buf[4..6].copy_from_slice(&version.to_le_bytes());
        Self { buf, pos: HEADER_SIZE, root_offset: 0 }
    }

    /// Rewind for reuse, keeping the grown backing buffer.
    ///
    /// Safe without re-zeroing: every span the builder later exposes is
    /// zero-filled when it is reserved, and padding is zero-filled when it
    /// is skipped, so a reused buffer emits the same bytes as a fresh one.
    pub fn reset(&mut self) {
        self.pos = HEADER_SIZE;
        self.root_offset = 0;
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

    /// Begin an object, reserving its whole fixed section up front.
    ///
    /// Reserving eagerly is what lets a variable field append its tail on
    /// the spot and patch its own pointer immediately — no list of deferred
    /// intentions to replay later. It is also the fix for a silent
    /// corruption: without the reservation, a tail written early is
    /// overwritten by a fixed field set later, and the result is a
    /// well-formed message carrying the wrong bytes, with no error anywhere.
    ///
    /// Children are built BEFORE their parent's `start_object` and attached
    /// by offset with [`ObjectBuilder::set_object`] or
    /// [`ObjectBuilder::set_list`]; the borrow checker enforces that order,
    /// which is the same order Go's builder documents.
    pub fn start_object(&mut self, data_size: usize) -> ObjectBuilder<'_> {
        self.align(ALIGNMENT);
        let start_pos = self.pos;
        let mut ob = ObjectBuilder { start_pos, builder: self };
        ob.reserve_fixed(data_size);
        ob
    }

    /// Begin a list. Elements are appended by the returned builder, which
    /// hands back the (offset, length) pair to attach with
    /// [`ObjectBuilder::set_list`].
    pub fn start_list(&mut self) -> ListBuilder<'_> {
        self.align(ALIGNMENT);
        ListBuilder { start_pos: self.pos, count: 0, builder: self }
    }

    /// Write raw bytes into the variable section and return their offset.
    /// Zero-length input writes nothing and returns 0, the null offset.
    pub fn write_bytes(&mut self, data: &[u8]) -> usize {
        if data.is_empty() { return 0; }
        self.align(ALIGNMENT);
        let offset = self.pos;
        self.grow(data.len());
        self.buf[offset..offset + data.len()].copy_from_slice(data);
        self.pos += data.len();
        offset
    }

    pub fn write_text(&mut self, s: &str) -> usize {
        self.write_bytes(s.as_bytes())
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

pub struct ObjectBuilder<'a> {
    builder: &'a mut Builder,
    start_pos: usize,
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

    /// Materialize the object's fixed payload up to `data_size`.
    /// Idempotent, and never moves the cursor backwards.
    pub fn reserve_fixed(&mut self, data_size: usize) {
        self.ensure_field(data_size);
    }

    /// This object's offset, for a parent to point at.
    pub fn offset(&self) -> usize {
        self.start_pos
    }

    pub fn set_bool(&mut self, field_offset: usize, v: bool) {
        self.set_uint8(field_offset, if v { 1 } else { 0 });
    }

    pub fn set_uint8(&mut self, field_offset: usize, v: u8) {
        self.ensure_field(field_offset + 1);
        self.builder.buf[self.start_pos + field_offset] = v;
    }

    pub fn set_uint16(&mut self, field_offset: usize, v: u16) {
        self.ensure_field(field_offset + 2);
        let pos = self.start_pos + field_offset;
        self.builder.buf[pos..pos + 2].copy_from_slice(&v.to_le_bytes());
    }

    pub fn set_uint32(&mut self, field_offset: usize, v: u32) {
        self.ensure_field(field_offset + 4);
        let pos = self.start_pos + field_offset;
        self.builder.buf[pos..pos + 4].copy_from_slice(&v.to_le_bytes());
    }

    pub fn set_uint64(&mut self, field_offset: usize, v: u64) {
        self.ensure_field(field_offset + 8);
        let pos = self.start_pos + field_offset;
        self.builder.buf[pos..pos + 8].copy_from_slice(&v.to_le_bytes());
    }

    pub fn set_int8(&mut self, field_offset: usize, v: i8) {
        self.set_uint8(field_offset, v as u8);
    }

    pub fn set_int16(&mut self, field_offset: usize, v: i16) {
        self.set_uint16(field_offset, v as u16);
    }

    pub fn set_int32(&mut self, field_offset: usize, v: i32) {
        self.set_uint32(field_offset, v as u32);
    }

    pub fn set_int64(&mut self, field_offset: usize, v: i64) {
        self.set_uint64(field_offset, v as u64);
    }

    pub fn set_float32(&mut self, field_offset: usize, v: f32) {
        self.set_uint32(field_offset, v.to_bits());
    }

    pub fn set_float64(&mut self, field_offset: usize, v: f64) {
        self.set_uint64(field_offset, v.to_bits());
    }

    /// Write a fixed-width byte array in place in the fixed payload — a
    /// hash, an address, a witness. The variable-length counterpart is
    /// [`ObjectBuilder::set_bytes`].
    pub fn set_bytes_fixed(&mut self, field_offset: usize, v: &[u8]) {
        if v.is_empty() { return; }
        self.ensure_field(field_offset + v.len());
        let pos = self.start_pos + field_offset;
        self.builder.buf[pos..pos + v.len()].copy_from_slice(v);
    }

    /// Write a variable-length byte field: the tail goes to the end of the
    /// buffer now, and this field's (offset, length) pair is patched on the
    /// spot. The offset is always forward, which is why the reader can treat
    /// it as unsigned.
    pub fn set_bytes(&mut self, field_offset: usize, data: &[u8]) {
        self.ensure_field(field_offset + 8);
        let field_abs_pos = self.start_pos + field_offset;
        if data.is_empty() {
            self.builder.buf[field_abs_pos..field_abs_pos + 8].fill(0);
            return;
        }
        let data_pos = self.builder.pos;
        self.builder.grow(data.len());
        self.builder.buf[data_pos..data_pos + data.len()].copy_from_slice(data);
        self.builder.pos = data_pos + data.len();

        let rel = (data_pos - field_abs_pos) as u32;
        self.builder.buf[field_abs_pos..field_abs_pos + 4].copy_from_slice(&rel.to_le_bytes());
        self.builder.buf[field_abs_pos + 4..field_abs_pos + 8]
            .copy_from_slice(&(data.len() as u32).to_le_bytes());
    }

    pub fn set_text(&mut self, field_offset: usize, text: &str) {
        self.set_bytes(field_offset, text.as_bytes());
    }

    /// Point a field at a nested object already written at `obj_offset`.
    /// Offset 0 writes the null pointer.
    pub fn set_object(&mut self, field_offset: usize, obj_offset: usize) {
        self.ensure_field(field_offset + 4);
        let field_abs_pos = self.start_pos + field_offset;
        let rel = if obj_offset == 0 {
            0i32
        } else {
            obj_offset as i64 as i32 - field_abs_pos as i64 as i32
        };
        self.builder.buf[field_abs_pos..field_abs_pos + 4]
            .copy_from_slice(&(rel as u32).to_le_bytes());
    }

    /// Point a field at a list already written at `list_offset`.
    /// An empty list writes the null pair.
    pub fn set_list(&mut self, field_offset: usize, list_offset: usize, length: usize) {
        self.ensure_field(field_offset + 8);
        let field_abs_pos = self.start_pos + field_offset;
        if list_offset == 0 || length == 0 {
            self.builder.buf[field_abs_pos..field_abs_pos + 8].fill(0);
            return;
        }
        let rel = list_offset as i64 as i32 - field_abs_pos as i64 as i32;
        self.builder.buf[field_abs_pos..field_abs_pos + 4]
            .copy_from_slice(&(rel as u32).to_le_bytes());
        self.builder.buf[field_abs_pos + 4..field_abs_pos + 8]
            .copy_from_slice(&(length as u32).to_le_bytes());
    }

    /// Finalize and return this object's offset. Every field was written
    /// eagerly by its setter, so there is nothing to replay.
    pub fn finish(self) -> usize {
        self.start_pos
    }

    /// Finalize as the message root, returning its offset.
    pub fn finish_as_root(self) -> usize {
        let offset = self.start_pos;
        self.builder.root_offset = offset;
        offset
    }
}

/// Builds a ZAP list. The stride is implicit in which `add_*` is called;
/// mixing kinds in one list is a schema error, not a wire one.
pub struct ListBuilder<'a> {
    builder: &'a mut Builder,
    start_pos: usize,
    count: usize,
}

impl<'a> ListBuilder<'a> {
    pub fn add_uint8(&mut self, v: u8) {
        self.builder.grow(1);
        self.builder.buf[self.builder.pos] = v;
        self.builder.pos += 1;
        self.count += 1;
    }

    pub fn add_uint32(&mut self, v: u32) {
        self.builder.grow(4);
        let p = self.builder.pos;
        self.builder.buf[p..p + 4].copy_from_slice(&v.to_le_bytes());
        self.builder.pos = p + 4;
        self.count += 1;
    }

    pub fn add_uint64(&mut self, v: u64) {
        self.builder.grow(8);
        let p = self.builder.pos;
        self.builder.buf[p..p + 8].copy_from_slice(&v.to_le_bytes());
        self.builder.pos = p + 8;
        self.count += 1;
    }

    /// Append raw bytes to a byte list. The element count advances by the
    /// byte count, matching the 1-byte stride a byte list reads with.
    pub fn add_bytes(&mut self, data: &[u8]) {
        self.builder.grow(data.len());
        let p = self.builder.pos;
        self.builder.buf[p..p + data.len()].copy_from_slice(data);
        self.builder.pos = p + data.len();
        self.count += data.len();
    }

    /// Append a signed relative pointer to an object at `target_pos`
    /// (0 for a null element) — the element kind of a repeated message
    /// field. Targets usually precede the slot, since the objects are
    /// written before the pointer array.
    pub fn add_object_ptr(&mut self, target_pos: usize) {
        self.builder.grow(4);
        let p = self.builder.pos;
        let rel: i32 = if target_pos == 0 {
            0
        } else {
            target_pos as i64 as i32 - p as i64 as i32
        };
        self.builder.buf[p..p + 4].copy_from_slice(&(rel as u32).to_le_bytes());
        self.builder.pos = p + 4;
        self.count += 1;
    }

    /// The list's offset and element count, to hand to
    /// [`ObjectBuilder::set_list`].
    pub fn finish(self) -> (usize, usize) {
        (self.start_pos, self.count)
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

/// Read a peer's node id out of the handshake frame, or refuse the frame.
///
/// Refusal is part of the wire protocol, so the bounds are Go's, exactly:
/// `DecodeNodeIDHandshake` rejects a declared length of 0 or one above
/// [`HANDSHAKE_ID_MAX`], and a reader that clamps instead of refusing admits a
/// peer the network does not. That is why this returns `Option` rather than a
/// `String` it can always produce.
///
/// One narrowing over Go, deliberate: Go builds its id with `string(idBytes)`,
/// which carries arbitrary bytes, while this requires UTF-8. Replacing a bad
/// byte with U+FFFD would let two peers hold different ids for the same
/// connection, so a non-UTF-8 id is refused rather than repaired. Every id the
/// network actually issues is ASCII, so nothing legitimate is turned away.
pub fn parse_handshake(msg: &Message) -> Option<String> {
    let root = msg.root();
    let id_len = root.uint32(HANDSHAKE_ID_LEN_OFFSET) as usize;
    if id_len == 0 || id_len > HANDSHAKE_ID_MAX {
        return None;
    }
    let mut id = Vec::with_capacity(id_len);
    for i in 0..id_len { id.push(root.uint8(i)); }
    String::from_utf8(id).ok()
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
        assert_eq!(parse_handshake(&msg).as_deref(), Some("hanzo-node"));
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

    // ── Conformance to the Go oracle ────────────────────────────────────
    //
    // Go is the network. These vectors are not hand-written: they are the
    // output of `github.com/luxfi/zap` v1.2.6 — the exact version
    // `luxfi/node` ships — captured by calling `zap.EncodeNodeIDHandshake`
    // and `zap.Parse` directly. luxd emits Version2 by default, so a reader
    // that refuses 2 cannot read the live network at all.

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// Go `zap.EncodeNodeIDHandshake("hanzo-node")`, v1.2.6, verbatim.
    const GO_HANDSHAKE_V2: &str = "5a41500002000000100000005000000068616e7a6f2d6e6f646500000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000a000000";

    /// Go `zap.EncodeNodeIDHandshake("NodeID-AUrEBpfnoYUiCcHFXQdj7dAZ64S9mSMZs")`.
    const GO_HANDSHAKE_NODEID_V2: &str = "5a4150000200000010000000500000004e6f646549442d415572454270666e6f595569436348465851646a3764415a363453396d534d5a73000000000000000000000000000000000000000028000000";

    #[test]
    fn reads_the_go_oracles_v2_handshake() {
        // The regression test for the break: luxd emits Version2, and this
        // crate refused it. Every field must still decode.
        for (hex, want) in [
            (GO_HANDSHAKE_V2, "hanzo-node"),
            (GO_HANDSHAKE_NODEID_V2, "NodeID-AUrEBpfnoYUiCcHFXQdj7dAZ64S9mSMZs"),
        ] {
            let bytes = unhex(hex);
            assert_eq!(bytes.len(), 80, "Go handshake is 80 bytes");
            let msg = Message::parse(bytes).expect("must read what luxd emits");
            assert_eq!(msg.version(), VERSION_2);
            assert_eq!(parse_handshake(&msg).as_deref(), Some(want));
        }
    }

    #[test]
    fn version_acceptance_matches_go() {
        // Go's Parse accepts exactly {1, 2}. Not 0, not 3. Accepting a frame
        // the network refuses is as wrong as refusing one it accepts.
        let base = unhex(GO_HANDSHAKE_V2);
        for (version, accepted) in [(0u8, false), (1, true), (2, true), (3, false)] {
            let mut b = base.clone();
            b[4] = version;
            assert_eq!(
                Message::parse(b).is_ok(),
                accepted,
                "version {version} acceptance must match Go"
            );
        }
    }

    #[test]
    fn payload_is_byte_identical_to_go() {
        // Our handshake equals Go's in every byte but the version. That is
        // what makes the emitter flip to VERSION_2 a one-byte change rather
        // than a re-encoding, and it proves the arena layout agrees.
        let mut want = unhex(GO_HANDSHAKE_V2);
        want[4] = VERSION_1 as u8;
        assert_eq!(build_handshake("hanzo-node"), want);
    }

    #[test]
    fn version_accessor_reports_what_was_emitted() {
        let msg = Message::parse(build_handshake("hanzo-node")).unwrap();
        assert_eq!(msg.version(), VERSION);
    }
}
