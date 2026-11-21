//! `/v1/dev`: the Dev backend. Today that is the TypeScript server, run as a
//! child worker over a socketpair; the engine moves into this process one
//! module at a time (HIP-1330 phases 2-5) and the worker shrinks with it.
//!
//!   POST /v1/dev/session   register how to run the worker and start it
//!   DELETE /v1/dev/session   the app is done: stop the worker, forget the spec
//!   (router sessions)      a ZAP session, bearer in the first envelope, is
//!                          carried to the worker over the socketpair
//!
//! A session's first envelope carries the caller's IAM bearer; it is verified
//! (and must be the machine's own person) before a byte reaches the worker. The
//! worker verifies it again, and holds no credential of its own.
//!
//! If the worker dies, the next session or registration starts it again from
//! the last registered spec. Dev is event-sourced, so nothing committed is lost.

use crate::ledger::Ledger;
use crate::pins;
use crate::proc;
use crate::table::{Conn, Fut, Reply, Subsystem};
use crate::worker::{self, Spec, Worker};
use crate::zap::{Request, Response};
use serde::Deserialize;
use serde_json::json;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

/// What the app registers. `sha256` is the digest of `program` from the app's
/// signed bundle; hanzod checks the file against it at every exec.
#[derive(Deserialize, Clone)]
struct Register {
    program: PathBuf,
    sha256: String,
    #[serde(default)]
    args: Vec<String>,
    cwd: Option<PathBuf>,
    #[serde(default)]
    env: Vec<(String, String)>,
    /// The worker's bootstrap, written to its stdin once.
    bootstrap: Option<serde_json::Value>,
}

pub struct Dev {
    pub ledger: Arc<Ledger>,
    spec: Mutex<Option<Register>>,
    worker: Mutex<Option<Worker>>,
}

impl Dev {
    pub fn new(ledger: Arc<Ledger>) -> Dev {
        Dev { ledger, spec: Mutex::new(None), worker: Mutex::new(None) }
    }

    /// Make sure a worker is running; start one from the registered spec if the
    /// last one is gone.
    async fn ensure(&self) -> io::Result<Arc<worker::Mux>> {
        let mut w = self.worker.lock().await;
        if let Some(cur) = w.as_mut() {
            if cur.child.try_wait()?.is_none() {
                return Ok(cur.mux.clone());
            }
        }
        let reg = self.spec.lock().await.clone().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no Dev worker is registered; POST /v1/dev/session first"))?;
        proc::owned_by_us_alone(&reg.program)?;
        let got = pins::sha256_file(&reg.program)?;
        if got != reg.sha256.to_ascii_lowercase() {
            return Err(proc::refuse(format!("{} does not match the digest the app registered", reg.program.display())));
        }
        // The person this worker serves is the one signed in on this machine; the
        // worker checks every session's bearer against it.
        let who = crate::me::active().map(|id| crate::me::label(&id)).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "nobody is signed in on this machine"))?;
        let mut env = reg.env;
        env.retain(|(k, _)| k != "HANZO_DEV_ZAP_PRINCIPAL");
        env.push(("HANZO_DEV_ZAP_PRINCIPAL".into(), who));
        let spec = Spec {
            program: reg.program,
            args: reg.args,
            cwd: reg.cwd,
            env,
            stdin: reg.bootstrap.map(|b| b.to_string().into_bytes()),
        };
        let mut fresh = worker::spawn(&spec)?;
        fresh.link = Some(tokio::spawn(crate::link::serve(fresh.mux.clone(), fresh.mux.announcements())));
        let id = self.ledger.record("hanzod", "spawn", &format!("dev worker pid {}", fresh.child.id().unwrap_or(0)));
        let mux = fresh.mux.clone();
        *w = Some(fresh);
        let _ = id;
        Ok(mux)
    }
}

impl Subsystem for Dev {
    fn name(&self) -> &'static str {
        "dev"
    }
    fn prefix(&self) -> &'static str {
        "/v1/dev"
    }
    fn idle(&self) -> Duration {
        Duration::from_secs(600)
    }
    /// Starting needs a registered spec; with none, the table starts it only once
    /// a registration has arrived (see `frame`).
    fn start(&self) -> Fut<'_, io::Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn frame<'a>(&'a self, who: &'a str, req: Request) -> Fut<'a, io::Result<Reply>> {
        Box::pin(async move {
            if req.method == "DELETE" && req.path() == "/v1/dev/session" {
                // The app is done with its worker; forget the spec so nothing restarts it.
                self.ledger.record(who, "withdraw", "dev worker");
                *self.spec.lock().await = None;
                if let Some(mut w) = self.worker.lock().await.take() {
                    proc::terminate(&mut w.child, Duration::from_secs(10)).await;
                }
                return Ok(Reply::Whole(Response::json(200, &json!({ "running": false }))));
            }
            if req.method != "POST" || req.path() != "/v1/dev/session" {
                return Ok(Reply::Whole(Response::error(404, "POST /v1/dev/session registers and starts the Dev worker")));
            }
            let reg: Register = match serde_json::from_slice(&req.body) {
                Ok(r) => r,
                Err(e) => return Ok(Reply::Whole(Response::error(400, &format!("body: {e}")))),
            };
            self.ledger.record(who, "register", &format!("dev worker {}", reg.program.display()));
            *self.spec.lock().await = Some(reg);
            match self.ensure().await {
                Ok(_) => {
                    let pid = self.worker.lock().await.as_ref().and_then(|w| w.child.id());
                    Ok(Reply::Whole(Response::json(200, &json!({ "running": true, "pid": pid }))))
                }
                Err(e) => Ok(Reply::Whole(Response::error(502, &e.to_string()))),
            }
        })
    }
    fn stream<'a>(&'a self, first: Vec<u8>, mut conn: Conn) -> Fut<'a, io::Result<()>> {
        Box::pin(async move {
            let mux = self.ensure().await?;
            let (sid, mut rx) = mux.open().await?;
            mux.send(sid, &first).await?;
            let mut buf = vec![0u8; 64 * 1024];
            let outcome: io::Result<()> = loop {
                tokio::select! {
                    n = conn.read(&mut buf) => match n {
                        Ok(0) => break Ok(()),
                        Ok(n) => if let Err(e) = mux.send(sid, &buf[..n]).await { break Err(e) },
                        Err(e) => break Err(e),
                    },
                    got = rx.recv() => match got {
                        Some(bytes) => if let Err(e) = conn.write_all(&bytes).await { break Err(e) },
                        None => break Ok(()),
                    },
                }
            };
            mux.close(sid).await;
            let _ = conn.shutdown().await;
            outcome
        })
    }
    fn stop(&self) -> Fut<'_, ()> {
        Box::pin(async move {
            if let Some(mut w) = self.worker.lock().await.take() {
                proc::terminate(&mut w.child, Duration::from_secs(10)).await;
            }
        })
    }
}
