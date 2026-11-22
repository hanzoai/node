//! Where hanzod keeps things. One directory, `<data_local_dir>/hanzo`, which is
//! also where the CLI keeps its credential vault and the cloud host's state.

use std::path::PathBuf;

pub fn data() -> PathBuf {
    if let Ok(override_dir) = std::env::var("HANZO_DATA_DIR") {
        let trimmed = override_dir.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    dirs::data_local_dir().unwrap_or_else(|| PathBuf::from(".")).join("hanzo")
}

/// The one local ZAP socket (mode 0600).
pub fn socket() -> PathBuf {
    data().join("hanzod.zap.sock")
}

/// Where the cloud host's ZAP socket is (the CLI's historical location).
pub fn cloud_socket() -> PathBuf {
    data().join("host").join("host.zap.sock")
}

/// `~/.hanzo/bin`: the one binary of each program on the machine.
pub fn bin() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".hanzo").join("bin")
}

/// Loopback HTTP view of the same routes, for callers that cannot speak ZAP.
pub const HTTP_ADDR: &str = "127.0.0.1:3690";
