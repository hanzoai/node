// Locate libluxmachine, emit link directives, and run bindgen on lux_machine.h.
//
// Resolution order:
//   1. pkg-config (`luxmachine`)
//   2. $LUXMACHINE_DIR (header at $/include, lib at $/lib)
//   3. /usr/local
// If none of those have the header, we emit a `cargo:warning` and write a
// stub bindings.rs so the crate still compiles. `Manager::open` then returns
// `Error::NotInstalled` at runtime.

use std::env;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=LUXMACHINE_DIR");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let bindings_path = out_dir.join("bindings.rs");

    let resolved = resolve();

    // Tell cargo about the cfg flags we may emit so rustc doesn't warn under
    // `-D unexpected_cfgs` on stable.
    println!("cargo:rustc-check-cfg=cfg(hanzo_machine_native)");

    match resolved {
        Some(found) => {
            println!("cargo:rustc-cfg=hanzo_machine_native");
            for inc in &found.include_paths {
                println!("cargo:include={}", inc.display());
            }
            for lib in &found.link_paths {
                println!("cargo:rustc-link-search=native={}", lib.display());
            }
            // Default to dynamic; `static` feature flips to archive.
            let static_link = env::var_os("CARGO_FEATURE_STATIC").is_some();
            if static_link {
                println!("cargo:rustc-link-lib=static=luxmachine");
            } else {
                println!("cargo:rustc-link-lib=dylib=luxmachine");
            }
            // C++ runtime (libluxmachine is C ABI but C++ implementation).
            link_cxx_runtime();
            // Embed the build dir as an rpath for dev runs without install.
            for lib in &found.link_paths {
                println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib.display());
            }
            generate_bindings(&found.include_paths, &bindings_path);
        }
        None => {
            println!(
                "cargo:warning=hanzo-machine: libluxmachine not found via pkg-config, \
                 LUXMACHINE_DIR, or /usr/local. Building stub bindings; runtime calls \
                 will return Error::NotInstalled. Set LUXMACHINE_DIR=/path/to/install \
                 or `cmake --install` luxcpp/machine to enable native linkage."
            );
            write_stub_bindings(&bindings_path);
        }
    }
}

struct Found {
    include_paths: Vec<PathBuf>,
    link_paths: Vec<PathBuf>,
}

fn resolve() -> Option<Found> {
    // 1. pkg-config.
    if let Ok(lib) = pkg_config::Config::new()
        .cargo_metadata(false)
        .print_system_libs(false)
        .probe("luxmachine")
    {
        if header_exists(&lib.include_paths) {
            return Some(Found {
                include_paths: lib.include_paths,
                link_paths: lib.link_paths,
            });
        }
    }

    // 2. LUXMACHINE_DIR. Accept either an install prefix (header at
    //    $/include, lib at $/lib) or a CMake build root (header at
    //    ../include relative to lib). The latter lets dev runs work
    //    against an in-tree build without `cmake --install`.
    if let Some(dir) = env::var_os("LUXMACHINE_DIR") {
        let root = PathBuf::from(dir);
        let candidates: [(PathBuf, PathBuf); 3] = [
            (root.join("include"), root.join("lib")),
            (root.join("include"), root.clone()),
            (
                root.parent().map(|p| p.join("include")).unwrap_or_else(|| root.clone()),
                root.clone(),
            ),
        ];
        for (inc, lib) in candidates {
            if inc.join("lux_machine.h").exists() {
                return Some(Found {
                    include_paths: vec![inc],
                    link_paths: vec![lib],
                });
            }
        }
    }

    // 3. /usr/local.
    let usr = PathBuf::from("/usr/local");
    if usr.join("include/lux_machine.h").exists() {
        return Some(Found {
            include_paths: vec![usr.join("include")],
            link_paths: vec![usr.join("lib")],
        });
    }

    None
}

fn header_exists(includes: &[PathBuf]) -> bool {
    includes.iter().any(|p| p.join("lux_machine.h").exists())
}

fn link_cxx_runtime() {
    let target = env::var("TARGET").unwrap_or_default();
    if target.contains("apple") {
        println!("cargo:rustc-link-lib=dylib=c++");
    } else if target.contains("linux") {
        println!("cargo:rustc-link-lib=dylib=stdc++");
    }
}

fn generate_bindings(include_paths: &[PathBuf], out: &Path) {
    let mut builder = bindgen::Builder::default()
        .header("wrapper.h")
        .allowlist_function("lux_machine_.*")
        .allowlist_type("lux_machine_.*")
        .allowlist_var("LUX_MACHINE_.*")
        .prepend_enum_name(false)
        .default_enum_style(bindgen::EnumVariation::Consts)
        .derive_default(true)
        .derive_debug(true)
        .layout_tests(false)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()));
    for inc in include_paths {
        builder = builder.clang_arg(format!("-I{}", inc.display()));
    }
    let bindings = builder
        .generate()
        .expect("bindgen failed to generate luxmachine bindings");
    bindings
        .write_to_file(out)
        .expect("failed to write bindings.rs");
}

// Stub bindings: opaque manager type plus extern fn declarations that resolve
// at link time only when the consumer provides their own libluxmachine. We
// don't emit link directives here, so a binary that pulls hanzo-machine
// without the lib installed will fail to link only if it actually references
// `sys::*`. The safe wrappers gate every call behind an availability check.
fn write_stub_bindings(out: &Path) {
    let stub = r#"// Auto-generated stub bindings (libluxmachine not found at build time).

pub const LUX_MACHINE_OK: lux_machine_status = 0;
pub const LUX_MACHINE_ERR_INVALID: lux_machine_status = 1;
pub const LUX_MACHINE_ERR_NOT_FOUND: lux_machine_status = 2;
pub const LUX_MACHINE_ERR_EXISTS: lux_machine_status = 3;
pub const LUX_MACHINE_ERR_IO: lux_machine_status = 4;
pub const LUX_MACHINE_ERR_VFKIT: lux_machine_status = 5;
pub const LUX_MACHINE_ERR_INTERNAL: lux_machine_status = 6;
pub type lux_machine_status = ::std::os::raw::c_uint;

pub const LUX_MACHINE_STATE_CREATING: lux_machine_state = 0;
pub const LUX_MACHINE_STATE_STOPPED: lux_machine_state = 1;
pub const LUX_MACHINE_STATE_STARTING: lux_machine_state = 2;
pub const LUX_MACHINE_STATE_RUNNING: lux_machine_state = 3;
pub const LUX_MACHINE_STATE_STOPPING: lux_machine_state = 4;
pub const LUX_MACHINE_STATE_DELETING: lux_machine_state = 5;
pub const LUX_MACHINE_STATE_ERROR: lux_machine_state = 6;
pub type lux_machine_state = ::std::os::raw::c_uint;

#[repr(C)]
#[derive(Debug)]
pub struct lux_machine_manager {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Default, Copy, Clone)]
pub struct lux_machine_spec {
    pub name: *const ::std::os::raw::c_char,
    pub distro: *const ::std::os::raw::c_char,
    pub cpus: u32,
    pub memory_mb: u64,
    pub disk_gb: u64,
    pub rosetta: u8,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct lux_machine_info {
    pub name: [::std::os::raw::c_char; 64usize],
    pub distro: [::std::os::raw::c_char; 16usize],
    pub cpus: u32,
    pub memory_mb: u64,
    pub disk_gb: u64,
    pub state: lux_machine_state,
    pub ip: [::std::os::raw::c_char; 64usize],
    pub created_at: i64,
    pub started_at: i64,
}

impl Default for lux_machine_info {
    fn default() -> Self {
        // Safety: POD with fixed-size char arrays; zero is valid for all fields.
        unsafe { ::std::mem::zeroed() }
    }
}

// Stub-only marker. Real bindings export real extern functions; the safe
// wrapper short-circuits with NotInstalled before reaching these.
pub const HANZO_MACHINE_STUB: bool = true;
"#;
    std::fs::write(out, stub).expect("write stub bindings");
}
