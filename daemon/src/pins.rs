//! The pinned checksum table: the sha256 of every program hanzod will execute.
//!
//! `daemon/pins.json` is compiled in, as the CLI compiles its own digests
//! (`link/zt.rs`). A program with no pin for this target is refused, not run:
//! floating binaries never start. `programs` maps a program name to
//! `{ "<target>": "<sha256 of the executable>" }`.
//!
//! A second table, `<data>/hanzo/pins.json`, holds digests the user's own
//! installer wrote (`hanzo upgrade` after it verified a release). It is read
//! only if the file is owned by the user and writable by nobody else, and it
//! can only add programs, never replace one the compiled table pins.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

pub const COMPILED: &str = include_str!("../pins.json");

pub struct Pins {
    compiled: Value,
    local: Value,
}

pub fn target() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        o => o,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        a => a,
    };
    format!("{os}-{arch}")
}

impl Pins {
    pub fn load(local: &Path) -> Pins {
        Pins {
            compiled: serde_json::from_str(COMPILED).unwrap_or(Value::Null),
            local: read_local(local).unwrap_or(Value::Null),
        }
    }

    #[cfg(test)]
    pub fn of(compiled: &str, local: &str) -> Pins {
        Pins { compiled: serde_json::from_str(compiled).unwrap(), local: serde_json::from_str(local).unwrap() }
    }

    fn digest(&self, name: &str) -> Option<String> {
        let t = target();
        let find = |v: &Value| v.get("programs")?.get(name)?.get(&t)?.as_str().map(str::to_owned);
        find(&self.compiled).or_else(|| find(&self.local))
    }

    /// Check that the file at `path` is the program pinned as `name`.
    pub fn verify(&self, name: &str, path: &Path) -> Result<(), String> {
        let want = self.digest(name).ok_or_else(|| format!("no pinned digest for {name} on {}", target()))?;
        let got = sha256_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if got != want {
            return Err(format!("{} does not match the pinned digest of {name}", path.display()));
        }
        Ok(())
    }
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(unix)]
fn read_local(path: &Path) -> Option<Value> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let m = std::fs::metadata(path).ok()?;
    let mine = unsafe { libc::geteuid() };
    if m.uid() != mine || m.permissions().mode() & 0o022 != 0 {
        return None;
    }
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

#[cfg(not(unix))]
fn read_local(_: &Path) -> Option<Value> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unpinned_or_altered_program_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("prog");
        std::fs::write(&p, b"binary").unwrap();
        let none = Pins::of("{}", "{}");
        assert!(none.verify("cloud", &p).unwrap_err().contains("no pinned digest"));
        let digest = sha256_file(&p).unwrap();
        let pinned = Pins::of(&format!(r#"{{"programs":{{"cloud":{{"{}":"{digest}"}}}}}}"#, target()), "{}");
        assert!(pinned.verify("cloud", &p).is_ok());
        std::fs::write(&p, b"binary!").unwrap();
        assert!(pinned.verify("cloud", &p).unwrap_err().contains("does not match"));
    }

    #[test]
    fn the_local_table_adds_but_never_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("prog");
        std::fs::write(&p, b"x").unwrap();
        let d = sha256_file(&p).unwrap();
        let t = target();
        let compiled = format!(r#"{{"programs":{{"a":{{"{t}":"{}"}}}}}}"#, "0".repeat(64));
        let local = format!(r#"{{"programs":{{"a":{{"{t}":"{d}"}},"b":{{"{t}":"{d}"}}}}}}"#);
        let pins = Pins::of(&compiled, &local);
        assert!(pins.verify("a", &p).is_err(), "the compiled pin wins");
        assert!(pins.verify("b", &p).is_ok());
    }
}
