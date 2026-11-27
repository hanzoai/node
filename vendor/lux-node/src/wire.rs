// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! The ZAP stream framing luxd speaks to its peers.
//!
//! Not protobuf, despite the package name `proto/p2p`: `luxfi/proto`'s
//! `node/zap/p2p` is a hand-written positional codec, and the schema at
//! `p2p.zap` says plainly that the STREAM framing is the one shipping today —
//! fields in declaration order, big-endian, no tags and no offsets, so a reader
//! walks a frame from the front and cannot seek.
//!
//! A frame on the socket is a 4-byte big-endian length and then that many
//! bytes, of which the first is the message's discriminator
//! (`network/peer/msg_length.go`, `network/peer/peer.go readMessages`). There
//! is no magic, no version byte and no checksum.
//!
//! THE LENGTH PREFIX IS NOT PART OF THE LENGTH. `writeMessage` writes
//! `[4-byte len][len bytes]`, so a 1183-byte message occupies 1187 bytes of
//! stream. Getting that off by four is the classic way to desynchronise and
//! then blame the codec.
//!
//! A fixed-width field is STILL length-prefixed. A 32-byte chain id travels as
//! `00 00 00 20` and then the 32 bytes — the prefix is redundant and the
//! receiver checks it, so an id that arrives some other width is a frame that
//! lied rather than a frame that is merely wrong.

/// The largest frame either side will read: `constants.DefaultMaxMessageSize`.
pub const MESSAGE_MAX: u32 = 2 * 1024 * 1024;

/// The discriminator byte that names a message.
pub mod tag {
    pub const COMPRESSED_ZSTD: u8 = 1;
    pub const PING: u8 = 2;
    pub const PONG: u8 = 3;
    pub const HANDSHAKE: u8 = 4;
    pub const GET_PEER_LIST: u8 = 5;
    pub const PEER_LIST: u8 = 6;
    pub const GET_ACCEPTED_FRONTIER: u8 = 11;
    pub const ACCEPTED_FRONTIER: u8 = 12;
    pub const GET_ACCEPTED: u8 = 13;
    pub const ACCEPTED: u8 = 14;
    pub const PUSH_QUERY: u8 = 19;
    pub const PULL_QUERY: u8 = 20;
    pub const CHITS: u8 = 21;
    pub const REQUEST: u8 = 22;
    pub const RESPONSE: u8 = 23;
    /// AppGossip. What consensus votes and certificates actually ride on.
    pub const GOSSIP: u8 = 24;
    pub const ERROR: u8 = 25;
}

/// Builds a message body.
#[derive(Default)]
pub struct Writer(Vec<u8>);

impl Writer {
    pub fn new() -> Self {
        Self(Vec::with_capacity(256))
    }

    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }

    /// A bool is one byte, and ONLY 1 is true — luxd's `ReadBool` is `v == 1`,
    /// so writing 2 for true would read back as false.
    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.0.push(u8::from(v));
        self
    }

    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }

    /// A byte string: its length, then the bytes.
    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.u32(b.len() as u32);
        self.0.extend_from_slice(b);
        self
    }

    pub fn text(&mut self, s: &str) -> &mut Self {
        self.bytes(s.as_bytes())
    }

    pub fn list_bytes<T: AsRef<[u8]>>(&mut self, items: &[T]) -> &mut Self {
        self.u32(items.len() as u32);
        for i in items {
            self.bytes(i.as_ref());
        }
        self
    }

    pub fn list_u32(&mut self, items: &[u32]) -> &mut Self {
        self.u32(items.len() as u32);
        for i in items {
            self.u32(*i);
        }
        self
    }

    /// An absent optional struct: one zero byte and nothing else.
    pub fn absent(&mut self) -> &mut Self {
        self.u8(0)
    }

    pub fn present(&mut self) -> &mut Self {
        self.u8(1)
    }

    pub fn take(self) -> Vec<u8> {
        self.0
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

/// Reads a message body.
pub struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    /// Whether anything is left. luxd uses this to tell a peer that omitted an
    /// append-only trailing field from one that sent it empty, so it is part of
    /// the encoding rather than a convenience.
    pub fn more(&self) -> bool {
        self.at < self.data.len()
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.at)
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let out = self.data.get(self.at..self.at + n)?;
        self.at += n;
        Some(out)
    }

    pub fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    pub fn bool(&mut self) -> Option<bool> {
        self.u8().map(|v| v == 1)
    }

    pub fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn u64(&mut self) -> Option<u64> {
        self.take(8).map(|b| u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
    }

    pub fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        // A length that outruns the frame is a lie, not a short read.
        if n > self.remaining() {
            return None;
        }
        self.take(n)
    }
}

/// Wrap a body in the length prefix the socket carries.
pub fn frame(tag: u8, body: &[u8]) -> Vec<u8> {
    let len = (body.len() + 1) as u32;
    let mut out = Vec::with_capacity(4 + len as usize);
    out.extend_from_slice(&len.to_be_bytes());
    out.push(tag);
    out.extend_from_slice(body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_its_length_then_its_tag_then_its_body() {
        let f = frame(tag::PING, &[0xaa, 0xbb]);
        assert_eq!(f, vec![0, 0, 0, 3, 2, 0xaa, 0xbb]);
        // The prefix is NOT counted in itself: three bytes of message, four of
        // prefix, seven on the wire.
        assert_eq!(f.len(), 7);
        assert_eq!(u32::from_be_bytes([f[0], f[1], f[2], f[3]]) as usize, f.len() - 4);
    }

    #[test]
    fn a_fixed_width_id_is_still_length_prefixed() {
        let mut w = Writer::new();
        w.bytes(&[7u8; 32]);
        let out = w.take();
        assert_eq!(out.len(), 36);
        assert_eq!(&out[..4], &[0, 0, 0, 32]);
    }

    #[test]
    fn everything_written_reads_back() {
        let mut w = Writer::new();
        w.u32(1337).u64(0x6a98b856).bytes(&[127, 0, 0, 1]).u32(19731).bool(true);
        w.list_bytes(&[[9u8; 32]]).list_u32(&[1, 2, 3]).absent();
        let body = w.take();
        let mut r = Reader::new(&body);
        assert_eq!(r.u32(), Some(1337));
        assert_eq!(r.u64(), Some(0x6a98b856));
        assert_eq!(r.bytes(), Some([127, 0, 0, 1].as_slice()));
        assert_eq!(r.u32(), Some(19731));
        assert_eq!(r.bool(), Some(true));
        assert_eq!(r.u32(), Some(1)); // list count
        assert_eq!(r.bytes(), Some([9u8; 32].as_slice()));
        assert_eq!(r.u32(), Some(3)); // list count
        assert_eq!((r.u32(), r.u32(), r.u32()), (Some(1), Some(2), Some(3)));
        assert_eq!(r.u8(), Some(0)); // absent
        assert!(!r.more());
    }

    #[test]
    fn only_one_is_true() {
        // luxd's ReadBool is `v == 1`, so a peer writing 2 for true is a peer
        // whose flag reads back false.
        assert_eq!(Reader::new(&[1]).bool(), Some(true));
        assert_eq!(Reader::new(&[2]).bool(), Some(false));
        assert_eq!(Reader::new(&[0]).bool(), Some(false));
    }

    #[test]
    fn a_length_that_outruns_the_frame_is_refused() {
        // The bounds check that keeps a hostile length from being a panic.
        let mut r = Reader::new(&[0, 0, 0xff, 0xff, 1, 2]);
        assert_eq!(r.bytes(), None);
    }

    /// The exact bytes a live luxd sent on this machine, as its first message.
    #[test]
    fn the_handshake_luxd_actually_sent_parses() {
        // len=1183, tag=4 (Handshake), then networkID 1337, a Unix time, the
        // 4-byte IP 127.0.0.1, port 19731, a signing time and a 72-byte DER
        // signature. Captured off the wire, not constructed here.
        let head: [u8; 44] = [
            0x00, 0x00, 0x04, 0x9f, 0x04, 0x00, 0x00, 0x05, 0x39, 0x00, 0x00, 0x00, 0x00, 0x6a,
            0x98, 0xb8, 0x7a, 0x00, 0x00, 0x00, 0x04, 0x7f, 0x00, 0x00, 0x01, 0x00, 0x00, 0x4d,
            0x13, 0x00, 0x00, 0x00, 0x00, 0x6a, 0x98, 0xb8, 0x38, 0x00, 0x00, 0x00, 0x48, 0x30,
            0x46, 0x02,
        ];
        assert_eq!(u32::from_be_bytes([head[0], head[1], head[2], head[3]]), 1183);
        assert_eq!(head[4], tag::HANDSHAKE);

        let mut r = Reader::new(&head[5..]);
        assert_eq!(r.u32(), Some(1337), "the localnet network id");
        assert!(r.u64().is_some(), "myTime");
        assert_eq!(r.bytes(), Some([127, 0, 0, 1].as_slice()), "IpAddr is 4 bytes for v4");
        assert_eq!(r.u32(), Some(19731), "IpPort, a u16 in a u32");
        assert!(r.u64().is_some(), "IpSigningTime");
        assert_eq!(r.u32(), Some(0x48), "a 72-byte DER signature follows");
    }
}
