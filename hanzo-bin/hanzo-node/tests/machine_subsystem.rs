// Machine subsystem smoke tests. Skipped when the feature is off.

#![cfg(feature = "machine")]

use hanzo_node::machine::{version, Subsystem};

#[test]
fn version_is_non_empty() {
    let v = version();
    assert!(!v.is_empty(), "machine version() must not be empty");
}

#[test]
fn subsystem_init_against_tempdir() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sub = Subsystem::init(dir.path());
    // WHY: regardless of native vs shim we must hold a usable handle.
    let listed = sub.backend().list();
    if sub.is_installed() {
        let infos = listed.expect("list ok on installed backend");
        assert!(infos.is_empty(), "fresh state dir should have no machines");
    } else {
        assert!(listed.is_err(), "shim must surface NotInstalled");
    }
}
