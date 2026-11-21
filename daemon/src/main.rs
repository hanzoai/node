// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! `hanzod` — the one Hanzo daemon on a machine (HIP-1330).
//!
//! It owns the local ZAP socket (`<data>/hanzo/hanzod.zap.sock`, 0600) and a
//! loopback view of the same routes on `127.0.0.1:3690`; it reads the machine's
//! identity through the crate the CLI shares; and it starts everything else
//! (the cloud host, the Dev worker, the inference engine) on the first frame to
//! its `/v1` prefix and stops it when idle. Idle, it holds a table and two
//! listeners and nothing else.

mod auth;
mod catalog;
mod chain;
mod cloud;
mod daemon;
mod dev;
mod engine;
mod http;
mod ledger;
mod link;
mod me;
mod paths;
mod pins;
mod proc;
mod routes;
mod socket;
mod table;
mod worker;

use std::sync::{Arc, OnceLock};
use zaphttp as zap;
use tokio::net::TcpListener;
use tokio::signal::unix::{signal, SignalKind};

/// Where sign-in lives. Discovery at `<origin>/.well-known/openid-configuration`
/// names the issuer and keys; `signin_origin` in `hanzod.json` overrides this.
const SIGNIN_ORIGIN: &str = "https://hanzo.ai";

/// Where `hanzo-chain` releases are published; the steward pins what it fetches.
const CHAIN_RELEASES: &str = "https://github.com/hanzoai/node/releases/download";

fn config() -> serde_json::Value {
    std::fs::read(paths::data().join("hanzod.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("hanzod {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
    if let Err(e) = rt.block_on(run()) {
        eprintln!("hanzod: {e}");
        std::process::exit(1);
    }
}

async fn run() -> std::io::Result<()> {
    let pins = Arc::new(pins::Pins::load(&paths::data().join("pins.json")));
    let ledger = Arc::new(ledger::Ledger::open(paths::data().join("ledger")));
    let table_cell: Arc<OnceLock<Arc<table::Table>>> = Arc::new(OnceLock::new());

    let upstream: cloud::Upstream = Arc::new(std::sync::Mutex::new(None));
    let home = dirs::home_dir().unwrap_or_default();
    let chain_pins = steward::Pins::parse(pins::COMPILED).map_err(std::io::Error::other)?;
    let steward = Arc::new(steward::Steward::new(home.join(".hanzod"), paths::bin(), CHAIN_RELEASES.into(), chain_pins));

    let engine = Arc::new(engine::Engine::new(pins.clone()));
    let subs: Vec<Arc<dyn table::Subsystem>> = vec![
        Arc::new(routes::Hanzod { table: table_cell.clone(), upstream: upstream.clone(), ledger: ledger.clone() }),
        Arc::new(routes::Identity),
        Arc::new(cloud::Cloud::new(pins.clone(), upstream)),
        Arc::new(dev::Dev::new(ledger.clone())),
        engine.clone(),
        Arc::new(catalog::Catalog::new(engine.clone(), ledger.clone())),
        Arc::new(chain::Chain { steward: steward.clone(), ledger: ledger.clone() }),
    ];
    let table = Arc::new(table::Table::new(subs));
    let _ = table_cell.set(table.clone());

    let origin = config().get("signin_origin").and_then(|v| v.as_str()).map(str::to_owned).unwrap_or_else(|| SIGNIN_ORIGIN.into());
    let d = Arc::new(daemon::Daemon { table: table.clone(), verifier: Arc::new(auth::Verifier::new(origin)) });

    let sock = socket::bind().await?;
    // Clients stop the daemon by this pid; it is removed on a clean exit.
    let pidfile = paths::data().join("hanzod.pid");
    std::fs::write(&pidfile, std::process::id().to_string())?;
    tokio::spawn(socket::serve(sock, d.clone()));
    // `http_addr` in `hanzod.json` moves the loopback view when :3690 is taken (a second daemon, a second user).
    let http = config().get("http_addr").and_then(|v| v.as_str()).map(str::to_owned).unwrap_or_else(|| paths::HTTP_ADDR.into());
    match TcpListener::bind(&http).await {
        Ok(l) => {
            tokio::spawn(http::serve(l, d.clone()));
        }
        // :3690 held by something else is reported, not fought over: the socket still serves.
        Err(e) => eprintln!("hanzod: {http}: {e}; serving the socket only"),
    }
    // Bring the chain back if the user had turned it on; a no-op otherwise.
    tokio::task::spawn_blocking({
        let s = steward.clone();
        move || s.resume()
    });

    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
    table.stop_all().await;
    let _ = std::fs::remove_file(paths::socket());
    let _ = std::fs::remove_file(pidfile);
    Ok(())
}
