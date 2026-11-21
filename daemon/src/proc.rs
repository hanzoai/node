//! Child-process helpers shared by the subsystems.

use std::io;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Child;

/// A free loopback port. The probe socket is closed before the child binds it,
/// so another process could take it in between; the child then fails to start
/// and the next frame retries with a new port.
pub fn free_port() -> io::Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// SIGTERM, then SIGKILL after `grace`. SIGTERM is what lets a child drain its
/// own children; SIGKILL is what orphans them.
pub async fn terminate(child: &mut Child, grace: Duration) {
    if let Some(pid) = child.id() {
        unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    }
    if tokio::time::timeout(grace, child.wait()).await.is_err() {
        let _ = child.kill().await;
    }
}

/// `~/.hanzo/bin/<name>`, else beside this executable.
pub fn find(name: &str) -> Option<PathBuf> {
    let a = crate::paths::bin().join(name);
    if a.is_file() {
        return Some(a);
    }
    let b = std::env::current_exe().ok()?.parent()?.join(name);
    b.is_file().then_some(b)
}

/// A program another user could replace is not run, whatever its digest was.
#[cfg(unix)]
pub fn owned_by_us_alone(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let m = std::fs::metadata(path)?;
    let me = unsafe { libc::geteuid() };
    if (m.uid() != me && m.uid() != 0) || m.permissions().mode() & 0o022 != 0 {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("{} is writable by someone else", path.display())));
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn owned_by_us_alone(_: &Path) -> io::Result<()> {
    Ok(())
}

/// Retry `probe` every 100 ms until it answers or `limit` passes.
pub async fn until<F, Fut>(limit: Duration, mut probe: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let end = tokio::time::Instant::now() + limit;
    loop {
        if probe().await {
            return true;
        }
        if tokio::time::Instant::now() >= end {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

pub fn refuse(why: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, why.into())
}
