// Error enum mirroring `lux_machine_status` plus the NotInstalled variant for
// the no-lib case.

use thiserror::Error;

use crate::sys;

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid argument")]
    Invalid,
    #[error("not found")]
    NotFound,
    #[error("already exists")]
    Exists,
    #[error("io: {0}")]
    Io(String),
    #[error("vfkit: {0}")]
    Vfkit(String),
    #[error("internal: {0}")]
    Internal(String),
    #[error("invalid utf-8 in name or path")]
    Utf8,
    #[error("nul byte in name or path")]
    Nul,
    #[error("libluxmachine not installed; build with LUXMACHINE_DIR set or install via `cmake --install` from luxcpp/machine, or use the sidecar backend")]
    NotInstalled,
    #[error("manager open returned null pointer")]
    OpenFailed,
    #[error("sidecar: {0}")]
    Sidecar(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg_attr(not(hanzo_machine_native), allow(dead_code))]
pub(crate) fn from_status(status: sys::lux_machine_status, last_error: Option<String>) -> Error {
    let detail = last_error.unwrap_or_default();
    match status {
        sys::LUX_MACHINE_ERR_INVALID => Error::Invalid,
        sys::LUX_MACHINE_ERR_NOT_FOUND => Error::NotFound,
        sys::LUX_MACHINE_ERR_EXISTS => Error::Exists,
        sys::LUX_MACHINE_ERR_IO => Error::Io(detail),
        sys::LUX_MACHINE_ERR_VFKIT => Error::Vfkit(detail),
        sys::LUX_MACHINE_ERR_INTERNAL => Error::Internal(detail),
        _ => Error::Internal(format!("unknown status {status}: {detail}")),
    }
}
