// Verifies the FFI link works end-to-end. Skipped when libluxmachine isn't
// linked at build time (stub bindings) so CI on machines without the C++ lib
// still passes.

use hanzo_machine::{native_available, Manager};

#[test]
fn version_string_is_non_empty() {
    if !native_available() {
        eprintln!("skip: libluxmachine not linked (build with LUXMACHINE_DIR set)");
        return;
    }
    let v = Manager::version();
    assert!(!v.is_empty(), "version() returned empty string");
    assert_ne!(v, "unknown", "version() returned 'unknown' (NULL ptr)");
    eprintln!("luxmachine version: {v}");
}
