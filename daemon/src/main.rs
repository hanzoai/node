// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! `hanzod` — the single unified Hanzo daemon per machine (HIP-1330).
//!
//! One Rust binary for:
//! - Local app & native Rust Dev runtime (`/v1/dev`, SQLite + `dev-core` + `dev-orchestrator`)
//! - Local inference (`/v1/engine`, `/v1/engine/catalog` — Metal offline from shared HF cache)
//! - Opt-in blockchain validator (`/v1/chain`, embedded via `--features embedded-chain` or steward)
//! - Local cloud host (`/v1/cloud`) & outbound WebTransport/ZAP relay (`link.rs`)
//! - Shared machine identity (`/v1/identity` via `hanzo-identity`)
//!
//! Owns `<data>/hanzo/hanzod.zap.sock` (0600) and loopback view `127.0.0.1:3690`.

mod auth;
mod bot;
mod catalog;
mod chain;
mod cloud;
mod cluster;
mod coding;
mod computer;
mod daemon;
mod dev;
mod dev_native;
mod engine;
mod gen_runner;
mod http;
mod ledger;
mod lifecycle;
mod link;
mod me;
mod paths;
mod pins;
mod proc;
mod research;
mod routes;
mod socket;
mod table;
mod tasks_worker;
mod team;
mod worker;

use std::process::ExitCode;
use std::sync::{Arc, OnceLock};
use tokio::net::TcpListener;
#[cfg(unix)]
use tokio::signal::unix::{signal, SignalKind};
use zaphttp as zap;

/// Where sign-in lives. Discovery at `<origin>/.well-known/openid-configuration`
/// names the issuer and keys; `signin_origin` in `hanzod.json` overrides this.
const SIGNIN_ORIGIN: &str = "https://hanzo.ai";

/// Where `hanzo-chain` releases are published when built without `embedded-chain`.
const CHAIN_RELEASES: &str = "https://github.com/hanzoai/node/releases/download";

fn config() -> serde_json::Value {
    std::fs::read(paths::data().join("hanzod.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn main() -> ExitCode {
    // Self-exec chain mode invoked by Steward (preserves clean argv for `lux-node`).
    if std::env::var("HANZOD_MODE").as_deref() == Ok("chain") {
        #[cfg(feature = "embedded-chain")]
        {
            return hanzo_chain::run();
        }
        #[cfg(not(feature = "embedded-chain"))]
        {
            eprintln!("hanzod: built without embedded-chain feature");
            return ExitCode::from(2);
        }
    }

    let arg1 = std::env::args().nth(1);
    match arg1.as_deref() {
        Some("--version") | Some("-V") | Some("version") => {
            println!(
                "hanzod {} (dev={} harness={}/{} chain={} engine=managed)",
                env!("CARGO_PKG_VERSION"),
                dev_native::HARNESS_NAME,
                dev_native::HARNESS_NAME,
                dev_native::HARNESS_VERSION,
                if cfg!(feature = "embedded-chain") { "embedded" } else { "steward" }
            );
            return ExitCode::SUCCESS;
        }
        Some("chain") => {
            #[cfg(feature = "embedded-chain")]
            {
                // Re-exec self with HANZOD_MODE=chain and stripped "chain" subcommand
                // so `lux_node::run` parses standard validator flags cleanly.
                let exe = match std::env::current_exe() {
                    Ok(e) => e,
                    Err(e) => {
                        eprintln!("hanzod chain: {e}");
                        return ExitCode::from(1);
                    }
                };
                let rest: Vec<String> = std::env::args().skip(2).collect();
                let status = proc::bg_std_cmd(exe)
                    .env("HANZOD_MODE", "chain")
                    .args(rest)
                    .status();
                return match status {
                    Ok(s) => ExitCode::from(s.code().unwrap_or(1) as u8),
                    Err(e) => {
                        eprintln!("hanzod chain: {e}");
                        ExitCode::from(1)
                    }
                };
            }
            #[cfg(not(feature = "embedded-chain"))]
            {
                eprintln!("hanzod: chain subcommand requires `--features embedded-chain` (or use PUT /v1/chain)");
                return ExitCode::from(2);
            }
        }
        Some("status") => {
            let addr = config()
                .get("http_addr")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| paths::HTTP_ADDR.into());
            match ureq::get(&format!("http://{addr}/v1/hanzod"))
                .timeout(std::time::Duration::from_secs(2))
                .call()
            {
                Ok(r) => {
                    if let Ok(text) = r.into_string() {
                        println!("{text}");
                        return ExitCode::SUCCESS;
                    }
                }
                Err(e) => eprintln!("hanzod not reachable on {addr}: {e}"),
            }
            return ExitCode::from(1);
        }
        _ => {}
    }

    proc::detach_standalone_console();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    if let Err(e) = rt.block_on(run()) {
        eprintln!("hanzod: {e}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

async fn run() -> std::io::Result<()> {
    let pins = Arc::new(pins::Pins::load(&paths::data().join("pins.json")));
    let ledger = Arc::new(ledger::Ledger::open(paths::data().join("ledger")));
    let table_cell: Arc<OnceLock<Arc<table::Table>>> = Arc::new(OnceLock::new());

    let upstream: cloud::Upstream = Arc::new(std::sync::Mutex::new(None));
    let home = dirs::home_dir().unwrap_or_default();
    let chain_pins = steward::Pins::parse(pins::COMPILED).map_err(std::io::Error::other)?;
    #[allow(unused_mut)]
    let mut steward_inner = steward::Steward::new(
        home.join(".hanzod"),
        paths::bin(),
        CHAIN_RELEASES.into(),
        chain_pins,
    );
    #[cfg(feature = "embedded-chain")]
    if let Ok(exe) = std::env::current_exe() {
        steward_inner = steward_inner.with_embedded_exe(exe);
    }
    let steward = Arc::new(steward_inner);

    let engine = Arc::new(engine::Engine::new(pins.clone()));
    let idle_notify = Arc::new(tokio::sync::Notify::new());
    let dev_sub = Arc::new(dev::Dev::with_notify(
        ledger.clone(),
        Some(engine.clone()),
        idle_notify.clone(),
    ));
    let research_sub = Arc::new(research::Research::with_notify(
        ledger.clone(),
        idle_notify.clone(),
    ));
    let bot_sub = Arc::new(bot::Bot::new(ledger.clone(), engine.clone()));
    let team_sub = Arc::new(team::Team::new(ledger.clone(), upstream.clone()));
    let cluster_core = cluster::ClusterCore::new(ledger.clone(), upstream.clone());
    let coding_core = coding::CodingCore::new(dev_sub.clone(), ledger.clone());
    let lifecycle_sub = Arc::new(lifecycle::Lifecycle::new(
        cluster_core.clone(),
        engine.clone(),
        upstream.clone(),
        dev_sub.clone(),
        ledger.clone(),
    ));
    let supervisor = lifecycle_sub.spawn_supervisor(std::time::Duration::from_secs(1));
    let tasks_worker = tasks_worker::TasksWorker::new(
        dev_sub.clone(),
        research_sub.clone(),
        bot_sub.clone(),
        team_sub.clone(),
        engine.clone(),
        cluster_core.clone(),
        idle_notify,
    );

    let subs: Vec<Arc<dyn table::Subsystem>> = vec![
        Arc::new(routes::Hanzod {
            table: table_cell.clone(),
            upstream: upstream.clone(),
            ledger: ledger.clone(),
            tasks_worker: tasks_worker.clone(),
        }),
        Arc::new(routes::Identity),
        Arc::new(computer::Computer::new()),
        lifecycle_sub,
        Arc::new(cloud::Cloud::new(pins.clone(), upstream)),
        dev_sub,
        Arc::new(coding::CodingSub {
            core: coding_core.clone(),
            prefix: "/v1/agent",
            name: "agent",
        }),
        Arc::new(coding::CodingSub {
            core: coding_core,
            prefix: "/v1/agents",
            name: "agents",
        }),
        research_sub,
        bot_sub,
        team_sub,
        Arc::new(cluster::ClusterSub {
            core: cluster_core.clone(),
            prefix: "/v1/cluster",
            name: "cluster",
        }),
        Arc::new(cluster::ClusterSub {
            core: cluster_core,
            prefix: "/v1/k3s",
            name: "k3s",
        }),
        engine.clone(),
        Arc::new(catalog::Catalog::new(engine.clone(), ledger.clone())),
        Arc::new(chain::Chain {
            steward: steward.clone(),
            ledger: ledger.clone(),
        }),
    ];
    let table = Arc::new(table::Table::new(subs));
    let _ = table_cell.set(table.clone());
    let _worker_handle = tasks_worker.spawn();
    let target_runner = gen_runner::maybe_spawn_target_runner();

    let origin = config()
        .get("signin_origin")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| SIGNIN_ORIGIN.into());
    let d = Arc::new(daemon::Daemon {
        table: table.clone(),
        verifier: Arc::new(auth::Verifier::new(origin)),
    });

    let sock = socket::bind().await?;
    let pidfile = paths::data().join("hanzod.pid");
    std::fs::write(&pidfile, std::process::id().to_string())?;
    tokio::spawn(socket::serve(sock, d.clone()));

    let http = config()
        .get("http_addr")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| paths::HTTP_ADDR.into());
    match TcpListener::bind(&http).await {
        Ok(l) => {
            tokio::spawn(http::serve(l, d.clone()));
        }
        Err(e) => eprintln!("hanzod: {http}: {e}; serving the socket only"),
    }

    tokio::task::spawn_blocking({
        let s = steward.clone();
        move || s.resume()
    });

    #[cfg(unix)]
    {
        let mut term = signal(SignalKind::terminate())?;
        let mut int = signal(SignalKind::interrupt())?;
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    if let Some(tr) = target_runner {
        tr.shutdown();
    }
    supervisor.shutdown();
    tasks_worker.shutdown();
    table.stop_all().await;
    let _ = std::fs::remove_file(paths::socket());
    let _ = std::fs::remove_file(pidfile);
    Ok(())
}
