//! The subsystems hanzod answers itself, with no process behind them.

use crate::me;
use crate::table::{Fut, Reply, Subsystem, Table};
use crate::zap::{Request, Response};
use serde_json::json;
use std::io;
use std::sync::{Arc, OnceLock};

/// `GET /v1/hanzod`: what this daemon is and which prefixes it serves.
pub struct Hanzod {
    pub table: Arc<OnceLock<Arc<Table>>>,
    pub upstream: crate::cloud::Upstream,
    pub ledger: Arc<crate::ledger::Ledger>,
}

impl Hanzod {
    /// `PUT /v1/hanzod/upstream {"addr": "127.0.0.1:PORT"}` registers a cloud
    /// that something else serves; `DELETE` withdraws it. Loopback addresses
    /// only, and socket-only (the daemon refuses it on the HTTP view).
    fn register(&self, who: &str, req: &Request) -> Response {
        match req.method.as_str() {
            "PUT" => {
                let addr = serde_json::from_slice::<serde_json::Value>(&req.body)
                    .ok()
                    .and_then(|v| v.get("addr")?.as_str()?.parse::<std::net::SocketAddr>().ok());
                match addr {
                    Some(a) if a.ip().is_loopback() => {
                        self.ledger.record(who, "register", &format!("cloud upstream {a}"));
                        *self.upstream.lock().unwrap() = Some(a);
                        Response::json(200, &json!({ "upstream": a.to_string() }))
                    }
                    _ => Response::error(400, "body must be {\"addr\": \"127.0.0.1:PORT\"}, a loopback address"),
                }
            }
            "DELETE" => {
                self.ledger.record(who, "withdraw", "cloud upstream");
                *self.upstream.lock().unwrap() = None;
                Response::json(200, &json!({ "upstream": null }))
            }
            _ => Response::error(405, "PUT or DELETE"),
        }
    }
}

impl Subsystem for Hanzod {
    fn name(&self) -> &'static str {
        "hanzod"
    }
    fn prefix(&self) -> &'static str {
        "/v1/hanzod"
    }
    fn resident(&self) -> bool {
        true
    }
    fn start(&self) -> Fut<'_, io::Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn frame<'a>(&'a self, who: &'a str, req: Request) -> Fut<'a, io::Result<Reply>> {
        Box::pin(async move {
            if req.path() == "/v1/hanzod/upstream" {
                return Ok(Reply::Whole(self.register(who, &req)));
            }
            let routes: Vec<_> = self
                .table
                .get()
                .map(|t| t.prefixes())
                .unwrap_or_default()
                .into_iter()
                .map(|(prefix, name, running)| json!({ "prefix": prefix, "name": name, "running": running }))
                .collect();
            Ok(Reply::Whole(Response::json(200, &json!({ "name": "hanzod", "version": env!("CARGO_PKG_VERSION"), "routes": routes }))))
        })
    }
    fn stop(&self) -> Fut<'_, ()> {
        Box::pin(async {})
    }
}

/// `/v1/identity`: who this machine is. Reads only; tokens stay IAM's.
///
///   GET /v1/identity         the active identity (owner, name; email when the token has one)
///   GET /v1/identity/list    every identity signed in here (metadata only)
///   GET /v1/identity/token   the active access token, if unexpired (socket only)
pub struct Identity;

impl Subsystem for Identity {
    fn name(&self) -> &'static str {
        "identity"
    }
    fn prefix(&self) -> &'static str {
        "/v1/identity"
    }
    fn resident(&self) -> bool {
        true
    }
    fn start(&self) -> Fut<'_, io::Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn frame<'a>(&'a self, _who: &'a str, req: Request) -> Fut<'a, io::Result<Reply>> {
        Box::pin(async move {
            let r = tokio::task::spawn_blocking(move || answer(&req)).await.map_err(io::Error::other)?;
            Ok(Reply::Whole(r))
        })
    }
    fn stop(&self) -> Fut<'_, ()> {
        Box::pin(async {})
    }
}

fn answer(req: &Request) -> Response {
    if req.method != "GET" {
        return Response::error(405, "GET only; sign in and switch with `hanzo auth`");
    }
    match req.path() {
        "/v1/identity" => match me::tokens() {
            Some((id, t)) => {
                let mut v = json!({ "owner": id.owner, "name": id.name, "active": true });
                if let Some(email) = hanzo_identity::identity::email(&t.access_token) {
                    v["email"] = json!(email);
                }
                Response::json(200, &v)
            }
            None => match me::active() {
                Some(id) => Response::json(200, &json!({ "owner": id.owner, "name": id.name, "active": true, "token": false })),
                None => Response::error(404, "nobody is signed in"),
            },
        },
        "/v1/identity/list" => {
            let text = std::fs::read_to_string(hanzo_identity::index::config_path()).unwrap_or_default();
            let doc: toml::Table = text.parse().unwrap_or_default();
            let held = doc.get("auth").and_then(|a| a.get("identities")).cloned();
            let v = serde_json::to_value(held.unwrap_or(toml::Value::Array(vec![]))).unwrap_or(json!([]));
            Response::json(200, &json!({ "identities": v }))
        }
        "/v1/identity/token" => match me::tokens() {
            Some((_, t)) => match me::expiry(&t.access_token) {
                Some(exp) if exp > now() + 30 => {
                    Response::json(200, &json!({ "access_token": t.access_token, "expires_at": exp }))
                }
                _ => Response::error(401, "the stored token is spent; run `hanzo auth login`"),
            },
            None => Response::error(404, "nobody is signed in"),
        },
        _ => Response::error(404, "no such identity route"),
    }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
