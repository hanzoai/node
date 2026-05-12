// Hermetic Manager test: open against a tempdir, list (empty), create a
// spec, list again, get it back, delete, list empty.
//
// We don't actually start a VM — that requires real vfkit. We only exercise
// the metadata-management surface, which doesn't touch the hypervisor.

use hanzo_machine::{native_available, Manager, Spec};
use tempfile::TempDir;

#[test]
fn create_list_get_delete_roundtrip() {
    if !native_available() {
        eprintln!("skip: libluxmachine not linked");
        return;
    }
    let dir = TempDir::new().expect("tempdir");
    let mgr = Manager::open(dir.path()).expect("open manager");

    // Empty initially.
    let v = mgr.list().expect("list empty");
    assert_eq!(v.len(), 0, "expected empty state, got {v:?}");

    // Create.
    let spec = Spec {
        name: "test-vm".into(),
        distro: "ubuntu".into(),
        cpus: 2,
        memory_mb: 2048,
        disk_gb: 20,
        rosetta: false,
    };
    mgr.create(&spec).expect("create");

    // List shows it.
    let v = mgr.list().expect("list one");
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].name, "test-vm");
    assert_eq!(v[0].distro, "ubuntu");
    assert_eq!(v[0].cpus, 2);

    // Get returns the same thing.
    let info = mgr.get("test-vm").expect("get");
    assert_eq!(info.name, "test-vm");
    assert_eq!(info.memory_mb, 2048);

    // Delete (this only succeeds for stopped machines, which is the state
    // immediately after create per the C++ impl).
    mgr.delete("test-vm").expect("delete");

    // List is empty again.
    let v = mgr.list().expect("list after delete");
    assert_eq!(v.len(), 0, "expected empty after delete, got {v:?}");
}

#[test]
fn get_nonexistent_returns_not_found() {
    if !native_available() {
        eprintln!("skip: libluxmachine not linked");
        return;
    }
    let dir = TempDir::new().expect("tempdir");
    let mgr = Manager::open(dir.path()).expect("open manager");

    match mgr.get("does-not-exist") {
        Err(hanzo_machine::Error::NotFound) => {}
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn duplicate_create_returns_exists() {
    if !native_available() {
        eprintln!("skip: libluxmachine not linked");
        return;
    }
    let dir = TempDir::new().expect("tempdir");
    let mgr = Manager::open(dir.path()).expect("open manager");

    let spec = Spec {
        name: "dup".into(),
        distro: "alpine".into(),
        cpus: 1,
        memory_mb: 512,
        disk_gb: 8,
        rosetta: false,
    };
    mgr.create(&spec).expect("create first");
    match mgr.create(&spec) {
        Err(hanzo_machine::Error::Exists) => {}
        other => panic!("expected Exists, got {other:?}"),
    }
    let _ = mgr.delete("dup");
}
