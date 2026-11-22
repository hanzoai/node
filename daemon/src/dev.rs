// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! `/v1/dev`: Unified Hanzo Dev subsystem inside `hanzod` (HIP-1330).
//!
//! Native Rust runtime (`dev_native::DevNative`) runs in-process with zero
//! Node.js dependency:
//!   GET    /v1/dev/health
//!   GET    /v1/dev/sessions
//!   POST   /v1/dev/sessions
//!   GET    /v1/dev/sessions/:id
//!   DELETE /v1/dev/sessions/:id
//!   POST   /v1/dev/sessions/:id/turns
//!   POST   /v1/dev/sessions/:id/interrupt
//!   POST   /v1/dev/sessions/:id/suspend
//!   POST   /v1/dev/sessions/:id/resume
//!   POST   /v1/dev/decide
//!
//! Backward-compatible external worker registration is preserved for older
//! desktop builds that still register a worker:
//!   POST   /v1/dev/session   register and start an external Dev worker
//!   DELETE /v1/dev/session   stop and withdraw the external worker

use crate::dev_native::{CreateSessionReq, DevNative, RunTurnReq};
use crate::engine::Engine;
use crate::ledger::Ledger;
use crate::pins;
use crate::proc;
use crate::table::{Conn, Fut, Reply, Subsystem};
use crate::worker::{self, Spec, Worker};
use crate::zap::{Request, Response};
use serde::Deserialize;
use serde_json::{json, value::RawValue};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

/// What an older client registers when using an external worker.
#[derive(Deserialize, Clone)]
struct Register {
    program: PathBuf,
    sha256: String,
    #[serde(default)]
    args: Vec<String>,
    cwd: Option<PathBuf>,
    #[serde(default)]
    env: Vec<(String, String)>,
    bootstrap: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct DecideReq<'a> {
    name: String,
    #[serde(borrow)]
    input: &'a RawValue,
}

pub struct Dev {
    pub ledger: Arc<Ledger>,
    pub native: Arc<DevNative>,
    spec: Mutex<Option<Register>>,
    worker: Mutex<Option<Worker>>,
}

impl Dev {
    pub fn new(ledger: Arc<Ledger>, engine: Option<Arc<Engine>>) -> Dev {
        let native = Arc::new(DevNative::new(ledger.clone(), engine));
        Dev {
            ledger,
            native,
            spec: Mutex::new(None),
            worker: Mutex::new(None),
        }
    }

    async fn worker_running(&self) -> bool {
        let mut w = self.worker.lock().await;
        match w.as_mut() {
            Some(cur) => matches!(cur.child.try_wait(), Ok(None)),
            None => false,
        }
    }

    /// Ensure an external worker is running if one was explicitly registered.
    async fn ensure(&self) -> io::Result<Arc<worker::Mux>> {
        let mut w = self.worker.lock().await;
        if let Some(cur) = w.as_mut() {
            if cur.child.try_wait()?.is_none() {
                return Ok(cur.mux.clone());
            }
        }
        let reg = self.spec.lock().await.clone().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no external Dev worker registered; use native /v1/dev/sessions",
            )
        })?;
        proc::owned_by_us_alone(&reg.program)?;
        let got = pins::sha256_file(&reg.program)?;
        if got != reg.sha256.to_ascii_lowercase() {
            return Err(proc::refuse(format!(
                "{} does not match the digest the app registered",
                reg.program.display()
            )));
        }
        let who = crate::me::active()
            .map(|id| crate::me::label(&id))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "nobody is signed in on this machine"))?;
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
        fresh.link = Some(tokio::spawn(crate::link::serve(
            fresh.mux.clone(),
            fresh.mux.announcements(),
        )));
        let _ = self.ledger.record(
            "hanzod",
            "spawn",
            &format!("dev worker pid {}", fresh.child.id().unwrap_or(0)),
        );
        let mux = fresh.mux.clone();
        *w = Some(fresh);
        Ok(mux)
    }

    async fn handle_native(&self, who: &str, req: Request) -> Response {
        let path = req.path().to_string();
        let method = req.method.as_str();

        // Health
        if method == "GET" && (path == "/v1/dev/health" || path == "/v1/dev") {
            let ext = self.worker_running().await;
            return Response::json(200, &self.native.health(ext));
        }

        // Pure Rust orchestrator decide endpoint
        if method == "POST" && path == "/v1/dev/decide" {
            let body_str = match std::str::from_utf8(&req.body) {
                Ok(s) => s,
                Err(_) => return Response::error(400, "body must be utf-8 json"),
            };
            let parsed: DecideReq<'_> = match serde_json::from_str(body_str) {
                Ok(p) => p,
                Err(e) => return Response::error(400, &format!("body: {e}")),
            };
            return match self.native.decide(&parsed.name, parsed.input) {
                Ok(Some(raw)) => Response {
                    status: 200,
                    headers: vec![("content-type".into(), "application/json".into())],
                    body: raw.into_bytes(),
                },
                Ok(None) => Response::error(404, &format!("unknown decide rule {}", parsed.name)),
                Err(e) => Response::error(400, &e),
            };
        }

        // Collection: /v1/dev/sessions
        if path == "/v1/dev/sessions" {
            return match method {
                "GET" => match self.native.list_sessions() {
                    Ok(sessions) => Response::json(200, &json!({ "sessions": sessions })),
                    Err(e) => Response::error(500, &e.to_string()),
                },
                "POST" => {
                    let create_req: CreateSessionReq = if req.body.is_empty() {
                        CreateSessionReq {
                            label: None,
                            workspace: None,
                            model: None,
                            prelude: None,
                        }
                    } else {
                        match serde_json::from_slice(&req.body) {
                            Ok(r) => r,
                            Err(e) => return Response::error(400, &format!("body: {e}")),
                        }
                    };
                    match self.native.create_session(who, create_req) {
                        Ok(s) => Response::json(201, &json!({ "session": s, "id": s.id, "runtime": s.runtime })),
                        Err(e) => Response::error(500, &e.to_string()),
                    }
                }
                _ => Response::error(405, "GET or POST /v1/dev/sessions"),
            };
        }

        // Item routes: /v1/dev/sessions/:id[/:action]
        if let Some(rest) = path.strip_prefix("/v1/dev/sessions/") {
            let parts: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
            match parts.as_slice() {
                [id] => match method {
                    "GET" => match self.native.get_session_detail(id) {
                        Ok(detail) => Response::json(200, &detail),
                        Err(e) if e.kind() == io::ErrorKind::NotFound => Response::error(404, &e.to_string()),
                        Err(e) => Response::error(500, &e.to_string()),
                    },
                    "DELETE" => match self.native.delete_session(who, id) {
                        Ok(true) => Response::json(200, &json!({ "deleted": true, "id": id })),
                        Ok(false) => Response::error(404, &format!("session {id} not found")),
                        Err(e) => Response::error(500, &e.to_string()),
                    },
                    _ => Response::error(405, "GET or DELETE /v1/dev/sessions/:id"),
                },
                [id, "turns"] if method == "POST" => {
                    let turn_req: RunTurnReq = match serde_json::from_slice(&req.body) {
                        Ok(r) => r,
                        Err(e) => return Response::error(400, &format!("body: {e}")),
                    };
                    let bearer = req.bearer().map(str::to_owned);
                    match self.native.run_turn(who, id, turn_req, bearer).await {
                        Ok(res) => Response::json(200, &json!(res)),
                        Err(e) if e.kind() == io::ErrorKind::NotFound => Response::error(404, &e.to_string()),
                        Err(e) => Response::error(502, &e.to_string()),
                    }
                }
                [id, "interrupt"] if method == "POST" => match self.native.set_status(who, id, "interrupted") {
                    Ok(s) => Response::json(200, &json!({ "session": s })),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => Response::error(404, &e.to_string()),
                    Err(e) => Response::error(500, &e.to_string()),
                },
                [id, "suspend"] if method == "POST" => match self.native.set_status(who, id, "suspended") {
                    Ok(s) => Response::json(200, &json!({ "session": s })),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => Response::error(404, &e.to_string()),
                    Err(e) => Response::error(500, &e.to_string()),
                },
                [id, "resume"] if method == "POST" => match self.native.set_status(who, id, "active") {
                    Ok(s) => Response::json(200, &json!({ "session": s })),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => Response::error(404, &e.to_string()),
                    Err(e) => Response::error(500, &e.to_string()),
                },
                _ => Response::error(404, "unknown /v1/dev/sessions route"),
            }
        } else {
            Response::error(
                404,
                "use /v1/dev/health, /v1/dev/sessions, /v1/dev/decide, or POST /v1/dev/session",
            )
        }
    }
}

impl Subsystem for Dev {
    fn name(&self) -> &'static str {
        "dev"
    }
    fn prefix(&self) -> &'static str {
        "/v1/dev"
    }
    fn resident(&self) -> bool {
        true
    }
    fn idle(&self) -> Duration {
        Duration::from_secs(600)
    }
    fn start(&self) -> Fut<'_, io::Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn frame<'a>(&'a self, who: &'a str, req: Request) -> Fut<'a, io::Result<Reply>> {
        Box::pin(async move {
            // Legacy external worker registration/withdrawal
            if req.path() == "/v1/dev/session" {
                if req.method == "DELETE" {
                    self.ledger.record(who, "withdraw", "dev worker");
                    *self.spec.lock().await = None;
                    if let Some(mut w) = self.worker.lock().await.take() {
                        proc::terminate(&mut w.child, Duration::from_secs(10)).await;
                    }
                    return Ok(Reply::Whole(Response::json(200, &json!({ "running": false }))));
                }
                if req.method == "POST" {
                    let reg: Register = match serde_json::from_slice(&req.body) {
                        Ok(r) => r,
                        Err(e) => return Ok(Reply::Whole(Response::error(400, &format!("body: {e}")))),
                    };
                    self.ledger
                        .record(who, "register", &format!("dev worker {}", reg.program.display()));
                    *self.spec.lock().await = Some(reg);
                    return match self.ensure().await {
                        Ok(_) => {
                            let pid = self.worker.lock().await.as_ref().and_then(|w| w.child.id());
                            Ok(Reply::Whole(Response::json(200, &json!({ "running": true, "pid": pid }))))
                        }
                        Err(e) => Ok(Reply::Whole(Response::error(502, &e.to_string()))),
                    };
                }
            }

            Ok(Reply::Whole(self.handle_native(who, req).await))
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
