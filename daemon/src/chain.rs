//! `/v1/chain`: opt-in custody of `hanzo-chain`, by the `steward` crate.
//! Until a PUT there is no listener, no child and no state.

use crate::ledger::Ledger;
use crate::table::{Fut, Reply, Subsystem};
use crate::zap::{Request, Response};
use std::io;
use std::sync::Arc;

pub struct Chain {
    pub steward: Arc<steward::Steward>,
    pub ledger: Arc<Ledger>,
}

impl Subsystem for Chain {
    fn name(&self) -> &'static str {
        "chain"
    }
    fn prefix(&self) -> &'static str {
        "/v1/chain"
    }
    /// The steward owns its child's life: it starts on PUT and stops on DELETE.
    fn resident(&self) -> bool {
        true
    }
    fn start(&self) -> Fut<'_, io::Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn frame<'a>(&'a self, who: &'a str, req: Request) -> Fut<'a, io::Result<Reply>> {
        Box::pin(async move {
            if req.path() != "/v1/chain" {
                return Ok(Reply::Whole(Response::error(404, "GET, PUT or DELETE /v1/chain")));
            }
            if req.method != "GET" {
                self.ledger.record(who, &req.method, "chain");
            }
            let s = self.steward.clone();
            let r = tokio::task::spawn_blocking(move || s.handle(&req.method, &req.body)).await.map_err(io::Error::other)?;
            Ok(Reply::Whole(Response::json(r.status, &r.body)))
        })
    }
    fn stop(&self) -> Fut<'_, ()> {
        Box::pin(async {})
    }
}
