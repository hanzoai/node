//! Conformance to the Go oracle.
//!
//! Go is the network. Every vector below is the literal output of
//! `github.com/luxfi/zap` v1.2.6 — the version `luxfi/node` pins in its
//! `go.mod` — whose codec (`zap.go`, `builder.go`) is byte-identical through
//! v1.2.9, so the oracle is not a moving target.
//!
//! Two claims are under test, and they are different claims:
//!
//!   ENCODE — building the same message here produces the same bytes Go
//!   produces, byte for byte, including the version word. A codec that
//!   round-trips with itself proves nothing about the network.
//!
//!   ACCEPT — a frame Go refuses is refused here, and a frame Go reads is
//!   read here. Accepting what the network refuses is the same defect as
//!   refusing what it accepts, pointed the other way: it admits frames no
//!   honest peer can produce.
//!
//! The vectors are built with `Builder::with_version(_, VERSION_2)` so the
//! comparison covers the header word too. The crate's default emitter still
//! writes VERSION_1 while the fleet's readers are widened; that these bytes
//! agree in every other position is what makes the eventual flip a one-byte
//! change rather than a re-encoding.

use hanzo_zap::*;

fn unhex(s: &str) -> Vec<u8> {
    assert!(s.len().is_multiple_of(2), "hex must be whole bytes");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex digit"))
        .collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Assert the built message equals Go's, and report the divergence in hex
/// rather than as a bare length mismatch when it does not.
fn same_as_go(name: &str, want_hex: &str, got: Vec<u8>) -> Vec<u8> {
    let want = unhex(want_hex);
    assert_eq!(
        hex(&got),
        hex(&want),
        "{name}: encoded bytes diverge from the Go oracle"
    );
    got
}

// ── ENCODE ──────────────────────────────────────────────────────────────

const SCALARS: &str = "5a41500002000000100000004000000001a5efbeefbeaddeefcdab8967452301fd002efbc01dfeff35fb048ee0feffff00006040000000006957148b0abf05c0";

#[test]
fn scalars_encode_and_read_back_as_go() {
    let mut b = Builder::with_version(256, VERSION_2);
    let mut ob = b.start_object(48);
    ob.set_bool(0, true);
    ob.set_uint8(1, 0xA5);
    ob.set_uint16(2, 0xBEEF);
    ob.set_uint32(4, 0xDEAD_BEEF);
    ob.set_uint64(8, 0x0123_4567_89AB_CDEF);
    ob.set_int8(16, -3);
    ob.set_int16(18, -1234);
    ob.set_int32(20, -123456);
    ob.set_int64(24, -1_234_567_890_123);
    ob.set_float32(32, 3.5);
    ob.set_float64(40, -std::f64::consts::E);
    ob.finish_as_root();
    let bytes = same_as_go("scalars", SCALARS, b.finish());

    let msg = Message::parse(bytes).expect("Go's own bytes must parse");
    let r = msg.root();
    assert!(r.bool(0));
    assert_eq!(r.uint8(1), 0xA5);
    assert_eq!(r.uint16(2), 0xBEEF);
    assert_eq!(r.uint32(4), 0xDEAD_BEEF);
    assert_eq!(r.uint64(8), 0x0123_4567_89AB_CDEF);
    assert_eq!(r.int8(16), -3);
    assert_eq!(r.int16(18), -1234);
    assert_eq!(r.int32(20), -123456);
    assert_eq!(r.int64(24), -1_234_567_890_123);
    assert_eq!(r.float32(32), 3.5);
    assert_eq!(r.float64(40), -std::f64::consts::E);
    assert!(!r.is_null());
}

const BYTES_FIXED: &str = "5a41500002000000100000005e000000000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1ff0f1f2f3f4f5f6f7f8f9fafbfcfdfefff0f1f2f300000000080000000e00000066697865642d616e642d7461696c";

#[test]
fn fixed_width_arrays_sit_inline_beside_a_tail() {
    let hash: Vec<u8> = (0..32u8).collect();
    let addr: Vec<u8> = (0..20).map(|i| 0xF0 + (i % 16) as u8).collect();

    let mut b = Builder::with_version(256, VERSION_2);
    let mut ob = b.start_object(64);
    ob.set_bytes_fixed(0, &hash);
    ob.set_bytes_fixed(32, &addr);
    ob.set_text(56, "fixed-and-tail");
    ob.finish_as_root();
    let bytes = same_as_go("bytes_fixed", BYTES_FIXED, b.finish());

    let msg = Message::parse(bytes).unwrap();
    let r = msg.root();
    assert_eq!(r.bytes_fixed(0, 32), &hash[..]);
    assert_eq!(r.bytes_fixed(32, 20), &addr[..]);
    assert_eq!(r.text(56), "fixed-and-tail");
    // A fixed slice that runs off the end reads as empty, never a panic.
    assert!(r.bytes_fixed(56, 1024).is_empty());
    assert!(r.bytes_fixed(0, 0).is_empty());
}

const NESTED: &str = "5a4150000200000030000000460000000700000000000000efbefecacefaedfe08000000050000006368696c64000000e0ffffff000000000800000006000000706172656e74";

#[test]
fn a_parent_points_backward_at_a_child_written_first() {
    let mut b = Builder::with_version(256, VERSION_2);
    let mut cb = b.start_object(24);
    cb.set_uint32(0, 7);
    cb.set_uint64(8, 0xFEED_FACE_CAFE_BEEF);
    cb.set_text(16, "child");
    let child = cb.finish();

    let mut ob = b.start_object(16);
    ob.set_object(0, child);
    ob.set_text(8, "parent");
    ob.finish_as_root();
    let bytes = same_as_go("nested_object", NESTED, b.finish());

    let msg = Message::parse(bytes).unwrap();
    let r = msg.root();
    assert_eq!(r.text(8), "parent");
    let c = r.object(0);
    assert!(!c.is_null(), "the child must resolve through a negative offset");
    assert_eq!(c.uint32(0), 7);
    assert_eq!(c.uint64(8), 0xFEED_FACE_CAFE_BEEF);
    assert_eq!(c.text(16), "child");
    // An absent nested field is null, and null reads as zero throughout.
    let absent = r.object(4);
    assert!(absent.is_null());
    assert_eq!(absent.uint64(0), 0);
}

const LIST_U32: &str = "5a415000020000002800000038000000010000000200000003000000ffffffff2a00000000000000e8ffffff050000000500000000000000";

#[test]
fn uint32_list_encodes_and_reads_as_go() {
    let want = [1u32, 2, 3, 0xFFFF_FFFF, 42];
    let mut b = Builder::with_version(256, VERSION_2);
    let mut lb = b.start_list();
    for v in want {
        lb.add_uint32(v);
    }
    let (off, n) = lb.finish();
    let mut ob = b.start_object(16);
    ob.set_list(0, off, n);
    ob.set_uint32(8, n as u32);
    ob.finish_as_root();
    let bytes = same_as_go("list_uint32", LIST_U32, b.finish());

    let msg = Message::parse(bytes).unwrap();
    let l = msg.root().list(0);
    assert!(!l.is_null());
    assert_eq!(l.len(), 5);
    for (i, v) in want.iter().enumerate() {
        assert_eq!(l.uint32(i), *v);
    }
    // Past the end reads zero, as Go's does — never a panic, never a wrap.
    assert_eq!(l.uint32(5), 0);
    assert_eq!(l.uint32(usize::MAX), 0);
    // The stride-checked accessor accepts the honest frame unchanged.
    assert_eq!(msg.root().list_stride(0, 4).len(), 5);
}

const LIST_U64: &str = "5a41500002000000280000003000000000000000000000000100000000000000efcdab8967452301e8ffffff03000000";

#[test]
fn uint64_list_encodes_and_reads_as_go() {
    let want = [0u64, 1, 0x0123_4567_89AB_CDEF];
    let mut b = Builder::with_version(256, VERSION_2);
    let mut lb = b.start_list();
    for v in want {
        lb.add_uint64(v);
    }
    let (off, n) = lb.finish();
    let mut ob = b.start_object(8);
    ob.set_list(0, off, n);
    ob.finish_as_root();
    let bytes = same_as_go("list_uint64", LIST_U64, b.finish());

    let msg = Message::parse(bytes).unwrap();
    let l = msg.root().list(0);
    assert_eq!(l.len(), 3);
    for (i, v) in want.iter().enumerate() {
        assert_eq!(l.uint64(i), *v);
    }
}

const LIST_BYTES: &str = "5a4150000200000020000000280000007a61702d6279746573ff000000000000f0ffffff0a000000";

#[test]
fn a_byte_list_counts_bytes_not_calls() {
    let mut b = Builder::with_version(256, VERSION_2);
    let mut lb = b.start_list();
    lb.add_bytes(b"zap-bytes");
    lb.add_uint8(0xFF);
    let (off, n) = lb.finish();
    assert_eq!(n, 10, "nine bytes plus one is ten elements, not two");
    let mut ob = b.start_object(8);
    ob.set_list(0, off, n);
    ob.finish_as_root();
    let bytes = same_as_go("list_bytes", LIST_BYTES, b.finish());

    let msg = Message::parse(bytes).unwrap();
    let l = msg.root().list(0);
    assert_eq!(l.len(), 10);
    assert_eq!(l.bytes(), b"zap-bytes\xff");
    assert_eq!(l.uint8(0), b'z');
    assert_eq!(l.uint8(9), 0xFF);
}

const LIST_OBJ_PTR: &str = "5a41500002000000680000007000000064000000000000000800000006000000656c656d2d30000065000000000000000800000006000000656c656d2d31000066000000000000000800000006000000656c656d2d320000b8ffffffccffffffe0ffffff00000000f0ffffff04000000";

#[test]
fn a_repeated_message_field_is_a_pointer_array() {
    let mut b = Builder::with_version(512, VERSION_2);
    let mut offs = Vec::new();
    for i in 0..3u32 {
        let mut eb = b.start_object(16);
        eb.set_uint32(0, 100 + i);
        eb.set_text(8, &format!("elem-{i}"));
        offs.push(eb.finish());
    }
    let mut lb = b.start_list();
    for o in &offs {
        lb.add_object_ptr(*o);
    }
    lb.add_object_ptr(0); // an explicitly null element
    let (off, n) = lb.finish();

    let mut ob = b.start_object(8);
    ob.set_list(0, off, n);
    ob.finish_as_root();
    let bytes = same_as_go("list_object_ptr", LIST_OBJ_PTR, b.finish());

    let msg = Message::parse(bytes).unwrap();
    let l = msg.root().list(0);
    assert_eq!(l.len(), 4);
    for i in 0..3 {
        let e = l.object_ptr(i);
        assert!(!e.is_null());
        assert_eq!(e.uint32(0), 100 + i as u32);
        assert_eq!(e.text(8), format!("elem-{i}"));
    }
    assert!(l.object_ptr(3).is_null(), "the null element stays null");
    assert!(l.object_ptr(4).is_null(), "past the end is null, not a panic");
}

const NULLS: &str = "5a415000020000001000000028000000000000000000000000000000000000000000000000000000";

#[test]
fn absent_fields_encode_as_zero_and_read_as_absent() {
    let mut b = Builder::with_version(128, VERSION_2);
    let mut ob = b.start_object(24);
    ob.set_bytes(0, &[]);
    ob.set_object(8, 0);
    ob.set_list(12, 0, 0);
    ob.finish_as_root();
    let bytes = same_as_go("nulls", NULLS, b.finish());

    let msg = Message::parse(bytes).unwrap();
    let r = msg.root();
    assert!(r.bytes_field(0).is_empty());
    assert_eq!(r.text(0), "");
    assert!(r.object(8).is_null());
    let l = r.list(12);
    assert!(l.is_null());
    assert_eq!(l.len(), 0);
    assert!(l.is_empty());
    assert!(l.bytes().is_empty());
}

const FLAGGED: &str = "5a4150000200046410000000180000008877665544332211";

#[test]
fn the_flags_word_carries_type_above_and_flags_below() {
    let mut b = Builder::with_version(128, VERSION_2);
    let mut ob = b.start_object(8);
    ob.set_uint64(0, 0x1122_3344_5566_7788);
    ob.finish_as_root();
    let bytes = same_as_go("flagged", FLAGGED, b.finish_with_flags(100 << 8 | FLAG_SIGNED));

    let msg = Message::parse(bytes).unwrap();
    assert_eq!(msg.msg_type(), 100);
    assert_eq!(msg.flags() & 0xFF, FLAG_SIGNED);
    assert_eq!(msg.root().uint64(0), 0x1122_3344_5566_7788);
}

const WRITE_BYTES: &str = "5a4150000200000020000000300000007261772d626c6f622d7061796c6f6164f0ffffff000000001000000000000000";

#[test]
fn a_raw_blob_can_be_written_then_addressed_by_offset() {
    let mut b = Builder::with_version(256, VERSION_2);
    let blob = b.write_bytes(b"raw-blob-payload");
    let mut ob = b.start_object(16);
    ob.set_object(0, blob);
    ob.set_uint32(8, blob as u32);
    ob.finish_as_root();
    let bytes = same_as_go("write_bytes", WRITE_BYTES, b.finish());

    let msg = Message::parse(bytes).unwrap();
    let r = msg.root();
    assert_eq!(r.uint32(8), 16, "the blob sits right after the header");
    let at = r.object(0);
    assert!(!at.is_null());
    assert_eq!(at.offset(), 16);
    assert_eq!(&at.message_bytes()[16..32], b"raw-blob-payload");
    // Nothing to write is the null offset, not an empty span at the cursor.
    let mut b2 = Builder::with_version(64, VERSION_2);
    assert_eq!(b2.write_bytes(&[]), 0);
    assert_eq!(b2.write_text(""), 0);
}

// ── ACCEPT ──────────────────────────────────────────────────────────────
//
// Each frame below was fed to Go's reader; the boolean is what Go answered.

#[test]
fn an_honest_forward_pointer_is_read() {
    let m = Message::parse(unhex(
        "5a41500002000000100000002000000008000000080000005041594c4f414421",
    ))
    .unwrap();
    assert_eq!(m.root().bytes_field(0), b"PAYLOAD!");
}

#[test]
fn a_pointer_aimed_into_the_wire_header_is_refused() {
    // Go: nil. The relative offset is an unsigned forward pointer, so this
    // lands past the end and is refused. Read as signed it would resolve to
    // offset 0 and hand back the magic, version and flags as if they were a
    // payload — a peer choosing what another peer "received".
    let m = Message::parse(unhex(
        "5a415000020000001000000020000000f0ffffff080000005041594c4f414421",
    ))
    .unwrap();
    let got = m.root().bytes_field(0);
    assert!(got.is_empty(), "leaked {} header bytes as a payload", got.len());
    assert_ne!(got, ZAP_MAGIC);
}

#[test]
fn a_length_past_the_end_is_refused() {
    let m = Message::parse(unhex(
        "5a41500002000000100000002000000008000000f0ffffff5041594c4f414421",
    ))
    .unwrap();
    assert!(m.root().bytes_field(0).is_empty());
}

#[test]
fn a_list_declaring_four_billion_elements_is_refused() {
    // Go: null. Every element accessor would return zero anyway, so the harm
    // is not a bad read — it is the caller's `for i in 0..len()` running four
    // billion times.
    let m = Message::parse(unhex(
        "5a4150000200000018000000200000000100000002000000f8ffffffffffffff",
    ))
    .unwrap();
    let l = m.root().list(0);
    assert!(l.is_null());
    assert_eq!(l.len(), 0);
}

#[test]
fn a_wildly_long_list_is_refused_by_either_accessor() {
    // 4096 elements is already more than the whole 32-byte frame, so even the
    // permissive count clamp catches it.
    let m = Message::parse(unhex(
        "5a4150000200000018000000200000000100000002000000f8ffffff00100000",
    ))
    .unwrap();
    assert!(m.root().list(0).is_null());
    assert!(m.root().list_stride(0, 4).is_null());
}

#[test]
fn the_stride_hint_is_what_separates_the_two_accessors() {
    // The interesting frame: eight elements fit the buffer when counted, and
    // do not when measured. Go accepts this with the bare accessor and
    // refuses it with the stride told — so both answers must match, or the
    // stride hint is either useless or a false rejection.
    let frame = unhex("5a4150000200000018000000200000000100000002000000f8ffffff08000000");
    let m = Message::parse(frame).unwrap();

    let bare = m.root().list(0);
    assert!(!bare.is_null(), "Go accepts this frame bare");
    assert_eq!(bare.len(), 8);
    // The count is honest about the wire and dishonest about the buffer, so
    // elements past the end still read as zero rather than as neighbours.
    assert_eq!(bare.uint32(0), 1);
    assert_eq!(bare.uint32(7), 0);

    assert!(
        m.root().list_stride(0, 4).is_null(),
        "Go refuses this frame once told the elements are four bytes wide"
    );
    // A stride of zero means the caller does not know, and falls back.
    assert!(!m.root().list_stride(0, 0).is_null());
}

#[test]
fn a_nested_pointer_aimed_into_the_wire_header_is_refused() {
    // The child pointer was rewritten to resolve to offset 0. Nothing in the
    // header is an object payload, so the reference is refused outright
    // rather than yielding an object view over magic and version bytes.
    let m = Message::parse(unhex(
        "5a4150000200000018000000200000000900000000000000e8ffffff00000000",
    ))
    .unwrap();
    assert!(m.root().object(0).is_null());
}

// ── Frame-level bounds ──────────────────────────────────────────────────

#[test]
fn a_short_or_mislabelled_frame_is_refused_at_parse() {
    assert!(Message::parse(vec![]).is_err());
    assert!(Message::parse(vec![0u8; HEADER_SIZE - 1]).is_err());
    let mut bad_magic = unhex(NULLS);
    bad_magic[0] = b'X';
    assert!(Message::parse(bad_magic).is_err());
}

#[test]
fn reading_past_a_truncated_frame_yields_zero_not_a_panic() {
    // A peer that lies about its own length must not be able to crash a
    // reader. Every accessor is total.
    let full = unhex(SCALARS);
    for cut in HEADER_SIZE..full.len() {
        let msg = Message::parse(full[..cut].to_vec()).unwrap();
        let r = msg.root();
        let _ = (
            r.bool(0), r.uint8(1), r.uint16(2), r.uint32(4), r.uint64(8),
            r.int8(16), r.int16(18), r.int32(20), r.int64(24),
            r.float32(32), r.float64(40),
        );
        let _ = r.bytes_field(0);
        let _ = r.text(0);
        let _ = r.object(0);
        let l = r.list(0);
        let _ = (l.len(), l.bytes(), l.uint8(0), l.uint32(0), l.uint64(0));
        let _ = l.object_ptr(0);
        let _ = l.object(0, 8);
    }
}

// ── Correlation preamble ────────────────────────────────────────────────

#[test]
fn a_correlated_frame_round_trips_and_refuses_what_go_refuses() {
    let body = build_cloud_request("chat.completions", "Bearer tok", b"{}");
    let framed = wrap_correlated(0xDEAD_BEEF, REQ_FLAG_REQ, &body);

    // The preamble is request id then flag, both little-endian.
    assert_eq!(&framed[0..4], &0xDEAD_BEEFu32.to_le_bytes());
    assert_eq!(&framed[4..8], &REQ_FLAG_REQ.to_le_bytes());
    assert_eq!(&framed[CORRELATED_HEADER_SIZE..], &body[..]);

    let (id, flag, payload) = unwrap_correlated(&framed).expect("our own frame must read back");
    assert_eq!(id, 0xDEAD_BEEF);
    assert_eq!(flag, REQ_FLAG_REQ);
    assert_eq!(payload, &body[..]);

    // Go refuses a flag that is neither request nor response, so a caller
    // cannot be handed the body of a frame that was never a call.
    for bad in [0u32, 3, 0xFFFF_FFFF] {
        let mut f = framed.clone();
        f[4..8].copy_from_slice(&bad.to_le_bytes());
        assert!(unwrap_correlated(&f).is_none(), "flag {bad} must be refused");
    }
    // Short of the preamble there is nothing to read.
    for n in 0..CORRELATED_HEADER_SIZE {
        assert!(unwrap_correlated(&framed[..n]).is_none());
    }
    // A preamble with an empty body is still a valid frame shape.
    let empty = wrap_correlated(1, REQ_FLAG_RESP, &[]);
    assert_eq!(unwrap_correlated(&empty), Some((1, REQ_FLAG_RESP, &[][..])));
}

// ── Builder reuse ───────────────────────────────────────────────────────

#[test]
fn a_reset_builder_emits_the_same_bytes_as_a_fresh_one() {
    // The pooled path is only safe if reuse cannot leak the previous
    // message's bytes into the next one's padding or reserved spans.
    let mut b = Builder::with_version(512, VERSION_2);
    let mut noisy = b.start_object(64);
    noisy.set_bytes_fixed(0, &[0xEE; 64]);
    noisy.set_text(0, "discarded");
    noisy.finish_as_root();

    b.reset();
    let mut ob = b.start_object(24);
    ob.set_bytes(0, &[]);
    ob.set_object(8, 0);
    ob.set_list(12, 0, 0);
    ob.finish_as_root();
    same_as_go("nulls after reset", NULLS, b.finish());
}
