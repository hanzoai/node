//! A worker is a child process that holds no credential. It gets one end of a
//! socketpair as file descriptor 3 and nothing else: no socket path to open, no
//! token in its environment. hanzod multiplexes sessions over that one stream.
//!
//! Mux record: `[u32 LE session][u8 kind][u32 LE length][payload]`, where kind
//! is 0 data, 1 open (a new session), 2 close. The worker answers on the same
//! pair with the same records.

use std::collections::HashMap;
use std::io;
use std::os::fd::{FromRawFd, RawFd};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

pub const KIND_DATA: u8 = 0;
pub const KIND_OPEN: u8 = 1;
pub const KIND_CLOSE: u8 = 2;
/// The descriptor number a worker finds its end of the pair on.
pub const WORKER_FD: RawFd = 3;
const MAX_RECORD: u32 = 33 << 20;

pub struct Spec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    /// Written to the worker's stdin, then stdin is closed (its bootstrap).
    pub stdin: Option<Vec<u8>>,
}

pub struct Worker {
    pub child: Child,
    pub mux: Arc<Mux>,
}

pub struct Mux {
    write: tokio::sync::Mutex<OwnedWriteHalf>,
    sessions: Mutex<HashMap<u32, mpsc::Sender<Vec<u8>>>>,
    next: AtomicU32,
}

pub fn spawn(spec: &Spec) -> io::Result<Worker> {
    let mut fds = [0 as RawFd; 2];
    if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let (ours, theirs) = (fds[0], fds[1]);
    for fd in [ours, theirs] {
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args)
        .env_clear()
        .envs(spec.env.iter().map(|(k, v)| (k, v)))
        .env("HANZO_DEV_ZAP_FD", WORKER_FD.to_string())
        .stdin(if spec.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    if let Some(cwd) = &spec.cwd {
        cmd.current_dir(cwd);
    }
    unsafe {
        cmd.pre_exec(move || {
            // dup2 clears FD_CLOEXEC on the new descriptor; the originals keep it.
            if libc::dup2(theirs, WORKER_FD) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let spawned = cmd.spawn();
    unsafe { libc::close(theirs) };
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            unsafe { libc::close(ours) };
            return Err(e);
        }
    };
    let std_stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(ours) };
    std_stream.set_nonblocking(true)?;
    let stream = UnixStream::from_std(std_stream)?;
    let (mut read, write) = stream.into_split();
    let mux = Arc::new(Mux { write: tokio::sync::Mutex::new(write), sessions: Mutex::new(HashMap::new()), next: AtomicU32::new(1) });

    if let (Some(bytes), Some(mut stdin)) = (spec.stdin.clone(), child.stdin.take()) {
        tokio::spawn(async move {
            let _ = stdin.write_all(&bytes).await;
            let _ = stdin.shutdown().await;
        });
    }

    let m = mux.clone();
    tokio::spawn(async move {
        let mut head = [0u8; 9];
        loop {
            if read.read_exact(&mut head).await.is_err() {
                break;
            }
            let sid = u32::from_le_bytes(head[0..4].try_into().unwrap());
            let kind = head[4];
            let len = u32::from_le_bytes(head[5..9].try_into().unwrap());
            if len > MAX_RECORD {
                break;
            }
            let mut buf = vec![0u8; len as usize];
            if read.read_exact(&mut buf).await.is_err() {
                break;
            }
            let tx = m.sessions.lock().unwrap().get(&sid).cloned();
            match (kind, tx) {
                (KIND_DATA, Some(tx)) => {
                    // A full queue stalls this loop, which stalls the worker's writes:
                    // flow control, not unbounded buffering.
                    let _ = tx.send(buf).await;
                }
                (KIND_CLOSE, _) => {
                    m.sessions.lock().unwrap().remove(&sid);
                }
                _ => {}
            }
        }
        // The worker is gone: every session ends.
        m.sessions.lock().unwrap().clear();
    });
    Ok(Worker { child, mux })
}

impl Mux {
    async fn record(&self, sid: u32, kind: u8, payload: &[u8]) -> io::Result<()> {
        let mut w = self.write.lock().await;
        let mut head = [0u8; 9];
        head[0..4].copy_from_slice(&sid.to_le_bytes());
        head[4] = kind;
        head[5..9].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        w.write_all(&head).await?;
        w.write_all(payload).await?;
        w.flush().await
    }

    /// Open a session. The receiver yields the worker's bytes and ends when the
    /// session or the worker ends.
    pub async fn open(&self) -> io::Result<(u32, mpsc::Receiver<Vec<u8>>)> {
        let sid = self.next.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel(64);
        self.sessions.lock().unwrap().insert(sid, tx);
        if let Err(e) = self.record(sid, KIND_OPEN, &[]).await {
            self.sessions.lock().unwrap().remove(&sid);
            return Err(e);
        }
        Ok((sid, rx))
    }

    pub async fn send(&self, sid: u32, data: &[u8]) -> io::Result<()> {
        self.record(sid, KIND_DATA, data).await
    }

    pub async fn close(&self, sid: u32) {
        self.sessions.lock().unwrap().remove(&sid);
        let _ = self.record(sid, KIND_CLOSE, &[]).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The worker here is a shell loop that echoes everything it reads from fd 3
    /// back to fd 3, record for record: enough to prove the descriptor is the
    /// worker's only way in and that a session round-trips.
    #[tokio::test]
    async fn a_session_round_trips_over_the_inherited_descriptor() {
        let spec = Spec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "exec cat <&3 >&3".into()],
            cwd: None,
            env: vec![("PATH".into(), "/usr/bin:/bin".into())],
            stdin: None,
        };
        let mut w = spawn(&spec).unwrap();
        let (sid, mut rx) = w.mux.open().await.unwrap();
        w.mux.send(sid, b"hello").await.unwrap();
        // cat echoes the OPEN record (ignored) and then the DATA record back.
        let got = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
        assert_eq!(got, b"hello");
        w.child.kill().await.unwrap();
        assert!(tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_worker_sees_no_environment_but_the_one_it_was_given() {
        let spec = Spec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "test -z \"$HOME\" && exec cat <&3 >&3".into()],
            cwd: None,
            env: vec![("PATH".into(), "/usr/bin:/bin".into())],
            stdin: None,
        };
        std::env::set_var("HOME", "/somewhere");
        let mut w = spawn(&spec).unwrap();
        let (sid, mut rx) = w.mux.open().await.unwrap();
        w.mux.send(sid, b"x").await.unwrap();
        assert!(tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await.unwrap().is_some());
        let _ = w.child.kill().await;
    }
}
