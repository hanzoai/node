//! The action ledger: an append-only record of every privileged action
//! (a worker or child started, an identity switched, a chain turned on).
//! One JSON line per action with a monotonic id, written 0600 and never
//! rewritten. No token, key or bearer is ever written to it.

use serde_json::json;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

pub struct Ledger {
    path: PathBuf,
    next: Mutex<u64>,
}

impl Ledger {
    pub fn open(path: PathBuf) -> Ledger {
        let next = std::fs::read_to_string(&path).map(|s| s.lines().count() as u64 + 1).unwrap_or(1);
        Ledger { path, next: Mutex::new(next) }
    }

    /// Record `action` by `who` on `what`. Returns the ledger id.
    pub fn record(&self, who: &str, action: &str, what: &str) -> u64 {
        let mut next = self.next.lock().unwrap();
        let id = *next;
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let line = json!({ "id": id, "at": at, "who": who, "action": action, "what": what }).to_string();
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        if let Ok(mut f) = opts.open(&self.path) {
            let _ = writeln!(f, "{line}");
            *next += 1;
        }
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_monotonic_across_reopen() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("ledger");
        let l = Ledger::open(p.clone());
        assert_eq!(l.record("hanzo/z", "spawn", "cloud"), 1);
        assert_eq!(l.record("hanzo/z", "spawn", "dev"), 2);
        assert_eq!(Ledger::open(p).record("hanzo/z", "stop", "dev"), 3);
    }
}
