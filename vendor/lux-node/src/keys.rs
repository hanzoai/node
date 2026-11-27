// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! What a validator holds, and what it publishes.
//!
//! Two keys, because they answer two questions and a key that answered both
//! would have to be rotated for either:
//!
//! - **ML-DSA-65** is WHO. It names the validator and proves that name on every
//!   link. See [`crate::pq`].
//! - **BLS12-381** is WHAT IT SAID. It signs votes, and it is the key the
//!   certificate aggregates. See `lux-consensus`.
//!
//! Both are generated once and kept on disk, the way staking keys are. A
//! validator whose keys changed on restart would be a different validator every
//! restart — and to the committee, an absent one.
//!
//! NOTHING IS DERIVED FROM A SHARED SEED. A network brought up by handing every
//! node one seed is a network where every node holds every other node's signing
//! key, which is one validator wearing several hats and no fault tolerance at
//! all. Each node makes its own keys and PUBLISHES the public halves; the
//! committee is the list of what they published.

use std::path::Path;

use blst::min_pk::SecretKey;

use crate::engine::Committee;
use crate::pq::Identity;

/// The keys this validator holds.
pub struct Keys {
    pub identity: Identity,
    pub consensus: SecretKey,
}

impl Keys {
    /// Load the keys in `dir`, making either that is missing.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, String> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
        let identity =
            Identity::open(dir.join("identity.key")).map_err(|e| format!("identity: {e}"))?;
        let consensus = open_consensus(&dir.join("consensus.key"))?;
        Ok(Self { identity, consensus })
    }

    /// The line this validator publishes so others can put it in a committee.
    ///
    /// Public halves only. The proof is over this chain, so a line published
    /// for one network entitles nothing on another — while the name it is
    /// published under is the same name everywhere, which is the one the link
    /// proves.
    pub fn publish(&self, chain: &crate::vm::Id) -> String {
        let node = self.identity.node();
        let key = self.consensus.sk_to_pk().compress();
        let proof =
            lux_consensus::pop::sign_over(&self.consensus, &Committee::claim(chain, &node, &key));
        Committee::line(self.identity.public(), &key, &proof)
    }
}

/// Load a BLS signing key, or make one.
fn open_consensus(path: &Path) -> Result<SecretKey, String> {
    match std::fs::read(path) {
        Ok(raw) => SecretKey::from_bytes(&raw)
            .map_err(|e| format!("the consensus key at {} is not one: {e:?}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // 32 bytes of the system's randomness, which is what `key_gen` is
            // specified to take. Deriving this from anything reproducible would
            // be a signing key someone else can compute.
            let mut material = [0u8; 32];
            fill(&mut material)?;
            let key = SecretKey::key_gen(&material, &[])
                .map_err(|e| format!("cannot make a consensus key: {e:?}"))?;
            crate::pq::write_private(path, &key.to_bytes()).map_err(|e| e.to_string())?;
            Ok(key)
        }
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

/// Fill `out` with randomness from the system.
///
/// `/dev/urandom` rather than a crate: this is the one place the node needs
/// entropy, and the kernel is already the source every crate would wrap.
fn fill(out: &mut [u8]) -> Result<(), String> {
    use std::io::Read;
    let mut f = std::fs::File::open("/dev/urandom").map_err(|e| format!("no randomness: {e}"))?;
    f.read_exact(out).map_err(|e| format!("short read from /dev/urandom: {e}"))
}

#[cfg(test)]
mod tests {
    /// The chain these fixtures are entitled on. Any fixed 32 bytes: what the
    /// tests exercise is that the SAME chain admits and a different one does
    /// not, not which chain it is.
    const TEST_CHAIN: crate::vm::Id = [9u8; 32];

    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("lux-keys-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn keys_are_made_once_and_kept() {
        let dir = scratch("kept");
        let first = Keys::open(&dir).expect("made");
        let second = Keys::open(&dir).expect("loaded");
        assert_eq!(
            first.identity.node(),
            second.identity.node(),
            "a validator that renamed itself on restart is a new validator"
        );
        assert_eq!(
            first.consensus.to_bytes(),
            second.consensus.to_bytes(),
            "and one that re-keyed could not be counted for what it already said"
        );
        assert_eq!(first.publish(&TEST_CHAIN), second.publish(&TEST_CHAIN));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_validators_are_two_validators() {
        let (a, b) = (scratch("a"), scratch("b"));
        let one = Keys::open(&a).expect("one");
        let two = Keys::open(&b).expect("two");
        assert_ne!(one.identity.node(), two.identity.node());
        assert_ne!(one.consensus.to_bytes(), two.consensus.to_bytes());
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn what_a_validator_publishes_is_a_committee_of_one() {
        let dir = scratch("publish");
        let keys = Keys::open(&dir).expect("made");
        let c = Committee::read(&keys.publish(&TEST_CHAIN), TEST_CHAIN)
            .expect("a published line is a committee line");
        assert_eq!(c.len(), 1);
        assert_eq!(
            c.seat(&keys.identity.node()),
            Some(0),
            "and the validator is in it under its own name"
        );
        // The proof of possession is checked here, so a line that claimed a key
        // it does not hold would not build a set.
        c.set().expect("the proof verifies");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_key_file_is_readable_by_nobody_else() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("mode");
        let _ = Keys::open(&dir).expect("made");
        // The journal belongs here too: anyone who can WRITE it can make this
        // validator forget what it already said, which is the whole defence
        // against equivocating.
        let _ = crate::journal::Journal::open(dir.join("votes")).expect("journal");
        for name in ["identity.key", "consensus.key", "votes"] {
            let mode =
                std::fs::metadata(dir.join(name)).expect("the key exists").permissions().mode()
                    & 0o777;
            assert_eq!(mode, 0o600, "{name} is readable by someone else");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
