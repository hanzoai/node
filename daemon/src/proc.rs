//! Child-process helpers shared by the subsystems.

use std::ffi::OsStr;
use std::io;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Child;

/// Win32 `CREATE_NO_WINDOW` process creation flag (`0x0800_0000`).
#[allow(dead_code)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Build a background `std::process::Command` that never pops up a console window on Windows.
pub fn bg_std_cmd(program: impl AsRef<OsStr>) -> std::process::Command {
    #[allow(unused_mut)]
    let mut cmd = std::process::Command::new(program);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt as _;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Build a background `tokio::process::Command` that never pops up a console window on Windows.
pub fn bg_tokio_cmd(program: impl AsRef<OsStr>) -> tokio::process::Command {
    #[allow(unused_mut)]
    let mut cmd = tokio::process::Command::new(program);
    #[cfg(target_os = "windows")]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// If Windows allocated a dedicated console solely for `hanzod.exe` (e.g. launched
/// without `CREATE_NO_WINDOW`), detach immediately so no console window stays open.
/// Interactive shells (`powershell`/`cmd`, where `count >= 2`) keep their console.
#[cfg(target_os = "windows")]
#[allow(dead_code)]
pub fn detach_standalone_console() {
    extern "system" {
        fn GetConsoleProcessList(lpdw_process_list: *mut u32, dw_process_count: u32) -> u32;
        fn FreeConsole() -> i32;
    }
    let mut pids = [0u32; 2];
    unsafe {
        if GetConsoleProcessList(pids.as_mut_ptr(), 2) == 1 {
            let _ = FreeConsole();
        }
    }
}

#[cfg(not(target_os = "windows"))]
#[allow(dead_code)]
pub fn detach_standalone_console() {}

/// A free loopback port. The probe socket is closed before the child binds it,
/// so another process could take it in between; the child then fails to start
/// and the next frame retries with a new port.
pub fn free_port() -> io::Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// SIGTERM, then SIGKILL after `grace`. SIGTERM is what lets a child drain its
/// own children; SIGKILL is what orphans them.
pub async fn terminate(child: &mut Child, grace: Duration) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill().await;
    }
    if tokio::time::timeout(grace, child.wait()).await.is_err() {
        let _ = child.kill().await;
    }
}

/// `~/.hanzo/bin/<name>`, else beside this executable.
pub fn find(name: &str) -> Option<PathBuf> {
    let candidates: Vec<String> = if cfg!(windows) && !name.ends_with(".exe") {
        vec![format!("{name}.exe"), name.to_string()]
    } else {
        vec![name.to_string()]
    };
    for cand in &candidates {
        let a = crate::paths::bin().join(cand);
        if a.is_file() {
            return Some(a);
        }
    }
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    for cand in &candidates {
        let b = dir.join(cand);
        if b.is_file() {
            return Some(b);
        }
    }
    None
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

#[cfg(test)]
mod tests {
    use super::*;

    fn strip_inline_test_module(src: &str) -> &str {
        for marker in ["\n#[cfg(test)]", "\n#[cfg(all(test,"] {
            if let Some(idx) = src.find(marker) {
                return &src[..idx];
            }
        }
        src
    }

    #[test]
    fn bg_std_cmd_executes_cleanly() {
        let mut cmd = if cfg!(target_os = "windows") {
            let mut c = bg_std_cmd("cmd.exe");
            c.args(["/C", "echo", "ok"]);
            c
        } else {
            let mut c = bg_std_cmd("sh");
            c.args(["-c", "printf ok"]);
            c
        };
        let out = cmd.output().expect("bg_std_cmd output");
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
    }

    #[tokio::test]
    async fn bg_tokio_cmd_executes_cleanly() {
        let mut cmd = if cfg!(target_os = "windows") {
            let mut c = bg_tokio_cmd("cmd.exe");
            c.args(["/C", "echo", "ok"]);
            c
        } else {
            let mut c = bg_tokio_cmd("sh");
            c.args(["-c", "printf ok"]);
            c
        };
        let out = cmd.output().await.expect("bg_tokio_cmd output");
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
    }

    #[test]
    fn no_unguarded_command_new_in_daemon_src() {
        let src_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let proc_file = src_root.join("proc.rs");
        let proc_src = std::fs::read_to_string(&proc_file).expect("read proc.rs");
        let proc_prod = strip_inline_test_module(&proc_src);
        assert!(
            proc_prod.contains("pub fn bg_std_cmd(")
                && proc_prod.contains("pub fn bg_tokio_cmd(")
                && proc_prod.contains("creation_flags(CREATE_NO_WINDOW)"),
            "proc.rs must define bg_std_cmd and bg_tokio_cmd with CREATE_NO_WINDOW"
        );

        let needle = ["Command", "::", "new("].concat();
        let mut stack = vec![src_root];
        let mut violations = Vec::new();
        let mut scanned = 0usize;

        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read_dir").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                if path == proc_file
                    || path.file_name().and_then(|n| n.to_str()) == Some("tests.rs")
                {
                    continue;
                }
                scanned += 1;
                let content = std::fs::read_to_string(&path).expect("read rs");
                let prod = strip_inline_test_module(&content);
                for (lineno, line) in prod.lines().enumerate() {
                    let trimmed = line.trim_start();
                    if trimmed.starts_with("//") {
                        continue;
                    }
                    if line.contains(&needle) {
                        violations.push(format!(
                            "{}:{}: {}",
                            path.display(),
                            lineno + 1,
                            trimmed
                        ));
                    }
                }
            }
        }

        assert!(scanned >= 25, "expected daemon/src/*.rs files, scanned {scanned}");
        assert!(
            violations.is_empty(),
            "all daemon/src subprocesses must use proc::bg_std_cmd or proc::bg_tokio_cmd (CREATE_NO_WINDOW on Windows); found unguarded Command::new:\n{}",
            violations.join("\n")
        );
    }
}
