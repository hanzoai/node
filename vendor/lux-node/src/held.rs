// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! The block a validator signed, kept until its height is decided.
//!
//! A validator's one signature at a height reaches the device before its key
//! is used ([`crate::journal`]). The block it signs reaches the device before
//! that, here. So a validator that restarts bound at an open height — every
//! seat at once, in an upgrade — says its signature again AND offers the block
//! again, and the height is never stranded on a block nobody holds. Go's clone
//! recovery re-signs a binding whose block its chain still tracks
//! (`emitConvergedVote`, integration.go:1018); this is what makes that true of
//! a chain that keeps an undecided block only in memory.
//!
//! ONE FILE, ONE PROPOSAL. `<data>/held` is the signed proposal as the link
//! carries it. A validator signs once a height and heights are decided in
//! order, so only the latest can still be open. Written whole or not at all —
//! a new file, synced, renamed over the old one, the rename synced — and
//! removed once the height it names is decided.
//!
//! A FILE THAT IS NOT A PROPOSAL IS SET ASIDE, NOT OBEYED. This file is what
//! lets a restart offer a block again; the journal is what stops a second
//! signature. So a damaged one — a copied directory, a failing disk — costs
//! the re-offer and nothing else: it is moved to `<data>/held.damaged`, the
//! node says so, and it starts.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use crate::mesh::{Proposal, LINK_MAX};

/// The proposal a validator signed, when its height may still be open.
pub struct Held {
    /// Where it is kept. None for a node that keeps nothing across a restart.
    path: Option<PathBuf>,
    kept: Mutex<Option<Proposal>>,
    /// Where a file found here that was not a proposal was set aside.
    damaged: Option<PathBuf>,
}

impl Held {
    /// Kept in memory only. For a chain that keeps nothing either.
    pub fn memory() -> Held {
        Held { path: None, kept: Mutex::new(None), damaged: None }
    }

    /// What is kept at `path`, as it was left.
    ///
    /// Absent is nothing kept. Present and not a proposal is nothing kept
    /// either: the file is set aside ([`Held::damaged`] says where) and the
    /// node starts. Only a disk that cannot be read is an error.
    pub fn open(path: impl Into<PathBuf>) -> io::Result<Held> {
        let path = path.into();
        let raw = match File::open(&path) {
            Ok(f) => {
                let mut raw = Vec::new();
                f.take(u64::from(LINK_MAX) + 1).read_to_end(&mut raw)?;
                raw
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(Held { path: Some(path), kept: Mutex::new(None), damaged: None })
            }
            Err(e) => return Err(e),
        };
        let (kept, damaged) = match Proposal::decode([0; 20], raw) {
            Some(p) => (Some(p), None),
            None => {
                let aside = path.with_extension("damaged");
                let moved = std::fs::rename(&path, &aside).and_then(|()| sync_dir(&path));
                (None, Some(if moved.is_ok() { aside } else { path.clone() }))
            }
        };
        Ok(Held { path: Some(path), kept: Mutex::new(kept), damaged })
    }

    /// Where a file that was not a proposal was set aside when this opened.
    pub fn damaged(&self) -> Option<&Path> {
        self.damaged.as_deref()
    }

    fn lock(&self) -> MutexGuard<'_, Option<Proposal>> {
        self.kept.lock().expect("the held lock is not poisoned")
    }

    /// The proposal kept for `height`, if that is the height it names.
    pub fn get(&self, height: u64) -> Option<Proposal> {
        self.lock().clone().filter(|p| p.height == height)
    }

    /// Keep `p`, in place of what was kept. Returns once it is on the device:
    /// call it BEFORE signing the block it carries.
    pub fn keep(&self, p: &Proposal) -> io::Result<()> {
        let mut kept = self.lock();
        if kept.as_ref() == Some(p) {
            return Ok(());
        }
        if let Some(path) = &self.path {
            write(path, &p.encode())?;
        }
        *kept = Some(p.clone());
        Ok(())
    }

    /// The heights below `height` are decided: let go of what was kept for
    /// one of them.
    pub fn decided(&self, height: u64) -> io::Result<()> {
        let mut kept = self.lock();
        if kept.as_ref().is_none_or(|p| p.height >= height) {
            return Ok(());
        }
        if let Some(path) = &self.path {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            sync_dir(path)?;
        }
        *kept = None;
        Ok(())
    }
}

/// Put `bytes` at `path` whole or not at all.
fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let next = path.with_extension("new");
    let mut f =
        OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&next)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&next, path)?;
    sync_dir(path)
}

/// Sync the directory `path` is in, so a name created, renamed or removed
/// there is on the device.
fn sync_dir(path: &Path) -> io::Result<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    File::open(dir)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("lux-held-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir.join("held")
    }

    fn proposal(height: u64, fill: u8) -> Proposal {
        Proposal {
            from: [0; 20],
            height,
            slot: 3,
            signature: vec![fill; crate::pq::IDENTITY_SIGNATURE_LEN],
            block: vec![fill; 900],
        }
    }

    #[test]
    fn what_was_kept_is_there_after_a_restart_until_its_height_is_decided() {
        let path = scratch("restart");
        assert!(Held::open(&path).expect("absent").get(1).is_none(), "absent is nothing kept");
        Held::open(&path).expect("open").keep(&proposal(7, 0xa1)).expect("keep");

        let again = Held::open(&path).expect("reopen");
        assert_eq!(again.get(7), Some(proposal(7, 0xa1)), "the proposal, as it was signed");
        assert_eq!(again.get(8), None, "only for the height it names");
        again.keep(&proposal(8, 0xb2)).expect("the next height's");
        assert_eq!(Held::open(&path).expect("reopen").get(8), Some(proposal(8, 0xb2)));

        again.decided(8).expect("height 8 still open");
        assert!(Held::open(&path).expect("reopen").get(8).is_some(), "an open height keeps it");
        again.decided(9).expect("height 8 decided");
        assert!(!path.exists(), "a decided height's block is let go");
        assert!(Held::open(&path).expect("reopen").get(8).is_none());
    }

    #[test]
    fn a_file_that_is_not_a_proposal_is_set_aside_and_nothing_is_kept() {
        let path = scratch("garbage");
        let whole = proposal(4, 0xc3).encode();
        for bad in [&b""[..], b"\xf9\x10\x00 a bare block", &[0u8; 4096][..], &whole[..900]] {
            std::fs::write(&path, bad).expect("write");
            let held = Held::open(&path).expect("a damaged file does not keep a node down");
            assert_eq!(held.get(4), None, "{:?} was read as a proposal", &bad[..bad.len().min(8)]);
            let aside = held.damaged().expect("said where it went");
            assert_eq!(std::fs::read(aside).expect("set aside"), bad, "kept as it was found");
            assert!(!path.exists(), "and out of the way");
            held.keep(&proposal(4, 0xc3)).expect("the next one is kept as ever");
            assert_eq!(Held::open(&path).expect("reopen").get(4), Some(proposal(4, 0xc3)));
            std::fs::remove_file(&path).expect("clean");
        }
    }
}
