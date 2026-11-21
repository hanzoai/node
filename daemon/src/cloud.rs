//! `/v1/*`: the local cloud host, the Go program the cluster runs, started as a
//! child on the first frame that no other prefix claims and stopped when idle.
//!
//! The host listens on a unix socket of its own (`host/host.zap.sock`) and a
//! private loopback port that hanzod picks; hanzod relays ZAP-HTTP frames to the
//! socket and holds the only public listener. The host's at-rest key is made
//! once per install in `<data>/hanzo/host/master.key` (0600) and passed in its
//! environment; hanzod never reads what the host seals.

use crate::paths;
use crate::pins::Pins;
use crate::proc;
use crate::table::{Fut, Reply, Subsystem};
use crate::zap::{self, Request};
use base64::Engine;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, Mutex};

const MASTER_ENV: &str = "CLOUD_KMS_MASTER_KEY_REF";

pub struct Cloud {
    pub pins: Arc<Pins>,
    child: Mutex<Option<Child>>,
}

impl Cloud {
    pub fn new(pins: Arc<Pins>) -> Cloud {
        Cloud { pins, child: Mutex::new(None) }
    }
}

fn state_dir() -> io::Result<PathBuf> {
    let d = paths::data().join("host");
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

/// This install's at-rest key: 32 random bytes, base64, made once and never
/// replaced (a replaced key strands every store sealed under it).
fn install_key(state: &std::path::Path) -> io::Result<String> {
    let path = state.join("master.key");
    if !path.exists() {
        let mut bytes = [0u8; 32];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        let key = base64::engine::general_purpose::STANDARD.encode(bytes);
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        match o.open(&path) {
            Ok(mut f) => io::Write::write_all(&mut f, key.as_bytes())?,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    let k = std::fs::read_to_string(&path)?.trim().to_string();
    match base64::engine::general_purpose::STANDARD.decode(&k) {
        Ok(b) if b.len() == 32 => Ok(k),
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, format!("{} is not an at-rest key", path.display()))),
    }
}

impl Subsystem for Cloud {
    fn name(&self) -> &'static str {
        "cloud"
    }
    fn prefix(&self) -> &'static str {
        "/v1"
    }
    fn idle(&self) -> Duration {
        Duration::from_secs(300)
    }
    fn start(&self) -> Fut<'_, io::Result<()>> {
        Box::pin(async move {
            let mut slot = self.child.lock().await;
            if let Some(mut old) = slot.take() {
                proc::terminate(&mut old, Duration::from_secs(5)).await;
            }
            let bin = proc::find("hanzo-cloud").ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "hanzo-cloud is not installed; run `hanzo upgrade cloud`"))?;
            proc::owned_by_us_alone(&bin)?;
            self.pins.verify("hanzo-cloud", &bin).map_err(proc::refuse)?;
            let state = state_dir()?;
            let key = install_key(&state)?;
            let sock = paths::cloud_socket();
            let _ = std::fs::remove_file(&sock);
            let (http, health) = (proc::free_port()?, proc::free_port()?);
            let log = std::fs::File::create(state.join("host.log"))?;
            let child = Command::new(&bin)
                .arg("-addr")
                .arg(format!("127.0.0.1:{http}"))
                .arg("-zap")
                .arg(&sock)
                .env_clear()
                .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                .env("HOME", dirs::home_dir().unwrap_or_default())
                .env("CLOUD_LISTEN", format!("127.0.0.1:{http}"))
                .env("CLOUD_HEALTH_LISTEN", format!("127.0.0.1:{health}"))
                .env("CLOUD_ENABLE_STAGED", "functions")
                .env("CLOUD_DATA_DIR", state.join("data"))
                .env(MASTER_ENV, key)
                .stdin(Stdio::null())
                .stdout(Stdio::from(log.try_clone()?))
                .stderr(Stdio::from(log))
                .kill_on_drop(true)
                .spawn()?;
            *slot = Some(child);
            let up = proc::until(Duration::from_secs(30), || async {
                let probe = Request { method: "GET".into(), target: "/healthz".into(), headers: vec![], body: vec![] };
                match exchange(&probe).await {
                    Ok(mut rx) => matches!(rx.recv().await.and_then(|f| zap::decode_response(&f).ok()), Some(r) if (200..300).contains(&r.status)),
                    Err(_) => false,
                }
            })
            .await;
            if !up {
                if let Some(mut c) = slot.take() {
                    proc::terminate(&mut c, Duration::from_secs(2)).await;
                }
                return Err(io::Error::other("the local cloud host did not come up; see host/host.log"));
            }
            Ok(())
        })
    }
    fn frame<'a>(&'a self, _who: &'a str, req: Request) -> Fut<'a, io::Result<Reply>> {
        Box::pin(async move { Ok(Reply::Frames(exchange(&req).await?)) })
    }
    fn stop(&self) -> Fut<'_, ()> {
        Box::pin(async move {
            if let Some(mut c) = self.child.lock().await.take() {
                proc::terminate(&mut c, Duration::from_secs(10)).await;
            }
        })
    }
}

/// Send one request to the host and relay its frames: one response frame, or a
/// head, data frames and an end.
async fn exchange(req: &Request) -> io::Result<mpsc::Receiver<Vec<u8>>> {
    let mut conn = UnixStream::connect(paths::cloud_socket()).await?;
    zap::write_frame(&mut conn, &zap::encode_request(req)).await?;
    let (tx, rx) = mpsc::channel(32);
    tokio::spawn(async move {
        while let Ok(Some(f)) = zap::read_frame(&mut conn).await {
            let last = matches!(zap::frame_type(&f), Some(zap::FRAME_RESPONSE) | Some(zap::FRAME_END));
            if tx.send(f).await.is_err() || last {
                break;
            }
        }
    });
    Ok(rx)
}
