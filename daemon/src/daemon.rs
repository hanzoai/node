//! The one decision point: every request, from the socket or from the loopback
//! view, passes through `Daemon::handle`. Who is calling is settled here, before
//! any subsystem sees the frame.
//!
//! * `/healthz` is open.
//! * A bearer, when present, is verified against the issuer's published keys and
//!   must name the person this machine is signed in as.
//! * On the socket (mode 0600, peer uid checked) no bearer is needed for hanzod's
//!   own routes; cloud's own routes enforce IAM themselves.
//! * On the loopback port (reachable by any local process, and by a web page)
//!   every route needs a verified bearer, except `/v1/engine`, which is local
//!   inference and is guarded by Host and Origin checks in `http`.
//! * The access token route is socket-only.

use crate::me;
use crate::table::{Busy, Reply, Table};
use crate::auth::Verifier;
use crate::zap::{Request, Response};
use std::io;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Via {
    Socket,
    Http,
}

pub struct Daemon {
    pub table: Arc<Table>,
    pub verifier: Arc<Verifier>,
}

fn refuse(status: u16, why: &str) -> (Reply, Option<Busy>) {
    (Reply::Whole(Response::error(status, why)), None)
}

impl Daemon {
    /// Verify `bearer` and check that it is the machine's own person. Returns the
    /// principal, `owner/name`.
    pub async fn who(&self, bearer: &str) -> Result<String, (u16, String)> {
        let v = self.verifier.clone();
        let bearer = bearer.to_string();
        let r = tokio::task::spawn_blocking(move || {
            let principal = v.verify(&bearer)?;
            match me::active() {
                Some(id) if me::label(&id) != principal => Err(format!("{principal} is not the person signed in on this machine")),
                Some(_) => Ok(principal),
                None => Err("nobody is signed in on this machine".to_string()),
            }
        })
        .await
        .map_err(|e| (500, e.to_string()))?;
        r.map_err(|e| (401, e))
    }

    pub async fn handle(&self, via: Via, req: Request) -> (Reply, Option<Busy>) {
        let path = req.path().to_string();
        if path == "/healthz" {
            return (Reply::Whole(Response::json(200, &serde_json::json!({ "ok": true }))), None);
        }
        if req.method == "GET" && path == "/.well-known/hanzo-standard.json" {
            let doc: serde_json::Value = serde_json::from_str(crate::research::STANDARD_JSON)
                .unwrap_or_else(|_| serde_json::json!({}));
            return (Reply::Whole(Response::json(200, &doc)), None);
        }
        let local_inference = path == "/v1/engine" || path.starts_with("/v1/engine/");
        let local_probe = req.method == "GET"
            && matches!(
                path.as_str(),
                "/v1/hanzod"
                    | "/v1/hanzod/worker"
                    | "/v1/dev/health"
                    | "/v1/research/health"
                    | "/v1/research/standard"
                    | "/v1/bot/health"
                    | "/v1/team/health"
                    | "/v1/cluster"
                    | "/v1/cluster/health"
                    | "/v1/k3s"
                    | "/v1/k3s/health"
            );
        let who = match req.bearer() {
            Some(b) => match self.who(b).await {
                Ok(p) => p,
                Err((s, e)) => return refuse(s, &e),
            },
            None if via == Via::Http && !local_inference && !local_probe => return refuse(401, "a bearer is required"),
            None => me::active().map(|id| me::label(&id)).unwrap_or_default(),
        };
        if via == Via::Http && (path == "/v1/identity/token" || path == "/v1/hanzod/upstream") {
            return refuse(403, "this route is served on the socket only");
        }
        let (sub, busy) = match self.table.route(&path).await {
            Ok(r) => r,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return refuse(404, "no such route"),
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => return refuse(403, &e.to_string()),
            Err(e) => return refuse(502, &e.to_string()),
        };
        match sub.frame(&who, req).await {
            Ok(r) => (r, Some(busy)),
            Err(e) => {
                self.table.down(sub.prefix());
                refuse(502, &e.to_string())
            }
        }
    }
}
