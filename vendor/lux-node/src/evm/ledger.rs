// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! The accepted chain, on disk.
//!
//! Two files in the chain's directory, both a run of records, each record four
//! big-endian bytes of length and then that many bytes:
//!
//! - `blocks` holds every accepted block after the genesis, in height order, as
//!   the bytes [`Block::bytes`](crate::vm::Block::bytes) returns. It is the
//!   chain: a record reaches the device before the block is accepted, so no
//!   accepted block is missing from it. This is the format hanzod 0.2.2 writes,
//!   so a directory it wrote opens here as it is.
//!
//!   History is the exception, and it says so on the device. A run of records
//!   written without waiting — an import's, which nothing outside the process
//!   sees until it is synced — begins by putting `blocks.loose` beside the file:
//!   the offset the run starts at, synced before the first of its records is
//!   written. The sync that ends the run removes it. So a reopen that finds it
//!   knows where records may be torn or missing after a crash, and reads the
//!   chain up to the first that is not whole; before that offset, as everywhere
//!   without one, a damaged record is damage and refuses the chain.
//! - `receipts` holds, per block, the RLP list of its receipts. It is DERIVED:
//!   opening a chain re-executes every block, compares what execution produced
//!   with what this file holds, and rewrites the file from the first record
//!   that differs or is missing. So it is written without waiting for the
//!   device, and losing it costs a rewrite, never a block.
//!
//! A record is written at the offset where the last whole record ends, never
//! appended blind. A write that fails is cut back off, so the next record lands
//! on a boundary; a sync that fails leaves the file refusing every later write,
//! because after a failed `fsync` the kernel may report a later one clean for
//! data that never arrived.

use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::{DirBuilderExt, FileExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::path::Path;

/// Where a record is: the offset of its length prefix, and its payload's size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Span {
    pub at: u64,
    pub len: u32,
}

/// One file of records, or the same held in memory for a chain that keeps
/// nothing across a restart.
pub(super) struct Records {
    backing: Backing,
    /// Where the last whole record ends. The next one is written here.
    end: u64,
    /// Whether a write waits for the device before it counts.
    durable: bool,
    /// A write could not be undone, or a sync failed. Nothing after it could be
    /// trusted to read back, so nothing more is written until a reopen reads
    /// the file again from the device.
    broken: bool,
    /// Where the open run of unwaited writes begins, and the file beside this
    /// one that says so. None for a file kept in memory.
    loose: Option<(Option<u64>, std::path::PathBuf)>,
}

enum Backing {
    Disk(File),
    Memory(Vec<u8>),
}

impl Records {
    fn memory() -> Self {
        Self {
            backing: Backing::Memory(Vec::new()),
            end: 0,
            durable: false,
            broken: false,
            loose: None,
        }
    }

    /// Open the file at `path`, creating it if absent. Nothing is read yet:
    /// [`Records::scan`] is how its records come back, and `end` stays at zero
    /// until a scan has found where they stop.
    fn open(path: &Path, durable: bool) -> io::Result<Self> {
        // `truncate(false)` is stated because the other default empties a
        // chain. 0600 because whoever can write this file chooses what this
        // node replays.
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)?;
        let marker = path.with_extension("loose");
        let from = match std::fs::read(&marker) {
            Ok(b) => Some(u64::from_be_bytes(b.try_into().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} is not an offset", marker.display()),
                )
            })?)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        Ok(Self {
            backing: Backing::Disk(file),
            end: 0,
            durable,
            broken: false,
            loose: Some((from, marker)),
        })
    }

    /// Where a run of records written without waiting began, if one was open
    /// when the file was last written: past it, a crash may have left records
    /// torn or missing.
    pub fn loose(&self) -> Option<u64> {
        self.loose.as_ref().and_then(|(from, _)| *from)
    }

    /// Wait for the device to hold every record written, and end the run of
    /// unwaited ones.
    pub fn sync(&mut self) -> io::Result<()> {
        let (Backing::Disk(f), Some((from @ Some(_), marker))) = (&self.backing, &mut self.loose)
        else {
            return Ok(());
        };
        if let Err(e) = f.sync_data() {
            self.broken = true;
            return Err(e);
        }
        match std::fs::remove_file(&*marker) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        sync_dir(marker)?;
        *from = None;
        Ok(())
    }

    /// Read the records from the start, one at a time.
    pub fn scan(&self) -> io::Result<Scan> {
        match &self.backing {
            Backing::Disk(f) => {
                let mut f = f.try_clone()?;
                let size = f.seek(SeekFrom::End(0))?;
                f.seek(SeekFrom::Start(0))?;
                Ok(Scan { source: Some(BufReader::new(f)), size, at: 0 })
            }
            Backing::Memory(_) => Ok(Scan { source: None, size: 0, at: 0 }),
        }
    }

    /// Cut the file at `at`, a record boundary, and write from there on.
    pub fn cut(&mut self, at: u64) -> io::Result<()> {
        match &mut self.backing {
            Backing::Disk(f) => {
                if f.metadata()?.len() != at {
                    f.set_len(at)?;
                    if self.durable {
                        f.sync_data()?;
                    }
                }
            }
            Backing::Memory(v) => v.truncate(at as usize),
        }
        self.end = at;
        Ok(())
    }

    /// Write one record where the last whole one ends. With `wait`, and on a
    /// durable file, it is on the device before this returns — and so is every
    /// record before it. Without, it is part of a run [`Records::sync`] ends.
    pub fn append(&mut self, payload: &[u8], wait: bool) -> io::Result<Span> {
        if self.broken {
            return Err(io::Error::other(
                "an earlier write to this file could not be undone; it takes nothing more until it is reopened",
            ));
        }
        let len = u32::try_from(payload.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a record over 4 GiB"))?;
        let at = self.end;
        match &mut self.backing {
            Backing::Memory(v) => {
                v.extend_from_slice(&len.to_be_bytes());
                v.extend_from_slice(payload);
            }
            Backing::Disk(f) => {
                // The run is marked on the device before its first record is
                // written, so no unwaited record can be on the device without
                // the mark that says it may be torn.
                if self.durable && !wait {
                    if let Some((from @ None, marker)) = &mut self.loose {
                        mark(marker, at)?;
                        *from = Some(at);
                    }
                }
                let mut record = Vec::with_capacity(4 + payload.len());
                record.extend_from_slice(&len.to_be_bytes());
                record.extend_from_slice(payload);
                if let Err(e) = f.write_all_at(&record, at) {
                    // Part of it may be there. Cut it off so the next record
                    // lands on a boundary; if even that fails, stop writing.
                    if f.set_len(at).is_err() {
                        self.broken = true;
                    }
                    return Err(e);
                }
            }
        }
        if self.durable && wait {
            self.sync_all()?;
        }
        self.end = at + 4 + u64::from(len);
        Ok(Span { at, len })
    }

    /// A waited write: this record and every one before it on the device, and
    /// any open run ended by it.
    fn sync_all(&mut self) -> io::Result<()> {
        if self.loose().is_some() {
            return self.sync();
        }
        if let Backing::Disk(f) = &self.backing {
            if let Err(e) = f.sync_data() {
                self.broken = true;
                return Err(e);
            }
        }
        Ok(())
    }

    /// The payload of the record at `span`.
    pub fn read(&self, span: Span) -> io::Result<Vec<u8>> {
        let from = span.at + 4;
        let to = from + u64::from(span.len);
        if to > self.end {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "past the last record"));
        }
        match &self.backing {
            Backing::Memory(v) => Ok(v[from as usize..to as usize].to_vec()),
            Backing::Disk(f) => {
                let mut out = vec![0u8; span.len as usize];
                f.read_exact_at(&mut out, from)?;
                Ok(out)
            }
        }
    }
}

/// Put `offset` at `marker`, whole and on the device, name and all.
fn mark(marker: &Path, offset: u64) -> io::Result<()> {
    let mut f =
        OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(marker)?;
    std::io::Write::write_all(&mut f, &offset.to_be_bytes())?;
    f.sync_all()?;
    sync_dir(marker)
}

/// Sync the directory `path` is in, so a name created or removed there is on
/// the device.
fn sync_dir(path: &Path) -> io::Result<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    File::open(dir)?.sync_all()
}

/// A pass over a file's records, oldest first.
pub(super) struct Scan {
    source: Option<BufReader<File>>,
    size: u64,
    at: u64,
}

impl Scan {
    /// The next whole record, or `None` where the whole records stop — the end
    /// of the file, or a record cut short by a crash.
    pub fn next(&mut self) -> io::Result<Option<(Span, Vec<u8>)>> {
        let Some(source) = self.source.as_mut() else {
            return Ok(None);
        };
        let left = self.size - self.at;
        if left < 4 {
            return Ok(None);
        }
        let mut prefix = [0u8; 4];
        source.read_exact(&mut prefix)?;
        let len = u32::from_be_bytes(prefix);
        // A length is four bytes read back from disk; allocating what it says
        // before knowing the bytes are there would let a torn prefix ask for
        // four gigabytes.
        if left - 4 < u64::from(len) {
            self.source = None;
            return Ok(None);
        }
        let mut payload = vec![0u8; len as usize];
        source.read_exact(&mut payload)?;
        let span = Span { at: self.at, len };
        self.at += 4 + u64::from(len);
        Ok(Some((span, payload)))
    }

    /// Whether the record just returned is the last thing in the file.
    pub fn at_end(&self) -> bool {
        self.at == self.size
    }

    /// Stop reading, so the file can be cut and written from here.
    pub fn stop(&mut self) {
        self.source = None;
    }
}

/// The chain's two files.
pub(super) struct Ledger {
    pub blocks: Records,
    pub receipts: Records,
}

/// Second handles on the chain's two files, for reading records without the
/// lock that guards writing them. A whole record never moves and never
/// changes once written, so reading one by its span needs nothing else.
pub(super) struct Files {
    blocks: File,
    receipts: File,
}

impl Files {
    /// The payload of the block record at `span`.
    pub fn block(&self, span: Span) -> io::Result<Vec<u8>> {
        whole(&self.blocks, span)
    }

    /// The payload of the receipts record at `span`.
    pub fn receipts(&self, span: Span) -> io::Result<Vec<u8>> {
        whole(&self.receipts, span)
    }

    /// `len` bytes of the block record at `span`, from `skip` bytes into its
    /// payload — refused, before anything is allocated, past the record's end
    /// or the file's.
    pub fn block_part(&self, span: Span, skip: u64, len: usize) -> io::Result<Vec<u8>> {
        let end = skip.checked_add(len as u64).ok_or_else(past)?;
        if end > u64::from(span.len) {
            return Err(past());
        }
        within(&self.blocks, span)?;
        let mut out = vec![0u8; len];
        self.blocks.read_exact_at(&mut out, span.at + 4 + skip)?;
        Ok(out)
    }
}

fn past() -> io::Error {
    io::Error::new(io::ErrorKind::UnexpectedEof, "past the record")
}

/// Refuse `span` unless the whole record is in `file`. A span is read back out
/// of a length the file itself holds, so this is the file checked against
/// itself — before a length from disk decides what is allocated.
fn within(file: &File, span: Span) -> io::Result<()> {
    let end = span.at.checked_add(4 + u64::from(span.len)).ok_or_else(past)?;
    if end > file.metadata()?.len() {
        return Err(past());
    }
    Ok(())
}

fn whole(file: &File, span: Span) -> io::Result<Vec<u8>> {
    within(file, span)?;
    let mut out = vec![0u8; span.len as usize];
    file.read_exact_at(&mut out, span.at + 4)?;
    Ok(out)
}

impl Ledger {
    /// Second handles on both files, or none for a ledger kept in memory.
    pub fn files(&self) -> io::Result<Option<Files>> {
        match (&self.blocks.backing, &self.receipts.backing) {
            (Backing::Disk(blocks), Backing::Disk(receipts)) => {
                Ok(Some(Files { blocks: blocks.try_clone()?, receipts: receipts.try_clone()? }))
            }
            _ => Ok(None),
        }
    }

    /// A ledger that keeps nothing across a restart.
    pub fn memory() -> Self {
        Self { blocks: Records::memory(), receipts: Records::memory() }
    }

    /// Open the ledger in `dir`, creating the directory and both files if
    /// they are absent, and take the chain's lock.
    ///
    /// THE LOCK. Two processes writing one ledger each write where THEY think
    /// the last record ends, and the second overwrites the first without
    /// either noticing. An exclusive `flock` on `blocks` makes the second open
    /// fail instead. It is held by the open file, so it goes when the chain is
    /// dropped or the process dies, and a crash never leaves it stuck.
    pub fn open(dir: &Path) -> io::Result<Self> {
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        let fresh = !dir.join("blocks").exists();
        let blocks = Records::open(&dir.join("blocks"), true)?;
        if let Backing::Disk(f) = &blocks.backing {
            // SAFETY: flock on a descriptor this function owns; it reads no
            // memory.
            if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                let e = io::Error::last_os_error();
                return Err(if e.kind() == io::ErrorKind::WouldBlock {
                    io::Error::new(
                        io::ErrorKind::WouldBlock,
                        format!("another process has the chain at {} open", dir.display()),
                    )
                } else {
                    e
                });
            }
        }
        let receipts = Records::open(&dir.join("receipts"), false)?;
        if fresh {
            // A created file is only durable once its directory entry is.
            File::open(dir)?.sync_all()?;
        }
        Ok(Self { blocks, receipts })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn a_record_reads_back_by_its_span_and_by_a_scan() {
        let dir = crate::evm::tests_support::scratch("records");
        let mut l = Ledger::open(&dir).expect("open");
        let a = l.blocks.append(b"one", true).expect("append");
        let b = l.blocks.append(b"", true).expect("append");
        let c = l.blocks.append(b"three", true).expect("append");
        assert_eq!(l.blocks.read(a).expect("read"), b"one");
        assert_eq!(l.blocks.read(b).expect("read"), b"");
        assert_eq!(l.blocks.read(c).expect("read"), b"three");
        drop(l);

        let l = Ledger::open(&dir).expect("reopen");
        let mut s = l.blocks.scan().expect("scan");
        let mut seen = Vec::new();
        while let Some((span, payload)) = s.next().expect("read") {
            seen.push((span, payload));
        }
        assert_eq!(
            seen,
            vec![(a, b"one".to_vec()), (b, Vec::new()), (c, b"three".to_vec())],
            "the same records, at the same offsets"
        );
    }

    #[test]
    fn a_run_of_unwaited_records_is_marked_on_the_device_until_it_is_synced() {
        let dir = crate::evm::tests_support::scratch("loose");
        let marker = dir.join("blocks.loose");
        let mut l = Ledger::open(&dir).expect("open");
        l.blocks.append(b"one", true).expect("append");
        assert_eq!((l.blocks.loose(), marker.exists()), (None, false), "a waited write is no run");
        l.blocks.append(b"two", false).expect("append");
        l.blocks.append(b"three", false).expect("append");
        assert_eq!(l.blocks.loose(), Some(7), "the run starts where its first record does");
        assert_eq!(std::fs::read(&marker).expect("marked"), 7u64.to_be_bytes());
        drop(l);

        // A crash is a reopen that finds the mark.
        let mut l = Ledger::open(&dir).expect("reopen");
        assert_eq!(l.blocks.loose(), Some(7));
        l.blocks.sync().expect("sync");
        assert_eq!((l.blocks.loose(), marker.exists()), (None, false), "a sync ends the run");
        drop(l);

        // A waited write after unwaited ones puts them all on the device.
        let mut l = Ledger::open(&dir).expect("reopen");
        l.blocks.cut(std::fs::metadata(dir.join("blocks")).expect("blocks").len()).expect("end");
        l.blocks.append(b"four", false).expect("append");
        assert!(marker.exists());
        l.blocks.append(b"five", true).expect("append");
        assert_eq!((l.blocks.loose(), marker.exists()), (None, false));
    }

    #[test]
    fn a_second_open_of_one_chain_is_refused() {
        let dir = crate::evm::tests_support::scratch("flock");
        let held = Ledger::open(&dir).expect("open");
        let again = Ledger::open(&dir).err().expect("a second writer is refused");
        assert_eq!(again.kind(), io::ErrorKind::WouldBlock);
        drop(held);
        Ledger::open(&dir).expect("and allowed once the first is gone");
    }

    #[test]
    fn a_torn_length_prefix_does_not_allocate_what_it_claims() {
        let dir = crate::evm::tests_support::scratch("prefix");
        let l = Ledger::open(&dir).expect("open");
        drop(l);
        let mut f = OpenOptions::new().append(true).open(dir.join("blocks")).expect("raw");
        f.write_all(&[0xff, 0xff, 0xff, 0xff, 1, 2, 3]).expect("a length promising 4 GiB");
        drop(f);
        let l = Ledger::open(&dir).expect("reopen");
        let mut s = l.blocks.scan().expect("scan");
        assert_eq!(s.next().expect("read"), None, "a torn record is where the records stop");
    }
}
