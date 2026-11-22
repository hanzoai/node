// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! Native Rust Dev Runtime embedded directly inside `hanzod` (HIP-1330 phases 2–5).
//!
//! Replaces the Node/TypeScript Dev server for local sessions, persistence,
//! checkpoints, orchestrator decide rules, and model + tool execution:
//!
//! - Reasoning core: `dev-core::Session` + `dev-protocol` (`Event` -> `Action`).
//! - Pure decide rules: `dev-orchestrator::apply`.
//! - Durable storage: WAL-mode SQLite (`<data>/hanzo/dev/state.sqlite`) holding
//!   session snapshots, at-most-once deduplicated `action_ledger`, and git
//!   checkpoints.
//! - Model routing: local `hanzod` inference (`/v1/engine/chat/completions`)
//!   when a local catalog model is requested, or Hanzo Cloud API
//!   (`https://api.hanzo.ai/v1/chat/completions`) with the machine's IAM bearer.
//! - Effects: workspace-confined `Read`, `Write`, `Patch`, `Exec`, `Git`, and
//!   bounded `Browse`, each audited to both SQLite `action_ledger` and `hanzod`'s
//!   global `Ledger`.

use crate::catalog;
use crate::engine::Engine;
use crate::ledger::Ledger;
use crate::me;
use crate::paths;
use dev_core::Session;
use dev_protocol::{
    Answer, Ask, Browse, Call, Cancel, Config, Exec, Git, Message, Op, Outcome, Output, Patch,
    Read, Reply, Turn, Write,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, value::RawValue, Value};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::process::Command;

const DEFAULT_CLOUD_API: &str = "https://api.hanzo.ai";
const MAX_TOOL_ROUNDS: usize = 24;
const MAX_OUTPUT_BYTES: usize = 256 * 1024;
const EXEC_TIMEOUT: Duration = Duration::from_secs(60);
const GIT_TIMEOUT: Duration = Duration::from_secs(30);

static ID_COUNTER: AtomicU64 = AtomicU64::new(1);

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn new_session_id() -> String {
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointRow {
    pub seq: u64,
    pub git_ref: String,
    pub created_at: i64,
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

#[derive(Debug, Deserialize)]
pub struct CreateSessionReq {
    pub label: Option<String>,
    pub workspace: Option<String>,
    pub model: Option<String>,
    pub prelude: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RunTurnReq {
    pub prompt: String,
    pub model: Option<String>,
}

pub struct DevNative {
    db_path: PathBuf,
    conn: Mutex<Option<Connection>>,
    ledger: Arc<Ledger>,
    engine: Option<Arc<Engine>>,
}

impl DevNative {
    pub fn new(ledger: Arc<Ledger>, engine: Option<Arc<Engine>>) -> Self {
        let db_path = paths::data().join("dev").join("state.sqlite");
        Self::with_db_path(db_path, ledger, engine)
    }

    pub fn with_db_path(
        db_path: PathBuf,
        ledger: Arc<Ledger>,
        engine: Option<Arc<Engine>>,
    ) -> Self {
        Self {
            db_path,
            conn: Mutex::new(None),
            ledger,
            engine,
        }
    }

    fn with_db<T>(&self, f: impl FnOnce(&Connection) -> io::Result<T>) -> io::Result<T> {
        let mut guard = self.conn.lock().unwrap();
        if guard.is_none() {
            if let Some(parent) = self.db_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let c = Connection::open(&self.db_path).map_err(io::Error::other)?;
            c.execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA synchronous = NORMAL;
                 PRAGMA foreign_keys = ON;
                 CREATE TABLE IF NOT EXISTS sessions (
                     id TEXT PRIMARY KEY,
                     label TEXT NOT NULL,
                     workspace TEXT NOT NULL,
                     model TEXT,
                     prelude TEXT,
                     status TEXT NOT NULL DEFAULT 'active',
                     next_turn_id INTEGER NOT NULL DEFAULT 1,
                     snapshot BLOB NOT NULL,
                     created_at INTEGER NOT NULL,
                     updated_at INTEGER NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS action_ledger (
                     session_id TEXT NOT NULL,
                     action_id INTEGER NOT NULL,
                     kind TEXT NOT NULL,
                     summary TEXT NOT NULL,
                     output_text TEXT NOT NULL,
                     failed INTEGER NOT NULL DEFAULT 0,
                     completed_at INTEGER NOT NULL,
                     PRIMARY KEY (session_id, action_id),
                     FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
                 );
                 CREATE TABLE IF NOT EXISTS checkpoints (
                     session_id TEXT NOT NULL,
                     seq INTEGER NOT NULL,
                     git_ref TEXT NOT NULL,
                     created_at INTEGER NOT NULL,
                     PRIMARY KEY (session_id, seq),
                     FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
                 );",
            )
            .map_err(io::Error::other)?;
            *guard = Some(c);
        }
        f(guard.as_ref().unwrap())
    }

    pub fn health(&self, external_worker_running: bool) -> Value {
        let (total, active) = self
            .with_db(|c| {
                let total: i64 = c
                    .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
                    .unwrap_or(0);
                let active: i64 = c
                    .query_row(
                        "SELECT COUNT(*) FROM sessions WHERE status = 'active'",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);
                Ok((total, active))
            })
            .unwrap_or((0, 0));

        json!({
            "ok": true,
            "runtime": "rust-native",
            "version": env!("CARGO_PKG_VERSION"),
            "sessions": total,
            "active_sessions": active,
            "external_worker": external_worker_running,
            "embedded_chain": cfg!(feature = "embedded-chain"),
            "engine_port": self.engine.as_ref().and_then(|e| e.port()),
            "sqlite_path": self.db_path.display().to_string(),
        })
    }

    pub fn create_session(&self, who: &str, req: CreateSessionReq) -> io::Result<SessionSummary> {
        let id = new_session_id();
        let workspace = req
            .workspace
            .filter(|w| !w.trim().is_empty())
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .unwrap_or_else(|_| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
                    .display()
                    .to_string()
            });
        let label = req
            .label
            .filter(|l| !l.trim().is_empty())
            .unwrap_or_else(|| {
                Path::new(&workspace)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(|s| format!("{s} ({id})"))
                    .unwrap_or_else(|| id.clone())
            });
        let session = Session::new(Config {
            model: req.model.clone(),
            prelude: req.prelude.clone(),
        });
        let snap = session.snapshot().map_err(io::Error::other)?;
        let ts = now_ms();
        self.with_db(|c| {
            c.execute(
                "INSERT INTO sessions (id, label, workspace, model, prelude, status, next_turn_id, snapshot, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'active', 1, ?6, ?7, ?7)",
                params![id, label, workspace, req.model, req.prelude, snap, ts],
            )
            .map_err(io::Error::other)?;
            Ok(())
        })?;
        self.ledger
            .record(who, "dev.session.create", &format!("{id} ({workspace})"));
        Ok(SessionSummary {
            id,
            label,
            workspace,
            model: req.model,
            status: "active".into(),
            runtime: "rust-native".into(),
            next_turn_id: 1,
            created_at: ts,
            updated_at: ts,
        })
    }

    pub fn list_sessions(&self) -> io::Result<Vec<SessionSummary>> {
        self.with_db(|c| {
            let mut stmt = c
                .prepare(
                    "SELECT id, label, workspace, model, status, next_turn_id, created_at, updated_at
                     FROM sessions ORDER BY updated_at DESC",
                )
                .map_err(io::Error::other)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(SessionSummary {
                        id: r.get(0)?,
                        label: r.get(1)?,
                        workspace: r.get(2)?,
                        model: r.get(3)?,
                        status: r.get(4)?,
                        runtime: "rust-native".into(),
                        next_turn_id: r.get::<_, i64>(5)? as u64,
                        created_at: r.get(6)?,
                        updated_at: r.get(7)?,
                    })
                })
                .map_err(io::Error::other)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(io::Error::other)?);
            }
            Ok(out)
        })
    }

    fn load_session_row(&self, id: &str) -> io::Result<(SessionSummary, Session)> {
        self.with_db(|c| {
            let row = c
                .query_row(
                    "SELECT id, label, workspace, model, status, next_turn_id, snapshot, created_at, updated_at
                     FROM sessions WHERE id = ?1",
                    params![id],
                    |r| {
                        let summary = SessionSummary {
                            id: r.get(0)?,
                            label: r.get(1)?,
                            workspace: r.get(2)?,
                            model: r.get(3)?,
                            status: r.get(4)?,
                            runtime: "rust-native".into(),
                            next_turn_id: r.get::<_, i64>(5)? as u64,
                            created_at: r.get(7)?,
                            updated_at: r.get(8)?,
                        };
                        let snap: Vec<u8> = r.get(6)?;
                        Ok((summary, snap))
                    },
                )
                .optional()
                .map_err(io::Error::other)?;
            let Some((summary, snap)) = row else {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("session {id} not found"),
                ));
            };
            let session = Session::restore(&snap).map_err(io::Error::other)?;
            Ok((summary, session))
        })
    }

    pub fn get_session_detail(&self, id: &str) -> io::Result<Value> {
        let (summary, session) = self.load_session_row(id)?;
        let checkpoints = self.with_db(|c| {
            let mut stmt = c
                .prepare(
                    "SELECT seq, git_ref, created_at FROM checkpoints WHERE session_id = ?1 ORDER BY seq DESC",
                )
                .map_err(io::Error::other)?;
            let rows = stmt
                .query_map(params![id], |r| {
                    Ok(CheckpointRow {
                        seq: r.get::<_, i64>(0)? as u64,
                        git_ref: r.get(1)?,
                        created_at: r.get(2)?,
                    })
                })
                .map_err(io::Error::other)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(io::Error::other)?);
            }
            Ok(out)
        })?;
        let action_count: i64 = self.with_db(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM action_ledger WHERE session_id = ?1",
                params![id],
                |r| r.get(0),
            )
            .map_err(io::Error::other)
        })?;
        Ok(json!({
            "session": summary,
            "next_action_id": session.next_id(),
            "action_count": action_count,
            "messages": session.messages(),
            "checkpoints": checkpoints,
        }))
    }

    pub fn delete_session(&self, who: &str, id: &str) -> io::Result<bool> {
        let deleted = self.with_db(|c| {
            let n = c
                .execute("DELETE FROM sessions WHERE id = ?1", params![id])
                .map_err(io::Error::other)?;
            Ok(n > 0)
        })?;
        if deleted {
            self.ledger.record(who, "dev.session.delete", id);
        }
        Ok(deleted)
    }

    pub fn set_status(&self, who: &str, id: &str, status: &str) -> io::Result<SessionSummary> {
        let (mut summary, mut session) = self.load_session_row(id)?;
        if status == "interrupted" && summary.next_turn_id > 1 {
            let active_turn = summary.next_turn_id - 1;
            let _ = session.step(dev_protocol::Event::Cancel(Cancel { turn: active_turn }));
        }
        let snap = session.snapshot().map_err(io::Error::other)?;
        let ts = now_ms();
        let persisted_status = if status == "interrupted" {
            "active"
        } else {
            status
        };
        self.with_db(|c| {
            c.execute(
                "UPDATE sessions SET status = ?1, snapshot = ?2, updated_at = ?3 WHERE id = ?4",
                params![persisted_status, snap, ts, id],
            )
            .map_err(io::Error::other)?;
            Ok(())
        })?;
        self.ledger
            .record(who, &format!("dev.session.{status}"), id);
        summary.status = persisted_status.into();
        summary.updated_at = ts;
        Ok(summary)
    }

    fn save_snapshot(
        &self,
        id: &str,
        next_turn_id: u64,
        session: &Session,
    ) -> io::Result<()> {
        let snap = session.snapshot().map_err(io::Error::other)?;
        let ts = now_ms();
        self.with_db(|c| {
            c.execute(
                "UPDATE sessions SET next_turn_id = ?1, snapshot = ?2, updated_at = ?3 WHERE id = ?4",
                params![next_turn_id as i64, snap, ts, id],
            )
            .map_err(io::Error::other)?;
            Ok(())
        })
    }

    fn ledger_lookup(&self, session_id: &str, action_id: u64) -> io::Result<Option<(String, bool)>> {
        self.with_db(|c| {
            c.query_row(
                "SELECT output_text, failed FROM action_ledger WHERE session_id = ?1 AND action_id = ?2",
                params![session_id, action_id as i64],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0)),
            )
            .optional()
            .map_err(io::Error::other)
        })
    }

    fn ledger_record(
        &self,
        session_id: &str,
        action_id: u64,
        kind: &str,
        summary: &str,
        output_text: &str,
        failed: bool,
    ) -> io::Result<()> {
        let ts = now_ms();
        self.with_db(|c| {
            c.execute(
                "INSERT OR IGNORE INTO action_ledger (session_id, action_id, kind, summary, output_text, failed, completed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    session_id,
                    action_id as i64,
                    kind,
                    summary,
                    output_text,
                    if failed { 1i64 } else { 0i64 },
                    ts
                ],
            )
            .map_err(io::Error::other)?;
            Ok(())
        })
    }

    fn record_checkpoint(&self, session_id: &str, seq: u64, workspace: &Path) {
        let git_dir = workspace.join(".git");
        if !git_dir.exists() {
            return;
        }
        let rev = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(workspace)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        if let Some(sha) = rev.filter(|s| !s.is_empty()) {
            let ref_name = format!("refs/hanzo/checkpoints/{session_id}/{seq}");
            let _ = std::process::Command::new("git")
                .args(["update-ref", &ref_name, &sha])
                .current_dir(workspace)
                .output();
            let ts = now_ms();
            let _ = self.with_db(|c| {
                c.execute(
                    "INSERT OR REPLACE INTO checkpoints (session_id, seq, git_ref, created_at) VALUES (?1, ?2, ?3, ?4)",
                    params![session_id, seq as i64, format!("{ref_name}@{sha}"), ts],
                )
                .map_err(io::Error::other)?;
                Ok(())
            });
        }
    }

    /// Execute one turn through the pure `dev_core::Session` loop with
    /// workspace-confined Rust effects, deduplicated action ledger, and
    /// local/cloud OpenAI-compatible inference.
    pub async fn run_turn(
        &self,
        who: &str,
        session_id: &str,
        req: RunTurnReq,
        request_bearer: Option<String>,
    ) -> io::Result<TurnResult> {
        let (summary, mut session) = self.load_session_row(session_id)?;
        let turn_id = summary.next_turn_id;
        let next_turn_id = turn_id + 1;
        let workspace = PathBuf::from(&summary.workspace);

        let mut pending = session.step(dev_protocol::Event::Turn(Turn {
            id: turn_id,
            prompt: req.prompt,
        }));
        self.save_snapshot(session_id, next_turn_id, &session)?;

        let mut emitted = Vec::new();
        let mut actions_log = Vec::new();
        let mut final_outcome = Outcome::Complete;
        let mut rounds = 0usize;

        while !pending.is_empty() && rounds < MAX_TOOL_ROUNDS {
            rounds += 1;
            let batch = std::mem::take(&mut pending);
            for action in batch {
                match action.op {
                    Op::Model(mut ask) => {
                        if let Some(ref ov) = req.model {
                            ask.model = Some(ov.clone());
                        }
                        let reply = self
                            .call_model(&ask, request_bearer.as_deref())
                            .await?;
                        actions_log.push(TurnActionRecord {
                            id: action.id,
                            kind: "model".into(),
                            summary: ask.model.clone().unwrap_or_else(|| "auto".into()),
                            failed: false,
                            replayed: false,
                        });
                        pending.extend(session.step(dev_protocol::Event::Model(Answer {
                            id: action.id,
                            reply,
                        })));
                    }
                    Op::Read(r) => {
                        let (out, replayed) = self
                            .exec_dedup(who, session_id, action.id, "read", &r.path, || async {
                                exec_read(&workspace, &r)
                            })
                            .await?;
                        actions_log.push(TurnActionRecord {
                            id: action.id,
                            kind: "read".into(),
                            summary: r.path,
                            failed: out.failed,
                            replayed,
                        });
                        pending.extend(session.step(dev_protocol::Event::File(out)));
                    }
                    Op::Write(w) => {
                        let (out, replayed) = self
                            .exec_dedup(who, session_id, action.id, "write", &w.path, || async {
                                exec_write(&workspace, &w)
                            })
                            .await?;
                        actions_log.push(TurnActionRecord {
                            id: action.id,
                            kind: "write".into(),
                            summary: w.path,
                            failed: out.failed,
                            replayed,
                        });
                        pending.extend(session.step(dev_protocol::Event::File(out)));
                    }
                    Op::Patch(p) => {
                        let (out, replayed) = self
                            .exec_dedup(who, session_id, action.id, "patch", &p.path, || async {
                                exec_patch(&workspace, &p)
                            })
                            .await?;
                        actions_log.push(TurnActionRecord {
                            id: action.id,
                            kind: "patch".into(),
                            summary: p.path,
                            failed: out.failed,
                            replayed,
                        });
                        pending.extend(session.step(dev_protocol::Event::File(out)));
                    }
                    Op::Exec(e) => {
                        let desc = e.argv.join(" ");
                        let (out, replayed) = self
                            .exec_dedup(who, session_id, action.id, "exec", &desc, || async {
                                exec_cmd(&workspace, &e).await
                            })
                            .await?;
                        actions_log.push(TurnActionRecord {
                            id: action.id,
                            kind: "exec".into(),
                            summary: desc,
                            failed: out.failed,
                            replayed,
                        });
                        pending.extend(session.step(dev_protocol::Event::Exec(out)));
                    }
                    Op::Git(g) => {
                        let desc = format!("git {}", g.argv.join(" "));
                        let (out, replayed) = self
                            .exec_dedup(who, session_id, action.id, "git", &desc, || async {
                                exec_git(&workspace, &g).await
                            })
                            .await?;
                        actions_log.push(TurnActionRecord {
                            id: action.id,
                            kind: "git".into(),
                            summary: desc,
                            failed: out.failed,
                            replayed,
                        });
                        pending.extend(session.step(dev_protocol::Event::Git(out)));
                    }
                    Op::Browse(b) => {
                        let url = b.url.clone();
                        let (out, replayed) = self
                            .exec_dedup(who, session_id, action.id, "browse", &url, || async {
                                tokio::task::spawn_blocking(move || exec_browse(&b))
                                    .await
                                    .unwrap_or_else(|e| (e.to_string(), true))
                            })
                            .await?;
                        actions_log.push(TurnActionRecord {
                            id: action.id,
                            kind: "browse".into(),
                            summary: url,
                            failed: out.failed,
                            replayed,
                        });
                        pending.extend(session.step(dev_protocol::Event::Browse(out)));
                    }
                    Op::Emit(text) => {
                        emitted.push(text);
                    }
                    Op::Save => {
                        self.save_snapshot(session_id, next_turn_id, &session)?;
                        self.record_checkpoint(session_id, action.id, &workspace);
                    }
                    Op::Done(done) => {
                        final_outcome = done.outcome;
                        self.save_snapshot(session_id, next_turn_id, &session)?;
                    }
                }
            }
        }

        self.save_snapshot(session_id, next_turn_id, &session)?;
        let last_text = emitted.last().cloned().or_else(|| {
            session.messages().iter().rev().find_map(|m| match m {
                Message::Agent { text: Some(t), .. } => Some(t.clone()),
                _ => None,
            })
        });

        Ok(TurnResult {
            session_id: session_id.to_string(),
            turn_id,
            outcome: match final_outcome {
                Outcome::Complete => "complete".into(),
                Outcome::Cancelled => "cancelled".into(),
            },
            text: last_text,
            emitted,
            actions: actions_log,
            messages: session.messages().to_vec(),
        })
    }

    async fn exec_dedup<F, Fut>(
        &self,
        who: &str,
        session_id: &str,
        action_id: u64,
        kind: &str,
        summary: &str,
        run: F,
    ) -> io::Result<(Output, bool)>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = (String, bool)>,
    {
        if let Some((text, failed)) = self.ledger_lookup(session_id, action_id)? {
            return Ok((
                Output {
                    id: action_id,
                    text,
                    failed,
                },
                true,
            ));
        }
        self.ledger
            .record(who, &format!("dev.{kind}"), &format!("{session_id}#{action_id}: {summary}"));
        let (text, failed) = run().await;
        let capped = truncate_utf8(&text, MAX_OUTPUT_BYTES);
        self.ledger_record(session_id, action_id, kind, summary, &capped, failed)?;
        Ok((
            Output {
                id: action_id,
                text: capped,
                failed,
            },
            false,
        ))
    }

    /// Route an `Op::Model` request to either local `hanzod` inference (`/v1/engine`)
    /// or Hanzo Cloud API (`/v1/chat/completions`), translating `dev_protocol`
    /// messages & tool definitions into OpenAI-compatible wire JSON.
    async fn call_model(&self, ask: &Ask, request_bearer: Option<&str>) -> io::Result<Reply> {
        let chosen_model = ask.model.clone().unwrap_or_else(|| "enso-auto".into());
        let is_local_model = chosen_model.starts_with("local:")
            || chosen_model.starts_with("zen")
            || catalog::find(&chosen_model).is_some();

        let (endpoint, model_id, bearer) = if is_local_model {
            let port = self.engine.as_ref().and_then(|e| e.port());
            if let Some(p) = port {
                let clean = chosen_model
                    .strip_prefix("local:")
                    .unwrap_or(&chosen_model)
                    .to_string();
                (
                    format!("http://127.0.0.1:{p}/v1/chat/completions"),
                    clean,
                    None,
                )
            } else {
                let clean = chosen_model
                    .strip_prefix("local:")
                    .unwrap_or(&chosen_model)
                    .to_string();
                (
                    format!("http://{}/v1/engine/chat/completions", paths::HTTP_ADDR),
                    clean,
                    None,
                )
            }
        } else {
            let base = std::env::var("HANZO_API_URL")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_CLOUD_API.into());
            let token = request_bearer
                .map(str::to_owned)
                .or_else(|| me::tokens().map(|(_, t)| t.access_token));
            (
                format!("{}/v1/chat/completions", base.trim_end_matches('/')),
                chosen_model,
                token,
            )
        };

        let mut oai_messages = Vec::new();
        if let Some(ref pre) = ask.prelude {
            if !pre.trim().is_empty() {
                oai_messages.push(json!({ "role": "system", "content": pre }));
            }
        }
        for msg in &ask.messages {
            match msg {
                Message::User(u) => oai_messages.push(json!({ "role": "user", "content": u })),
                Message::Agent { text, calls } => {
                    if calls.is_empty() {
                        oai_messages.push(json!({
                            "role": "assistant",
                            "content": text.clone().unwrap_or_default()
                        }));
                    } else {
                        let tc: Vec<Value> = calls
                            .iter()
                            .map(|d| {
                                let (name, args) = call_to_oai(&d.call);
                                json!({
                                    "id": format!("call_{}", d.id),
                                    "type": "function",
                                    "function": {
                                        "name": name,
                                        "arguments": args.to_string(),
                                    }
                                })
                            })
                            .collect();
                        oai_messages.push(json!({
                            "role": "assistant",
                            "content": text,
                            "tool_calls": tc
                        }));
                    }
                }
                Message::Tool { id, text, .. } => {
                    oai_messages.push(json!({
                        "role": "tool",
                        "tool_call_id": format!("call_{id}"),
                        "content": text,
                    }));
                }
            }
        }

        let payload = json!({
            "model": model_id,
            "messages": oai_messages,
            "tools": tool_schemas(),
            "stream": false,
        });

        let body: Value = tokio::task::spawn_blocking(move || -> io::Result<Value> {
            let mut req = ureq::post(&endpoint)
                .timeout(Duration::from_secs(120))
                .set("Content-Type", "application/json");
            if let Some(tok) = bearer {
                req = req.set("Authorization", &format!("Bearer {tok}"));
            }
            let resp = req.send_json(payload).map_err(|e| {
                io::Error::other(format!("model request to {endpoint} failed: {e}"))
            })?;
            resp.into_json::<Value>().map_err(io::Error::other)
        })
        .await
        .map_err(io::Error::other)??;

        let msg = body
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .ok_or_else(|| io::Error::other("model returned no choices[0].message"))?;

        let text = msg
            .get("content")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
            .filter(|s| !s.is_empty());

        let mut calls = Vec::new();
        if let Some(arr) = msg.get("tool_calls").and_then(|v| v.as_array()) {
            for tc in arr {
                let fn_obj = tc.get("function").unwrap_or(&Value::Null);
                let name = fn_obj.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let args_str = fn_obj
                    .get("arguments")
                    .and_then(|v| v.as_str())
                    .unwrap_or("{}");
                let args: Value = serde_json::from_str(args_str).unwrap_or(Value::Null);
                if let Some(call) = oai_to_call(name, &args) {
                    calls.push(call);
                }
            }
        }

        Ok(Reply { text, calls })
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

fn truncate_utf8(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n…[truncated]", &s[..end])
}

/// Resolve a workspace-relative path safely, refusing escapes and symlink breakouts.
fn resolve_inside(workspace: &Path, rel: &str) -> Result<PathBuf, String> {
    if !dev_protocol::inside(rel) {
        return Err(format!("refused: {rel} is outside the workspace"));
    }
    let root = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let joined = root.join(rel);
    if joined.exists() {
        let canon = joined
            .canonicalize()
            .map_err(|e| format!("resolve {rel}: {e}"))?;
        if !canon.starts_with(&root) {
            return Err(format!("refused: {rel} escapes workspace via symlink"));
        }
        Ok(canon)
    } else {
        Ok(joined)
    }
}

fn exec_read(workspace: &Path, r: &Read) -> (String, bool) {
    match resolve_inside(workspace, &r.path).and_then(|p| {
        std::fs::read_to_string(&p).map_err(|e| format!("read {}: {e}", r.path))
    }) {
        Ok(text) => (text, false),
        Err(e) => (e, true),
    }
}

fn exec_write(workspace: &Path, w: &Write) -> (String, bool) {
    let target = match resolve_inside(workspace, &w.path) {
        Ok(p) => p,
        Err(e) => return (e, true),
    };
    if let Some(parent) = target.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return (format!("mkdir {}: {e}", parent.display()), true);
        }
    }
    match std::fs::write(&target, &w.text) {
        Ok(()) => (format!("wrote {} bytes to {}", w.text.len(), w.path), false),
        Err(e) => (format!("write {}: {e}", w.path), true),
    }
}

fn exec_patch(workspace: &Path, p: &Patch) -> (String, bool) {
    let target = match resolve_inside(workspace, &p.path) {
        Ok(path) => path,
        Err(e) => return (e, true),
    };
    // Apply unified diff cleanly via `git apply` when formatted as a diff,
    // otherwise write the patched text directly when non-diff replacement text is supplied.
    if p.diff.contains("@@") {
        let mut child = match std::process::Command::new("git")
            .args(["apply", "--whitespace=nowarn", "-"])
            .current_dir(workspace)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => return (format!("git apply spawn: {e}"), true),
        };
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write as _;
            let _ = stdin.write_all(p.diff.as_bytes());
        }
        match child.wait_with_output() {
            Ok(out) if out.status.success() => (format!("patched {}", p.path), false),
            Ok(out) => (
                format!(
                    "git apply failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
                true,
            ),
            Err(e) => (format!("git apply wait: {e}"), true),
        }
    } else {
        match std::fs::write(&target, &p.diff) {
            Ok(()) => (format!("updated {}", p.path), false),
            Err(e) => (format!("patch {}: {e}", p.path), true),
        }
    }
}

async fn exec_cmd(workspace: &Path, e: &Exec) -> (String, bool) {
    let Some((prog, args)) = e.argv.split_first() else {
        return ("empty argv".into(), true);
    };
    let cwd = match e.cwd.as_deref() {
        Some(rel) => match resolve_inside(workspace, rel) {
            Ok(p) => p,
            Err(err) => return (err, true),
        },
        None => workspace.to_path_buf(),
    };
    let fut = Command::new(prog).args(args).current_dir(&cwd).output();
    match tokio::time::timeout(EXEC_TIMEOUT, fut).await {
        Ok(Ok(out)) => {
            let mut text = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !stderr.is_empty() {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(&stderr);
            }
            (text, !out.status.success())
        }
        Ok(Err(err)) => (format!("exec {prog}: {err}"), true),
        Err(_) => (format!("exec {prog}: timed out after {}s", EXEC_TIMEOUT.as_secs()), true),
    }
}

async fn exec_git(workspace: &Path, g: &Git) -> (String, bool) {
    let fut = Command::new("git")
        .args(&g.argv)
        .current_dir(workspace)
        .output();
    match tokio::time::timeout(GIT_TIMEOUT, fut).await {
        Ok(Ok(out)) => {
            let mut text = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !stderr.is_empty() {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(&stderr);
            }
            (text, !out.status.success())
        }
        Ok(Err(err)) => (format!("git: {err}"), true),
        Err(_) => ("git: timed out".into(), true),
    }
}

fn exec_browse(b: &Browse) -> (String, bool) {
    if !b.url.starts_with("http://") && !b.url.starts_with("https://") {
        return ("refused: only http/https URLs are permitted".into(), true);
    }
    let resp = match &b.body {
        Some(body) => ureq::post(&b.url)
            .timeout(Duration::from_secs(20))
            .send_string(body),
        None => ureq::get(&b.url).timeout(Duration::from_secs(20)).call(),
    };
    match resp {
        Ok(r) => match r.into_string() {
            Ok(s) => (truncate_utf8(&s, MAX_OUTPUT_BYTES), false),
            Err(e) => (format!("read {}: {e}", b.url), true),
        },
        Err(e) => (format!("browse {}: {e}", b.url), true),
    }
}

fn tool_schemas() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a workspace-relative file.",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "Write full contents to a workspace-relative file.",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string" }, "text": { "type": "string" } },
                    "required": ["path", "text"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "patch_file",
                "description": "Apply a unified diff to a workspace-relative file.",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string" }, "diff": { "type": "string" } },
                    "required": ["path", "diff"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "exec",
                "description": "Run a command inside the workspace.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "argv": { "type": "array", "items": { "type": "string" } },
                        "cwd": { "type": "string" }
                    },
                    "required": ["argv"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "git",
                "description": "Run a git command inside the workspace.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "argv": { "type": "array", "items": { "type": "string" } }
                    },
                    "required": ["argv"]
                }
            }
        }
    ])
}

fn call_to_oai(call: &Call) -> (&'static str, Value) {
    match call {
        Call::Read(r) => ("read_file", json!({ "path": r.path })),
        Call::Write(w) => ("write_file", json!({ "path": w.path, "text": w.text })),
        Call::Patch(p) => ("patch_file", json!({ "path": p.path, "diff": p.diff })),
        Call::Exec(e) => ("exec", json!({ "argv": e.argv, "cwd": e.cwd })),
        Call::Git(g) => ("git", json!({ "argv": g.argv })),
        Call::Browse(b) => ("browse", json!({ "url": b.url, "body": b.body })),
    }
}

fn oai_to_call(name: &str, args: &Value) -> Option<Call> {
    match name {
        "read_file" => Some(Call::Read(Read {
            path: args.get("path")?.as_str()?.to_string(),
        })),
        "write_file" => Some(Call::Write(Write {
            path: args.get("path")?.as_str()?.to_string(),
            text: args.get("text")?.as_str()?.to_string(),
        })),
        "patch_file" => Some(Call::Patch(Patch {
            path: args.get("path")?.as_str()?.to_string(),
            diff: args.get("diff")?.as_str()?.to_string(),
        })),
        "exec" => {
            let argv = args
                .get("argv")?
                .as_array()?
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect::<Vec<_>>();
            let cwd = args.get("cwd").and_then(|v| v.as_str()).map(str::to_owned);
            Some(Call::Exec(Exec { argv, cwd }))
        }
        "git" => {
            let argv = args
                .get("argv")?
                .as_array()?
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect::<Vec<_>>();
            Some(Call::Git(Git { argv }))
        }
        "browse" => Some(Call::Browse(Browse {
            url: args.get("url")?.as_str()?.to_string(),
            body: args.get("body").and_then(|v| v.as_str()).map(str::to_owned),
        })),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn sqlite_session_lifecycle_and_snapshot_restore() {
        let dir = tempdir().unwrap();
        let ledger = Arc::new(Ledger::open(dir.path().join("ledger")));
        let dev = DevNative::with_db_path(dir.path().join("dev.sqlite"), ledger, None);

        let created = dev
            .create_session(
                "hanzo/zach",
                CreateSessionReq {
                    label: Some("test-session".into()),
                    workspace: Some(dir.path().display().to_string()),
                    model: Some("zen-max".into()),
                    prelude: Some("Be concise.".into()),
                },
            )
            .unwrap();
        assert_eq!(created.runtime, "rust-native");
        assert_eq!(created.status, "active");

        let list = dev.list_sessions().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, created.id);

        let suspended = dev.set_status("hanzo/zach", &created.id, "suspended").unwrap();
        assert_eq!(suspended.status, "suspended");

        let resumed = dev.set_status("hanzo/zach", &created.id, "active").unwrap();
        assert_eq!(resumed.status, "active");

        assert!(dev.delete_session("hanzo/zach", &created.id).unwrap());
        assert!(dev.list_sessions().unwrap().is_empty());
    }

    #[test]
    fn workspace_confinement_blocks_escapes_and_allows_relative() {
        let dir = tempdir().unwrap();
        let (msg, failed) = exec_read(dir.path(), &Read { path: "../../etc/passwd".into() });
        assert!(failed);
        assert!(msg.contains("outside"));

        let (_, w_failed) = exec_write(
            dir.path(),
            &Write {
                path: "notes/hello.txt".into(),
                text: "rust-native-hanzod".into(),
            },
        );
        assert!(!w_failed);

        let (text, r_failed) = exec_read(dir.path(), &Read { path: "notes/hello.txt".into() });
        assert!(!r_failed);
        assert_eq!(text, "rust-native-hanzod");
    }

    #[tokio::test]
    async fn action_ledger_deduplicates_effects() {
        let dir = tempdir().unwrap();
        let ledger = Arc::new(Ledger::open(dir.path().join("ledger")));
        let dev = DevNative::with_db_path(dir.path().join("dev.sqlite"), ledger, None);
        let s = dev
            .create_session(
                "hanzo/zach",
                CreateSessionReq {
                    label: None,
                    workspace: Some(dir.path().display().to_string()),
                    model: None,
                    prelude: None,
                },
            )
            .unwrap();

        let count = Arc::new(AtomicU64::new(0));
        let c1 = count.clone();
        let (out1, rep1) = dev
            .exec_dedup("hanzo/zach", &s.id, 42, "exec", "echo hi", || async move {
                c1.fetch_add(1, Ordering::SeqCst);
                ("hi\n".into(), false)
            })
            .await
            .unwrap();
        assert!(!rep1);
        assert_eq!(out1.text, "hi\n");

        let c2 = count.clone();
        let (out2, rep2) = dev
            .exec_dedup("hanzo/zach", &s.id, 42, "exec", "echo hi", || async move {
                c2.fetch_add(1, Ordering::SeqCst);
                ("should-not-run".into(), false)
            })
            .await
            .unwrap();
        assert!(rep2);
        assert_eq!(out2.text, "hi\n");
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn orchestrator_decide_callable_in_process() {
        let dir = tempdir().unwrap();
        let ledger = Arc::new(Ledger::open(dir.path().join("ledger")));
        let dev = DevNative::with_db_path(dir.path().join("dev.sqlite"), ledger, None);
        let raw = RawValue::from_string(r#"{"unknown":true}"#.into()).unwrap();
        assert!(dev.decide("nonexistentRule", &raw).unwrap().is_none());
    }
}
