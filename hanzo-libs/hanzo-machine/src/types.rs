// Pure-Rust round-trip types for the C ABI structs.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::sys;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Creating,
    Stopped,
    Starting,
    Running,
    Stopping,
    Deleting,
    Error,
}

impl State {
    #[cfg_attr(not(hanzo_machine_native), allow(dead_code))]
    pub(crate) fn from_raw(raw: sys::lux_machine_state) -> State {
        match raw {
            sys::LUX_MACHINE_STATE_CREATING => State::Creating,
            sys::LUX_MACHINE_STATE_STOPPED => State::Stopped,
            sys::LUX_MACHINE_STATE_STARTING => State::Starting,
            sys::LUX_MACHINE_STATE_RUNNING => State::Running,
            sys::LUX_MACHINE_STATE_STOPPING => State::Stopping,
            sys::LUX_MACHINE_STATE_DELETING => State::Deleting,
            _ => State::Error,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Spec {
    pub name: String,
    pub distro: String,
    pub cpus: u32,
    pub memory_mb: u64,
    pub disk_gb: u64,
    #[serde(default)]
    pub rosetta: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Info {
    pub name: String,
    pub distro: String,
    pub cpus: u32,
    pub memory_mb: u64,
    pub disk_gb: u64,
    pub state: State,
    pub ip: Option<String>,
    pub created_at: i64,
    #[serde(default)]
    pub started_at: i64,
}

impl Info {
    #[cfg_attr(not(hanzo_machine_native), allow(dead_code))]
    pub(crate) fn from_raw(raw: &sys::lux_machine_info) -> Result<Self> {
        // Safety: name/distro/ip are fixed-size, NUL-terminated arrays of
        // c_char. Read up to first NUL.
        let name = c_array_to_string(&raw.name)?;
        let distro = c_array_to_string(&raw.distro)?;
        let ip_str = c_array_to_string(&raw.ip)?;
        let ip = if ip_str.is_empty() { None } else { Some(ip_str) };
        Ok(Info {
            name,
            distro,
            cpus: raw.cpus,
            memory_mb: raw.memory_mb,
            disk_gb: raw.disk_gb,
            state: State::from_raw(raw.state),
            ip,
            created_at: raw.created_at,
            started_at: raw.started_at,
        })
    }
}

#[cfg_attr(not(hanzo_machine_native), allow(dead_code))]
fn c_array_to_string(arr: &[std::os::raw::c_char]) -> Result<String> {
    let bytes: Vec<u8> = arr
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8(bytes).map_err(|_| Error::Utf8)
}
