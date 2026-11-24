// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! Durable SQLite store (`state.sqlite`) for `DevNative` sessions, seamless
//! `backend_session_id` migration, deduplicated `action_ledger`, and git checkpoints.

use super::{
    new_session_id, now_ms, CheckpointRow, CreateSessionReq, DevNative, LedgerEventRow,
    SessionSummary, HARNESS_NAME,
};
use dev_core::Session;
use dev_protocol::{Cancel, Config, Event};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::io;
use std::path::{Path, PathBuf};

pub(super) struct LoadedSession {
    pub summary: SessionSummary,
    pub session: Session,
    pub prelude: Option<String>,
    pub backend_session_id: Option<String>,
}

impl DevNative {
    pub(super) fn with_db<T>(&self, f: impl FnOnce(&Connection) -> io::Result<T>) -> io::Result<T> {
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
                     updated_at INTEGER NOT NULL,
                     backend_session_id TEXT
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
            // Seamless in-place upgrade for pre-v0.5.0 state.sqlite databases on disk.
            let _ = c.execute("ALTER TABLE sessions ADD COLUMN backend_session_id TEXT", []);
            *guard = Some(c);
        }
        f(guard.as_ref().unwrap())
    }

    pub(super) fn session_counts_db(&self) -> io::Result<(i64, i64)> {
        self.with_db(|c| {
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
                "INSERT INTO sessions (id, label, workspace, model, prelude, status, next_turn_id, snapshot, created_at, updated_at, backend_session_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'active', 1, ?6, ?7, ?7, NULL)",
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
            runtime: HARNESS_NAME.into(),
            next_turn_id: 1,
            created_at: ts,
            updated_at: ts,
            backend_session_id: None,
        })
    }

    pub fn list_sessions(&self) -> io::Result<Vec<SessionSummary>> {
        self.with_db(|c| {
            let mut stmt = c
                .prepare(
                    "SELECT id, label, workspace, model, status, next_turn_id, created_at, updated_at, backend_session_id
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
                        runtime: HARNESS_NAME.into(),
                        next_turn_id: r.get::<_, i64>(5)? as u64,
                        created_at: r.get(6)?,
                        updated_at: r.get(7)?,
                        backend_session_id: r.get(8)?,
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

    pub fn get_session_summary(&self, id: &str) -> io::Result<SessionSummary> {
        Ok(self.load_session_full(id)?.summary)
    }

    pub(super) fn load_session_full(&self, id: &str) -> io::Result<LoadedSession> {
        self.with_db(|c| {
            let row = c
                .query_row(
                    "SELECT id, label, workspace, model, prelude, status, next_turn_id, snapshot, created_at, updated_at, backend_session_id
                     FROM sessions WHERE id = ?1",
                    params![id],
                    |r| {
                        let backend_sid: Option<String> = r.get(10)?;
                        let summary = SessionSummary {
                            id: r.get(0)?,
                            label: r.get(1)?,
                            workspace: r.get(2)?,
                            model: r.get(3)?,
                            status: r.get(5)?,
                            runtime: HARNESS_NAME.into(),
                            next_turn_id: r.get::<_, i64>(6)? as u64,
                            created_at: r.get(8)?,
                            updated_at: r.get(9)?,
                            backend_session_id: backend_sid.clone(),
                        };
                        let prelude: Option<String> = r.get(4)?;
                        let snap: Vec<u8> = r.get(7)?;
                        Ok((summary, prelude, snap, backend_sid))
                    },
                )
                .optional()
                .map_err(io::Error::other)?;
            let Some((summary, prelude, snap, backend_session_id)) = row else {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("session {id} not found"),
                ));
            };
            let session = Session::restore(&snap).map_err(io::Error::other)?;
            Ok(LoadedSession {
                summary,
                session,
                prelude,
                backend_session_id,
            })
        })
    }

    pub fn list_checkpoints(&self, id: &str) -> io::Result<Vec<CheckpointRow>> {
        self.with_db(|c| {
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
        })
    }

    pub fn list_action_events(&self, id: &str) -> io::Result<Vec<LedgerEventRow>> {
        self.with_db(|c| {
            let mut stmt = c
                .prepare(
                    "SELECT action_id, kind, summary, failed, completed_at
                     FROM action_ledger WHERE session_id = ?1 ORDER BY completed_at ASC, action_id ASC",
                )
                .map_err(io::Error::other)?;
            let rows = stmt
                .query_map(params![id], |r| {
                    Ok(LedgerEventRow {
                        action_id: r.get::<_, i64>(0)? as u64,
                        kind: r.get(1)?,
                        summary: r.get(2)?,
                        failed: r.get::<_, i64>(3)? != 0,
                        completed_at: r.get(4)?,
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

    pub fn get_session_detail(&self, id: &str) -> io::Result<Value> {
        let loaded = self.load_session_full(id)?;
        let checkpoints = self.list_checkpoints(id)?;
        let action_count: i64 = self.with_db(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM action_ledger WHERE session_id = ?1",
                params![id],
                |r| r.get(0),
            )
            .map_err(io::Error::other)
        })?;
        Ok(json!({
            "session": loaded.summary,
            "backend_session_id": loaded.backend_session_id,
            "next_action_id": loaded.session.next_id(),
            "action_count": action_count,
            "messages": loaded.session.messages(),
            "checkpoints": checkpoints,
        }))
    }

    pub fn delete_session(&self, who: &str, id: &str) -> io::Result<bool> {
        if let Some((_, tx)) = self.tracker.active_cancels.lock().unwrap().get(id) {
            let _ = tx.send_replace(true);
        }
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
        if status == "interrupted" {
            if let Some((_, tx)) = self.tracker.active_cancels.lock().unwrap().get(id) {
                let _ = tx.send_replace(true);
            }
        }
        let mut loaded = self.load_session_full(id)?;
        if status == "interrupted" && loaded.summary.next_turn_id > 1 {
            let active_turn = loaded.summary.next_turn_id - 1;
            let _ = loaded
                .session
                .step(Event::Cancel(Cancel { turn: active_turn }));
        }
        let snap = loaded.session.snapshot().map_err(io::Error::other)?;
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
        loaded.summary.status = persisted_status.into();
        loaded.summary.updated_at = ts;
        Ok(loaded.summary)
    }

    pub(super) fn save_snapshot(
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

    pub(super) fn save_snapshot_and_backend(
        &self,
        id: &str,
        next_turn_id: u64,
        session: &Session,
        backend_session_id: Option<&str>,
    ) -> io::Result<()> {
        let snap = session.snapshot().map_err(io::Error::other)?;
        let ts = now_ms();
        self.with_db(|c| {
            c.execute(
                "UPDATE sessions
                 SET next_turn_id = ?1,
                     snapshot = ?2,
                     backend_session_id = COALESCE(?3, backend_session_id),
                     updated_at = ?4
                 WHERE id = ?5",
                params![next_turn_id as i64, snap, backend_session_id, ts, id],
            )
            .map_err(io::Error::other)?;
            Ok(())
        })
    }

    pub(super) fn next_ledger_action_id(&self, session_id: &str, min_next: u64) -> io::Result<u64> {
        let max_db: i64 = self.with_db(|c| {
            c.query_row(
                "SELECT COALESCE(MAX(action_id), 0) FROM action_ledger WHERE session_id = ?1",
                params![session_id],
                |r| r.get(0),
            )
            .map_err(io::Error::other)
        })?;
        Ok((max_db.max(0) as u64).max(min_next) + 1)
    }

    pub(super) fn ledger_lookup(
        &self,
        session_id: &str,
        action_id: u64,
        kind: &str,
    ) -> io::Result<Option<(String, bool)>> {
        self.with_db(|c| {
            c.query_row(
                "SELECT output_text, failed FROM action_ledger
                 WHERE session_id = ?1 AND action_id = ?2 AND kind = ?3",
                params![session_id, action_id as i64, kind],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0)),
            )
            .optional()
            .map_err(io::Error::other)
        })
    }

    pub(super) fn ledger_record(
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

    pub(super) fn record_checkpoint(&self, session_id: &str, seq: u64, workspace: &Path) {
        if !workspace.join(".git").exists() {
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
}
