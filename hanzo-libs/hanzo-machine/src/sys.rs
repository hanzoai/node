// Raw bindgen output for the canonical luxmachine C ABI.
//
// When libluxmachine is unavailable at build time, build.rs emits a stub
// bindings file that defines the types but no extern functions. We gate
// callers behind `cfg(hanzo_machine_native)` so the wrapper still compiles
// on machines without the lib (returning Error::NotInstalled at runtime).

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals, dead_code)]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
