// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! What one validator holds, and the line it publishes.
//!
//! ONE KEY, AND THE NAME IS THE KEY'S. A validator is named by the digest of
//! the key it signs with, so a name cannot be claimed by anyone who does not
//! hold that key, and a validator cannot rename itself without becoming a
//! different validator. There is no separate identity to keep in step and none
//! to steal.
//!
//! THE KEY IS MADE ONCE AND KEPT. A validator whose key changed on restart
//! would be a new validator every restart, and to the committee an absent one.
//! It is written with an owner-only mode, because a signing key readable by
//! another account on the host is that account's key too.
//!
//! NOTHING IS DERIVED FROM A SHARED SEED. A network brought up by handing every
//! node one seed is one validator wearing several hats, with no fault tolerance
//! at all. Each node makes its own key and publishes the public half.

use std::fs;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use blst::min_pk::SecretKey;
use lux_consensus::pop;

/// Width of the identity a proof is bound to.
pub const NAME_LEN: usize = 20;

/// The identity of a validator: 20 bytes, the head of its key's digest.
pub type Name = [u8; NAME_LEN];

/// The key this validator signs with, and the file it lives in.
pub struct Validator {
    secret: SecretKey,
}

impl Validator {
    /// Load the key in `dir`, making one if it is not there yet.
    pub fn open(dir: &Path) -> Result<Self, String> {
        fs::create_dir_all(dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
        let path = dir.join("validator.key");
        match fs::read(&path) {
            Ok(raw) => Ok(Self {
                secret: SecretKey::from_bytes(&raw).map_err(|e| {
                    format!("the key at {} is not one: {e:?}", path.display())
                })?,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let secret = make(&path)?;
                Ok(Self { secret })
            }
            Err(e) => Err(format!("cannot read {}: {e}", path.display())),
        }
    }

    /// The public half, compressed — 48 bytes.
    pub fn key(&self) -> [u8; pop::KEY_LEN] {
        self.secret.sk_to_pk().compress()
    }

    /// This validator's name: the head of its key's digest.
    ///
    /// Derived rather than chosen, so the pair the proof binds is a pair
    /// nobody else can present and this node cannot restate.
    pub fn name(&self) -> Name {
        name_of(&self.key())
    }

    /// The line this validator publishes so others can put it in a committee.
    ///
    /// Public halves only. The proof is over this validator's own name, so a
    /// line lifted from one committee cannot be pasted into another under a
    /// different name.
    pub fn line(&self, weight: u64) -> String {
        let key = self.key();
        let name = name_of(&key);
        let proof = pop::sign(&self.secret, &name, &key);
        format!("{} {} {weight}", hex(&key), hex(&proof))
    }
}

/// The name a public key carries.
pub fn name_of(key: &[u8]) -> Name {
    let mut name = [0u8; NAME_LEN];
    name.copy_from_slice(&lux_consensus::sha256(key)[..NAME_LEN]);
    name
}

/// Render bytes the way a published line spells them.
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Make a key and write it where only this account can read it.
fn make(path: &PathBuf) -> Result<SecretKey, String> {
    // 32 bytes from the kernel, which is what key_gen is specified to take.
    // Deriving this from anything reproducible would be a signing key someone
    // else can compute.
    let mut material = [0u8; 32];
    let mut urandom = fs::File::open("/dev/urandom")
        .map_err(|e| format!("no randomness: {e}"))?;
    urandom
        .read_exact(&mut material)
        .map_err(|e| format!("short read from /dev/urandom: {e}"))?;
    let secret = SecretKey::key_gen(&material, &[])
        .map_err(|e| format!("cannot make a key: {e:?}"))?;

    // Created 0600 rather than created and then chmodded: a key that is world
    // readable for the width of two syscalls has been world readable.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    use std::io::Write;
    file.write_all(&secret.to_bytes())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    file.sync_all()
        .map_err(|e| format!("cannot flush {}: {e}", path.display()))?;
    Ok(secret)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hexadecimal"))
            .collect()
    }

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hanzod-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_key_is_made_once_and_kept() {
        let dir = scratch("kept");
        let first = Validator::open(&dir).expect("made");
        let second = Validator::open(&dir).expect("loaded");
        assert_eq!(
            first.name(),
            second.name(),
            "a validator that renamed itself on restart is a new validator"
        );
        assert_eq!(first.line(1), second.line(1));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_key_is_readable_only_by_its_owner() {
        let dir = scratch("mode");
        Validator::open(&dir).expect("made");
        let mode = fs::metadata(dir.join("validator.key")).expect("written").permissions().mode();
        assert_eq!(
            mode & 0o077,
            0,
            "a signing key another account can read is that account's key"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_validators_do_not_share_a_name() {
        let a = scratch("a");
        let b = scratch("b");
        let one = Validator::open(&a).expect("made");
        let two = Validator::open(&b).expect("made");
        assert_ne!(one.name(), two.name());
        let _ = fs::remove_dir_all(&a);
        let _ = fs::remove_dir_all(&b);
    }

    #[test]
    fn a_published_proof_binds_the_name_to_the_key() {
        let dir = scratch("pop");
        let v = Validator::open(&dir).expect("made");
        // Read back the line this node actually publishes, so what is checked
        // is the artifact a committee is built from and not a helper beside it.
        let line = v.line(1);
        let mut field = line.split_whitespace();
        let key = unhex(field.next().expect("key"));
        let proof = unhex(field.next().expect("proof"));
        assert_eq!(field.next(), Some("1"), "the weight it was told to publish");
        let name = name_of(&key);
        assert_eq!(name, v.name());
        pop::verify(&name, &key, &proof).expect("the proof this node published");

        // And it binds THIS name: the same key under another name is refused,
        // which is what stops a line being pasted in under a borrowed identity.
        let mut other = name;
        other[0] ^= 0xff;
        assert!(pop::verify(&other, &key, &proof).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_name_is_the_head_of_the_keys_digest() {
        let dir = scratch("name");
        let v = Validator::open(&dir).expect("made");
        let key = v.key();
        assert_eq!(v.name(), name_of(&key));
        assert_eq!(&v.name()[..], &lux_consensus::sha256(&key)[..NAME_LEN]);
        let _ = fs::remove_dir_all(&dir);
    }
}
