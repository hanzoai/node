//! What this validator has already signed, on disk, before it signs it.
//!
//! A validator may make one statement per [`Slot`]. Making two is equivocation:
//! it is what a slashing rule punishes and what a fork is built out of. In one
//! process a map is enough. Across a restart it is not — a node that forgets
//! comes back willing to contradict itself, and the network cannot tell the
//! amnesia from the attack.
//!
//! WRITE FIRST, SIGN SECOND. [`Journal::admit`] appends and flushes to the
//! device before the caller is allowed to sign. A crash between the two leaves
//! a statement recorded that was never made, which costs one vote; the opposite
//! order would leave a statement made that was never recorded, which costs the
//! stake. Fail closed means erring toward the vote.
//!
//! THE RECORD IS THE SIGNED BYTES. Each entry is one [`VOTE_MESSAGE_LEN`]-byte
//! canonical vote message, exactly as signed — not a digest of it, not the
//! fields to rebuild it from. The slot is read back out of the message by the
//! same function a receiver uses, so the journal cannot key an entry under a
//! slot the message does not claim. There is one derivation and one place it
//! happens.
//!
//! Entries are fixed width, so a torn tail is arithmetic rather than a guess: a
//! file whose length is not a multiple of the record size ends in a write that
//! never completed, and under write-first ordering that write is a statement
//! never signed. It is dropped, and the file is cut back to the last whole
//! record so the next append lands on the boundary.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::Path;

use lux_consensus::finality::VOTE_MESSAGE_LEN;
use lux_consensus::vote::Slot;

/// One entry: the exact bytes signed, and nothing else.
pub const ENTRY: usize = VOTE_MESSAGE_LEN;

/// Why a statement was not admitted. Every variant means the caller must not
/// sign; there is no variant that means "probably fine".
#[derive(Debug)]
pub enum Refused {
    /// Not this network's signed bytes: the tag, version, role, length or
    /// accept byte disowns them.
    NotAStatement,
    /// This validator already said something else at this slot. The statement
    /// it stands by is carried, because the caller's only correct move is to
    /// re-send that one.
    Equivocation(Box<[u8]>),
    /// The record did not reach the device. Signing anyway is signing something
    /// a restart will not remember.
    NotDurable(io::Error),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::NotAStatement => f.write_str("not a canonical vote message"),
            Refused::Equivocation(_) => {
                f.write_str("a different statement is already signed at this slot")
            }
            Refused::NotDurable(e) => write!(f, "the record did not reach the device: {e}"),
        }
    }
}

impl std::error::Error for Refused {}

/// This validator's standing statements, durable.
pub struct Journal {
    file: File,
    said: BTreeMap<Slot, Box<[u8]>>,
    /// A failed write could not be cut back: the file's end is unknown, so
    /// nothing more is written to it by this process. A reopen reads it back
    /// to its last whole entry.
    broken: bool,
}

impl Journal {
    /// Open the journal at `path`, replaying what is already there.
    ///
    /// A torn final entry is dropped and the file truncated to the boundary.
    /// Two whole entries at one slot cannot both be trusted, so the file is
    /// refused rather than half-believed — that is corruption, not a crash, and
    /// guessing which one this validator actually sent is the one thing a
    /// signing record must never do.
    ///
    /// ONE PROCESS AT A TIME. The file is held under an exclusive `flock`
    /// for as long as the journal is open: two processes signing from one
    /// record would each believe the other's statements were never made.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Journal> {
        // `truncate(false)` is stated rather than left to the default, because
        // the default is the one that empties a signing record on open.
        //
        // 0600 for the same reason the keys are: this file is what stops this
        // validator equivocating, so anyone who can WRITE it can make it forget
        // what it already said, and the mode is set as the file is created
        // rather than after — a record that was briefly world-writable has been
        // world-writable.
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path.as_ref())?;
        // SAFETY: flock on a descriptor this function owns; it reads no
        // memory, and the lock goes with the descriptor.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                format!("{} is held by another process", path.as_ref().display()),
            ));
        }
        let mut raw = Vec::new();
        file.read_to_end(&mut raw)?;

        let whole = raw.len() - raw.len() % ENTRY;
        if whole != raw.len() {
            file.set_len(whole as u64)?;
        }

        let mut said: BTreeMap<Slot, Box<[u8]>> = BTreeMap::new();
        let (entries, _) = raw[..whole].as_chunks::<ENTRY>();
        for entry in entries {
            let slot = Slot::read(entry).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "journal holds a statement this network disowns",
                )
            })?;
            if let Some(standing) = said.insert(slot, entry[..].into()) {
                if standing.as_ref() != entry {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "journal holds two statements at one slot",
                    ));
                }
            }
        }

        file.seek(SeekFrom::End(0))?;
        Ok(Journal { file, said, broken: false })
    }

    /// Statements standing.
    pub fn len(&self) -> usize {
        self.said.len()
    }

    pub fn is_empty(&self) -> bool {
        self.said.is_empty()
    }

    /// Whether a failed write could not be cut back: this journal writes
    /// nothing more, and only a reopen — a restart — reads it back whole.
    pub fn broken(&self) -> bool {
        self.broken
    }

    /// What this validator has said at `slot`, if anything.
    pub fn standing(&self, slot: &Slot) -> Option<&[u8]> {
        self.said.get(slot).map(|m| m.as_ref())
    }

    /// Admit a statement, or refuse it.
    ///
    /// Returns once the statement is on the device. Admitting the identical
    /// statement twice is not equivocation and is not an error: a signer that
    /// crashed after recording and before sending has to be able to send it,
    /// and a retry that fails is a validator that cannot vote.
    pub fn admit(&mut self, message: &[u8]) -> Result<(), Refused> {
        let slot = Slot::read(message).map_err(|_| Refused::NotAStatement)?;
        if let Some(standing) = self.said.get(&slot) {
            return if standing.as_ref() == message {
                Ok(())
            } else {
                Err(Refused::Equivocation(standing.clone()))
            };
        }
        if self.broken {
            return Err(Refused::NotDurable(io::Error::other(
                "an earlier write could not be cut back; reopen to go on",
            )));
        }
        // Write, then flush to the device, then remember. An entry that is not
        // durable is not admitted, so a failure here leaves this validator free
        // to sign at this slot on the next attempt — and leaves none of it in
        // the file: part of an entry left behind would put every later entry
        // out of step with the boundaries a reopen reads.
        let end = self.file.seek(SeekFrom::End(0)).map_err(Refused::NotDurable)?;
        if let Err(e) = self.file.write_all(message).and_then(|()| self.file.sync_data()) {
            self.undo(end);
            return Err(Refused::NotDurable(e));
        }
        self.said.insert(slot, message.into());
        Ok(())
    }

    /// Cut the file back to `end`, the last whole entry, and write from there
    /// — or, when that fails too, write nothing more.
    fn undo(&mut self, end: u64) {
        let cut = self.file.set_len(end).and_then(|()| self.file.seek(SeekFrom::Start(end)));
        if cut.is_err() {
            self.broken = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lux_consensus::finality::{canonical_vote_message, Position};

    fn statement(height: u64, block: u8) -> Vec<u8> {
        let position =
            Position { chain_id: [0xc1; 32], height, block_id: [block; 32], ..Default::default() };
        canonical_vote_message(&position, true)
    }

    #[test]
    fn a_write_that_failed_part_way_leaves_nothing_behind() {
        let dir = std::env::temp_dir().join(format!("lux-journal-undo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        let path = dir.join("votes");
        let mut j = Journal::open(&path).expect("open");
        j.admit(&statement(1, 0xa1)).expect("height 1");

        // What a write that failed part way leaves: some of an entry.
        let end = j.file.seek(SeekFrom::End(0)).expect("end");
        j.file.write_all(&statement(2, 0xb2)[..ENTRY / 3]).expect("a partial entry");
        j.undo(end);

        // The next statement lands on the boundary, and a reopen reads both.
        j.admit(&statement(2, 0xc3)).expect("height 2, another block");
        drop(j);
        let j = Journal::open(&path).expect("reopen");
        assert_eq!(j.len(), 2);
        let slot = Slot::read(&statement(2, 0xc3)).expect("slot");
        assert_eq!(j.standing(&slot), Some(&statement(2, 0xc3)[..]), "no torn entry before it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_second_process_on_one_journal_is_refused() {
        let dir = std::env::temp_dir().join(format!("lux-journal-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        let path = dir.join("votes");
        let held = Journal::open(&path).expect("open");
        // A second open is a second open file description, which is what a
        // second process holds.
        let err = Journal::open(&path).err().expect("a second holder is refused");
        assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
        drop(held);
        Journal::open(&path).expect("let go, it opens");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_journal_that_cannot_be_cut_back_writes_nothing_more() {
        let dir = std::env::temp_dir().join(format!("lux-journal-broken-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        let path = dir.join("votes");
        let mut j = Journal::open(&path).expect("open");
        j.admit(&statement(1, 0xa1)).expect("height 1");
        j.broken = true;
        assert!(matches!(j.admit(&statement(2, 0xb2)), Err(Refused::NotDurable(_))));
        assert_eq!(std::fs::metadata(&path).expect("stat").len() as usize, ENTRY);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
