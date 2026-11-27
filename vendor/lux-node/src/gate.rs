// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! The heights this node accepted without a certificate, kept on disk.
//!
//! An RLP import accepts blocks no quorum spoke about. A node holding them may
//! serve them, and may not build or sign on top of them until a peer's
//! certificate anchors the imported tip ([`crate::engine::Engine`] builds
//! nothing and signs nothing while this gate stands). That is a fact about the data
//! directory, not about one run of the process, so it lives in the directory:
//! a node restarted without `--import-chain-data` holds the same unagreed
//! history it held with it.
//!
//! ONE FILE, ONE NUMBER. `<data>/uncertified` holds the first height accepted
//! without a certificate, in decimal. While it stands, every height from there
//! to the tip is history this node read rather than history it saw agreed: an
//! import is the only way a block is accepted without a certificate, and the
//! first block accepted ON one lifts the gate. So the file does not repeat the
//! tip — the chain already states it, and a crash part-way through an import
//! would leave a written copy behind it.
//!
//! WRITTEN BEFORE THE FIRST UNCERTIFIED BLOCK IS ACCEPTED, and removed only
//! after a certified block on top of the imported tip is. A crash anywhere in
//! between leaves the gate standing, which costs a wait for the next
//! certificate; the other orders leave uncertified history with no gate, which
//! costs a signature on a chain nobody agreed to.
//!
//! STATED, NEVER INFERRED. A chain with blocks and no certificates is also
//! what every seat of a network that ran hanzod 0.2.2 holds — it recorded no
//! certificates at all. Reading the gate off that shape would halt such a
//! network on upgrade. The gate is what the import says, and nothing else.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// The largest file this reads back. A height is at most twenty digits.
const LARGEST: u64 = 32;

/// Whether this node holds history no quorum certified, and from where.
pub struct Gate {
    /// Where the gate is kept. None for a node that keeps nothing across a
    /// restart.
    path: Option<PathBuf>,
    /// The heights held without a certificate, when there are any.
    held: Mutex<Option<(u64, u64)>>,
}

impl Gate {
    /// A gate that is not kept across a restart. For a chain that keeps
    /// nothing either.
    pub fn memory() -> Gate {
        Gate { path: None, held: Mutex::new(None) }
    }

    /// The gate kept at `path`, as it was left, on a chain whose tip is `tip`.
    ///
    /// Absent is open. Present is closed, from the height it names to `tip`.
    /// Present and not a height is refused: this node wrote nothing else
    /// there, and a gate that could not be read is not one to guess open.
    pub fn open(path: impl Into<PathBuf>, tip: u64) -> io::Result<Gate> {
        let path = path.into();
        let held = match File::open(&path) {
            Ok(f) => {
                let mut text = String::new();
                f.take(LARGEST).read_to_string(&mut text)?;
                let from = height(&text).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("{} does not name a height", path.display()),
                    )
                })?;
                Some((from, tip))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        Ok(Gate { path: Some(path), held: Mutex::new(held) })
    }

    fn lock(&self) -> MutexGuard<'_, Option<(u64, u64)>> {
        self.held.lock().expect("the gate lock is not poisoned")
    }

    /// The heights held without a certificate, if any.
    pub fn held(&self) -> Option<(u64, u64)> {
        *self.lock()
    }

    /// Close the gate over heights `from` through `to`, or widen it to them.
    ///
    /// Returns once the gate is on the device. Call it BEFORE the block at
    /// `from` is accepted: a block accepted with no gate behind it is one a
    /// crash leaves this node free to sign on. Only a lower first height is a
    /// write — the end of the run is the tip, which the chain keeps — so an
    /// import pays for this once, not once a block.
    pub fn hold(&self, from: u64, to: u64) -> io::Result<()> {
        let mut held = self.lock();
        let was = *held;
        let now = match was {
            Some((f, t)) => (f.min(from), t.max(to)),
            None => (from, to),
        };
        // Closed in memory first, so a write that fails leaves this process
        // refusing too — whatever its caller then does.
        *held = Some(now);
        if let Some(path) = &self.path {
            if was.map(|(f, _)| f) != Some(now.0) {
                write(path, now.0)?;
            }
        }
        Ok(())
    }

    /// Open the gate: a certified block now stands on the history it covered.
    ///
    /// Returns once the removal is on the device, and leaves the gate closed
    /// when it is not — a gate that reopens after a crash has only cost a
    /// wait for the next certificate.
    pub fn lift(&self) -> io::Result<()> {
        let mut held = self.lock();
        if held.is_none() {
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
        *held = None;
        Ok(())
    }
}

/// A height as this node writes one: decimal digits and a newline, above the
/// genesis — block zero is never imported.
fn height(text: &str) -> Option<u64> {
    let digits = text.strip_suffix('\n')?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().filter(|h| *h > 0)
}

/// Put `from` at `path` whole or not at all: a new file, synced, renamed over
/// the old one, and the rename synced. A reader finds the old gate or the new
/// one, never a torn one.
fn write(path: &Path, from: u64) -> io::Result<()> {
    let next = path.with_extension("new");
    let mut f =
        OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&next)?;
    f.write_all(format!("{from}\n").as_bytes())?;
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
        let dir = std::env::temp_dir().join(format!("lux-gate-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    #[test]
    fn a_gate_outlives_the_process_that_closed_it() {
        let dir = scratch("outlives");
        let path = dir.join("uncertified");
        let gate = Gate::open(&path, 0).expect("open");
        assert_eq!(gate.held(), None, "absent is open");
        gate.hold(5, 5).expect("hold");
        gate.hold(5, 9).expect("widen");
        assert_eq!(gate.held(), Some((5, 9)));
        assert_eq!(std::fs::read_to_string(&path).expect("kept"), "5\n");
        drop(gate);

        // The tip is the end of the run, whatever the file was last told.
        let again = Gate::open(&path, 12).expect("reopen");
        assert_eq!(again.held(), Some((5, 12)));
        again.lift().expect("lift");
        assert_eq!(again.held(), None);
        assert!(!path.exists(), "lifted on the device, not only in memory");
        assert_eq!(Gate::open(&path, 12).expect("reopen").held(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_gate_that_cannot_be_read_is_not_guessed_open() {
        let dir = scratch("unreadable");
        let path = dir.join("uncertified");
        for bad in ["", "\n", "5", "0\n", "+5\n", "5 9\n", "five\n", "99999999999999999999\n"] {
            std::fs::write(&path, bad).expect("write");
            assert!(Gate::open(&path, 3).is_err(), "{bad:?} was read as a gate");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_gate_that_cannot_be_written_is_still_closed_here() {
        let gate =
            Gate { path: Some("/nonexistent/lux/uncertified".into()), held: Mutex::new(None) };
        assert!(gate.hold(1, 1).is_err(), "the write is reported");
        assert_eq!(gate.held(), Some((1, 1)), "and this process refuses anyway");
    }
}
