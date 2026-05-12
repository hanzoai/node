// Safe Rust wrapper around `lux_machine_manager`.
//
// Each native method is a single `unsafe { sys::lux_machine_X(..) }` call
// followed by Result mapping. Strings cross the FFI boundary as CString;
// output is copied out before the C side can free it.
//
// When the crate is built without libluxmachine (stub bindings),
// `Manager::open` returns `Error::NotInstalled` and the type is otherwise
// uninhabited from Rust. Compilation always succeeds either way.

use std::path::Path;

use crate::error::{Error, Result};
use crate::types::{Info, Spec};

#[cfg(hanzo_machine_native)]
mod native {
    use super::*;
    use crate::error::from_status;
    use crate::sys;
    use std::ffi::{CStr, CString};
    use std::ptr::NonNull;

    pub struct Manager {
        pub(super) handle: NonNull<sys::lux_machine_manager>,
    }

    unsafe impl Send for Manager {}
    unsafe impl Sync for Manager {}

    impl Manager {
        pub fn open(state_dir: impl AsRef<Path>) -> Result<Self> {
            let dir = path_to_cstring(state_dir.as_ref())?;
            let raw = unsafe { sys::lux_machine_manager_open(dir.as_ptr()) };
            let handle = NonNull::new(raw).ok_or(Error::OpenFailed)?;
            Ok(Manager { handle })
        }

        pub fn create(&self, spec: &Spec) -> Result<()> {
            let name = CString::new(spec.name.as_bytes()).map_err(|_| Error::Nul)?;
            let distro = CString::new(spec.distro.as_bytes()).map_err(|_| Error::Nul)?;
            let raw = sys::lux_machine_spec {
                name: name.as_ptr(),
                distro: distro.as_ptr(),
                cpus: spec.cpus,
                memory_mb: spec.memory_mb,
                disk_gb: spec.disk_gb,
                rosetta: u8::from(spec.rosetta),
            };
            let status = unsafe { sys::lux_machine_create(self.handle.as_ptr(), &raw) };
            self.check(status)
        }

        pub fn start(&self, name: &str) -> Result<()> {
            let cname = CString::new(name).map_err(|_| Error::Nul)?;
            let status = unsafe { sys::lux_machine_start(self.handle.as_ptr(), cname.as_ptr()) };
            self.check(status)
        }

        pub fn stop(&self, name: &str) -> Result<()> {
            let cname = CString::new(name).map_err(|_| Error::Nul)?;
            let status = unsafe { sys::lux_machine_stop(self.handle.as_ptr(), cname.as_ptr()) };
            self.check(status)
        }

        pub fn delete(&self, name: &str) -> Result<()> {
            let cname = CString::new(name).map_err(|_| Error::Nul)?;
            let status = unsafe { sys::lux_machine_delete(self.handle.as_ptr(), cname.as_ptr()) };
            self.check(status)
        }

        pub fn get(&self, name: &str) -> Result<Info> {
            let cname = CString::new(name).map_err(|_| Error::Nul)?;
            let mut raw = sys::lux_machine_info::default();
            let status = unsafe {
                sys::lux_machine_get(self.handle.as_ptr(), cname.as_ptr(), &mut raw)
            };
            self.check(status)?;
            Info::from_raw(&raw)
        }

        pub fn list(&self) -> Result<Vec<Info>> {
            // Two-pass: cap=0 returns required size, then read into sized buf.
            let needed = unsafe {
                sys::lux_machine_list(self.handle.as_ptr(), std::ptr::null_mut(), 0)
            };
            if needed == 0 {
                return Ok(Vec::new());
            }
            let mut buf: Vec<sys::lux_machine_info> =
                vec![sys::lux_machine_info::default(); needed];
            let written = unsafe {
                sys::lux_machine_list(self.handle.as_ptr(), buf.as_mut_ptr(), needed)
            };
            buf.truncate(written);
            buf.iter().map(Info::from_raw).collect()
        }

        pub fn version() -> &'static str {
            let ptr = unsafe { sys::lux_machine_version() };
            if ptr.is_null() {
                return "unknown";
            }
            // Safety: C side returns a 'static string.
            unsafe { CStr::from_ptr(ptr) }.to_str().unwrap_or("unknown")
        }

        fn check(&self, status: sys::lux_machine_status) -> Result<()> {
            if status == sys::LUX_MACHINE_OK {
                return Ok(());
            }
            let detail = unsafe {
                let p = sys::lux_machine_last_error(self.handle.as_ptr());
                if p.is_null() {
                    None
                } else {
                    CStr::from_ptr(p).to_str().ok().map(|s| s.to_owned())
                }
            };
            Err(from_status(status, detail))
        }
    }

    impl Drop for Manager {
        fn drop(&mut self) {
            unsafe { sys::lux_machine_manager_close(self.handle.as_ptr()) };
        }
    }

    impl std::fmt::Debug for Manager {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Manager").finish_non_exhaustive()
        }
    }

    fn path_to_cstring(p: &Path) -> Result<CString> {
        let s = p.to_str().ok_or(Error::Utf8)?;
        CString::new(s).map_err(|_| Error::Nul)
    }
}

#[cfg(not(hanzo_machine_native))]
mod stub {
    use super::*;

    pub struct Manager {
        _never: std::convert::Infallible,
    }

    unsafe impl Send for Manager {}
    unsafe impl Sync for Manager {}

    impl Manager {
        pub fn open(_state_dir: impl AsRef<Path>) -> Result<Self> {
            Err(Error::NotInstalled)
        }
        pub fn create(&self, _spec: &Spec) -> Result<()> {
            match self._never {}
        }
        pub fn start(&self, _name: &str) -> Result<()> {
            match self._never {}
        }
        pub fn stop(&self, _name: &str) -> Result<()> {
            match self._never {}
        }
        pub fn delete(&self, _name: &str) -> Result<()> {
            match self._never {}
        }
        pub fn get(&self, _name: &str) -> Result<Info> {
            match self._never {}
        }
        pub fn list(&self) -> Result<Vec<Info>> {
            match self._never {}
        }
        pub fn version() -> &'static str {
            "not-installed"
        }
    }

    impl std::fmt::Debug for Manager {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Manager").field("native", &false).finish()
        }
    }
}

#[cfg(hanzo_machine_native)]
pub use native::Manager;

#[cfg(not(hanzo_machine_native))]
pub use stub::Manager;

/// True when this build links against a real libluxmachine.
pub fn native_available() -> bool {
    cfg!(hanzo_machine_native)
}
