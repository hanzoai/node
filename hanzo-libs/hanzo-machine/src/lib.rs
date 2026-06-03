// Hanzo machine: Rust bindings for the canonical luxmachine C ABI.
//
// Two backends, one trait. Native (`Manager`) links libluxmachine directly;
// sidecar (`SidecarClient`) talks HTTP/JSON over a Unix socket to
// `zoo-machine machined`. `open_backend` picks native when available and
// falls back to the sidecar.

#![deny(missing_debug_implementations)]
#![warn(rust_2018_idioms)]

pub mod error;
pub mod manager;
pub mod sidecar;
pub mod sys;
pub mod types;

pub use error::{Error, Result};
pub use manager::{native_available, Manager};
pub use sidecar::{Health, SidecarClient};
pub use types::{Info, Spec, State};

use std::path::{Path, PathBuf};

/// Common surface implemented by both the native FFI Manager and the sidecar
/// HTTP client. Lets callers be backend-agnostic.
pub trait MachineBackend: std::fmt::Debug + Send + Sync {
    fn create(&self, spec: &Spec) -> Result<Info>;
    fn start(&self, name: &str) -> Result<()>;
    fn stop(&self, name: &str) -> Result<()>;
    fn delete(&self, name: &str) -> Result<()>;
    fn get(&self, name: &str) -> Result<Info>;
    fn list(&self) -> Result<Vec<Info>>;
}

#[derive(Debug)]
struct NativeBackend(Manager);

impl MachineBackend for NativeBackend {
    fn create(&self, spec: &Spec) -> Result<Info> {
        self.0.create(spec)?;
        self.0.get(&spec.name)
    }
    fn start(&self, name: &str) -> Result<()> {
        self.0.start(name)
    }
    fn stop(&self, name: &str) -> Result<()> {
        self.0.stop(name)
    }
    fn delete(&self, name: &str) -> Result<()> {
        self.0.delete(name)
    }
    fn get(&self, name: &str) -> Result<Info> {
        self.0.get(name)
    }
    fn list(&self) -> Result<Vec<Info>> {
        self.0.list()
    }
}

impl MachineBackend for SidecarClient {
    fn create(&self, spec: &Spec) -> Result<Info> {
        SidecarClient::create(self, spec)
    }
    fn start(&self, name: &str) -> Result<()> {
        SidecarClient::start(self, name)
    }
    fn stop(&self, name: &str) -> Result<()> {
        SidecarClient::stop(self, name)
    }
    fn delete(&self, name: &str) -> Result<()> {
        SidecarClient::delete(self, name)
    }
    fn get(&self, name: &str) -> Result<Info> {
        SidecarClient::get(self, name)
    }
    fn list(&self) -> Result<Vec<Info>> {
        SidecarClient::list(self)
    }
}

/// True when this build links against a real libluxmachine.
pub fn prefer_native() -> bool {
    native_available()
}

/// Default machined Unix socket path: `~/.hanzo/run/machined.sock`.
pub fn default_socket_path() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".hanzo/run/machined.sock");
    }
    PathBuf::from("/tmp/hanzo-machined.sock")
}

/// Open the preferred backend. Native if libluxmachine is linked AND the
/// state directory opens successfully, sidecar otherwise. Sidecar requires
/// `zoo-machine machined` to be running at the standard socket.
pub fn open_backend(state_dir: impl AsRef<Path>) -> Result<Box<dyn MachineBackend>> {
    if native_available() {
        match Manager::open(state_dir.as_ref()) {
            Ok(m) => return Ok(Box::new(NativeBackend(m))),
            Err(e @ Error::NotInstalled) => {
                // Fall through to sidecar.
                let _ = e;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(Box::new(SidecarClient::new(default_socket_path())))
}
