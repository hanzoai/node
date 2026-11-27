// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! Every accepted block's certificate, on disk.
//!
//! A certificate is the only evidence a block was accepted. Nobody re-votes a
//! decided height, so a validator that fell behind is caught up by being handed
//! each block it missed TOGETHER WITH the certificate the network already
//! assembled for it — and that is only possible if the validators that were
//! there kept what they accepted on. This file is where they keep it.
//!
//! KEYED BY HEIGHT AND BLOCK ID. The block lives with the chain, stored however
//! the chain stores it; the proof lives here. The two are joined on the pair:
//! a peer is served a block only where the chain's id at that height is the id
//! recorded here, so a certificate for a block the chain does not hold, or a
//! block no certificate here names, is served as nothing.
//!
//! GO'S ENCODING, BYTE FOR BYTE. A certificate is written the way
//! `luxfi/consensus engine/chain/cert_codec.go` writes it, so what is on disk
//! and on the wire is the form a Go validator reads, and [`decode`] is exactly
//! as strict as `UnmarshalQuorumCert`: every short read refused, no trailing
//! bytes, the vote count capped against the buffer before anything is
//! reserved, and an accept byte that is neither 0 nor 1 refused. Decoding is
//! structure only — whether the certificate proves anything is
//! `QuorumCert::verify_weighted`'s question, asked by the engine.
//!
//! WRITTEN BEFORE THE BLOCK IS ACCEPTED, as the journal is written before the
//! key is used. A crash between the two leaves a certificate for a block this
//! node never accepted, which costs nothing: it is served only where the chain
//! agrees. The other order leaves an accepted block no file can prove.
//!
//! One record:
//!
//! ```text
//! length        4   big-endian: height ‖ id ‖ certificate
//! height        8
//! block id     32
//! certificate   …   cert_codec.go
//! check         8   the first 8 bytes of SHA-256 over height ‖ id ‖ certificate
//! ```
//!
//! A torn tail is the last write, cut short: a record that runs past the end of
//! the file, or a tail of zeros. It was never acknowledged, so it is dropped and
//! the file cut back to the last whole record. A bad record anywhere else is not
//! a crash, it is corruption, and the file is refused rather than half-believed.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, Read, Write};
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use lux_consensus::cert::{NodeId, QuorumCert, Vote};
use lux_consensus::finality::{Finality, Position};
use sha2::{Digest, Sha256};

use crate::vm::Id;

/// The largest certificate this node writes or reads: a mebibyte, which is some
/// eight thousand votes. A committee is bounded long before that, and a length
/// above it is a lie a torn write cannot tell.
pub const CERT_MAX: usize = 1 << 20;

/// Height and block id, ahead of the certificate in a record.
const KEY: usize = 8 + 32;

/// The check at the end of a record.
const CHECK: usize = 8;

/// The fixed part of an encoded certificate, ahead of its votes: `qcHeaderSize`.
const CERT_HEADER: usize = 2 + 1 + 1 + 32 + 8 + 4 + 32 * 7 + 4 + 4;

/// The fixed part of one encoded vote — node id, accept byte, signature length:
/// `qcVoteFixed`. What the vote count is capped against.
const VOTE_FIXED: usize = 20 + 1 + 4;

/// Bytes that are not a certificate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Malformed(pub &'static str);

impl std::fmt::Display for Malformed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "not a certificate: {}", self.0)
    }
}

impl std::error::Error for Malformed {}

/// A certificate as `QuorumCert.MarshalBinary` writes it.
pub fn encode(cert: &QuorumCert) -> Vec<u8> {
    let p = &cert.position;
    let votes: usize = cert.votes.iter().map(|v| VOTE_FIXED + v.signature.len()).sum();
    let mut out = Vec::with_capacity(CERT_HEADER + votes);
    out.extend_from_slice(&cert.version.to_be_bytes());
    out.push(cert.qc_type);
    out.push(cert.tier as u8);
    out.extend_from_slice(&p.chain_id);
    out.extend_from_slice(&p.height.to_be_bytes());
    out.extend_from_slice(&p.round.to_be_bytes());
    out.extend_from_slice(&p.block_id);
    out.extend_from_slice(&p.parent_id);
    out.extend_from_slice(&p.canonical_id);
    out.extend_from_slice(&p.parent_canonical_id);
    out.extend_from_slice(&p.execution_state_root);
    out.extend_from_slice(&p.payload_root);
    out.extend_from_slice(&p.validator_set_root);
    out.extend_from_slice(&cert.threshold.to_be_bytes());
    out.extend_from_slice(&(cert.votes.len() as u32).to_be_bytes());
    for v in &cert.votes {
        out.extend_from_slice(&v.node_id);
        out.push(u8::from(v.accept));
        out.extend_from_slice(&(v.signature.len() as u32).to_be_bytes());
        out.extend_from_slice(&v.signature);
    }
    out
}

/// A certificate out of the bytes [`encode`] writes, or a refusal.
pub fn decode(raw: &[u8]) -> Result<QuorumCert, Malformed> {
    let mut r = Cursor(raw);
    let version = u16::from_be_bytes(r.take()?);
    let qc_type = r.u8()?;
    let tier = match r.u8()? {
        0 => Finality::Photon,
        1 => Finality::Wave,
        2 => Finality::Nova,
        3 => Finality::Quasar,
        4 => Finality::Horizon,
        _ => return Err(Malformed("a tier no rung carries")),
    };
    let position = Position {
        chain_id: r.take()?,
        height: u64::from_be_bytes(r.take()?),
        round: u32::from_be_bytes(r.take()?),
        block_id: r.take()?,
        parent_id: r.take()?,
        canonical_id: r.take()?,
        parent_canonical_id: r.take()?,
        execution_state_root: r.take()?,
        payload_root: r.take()?,
        validator_set_root: r.take()?,
    };
    let threshold = u32::from_be_bytes(r.take()?);
    let count = u32::from_be_bytes(r.take()?) as usize;
    // Capped against what is left BEFORE anything is reserved: a count whose
    // smallest possible footprint is larger than the buffer is a lie, refused
    // in constant time rather than allocated for.
    if count.saturating_mul(VOTE_FIXED) > r.0.len() {
        return Err(Malformed("more votes than bytes to hold them"));
    }
    let mut votes = Vec::with_capacity(count);
    for _ in 0..count {
        let node_id: NodeId = r.take()?;
        let accept = match r.u8()? {
            0 => false,
            1 => true,
            // The encoder writes 0 or 1. A third value decoding as true would be
            // two byte strings naming one certificate.
            _ => return Err(Malformed("an accept byte that is neither 0 nor 1")),
        };
        let len = u32::from_be_bytes(r.take()?) as usize;
        let signature = r.bytes(len)?.to_vec();
        votes.push(Vote { node_id, accept, signature });
    }
    if !r.0.is_empty() {
        return Err(Malformed("trailing bytes"));
    }
    Ok(QuorumCert { version, qc_type, tier, position, threshold, votes })
}

/// A bounds-checked reader. Every read refuses a short buffer; nothing panics.
struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], Malformed> {
        if self.0.len() < n {
            return Err(Malformed("short"));
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], Malformed> {
        Ok(self.bytes(N)?.try_into().expect("N bytes"))
    }

    fn u8(&mut self) -> Result<u8, Malformed> {
        Ok(self.take::<1>()?[0])
    }
}

/// Where one certificate sits in the file.
#[derive(Clone, Copy, Debug)]
struct Entry {
    id: Id,
    at: u64,
    len: u32,
}

struct Held {
    file: File,
    /// The end of the last whole record — where the next one goes.
    end: u64,
    index: BTreeMap<u64, Entry>,
}

/// The certificates this node accepted blocks on, durable.
pub struct Ledger {
    held: Mutex<Held>,
}

impl Ledger {
    /// Open the ledger at `path`, reading back what is already there.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Ledger> {
        // 0600 and `truncate(false)` for the journal's reasons: whoever can
        // write this file can make the node serve a certificate it never saw,
        // and the default of the other flag empties it.
        let file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)?;
        let size = file.metadata()?.len();

        let mut index: BTreeMap<u64, Entry> = BTreeMap::new();
        let mut reader = BufReader::new(&file);
        let mut at = 0u64;
        while at < size {
            match next(&mut reader, at, size)? {
                Found::Whole { height, id, len, end } => {
                    // Two blocks certified at one height cannot both be right,
                    // and the file does not get to choose which to serve.
                    if let Some(e) = index.get(&height) {
                        if e.id != id {
                            return Err(corrupt("the ledger holds two blocks at one height"));
                        }
                    } else {
                        index.insert(height, Entry { id, at: at + 4 + KEY as u64, len });
                    }
                    at = end;
                }
                Found::Torn => break,
            }
        }
        drop(reader);
        if at < size {
            file.set_len(at)?;
        }
        Ok(Ledger { held: Mutex::new(Held { file, end: at, index }) })
    }

    fn lock(&self) -> MutexGuard<'_, Held> {
        self.held.lock().expect("the ledger lock is not poisoned")
    }

    /// Record the certificates blocks were accepted on: each entry is a
    /// height, the block accepted there, and its certificate.
    ///
    /// Returns once every record is on the device — one write and one sync for
    /// the lot, so a run of blocks taken together costs the device once. All or
    /// nothing: if any entry is refused, none is written.
    ///
    /// The same block again is not an error and writes nothing — a node that
    /// restarts re-accepts what it already holds proof of. A DIFFERENT block at
    /// a recorded height is refused: two certificates at one height is a fork,
    /// and this file is the last place that should paper over one.
    pub fn record(&self, entries: &[(u64, Id, &QuorumCert)]) -> io::Result<()> {
        let mut held = self.lock();
        let at = held.end;
        let mut out = Vec::new();
        let mut added: Vec<(u64, Entry)> = Vec::new();
        for (height, block, cert) in entries {
            let already = held
                .index
                .get(height)
                .or_else(|| added.iter().find(|(h, _)| h == height).map(|(_, e)| e));
            if let Some(e) = already {
                if e.id == *block {
                    continue;
                }
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "a different block is already certified at this height",
                ));
            }
            let cert = encode(cert);
            if cert.len() > CERT_MAX {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "certificate too large"));
            }
            let start = at + out.len() as u64;
            let mut body = Vec::with_capacity(KEY + cert.len());
            body.extend_from_slice(&height.to_be_bytes());
            body.extend_from_slice(block);
            body.extend_from_slice(&cert);
            out.extend_from_slice(&(body.len() as u32).to_be_bytes());
            out.extend_from_slice(&body);
            out.extend_from_slice(&check(&body));
            added.push((
                *height,
                Entry { id: *block, at: start + 4 + KEY as u64, len: cert.len() as u32 },
            ));
        }
        if out.is_empty() {
            return Ok(());
        }
        let wrote = held.file.write_all(&out).and_then(|_| held.file.sync_data());
        if let Err(e) = wrote {
            // A write that did not reach the device leaves nothing behind, so
            // the next record lands on a boundary and not after a fragment.
            let _ = held.file.set_len(at);
            return Err(e);
        }
        held.index.extend(added);
        held.end = at + out.len() as u64;
        Ok(())
    }

    /// The block recorded at `height` and the certificate it was accepted on,
    /// as `cert_codec.go` bytes.
    pub fn get(&self, height: u64) -> Option<(Id, Vec<u8>)> {
        let held = self.lock();
        let e = *held.index.get(&height)?;
        let mut cert = vec![0u8; e.len as usize];
        held.file.read_exact_at(&mut cert, e.at).ok()?;
        Some((e.id, cert))
    }

    /// The highest height a certificate is recorded at.
    pub fn tip(&self) -> Option<u64> {
        self.lock().index.keys().next_back().copied()
    }

    /// Certificates recorded.
    pub fn len(&self) -> usize {
        self.lock().index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn check(body: &[u8]) -> [u8; CHECK] {
    Sha256::digest(body)[..CHECK].try_into().expect("8 bytes")
}

fn corrupt(why: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why)
}

enum Found {
    Whole {
        height: u64,
        id: Id,
        len: u32,
        end: u64,
    },
    /// The last write, cut short.
    Torn,
}

/// Read the record at `at`.
fn next(r: &mut impl Read, at: u64, size: u64) -> io::Result<Found> {
    let rest = size - at;
    if rest < 4 {
        return Ok(Found::Torn);
    }
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as u64;

    // Zero is no record anyone wrote, and a filesystem that persisted a size
    // before the data shows exactly that. It is a torn tail only if it runs
    // to the end of the file, which is what a tail of zeros does.
    if len == 0 {
        let mut tail = Vec::new();
        r.read_to_end(&mut tail)?;
        return if tail.iter().all(|b| *b == 0) {
            Ok(Found::Torn)
        } else {
            Err(corrupt("a record of no length inside the ledger"))
        };
    }
    // Longer than any certificate is not a write that was cut short — a
    // prefix of a real record carries a real length.
    if len > (KEY + CERT_MAX) as u64 || (len as usize) < KEY + CERT_HEADER {
        return Err(corrupt("a record length no writer produces"));
    }
    let end = at + 4 + len + CHECK as u64;
    if end > size {
        return Ok(Found::Torn);
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body)?;
    let mut sum = [0u8; CHECK];
    r.read_exact(&mut sum)?;
    if sum != check(&body) {
        // The last record is the only one a crash can reach, because each is
        // on the device before the next is begun.
        return if end == size {
            Ok(Found::Torn)
        } else {
            Err(corrupt("a record in the ledger does not match its check"))
        };
    }
    let height = u64::from_be_bytes(body[..8].try_into().expect("8 bytes"));
    let id: Id = body[8..KEY].try_into().expect("32 bytes");
    decode(&body[KEY..]).map_err(|_| corrupt("a record in the ledger is not a certificate"))?;
    Ok(Found::Whole { height, id, len: (len as usize - KEY) as u32, end })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory that removes itself.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new() -> Scratch {
            static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("lux-ledger-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("scratch");
            Scratch(dir)
        }

        fn path(&self) -> std::path::PathBuf {
            self.0.join("certs")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn cert(height: u64, block: u8, votes: u8) -> QuorumCert {
        QuorumCert {
            version: 3,
            qc_type: 1,
            tier: Finality::Quasar,
            position: Position {
                chain_id: [0xc1; 32],
                height,
                round: 0,
                block_id: [block; 32],
                parent_id: [0xb2; 32],
                canonical_id: [block; 32],
                parent_canonical_id: [0xb2; 32],
                execution_state_root: [0xe5; 32],
                payload_root: [0x70; 32],
                validator_set_root: [0x88; 32],
            },
            threshold: 3,
            votes: (0..votes)
                .map(|i| Vote { node_id: [i; 20], accept: true, signature: vec![i; 96] })
                .collect(),
        }
    }

    #[test]
    fn the_encoding_is_the_one_go_writes() {
        // Written out field by field from cert_codec.go's layout comment, not
        // by calling `encode`, so a drift in the body fails here.
        let c = cert(9, 0xb1, 2);
        let mut want = Vec::new();
        want.extend_from_slice(&3u16.to_be_bytes()); // version
        want.push(1); // type
        want.push(3); // tier: Quasar
        want.extend_from_slice(&[0xc1; 32]); // chain
        want.extend_from_slice(&9u64.to_be_bytes()); // height
        want.extend_from_slice(&0u32.to_be_bytes()); // round
        want.extend_from_slice(&[0xb1; 32]); // block id
        want.extend_from_slice(&[0xb2; 32]); // parent id
        want.extend_from_slice(&[0xb1; 32]); // canonical id
        want.extend_from_slice(&[0xb2; 32]); // parent canonical id
        want.extend_from_slice(&[0xe5; 32]); // state root
        want.extend_from_slice(&[0x70; 32]); // payload root
        want.extend_from_slice(&[0x88; 32]); // set root
        want.extend_from_slice(&3u32.to_be_bytes()); // threshold
        want.extend_from_slice(&2u32.to_be_bytes()); // votes
        for i in 0..2u8 {
            want.extend_from_slice(&[i; 20]);
            want.push(1);
            want.extend_from_slice(&96u32.to_be_bytes());
            want.extend_from_slice(&[i; 96]);
        }
        assert_eq!(CERT_HEADER, 280, "qcHeaderSize");
        assert_eq!(encode(&c), want);
        assert_eq!(decode(&want).expect("decodes"), c, "and it reads back as itself");
    }

    #[test]
    fn bytes_that_are_not_a_certificate_are_refused() {
        let good = encode(&cert(1, 0xb1, 3));
        assert!(decode(&good[..good.len() - 1]).is_err(), "short");
        let mut long = good.clone();
        long.push(0);
        assert!(decode(&long).is_err(), "trailing bytes");
        let mut accept = good.clone();
        accept[CERT_HEADER + 20] = 2;
        assert!(decode(&accept).is_err(), "an accept byte of 2");
        let mut tier = good.clone();
        tier[3] = 9;
        assert!(decode(&tier).is_err(), "a tier no rung carries");
        // A header announcing four billion votes over a buffer holding none:
        // refused before anything is reserved for them.
        let mut greedy = good[..CERT_HEADER].to_vec();
        greedy[CERT_HEADER - 4..].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(decode(&greedy), Err(Malformed("more votes than bytes to hold them")));
        assert!(decode(&[]).is_err());
    }

    #[test]
    fn a_recorded_certificate_is_there_after_a_restart() {
        let s = Scratch::new();
        {
            let l = Ledger::open(s.path()).expect("open");
            assert!(l.is_empty());
            l.record(&[(1, [0xb1; 32], &cert(1, 0xb1, 3))]).expect("record");
            l.record(&[(2, [0xb2; 32], &cert(2, 0xb2, 4))]).expect("record");
        }
        let l = Ledger::open(s.path()).expect("reopen");
        assert_eq!(l.len(), 2);
        assert_eq!(l.tip(), Some(2));
        let (id, raw) = l.get(2).expect("height 2");
        assert_eq!(id, [0xb2; 32]);
        assert_eq!(decode(&raw).expect("a certificate"), cert(2, 0xb2, 4));
        assert!(l.get(3).is_none());
    }

    #[test]
    fn one_height_holds_one_block() {
        let s = Scratch::new();
        let l = Ledger::open(s.path()).expect("open");
        l.record(&[(1, [0xb1; 32], &cert(1, 0xb1, 3))]).expect("record");
        let size = std::fs::metadata(s.path()).expect("stat").len();
        l.record(&[(1, [0xb1; 32], &cert(1, 0xb1, 3))])
            .expect("the same block again is not an error");
        assert_eq!(std::fs::metadata(s.path()).expect("stat").len(), size, "and writes nothing");
        assert!(
            l.record(&[(1, [0xee; 32], &cert(1, 0xee, 3))]).is_err(),
            "a second block is a fork"
        );
        assert_eq!(l.get(1).expect("height 1").0, [0xb1; 32]);
    }

    #[test]
    fn a_run_is_recorded_whole_or_not_at_all() {
        let s = Scratch::new();
        let l = Ledger::open(s.path()).expect("open");
        l.record(&[(1, [0xb1; 32], &cert(1, 0xb1, 3))]).expect("record");
        let size = std::fs::metadata(s.path()).expect("stat").len();
        let (c2, c3, fork) = (cert(2, 0xb2, 3), cert(3, 0xb3, 3), cert(1, 0xee, 3));
        assert!(
            l.record(&[(2, [0xb2; 32], &c2), (3, [0xb3; 32], &c3), (1, [0xee; 32], &fork)])
                .is_err(),
            "one fork in a run refuses the run"
        );
        assert_eq!(l.len(), 1, "and nothing of it is kept");
        assert_eq!(std::fs::metadata(s.path()).expect("stat").len(), size, "or written");
        assert!(
            l.record(&[(2, [0xb2; 32], &c2), (2, [0xee; 32], &fork)]).is_err(),
            "nor two blocks at one height inside a run"
        );
        l.record(&[(2, [0xb2; 32], &c2), (3, [0xb3; 32], &c3)]).expect("a run");
        drop(l);
        let l = Ledger::open(s.path()).expect("reopen");
        assert_eq!((l.len(), l.tip()), (3, Some(3)));
    }

    #[test]
    fn a_torn_last_record_is_dropped_and_the_file_cut_back() {
        let s = Scratch::new();
        {
            let l = Ledger::open(s.path()).expect("open");
            l.record(&[(1, [0xb1; 32], &cert(1, 0xb1, 3))]).expect("record");
            l.record(&[(2, [0xb2; 32], &cert(2, 0xb2, 3))]).expect("record");
        }
        let whole = std::fs::metadata(s.path()).expect("stat").len();
        let one = {
            let raw = std::fs::read(s.path()).expect("read");
            4 + u32::from_be_bytes(raw[..4].try_into().expect("4")) as u64 + CHECK as u64
        };
        // Every prefix of the second record reads back as the first alone.
        for cut in [one + 1, one + 3, one + 40, whole - 1] {
            let f = OpenOptions::new().write(true).open(s.path()).expect("open");
            f.set_len(cut).expect("cut");
            drop(f);
            let l = Ledger::open(s.path()).expect("a torn tail is not corruption");
            assert_eq!(l.len(), 1, "cut at {cut}");
            assert_eq!(std::fs::metadata(s.path()).expect("stat").len(), one, "cut back");
            // and the next record lands on the boundary
            l.record(&[(2, [0xb2; 32], &cert(2, 0xb2, 3))]).expect("record");
            drop(l);
            assert_eq!(Ledger::open(s.path()).expect("reopen").len(), 2);
        }
        // A tail of zeros — a size persisted ahead of its data — is torn too.
        let f = OpenOptions::new().append(true).open(s.path()).expect("open");
        (&f).write_all(&[0u8; 700]).expect("zeros");
        drop(f);
        assert_eq!(Ledger::open(s.path()).expect("zeros are a torn tail").len(), 2);
        assert_eq!(std::fs::metadata(s.path()).expect("stat").len(), whole);
    }

    #[test]
    fn a_damaged_record_with_records_after_it_is_refused() {
        let s = Scratch::new();
        {
            let l = Ledger::open(s.path()).expect("open");
            l.record(&[(1, [0xb1; 32], &cert(1, 0xb1, 3))]).expect("record");
            l.record(&[(2, [0xb2; 32], &cert(2, 0xb2, 3))]).expect("record");
        }
        let mut raw = std::fs::read(s.path()).expect("read");
        raw[60] ^= 0xff; // inside the first record's certificate
        std::fs::write(s.path(), &raw).expect("write");
        let refused = Ledger::open(s.path()).err().expect("corruption is not a crash");
        assert_eq!(refused.kind(), io::ErrorKind::InvalidData);
    }
}
