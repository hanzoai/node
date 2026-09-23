// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! The blocks this validator has accepted, on disk, in order.
//!
//! The chain's state lives in memory, so a validator rebuilds it at start by
//! accepting these again. A record is the block's length as four big-endian
//! bytes, then the block's own bytes. Each record reaches the device before the
//! block is accepted, so no accepted block is missing from this file. A record
//! cut short by a crash is a block that was never accepted, and it is dropped.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;

pub struct Blocks {
    file: File,
}

impl Blocks {
    /// Open the log at `path`, creating it if absent, with every whole record
    /// it holds, oldest first.
    pub fn open(path: &Path) -> io::Result<(Self, Vec<Vec<u8>>)> {
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(path)?;
        let mut all = Vec::new();
        file.read_to_end(&mut all)?;

        let mut records = Vec::new();
        let mut at = 0;
        while all.len() - at >= 4 {
            let n = u32::from_be_bytes([all[at], all[at + 1], all[at + 2], all[at + 3]]) as usize;
            if all.len() - at - 4 < n {
                break;
            }
            records.push(all[at + 4..at + 4 + n].to_vec());
            at += 4 + n;
        }
        if at < all.len() {
            file.set_len(at as u64)?;
            file.sync_data()?;
        }
        Ok((Self { file }, records))
    }

    /// Append one block, and return once it has reached the device.
    pub fn append(&mut self, raw: &[u8]) -> io::Result<()> {
        let n = u32::try_from(raw.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a block over 4 GiB"))?;
        let mut record = Vec::with_capacity(4 + raw.len());
        record.extend_from_slice(&n.to_be_bytes());
        record.extend_from_slice(raw);
        self.file.write_all(&record)?;
        self.file.sync_data()
    }
}

/// A fresh log path under the system temp directory, one per call, because
/// tests run in parallel.
#[cfg(test)]
pub(crate) fn scratch(name: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("hanzod-{name}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir.join("blocks")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_was_appended_is_read_back_in_order() {
        let path = scratch("order");
        let (mut log, held) = Blocks::open(&path).expect("open");
        assert!(held.is_empty());
        for raw in [&b"one"[..], b"", b"three"] {
            log.append(raw).expect("append");
        }
        drop(log);
        let (_, held) = Blocks::open(&path).expect("reopen");
        assert_eq!(held, vec![b"one".to_vec(), Vec::new(), b"three".to_vec()]);
    }

    #[test]
    fn a_torn_last_record_is_dropped_and_the_log_goes_on() {
        let path = scratch("torn");
        let (mut log, _) = Blocks::open(&path).expect("open");
        log.append(b"whole").expect("append");
        drop(log);
        // A crash mid-write: a length promising more bytes than follow it.
        let mut f = OpenOptions::new().append(true).open(&path).expect("raw");
        f.write_all(&[0, 0, 0, 9, b'h', b'a']).expect("tear");
        drop(f);

        let (mut log, held) = Blocks::open(&path).expect("reopen");
        assert_eq!(held, vec![b"whole".to_vec()]);
        log.append(b"next").expect("append after a tear");
        drop(log);
        let (_, held) = Blocks::open(&path).expect("reopen again");
        assert_eq!(held, vec![b"whole".to_vec(), b"next".to_vec()]);
    }
}
