// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! In-process C-ABI library entry points for embedding `hanzod` inside iOS / mobile / desktop apps.

mod auth;
mod bot;
mod catalog;
mod chain;
mod cloud;
mod cluster;
mod coding;
mod daemon;
mod dev;
mod dev_native;
mod engine;
mod gen_runner;
mod http;
mod ledger;
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

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use zaphttp as zap;

const SIGNIN_ORIGIN: &str = "https://hanzo.ai";
const CHAIN_RELEASES: &str = "https://github.com/hanzoai/node/releases/download";

struct RuntimeHandle {
    shutdown: Option<oneshot::Sender<()>>,
    port: u16,
    data_dir: String,
}

static RUNNING: AtomicBool = AtomicBool::new(false);
static HANDLE: OnceLock<Mutex<Option<RuntimeHandle>>> = OnceLock::new();

fn handle_cell() -> &'static Mutex<Option<RuntimeHandle>> {
    HANDLE.get_or_init(|| Mutex::new(None))
}

fn config() -> serde_json::Value {
    std::fs::read(paths::data().join("hanzod.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Start the embedded `hanzod` daemon on a background thread.
/// Returns 0 on success (including when already running), non-zero on error.
#[no_mangle]
pub extern "C" fn hanzod_ffi_start(data_dir: *const c_char, port: u16) -> c_int {
    let mut guard = match handle_cell().lock() {
        Ok(g) => g,
        Err(_) => return -1,
    };
    if RUNNING.load(Ordering::SeqCst) {
        return 0;
    }

    let dir_str = if !data_dir.is_null() {
        match unsafe { CStr::from_ptr(data_dir) }.to_str() {
            Ok(s) if !s.trim().is_empty() => {
                std::env::set_var("HANZO_DATA_DIR", s.trim());
                s.trim().to_string()
            }
            _ => paths::data().display().to_string(),
        }
    } else {
        paths::data().display().to_string()
    };

    let listen_port = if port == 0 { 3690 } else { port };
    let (tx, rx) = oneshot::channel::<()>();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();

    let spawn_res = std::thread::Builder::new()
        .name("hanzod-embedded".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(r) => r,
                Err(e) => {
                    let _ = ready_tx.send(Err(e.to_string()));
                    return;
                }
            };
            rt.block_on(async move {
                let data = paths::data();
                let _ = std::fs::create_dir_all(&data);
                let pins = Arc::new(pins::Pins::load(&data.join("pins.json")));
                let ledger = Arc::new(ledger::Ledger::open(data.join("ledger")));
                let table_cell: Arc<OnceLock<Arc<table::Table>>> = Arc::new(OnceLock::new());
                let upstream: cloud::Upstream = Arc::new(Mutex::new(None));
                let chain_pins = match steward::Pins::parse(pins::COMPILED) {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.to_string()));
                        return;
                    }
                };
                #[allow(unused_mut)]
                let mut steward_inner = steward::Steward::new(
                    data.join("chain"),
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
                    Arc::new(catalog::Catalog::new(engine, ledger.clone())),
                    Arc::new(chain::Chain { steward, ledger }),
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

                if let Ok(sock) = socket::bind().await {
                    tokio::spawn(socket::serve(sock, d.clone()));
                }
                let addr = format!("127.0.0.1:{listen_port}");
                match TcpListener::bind(&addr).await {
                    Ok(l) => {
                        tokio::spawn(http::serve(l, d.clone()));
                        RUNNING.store(true, Ordering::SeqCst);
                        let _ = ready_tx.send(Ok(()));
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(format!("bind {addr}: {e}")));
                        return;
                    }
                }

                let _ = rx.await;
                if let Some(tr) = target_runner {
                    tr.shutdown();
                }
                tasks_worker.shutdown();
                table.stop_all().await;
                let _ = std::fs::remove_file(paths::socket());
                RUNNING.store(false, Ordering::SeqCst);
            });
        });

    if spawn_res.is_err() {
        return -2;
    }

    match ready_rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(Ok(())) => {
            *guard = Some(RuntimeHandle {
                shutdown: Some(tx),
                port: listen_port,
                data_dir: dir_str,
            });
            0
        }
        _ => -3,
    }
}

/// Stop the embedded `hanzod` daemon if running.
#[no_mangle]
pub extern "C" fn hanzod_ffi_stop() -> c_int {
    let mut guard = match handle_cell().lock() {
        Ok(g) => g,
        Err(_) => return -1,
    };
    if let Some(mut handle) = guard.take() {
        if let Some(tx) = handle.shutdown.take() {
            let _ = tx.send(());
        }
    }
    RUNNING.store(false, Ordering::SeqCst);
    0
}

/// Returns 1 if the embedded `hanzod` daemon is currently running, 0 otherwise.
#[no_mangle]
pub extern "C" fn hanzod_ffi_running() -> c_int {
    if RUNNING.load(Ordering::SeqCst) { 1 } else { 0 }
}

/// Returns a newly allocated JSON string describing daemon status; free with `hanzod_ffi_free_string`.
#[no_mangle]
pub extern "C" fn hanzod_ffi_status_json() -> *mut c_char {
    let (port, data_dir) = handle_cell()
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|h| (h.port, h.data_dir.clone())))
        .unwrap_or_else(|| (3690, paths::data().display().to_string()));
    let payload = serde_json::json!({
        "running": RUNNING.load(Ordering::SeqCst),
        "version": env!("CARGO_PKG_VERSION"),
        "harness": dev_native::HARNESS_NAME,
        "harness_version": dev_native::HARNESS_VERSION,
        "port": port,
        "dataDir": data_dir,
        "sole_desktop_backend": true,
        "capabilities": {
            "dev": dev_native::HARNESS_NAME,
            "coding": dev_native::HARNESS_NAME,
            "research": "rust-native",
            "bot": "rust-native",
            "team": "rust-native",
            "tasks": "linked-worker",
            "cluster": "k3s-orchestrator",
            "chain": if cfg!(feature = "embedded-chain") { "embedded" } else { "steward" },
            "engine": "managed",
            "cloud": true,
            "zap": true
        },
        "routes": ["/healthz", "/.well-known/hanzo-standard.json", "/v1/hanzod", "/v1/hanzod/worker", "/v1/identity", "/v1/dev", "/v1/agent", "/v1/agents", "/v1/research", "/v1/bot", "/v1/team", "/v1/cluster", "/v1/k3s", "/v1/chain", "/v1/engine", "/v1/engine/catalog"],
    });
    match CString::new(payload.to_string()) {
        Ok(cs) => cs.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Frees a string returned by `hanzod_ffi_status_json`.
#[no_mangle]
pub extern "C" fn hanzod_ffi_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        unsafe {
            drop(CString::from_raw(ptr));
        }
    }
}
