// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! Unified Rust `hanzo-gen` harness & session runtime inside `hanzod` (v0.5.0).
//!
//! - Primary execution path (`gen_driver`): embeds `hanzo_gen::embedded::run_embedded_turn`
//!   directly in-process with `StaticHost` (Cloud IAM token refresh or local `hanzod`
//!   engine), native `hanzo-mcp` + `hanzoskills.com`, multi-turn `backend_session_id`
//!   resume across `dev` / `claude` / `codex`, and live watch-channel cancellation.
//! - Zero-dependency fallback (`core_fallback`): pure `dev_core::Session` loop when no
//!   external coding agent binary is installed on minimal hosts or unit tests.
//! - Durable storage (`store`): WAL-mode SQLite (`<data>/hanzo/dev/state.sqlite`) with
//!   seamless `backend_session_id` migration, deduplicated `action_ledger`, and git
//!   checkpoints.

mod core_fallback;
mod gen_driver;
mod store;

use crate::engine::Engine;
use crate::ledger::Ledger;
use crate::paths;
use dev_protocol::Message;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, value::RawValue, Value};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{watch, Notify};

pub const HARNESS_NAME: &str = "hanzo-gen";
pub const HARNESS_VERSION: &str = "0.2.0";
pub(super) const DEFAULT_CLOUD_API: &str = "https://api.hanzo.ai";

static ID_COUNTER: AtomicU64 = AtomicU64::new(1);

pub(super) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub(super) fn new_session_id() -> String {
    let ts = now_ms() as u64;
    let seq = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("dev_{ts:x}_{seq:04x}")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub label: String,
    pub workspace: String,
    pub model: Option<String>,
    pub status: String,
    pub runtime: String,
    pub next_turn_id: u64,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub backend_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointRow {
    pub seq: u64,
    pub git_ref: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerEventRow {
    pub action_id: u64,
    pub kind: String,
    pub summary: String,
    pub failed: bool,
    pub completed_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnActionRecord {
    pub id: u64,
    pub kind: String,
    pub summary: String,
    pub failed: bool,
    pub replayed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnResult {
    pub session_id: String,
    pub turn_id: u64,
    pub outcome: String,
    pub text: Option<String>,
    pub emitted: Vec<String>,
    pub actions: Vec<TurnActionRecord>,
    pub messages: Vec<Message>,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct CreateSessionReq {
    pub label: Option<String>,
    pub workspace: Option<String>,
    pub model: Option<String>,
    pub prelude: Option<String>,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct RunTurnReq {
    pub prompt: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub backend: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub link_session_id: Option<String>,
}

pub(super) struct TurnTracker {
    pub(super) active_turns: AtomicUsize,
    pub(super) session_counts: Mutex<HashMap<String, usize>>,
    pub(super) active_cancels: Mutex<HashMap<String, (u64, watch::Sender<bool>)>>,
    pub(super) token_seq: AtomicU64,
    pub(super) idle_notify: Arc<Notify>,
}

pub struct OwnedTurnGuard {
    tracker: Arc<TurnTracker>,
    session_id: String,
    slot_token: u64,
}

impl Drop for OwnedTurnGuard {
    fn drop(&mut self) {
        {
            let mut counts = self.tracker.session_counts.lock().unwrap();
            if let Some(c) = counts.get_mut(&self.session_id) {
                *c = c.saturating_sub(1);
                if *c == 0 {
                    counts.remove(&self.session_id);
                }
            }
        }
        {
            let mut cancels = self.tracker.active_cancels.lock().unwrap();
            if cancels
                .get(&self.session_id)
                .is_some_and(|(tok, _)| *tok == self.slot_token)
            {
                cancels.remove(&self.session_id);
            }
        }
        if self.tracker.active_turns.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.tracker.idle_notify.notify_waiters();
        }
    }
}

pub struct DevNative {
    pub(super) db_path: PathBuf,
    pub(super) root_override: Option<PathBuf>,
    pub(super) conn: Mutex<Option<Connection>>,
    pub(super) ledger: Arc<Ledger>,
    pub(super) engine: Option<Arc<Engine>>,
    pub(super) tracker: Arc<TurnTracker>,
}

impl DevNative {
    #[allow(dead_code)]
    pub fn new(ledger: Arc<Ledger>, engine: Option<Arc<Engine>>) -> Self {
        Self::with_notify(ledger, engine, Arc::new(Notify::new()))
    }

    pub fn with_notify(
        ledger: Arc<Ledger>,
        engine: Option<Arc<Engine>>,
        idle_notify: Arc<Notify>,
    ) -> Self {
        let db_path = paths::data().join("dev").join("state.sqlite");
        Self::with_db_path_and_notify(db_path, ledger, engine, idle_notify)
    }

    #[allow(dead_code)]
    pub fn with_db_path(
        db_path: PathBuf,
        ledger: Arc<Ledger>,
        engine: Option<Arc<Engine>>,
    ) -> Self {
        Self::with_db_path_and_notify(db_path, ledger, engine, Arc::new(Notify::new()))
    }

    pub fn with_db_path_and_notify(
        db_path: PathBuf,
        ledger: Arc<Ledger>,
        engine: Option<Arc<Engine>>,
        idle_notify: Arc<Notify>,
    ) -> Self {
        Self::build(db_path, None, ledger, engine, idle_notify)
    }

    #[cfg(test)]
    pub fn with_db_and_root(
        db_path: PathBuf,
        root_dir: PathBuf,
        ledger: Arc<Ledger>,
        engine: Option<Arc<Engine>>,
    ) -> Self {
        Self::build(
            db_path,
            Some(root_dir),
            ledger,
            engine,
            Arc::new(Notify::new()),
        )
    }

    fn build(
        db_path: PathBuf,
        root_override: Option<PathBuf>,
        ledger: Arc<Ledger>,
        engine: Option<Arc<Engine>>,
        idle_notify: Arc<Notify>,
    ) -> Self {
        Self {
            db_path,
            root_override,
            conn: Mutex::new(None),
            ledger,
            engine,
            tracker: Arc::new(TurnTracker {
                active_turns: AtomicUsize::new(0),
                session_counts: Mutex::new(HashMap::new()),
                active_cancels: Mutex::new(HashMap::new()),
                token_seq: AtomicU64::new(1),
                idle_notify,
            }),
        }
    }

    pub fn active_turns(&self) -> usize {
        self.tracker.active_turns.load(Ordering::SeqCst)
    }

    pub fn is_session_running(&self, session_id: &str) -> bool {
        self.tracker
            .session_counts
            .lock()
            .unwrap()
            .get(session_id)
            .copied()
            .unwrap_or(0)
            > 0
    }

    pub fn acquire_turn_slot(&self, session_id: &str) -> (OwnedTurnGuard, watch::Receiver<bool>) {
        self.tracker.active_turns.fetch_add(1, Ordering::SeqCst);
        {
            let mut counts = self.tracker.session_counts.lock().unwrap();
            *counts.entry(session_id.to_string()).or_insert(0) += 1;
        }
        let slot_token = self.tracker.token_seq.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = watch::channel(false);
        self.tracker
            .active_cancels
            .lock()
            .unwrap()
            .insert(session_id.to_string(), (slot_token, tx));
        (
            OwnedTurnGuard {
                tracker: self.tracker.clone(),
                session_id: session_id.to_string(),
                slot_token,
            },
            rx,
        )
    }

    pub(super) fn resolve_gen_root(&self) -> hanzo_gen::Root {
        if let Some(ref dir) = self.root_override {
            return hanzo_gen::Root::at(dir.clone());
        }
        hanzo_gen::Root::user().unwrap_or_else(|| {
            hanzo_gen::Root::at(
                self.db_path
                    .parent()
                    .unwrap_or(Path::new("."))
                    .to_path_buf(),
            )
        })
    }

    pub fn health(&self, external_worker_running: bool) -> Value {
        let (total, active) = self.session_counts_db().unwrap_or((0, 0));
        json!({
            "ok": true,
            "runtime": HARNESS_NAME,
            "harness": HARNESS_NAME,
            "harness_version": HARNESS_VERSION,
            "version": env!("CARGO_PKG_VERSION"),
            "sessions": total,
            "active_sessions": active,
            "active_turns": self.active_turns(),
            "external_worker": external_worker_running,
            "embedded_chain": cfg!(feature = "embedded-chain"),
            "engine_port": self.engine.as_ref().and_then(|e| e.port()),
            "sqlite_path": self.db_path.display().to_string(),
        })
    }

    /// Execute one turn on `session_id` via `hanzo_gen::embedded` when the agent
    /// binary is installed, or fall back to in-process `dev_core::Session`.
    pub async fn run_turn(
        &self,
        who: &str,
        session_id: &str,
        req: RunTurnReq,
        request_bearer: Option<String>,
    ) -> io::Result<TurnResult> {
        let (_guard, cancel_rx) = self.acquire_turn_slot(session_id);
        self.run_turn_with_cancel(who, session_id, req, request_bearer, cancel_rx)
            .await
    }

    /// Execute a turn using a pre-acquired [`OwnedTurnGuard`] and `cancel_rx` (used by
    /// async `202 Accepted` `/v1/agent/coding` runs so status and cancellation are live
    /// before the HTTP response returns).
    pub async fn run_turn_with_cancel(
        &self,
        who: &str,
        session_id: &str,
        req: RunTurnReq,
        request_bearer: Option<String>,
        cancel_rx: watch::Receiver<bool>,
    ) -> io::Result<TurnResult> {
        let force_core = req.runtime.as_deref() == Some("wasm-core")
            || std::env::var("HANZOD_DEV_FORCE_CORE").ok().as_deref() == Some("1");

        if !force_core {
            let root = self.resolve_gen_root();
            let agent_bin = Self::agent_binary_name(req.backend.as_deref(), &root);
            if hanzo_gen::exe::find_for_root(&root, &agent_bin).is_some() {
                return self
                    .run_turn_gen(who, session_id, req, request_bearer, root, cancel_rx)
                    .await;
            }
        }

        self.run_turn_core_fallback(who, session_id, req, request_bearer, cancel_rx)
            .await
    }

    fn agent_binary_name(backend_opt: Option<&str>, root: &hanzo_gen::Root) -> String {
        match backend_opt.map(str::trim).filter(|s| !s.is_empty()) {
            Some("gen") | None => {
                hanzo_gen::configured_agent(root).unwrap_or_else(|| "dev".to_string())
            }
            Some("claude" | "claude-code" | "cc") => "claude".to_string(),
            Some("codex") => "codex".to_string(),
            Some("interpreter" | "open-interpreter" | "oi") => "interpreter".to_string(),
            Some(other) => other.to_string(),
        }
    }

    /// Invoke a pure `dev-orchestrator` decide rule directly in Rust.
    pub fn decide(&self, name: &str, input: &RawValue) -> Result<Option<String>, String> {
        match dev_orchestrator::apply(name, input) {
            Some(Ok(out)) => Ok(Some(out)),
            Some(Err(e)) => Err(e.to_string()),
            None => Ok(None),
        }
    }
}
