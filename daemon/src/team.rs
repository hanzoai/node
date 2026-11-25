// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! `/v1/team`: Unified Hanzo Team backend inside `hanzod` (HIP-0106).
//!
//! Priority 1: Proxies to a lazily started `hanzo-team` binary (`BASE_API_PREFIX=/v1/team`)
//! or registered local `upstream` when `/v1/team/transactor` or external routes are used.
//! Priority 2: Built-in WAL SQLite spaces, channels, human+bot threads, and Kanban board
//! tasks with automatic `hanzoai/tasks` dispatch (`gpu-jobs`) and Cloud forwarding.

use crate::bot::Bot;
use crate::cloud::Upstream;
use crate::engine;
use crate::ledger::Ledger;
use crate::me;
use crate::paths;
use crate::proc;
use crate::table::{Fut, Reply, Subsystem};
use crate::zap::{Request, Response};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::process::Child;
use tokio::sync::Mutex;

const DEFAULT_CLOUD_API: &str = "https://api.hanzo.ai";
static TEAM_COUNTER: AtomicU64 = AtomicU64::new(1);

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn next_id(prefix: &str) -> String {
    let ts = now_secs() as u64;
    let seq = TEAM_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}_{ts:x}_{seq:04x}")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamSpace {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamChannel {
    pub id: String,
    pub space_id: String,
    pub name: String,
    #[serde(default)]
    pub topic: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadMessage {
    pub id: String,
    pub channel_id: String,
    pub thread_id: String,
    pub author: String,
    pub author_kind: String, // "human" | "bot"
    pub content: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardTask {
    pub id: String,
    pub space_id: String,
    #[serde(default)]
    pub channel_id: String,
    #[serde(default)]
    pub thread_id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub status: String, // "todo" | "in_progress" | "done" | "faulted"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_activity_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Deserialize)]
struct CreateSpaceReq {
    id: Option<String>,
    name: String,
    #[serde(default)]
    description: String,
}

#[derive(Debug, Deserialize)]
struct CreateChannelReq {
    id: Option<String>,
    #[serde(default = "default_space")]
    space_id: String,
    name: String,
    #[serde(default)]
    topic: String,
}

fn default_space() -> String {
    "general".to_string()
}

#[derive(Debug, Deserialize)]
struct PostThreadReq {
    #[serde(default = "default_space")]
    channel_id: String,
    thread_id: Option<String>,
    author: Option<String>,
    #[serde(default = "default_human")]
    author_kind: String,
    content: String,
}

fn default_human() -> String {
    "human".to_string()
}

#[derive(Debug, Deserialize)]
pub struct UpsertTaskReq {
    pub id: Option<String>,
    #[serde(default = "default_space")]
    pub space_id: String,
    #[serde(default)]
    pub channel_id: String,
    #[serde(default)]
    pub thread_id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub status: Option<String>,
    pub assignee: Option<String>,
    pub bot_id: Option<String>,
    #[serde(default)]
    pub dispatch: bool,
    pub task_queue: Option<String>,
}

pub struct Team {
    db_path: PathBuf,
    conn: StdMutex<Option<Connection>>,
    ledger: Arc<Ledger>,
    upstream: Upstream,
    child: Mutex<Option<Child>>,
    port: StdMutex<Option<u16>>,
    cloud_api_override: Option<String>,
}

impl Team {
    pub fn new(ledger: Arc<Ledger>, upstream: Upstream) -> Self {
        Self::with_db_path(
            paths::data().join("team").join("state.sqlite"),
            ledger,
            upstream,
            None,
        )
    }

    pub fn with_db_path(
        db_path: PathBuf,
        ledger: Arc<Ledger>,
        upstream: Upstream,
        cloud_api_override: Option<String>,
    ) -> Self {
        Self {
            db_path,
            conn: StdMutex::new(None),
            ledger,
            upstream,
            child: Mutex::new(None),
            port: StdMutex::new(None),
            cloud_api_override,
        }
    }

    fn cloud_api(&self) -> String {
        if let Some(ref api) = self.cloud_api_override {
            return api.trim_end_matches('/').to_string();
        }
        std::env::var("HANZO_API_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_CLOUD_API.to_string())
            .trim_end_matches('/')
            .to_string()
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
                 CREATE TABLE IF NOT EXISTS team_spaces (
                     id TEXT PRIMARY KEY,
                     created_at INTEGER NOT NULL,
                     payload TEXT NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS team_channels (
                     id TEXT PRIMARY KEY,
                     space_id TEXT NOT NULL,
                     created_at INTEGER NOT NULL,
                     payload TEXT NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS team_threads (
                     id TEXT PRIMARY KEY,
                     channel_id TEXT NOT NULL,
                     thread_id TEXT NOT NULL,
                     created_at INTEGER NOT NULL,
                     payload TEXT NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS team_tasks (
                     id TEXT PRIMARY KEY,
                     updated_at INTEGER NOT NULL,
                     payload TEXT NOT NULL
                 );",
            )
            .map_err(io::Error::other)?;
            *guard = Some(c);
        }
        f(guard.as_ref().unwrap())
    }

    pub fn health(&self) -> Value {
        let (spaces, tasks) = self
            .with_db(|c| {
                let s: i64 = c
                    .query_row("SELECT COUNT(*) FROM team_spaces", [], |r| r.get(0))
                    .unwrap_or(0);
                let t: i64 = c
                    .query_row("SELECT COUNT(*) FROM team_tasks", [], |r| r.get(0))
                    .unwrap_or(0);
                Ok((s, t))
            })
            .unwrap_or((0, 0));
        let up = self.upstream.lock().unwrap().map(|a| a.to_string());
        json!({
            "ok": true,
            "runtime": if self.port.lock().unwrap().is_some() { "hanzo-team" } else { "hanzod-native" },
            "modelVersion": "0.6.0",
            "upstream": up,
            "team_bin_available": proc::find("hanzo-team").is_some(),
            "spaces": spaces,
            "tasks": tasks,
        })
    }

    async fn maybe_team_port(&self) -> Option<u16> {
        if let Some(p) = *self.port.lock().unwrap() {
            return Some(p);
        }
        let bin = proc::find("hanzo-team")?;
        if proc::owned_by_us_alone(&bin).is_err() {
            return None;
        }
        let mut slot = self.child.lock().await;
        if let Some(p) = *self.port.lock().unwrap() {
            return Some(p);
        }
        let port = proc::free_port().ok()?;
        let data_dir = paths::data().join("team");
        let _ = std::fs::create_dir_all(&data_dir);
        let child = proc::bg_tokio_cmd(&bin)
            .args([
                "serve",
                "--http",
                &format!("127.0.0.1:{port}"),
                "--dir",
                &data_dir.display().to_string(),
            ])
            .env("BASE_API_PREFIX", "/v1/team")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .ok()?;
        *slot = Some(child);
        *self.port.lock().unwrap() = Some(port);
        Some(port)
    }

    pub fn append_message(&self, msg: &ThreadMessage) -> io::Result<()> {
        let raw = serde_json::to_string(msg).map_err(io::Error::other)?;
        self.with_db(|c| {
            c.execute(
                "INSERT INTO team_threads (id, channel_id, thread_id, created_at, payload)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![msg.id, msg.channel_id, msg.thread_id, msg.created_at, raw],
            )
            .map_err(io::Error::other)?;
            Ok(())
        })
    }

    pub fn get_task(&self, id: &str) -> io::Result<Option<BoardTask>> {
        self.with_db(|c| {
            let raw: Option<String> = c
                .query_row("SELECT payload FROM team_tasks WHERE id = ?1", params![id], |r| r.get(0))
                .optional()
                .map_err(io::Error::other)?;
            Ok(raw.and_then(|s| serde_json::from_str(&s).ok()))
        })
    }

    fn save_task(&self, task: &BoardTask) -> io::Result<()> {
        let raw = serde_json::to_string(task).map_err(io::Error::other)?;
        self.with_db(|c| {
            c.execute(
                "INSERT INTO team_tasks (id, updated_at, payload) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET updated_at = excluded.updated_at, payload = excluded.payload",
                params![task.id, task.updated_at, raw],
            )
            .map_err(io::Error::other)?;
            Ok(())
        })
    }

    pub async fn upsert_task(
        &self,
        who: &str,
        existing_id: Option<&str>,
        req: UpsertTaskReq,
        bearer: Option<String>,
    ) -> io::Result<BoardTask> {
        let now = now_secs();
        let prev = match existing_id {
            Some(id) => self.get_task(id)?,
            None => req.id.as_deref().and_then(|id| self.get_task(id).ok().flatten()),
        };
        let id = existing_id
            .map(str::to_owned)
            .or_else(|| req.id.filter(|s| !s.trim().is_empty()))
            .or_else(|| prev.as_ref().map(|p| p.id.clone()))
            .unwrap_or_else(|| next_id("ttask"));

        let should_dispatch = req.dispatch || req.bot_id.is_some();
        let mut record = BoardTask {
            id: id.clone(),
            space_id: req.space_id,
            channel_id: req.channel_id,
            thread_id: req.thread_id,
            title: req.title,
            description: req.description,
            status: req
                .status
                .or_else(|| prev.as_ref().map(|p| p.status.clone()))
                .unwrap_or_else(|| if should_dispatch { "in_progress".into() } else { "todo".into() }),
            assignee: req.assignee.or_else(|| prev.as_ref().and_then(|p| p.assignee.clone())),
            bot_id: req.bot_id.or_else(|| prev.as_ref().and_then(|p| p.bot_id.clone())),
            task_activity_id: prev.as_ref().and_then(|p| p.task_activity_id.clone()),
            created_at: prev.as_ref().map(|p| p.created_at).unwrap_or(now),
            updated_at: now,
        };

        if should_dispatch {
            let tok = bearer.or_else(|| me::tokens().map(|(_, t)| t.access_token));
            if let Some(tok) = tok {
                let api = self.cloud_api();
                let queue = req.task_queue.unwrap_or_else(|| "gpu-jobs".into());
                let t_clone = record.clone();
                if let Ok(Ok(act_id)) = tokio::task::spawn_blocking(move || {
                    dispatch_board_task(&api, &tok, &queue, &t_clone)
                })
                .await
                {
                    record.task_activity_id = Some(act_id);
                }
            }
        }

        self.save_task(&record)?;
        self.ledger.record(who, "team.task.upsert", &record.id);
        Ok(record)
    }

    pub async fn execute_team_task(
        &self,
        who: &str,
        input: &Value,
        bot: &Bot,
        bearer: Option<String>,
    ) -> Result<Value, String> {
        let task_id = input
            .get("task_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let bot_id = input
            .get("bot_id")
            .and_then(Value::as_str)
            .unwrap_or("hanzo");
        let title = input.get("title").and_then(Value::as_str).unwrap_or("Board task");
        let desc = input.get("description").and_then(Value::as_str).unwrap_or("");
        let prompt = format!("Complete team task: {title}\n\n{desc}");

        let out = bot
            .execute_bot_job(
                who,
                &json!({ "bot_id": bot_id, "prompt": prompt }),
                bearer,
            )
            .await?;

        if !task_id.is_empty() {
            if let Ok(Some(mut task)) = self.get_task(task_id) {
                task.status = "done".into();
                task.updated_at = now_secs();
                let _ = self.save_task(&task);
                if !task.channel_id.is_empty() || !task.thread_id.is_empty() {
                    let reply_text = out["output"]
                        .as_str()
                        .unwrap_or("Completed task.")
                        .to_string();
                    let _ = self.append_message(&ThreadMessage {
                        id: next_id("tmsg"),
                        channel_id: if task.channel_id.is_empty() {
                            "general".into()
                        } else {
                            task.channel_id
                        },
                        thread_id: if task.thread_id.is_empty() {
                            task.id.clone()
                        } else {
                            task.thread_id
                        },
                        author: bot_id.to_string(),
                        author_kind: "bot".into(),
                        content: reply_text,
                        created_at: now_secs(),
                    });
                }
            }
        }
        Ok(json!({ "task_id": task_id, "bot_id": bot_id, "result": out }))
    }

    async fn handle(&self, who: &str, req: Request) -> Response {
        let path = req.path().to_string();
        let method = req.method.as_str();

        if path == "/v1/team" || path == "/v1/team/health" {
            return if method == "GET" {
                Response::json(200, &self.health())
            } else {
                Response::error(405, "GET only")
            };
        }

        // Priority 1: standalone `hanzo-team` binary or registered local upstream for transactor.
        if let Some(port) = self.maybe_team_port().await {
            if let Ok(r) = engine::fetch(([127, 0, 0, 1], port).into(), &req).await {
                return r;
            }
        }
        if path.starts_with("/v1/team/transactor") {
            let up_addr = *self.upstream.lock().unwrap();
            if let Some(addr) = up_addr {
                if let Ok(r) = engine::fetch(addr, &req).await {
                    return r;
                }
            }
        }

        // Optional explicit Cloud forward (`?cloud=1` or `X-Team-Forward: cloud`).
        let want_cloud = req.target.contains("cloud=1")
            || req
                .headers
                .iter()
                .any(|(k, v)| k.eq_ignore_ascii_case("x-team-forward") && v == "cloud");
        if want_cloud {
            return self.forward_cloud(req).await;
        }

        if path == "/v1/team/spaces" {
            return match method {
                "GET" => match self.with_db(|c| {
                    let mut stmt = c
                        .prepare("SELECT payload FROM team_spaces ORDER BY created_at ASC")
                        .map_err(io::Error::other)?;
                    let rows = stmt
                        .query_map([], |r| r.get::<_, String>(0))
                        .map_err(io::Error::other)?;
                    let mut out = Vec::new();
                    for r in rows {
                        if let Ok(s) = serde_json::from_str::<TeamSpace>(&r.map_err(io::Error::other)?) {
                            out.push(s);
                        }
                    }
                    Ok(out)
                }) {
                    Ok(spaces) => Response::json(200, &json!({ "spaces": spaces })),
                    Err(e) => Response::error(500, &e.to_string()),
                },
                "POST" => {
                    let parsed: CreateSpaceReq = match serde_json::from_slice(&req.body) {
                        Ok(r) => r,
                        Err(e) => return Response::error(400, &format!("invalid JSON: {e}")),
                    };
                    let space = TeamSpace {
                        id: parsed.id.unwrap_or_else(|| next_id("space")),
                        name: parsed.name,
                        description: parsed.description,
                        created_at: now_secs(),
                    };
                    let raw = serde_json::to_string(&space).unwrap_or_default();
                    if let Err(e) = self.with_db(|c| {
                        c.execute(
                            "INSERT OR REPLACE INTO team_spaces (id, created_at, payload) VALUES (?1, ?2, ?3)",
                            params![space.id, space.created_at, raw],
                        )
                        .map_err(io::Error::other)?;
                        Ok(())
                    }) {
                        return Response::error(500, &e.to_string());
                    }
                    self.ledger.record(who, "team.space.create", &space.id);
                    Response::json(201, &json!({ "space": space }))
                }
                _ => Response::error(405, "GET or POST"),
            };
        }

        if path == "/v1/team/channels" {
            return match method {
                "GET" => match self.with_db(|c| {
                    let mut stmt = c
                        .prepare("SELECT payload FROM team_channels ORDER BY created_at ASC")
                        .map_err(io::Error::other)?;
                    let rows = stmt
                        .query_map([], |r| r.get::<_, String>(0))
                        .map_err(io::Error::other)?;
                    let mut out = Vec::new();
                    for r in rows {
                        if let Ok(ch) = serde_json::from_str::<TeamChannel>(&r.map_err(io::Error::other)?) {
                            out.push(ch);
                        }
                    }
                    Ok(out)
                }) {
                    Ok(channels) => Response::json(200, &json!({ "channels": channels })),
                    Err(e) => Response::error(500, &e.to_string()),
                },
                "POST" => {
                    let parsed: CreateChannelReq = match serde_json::from_slice(&req.body) {
                        Ok(r) => r,
                        Err(e) => return Response::error(400, &format!("invalid JSON: {e}")),
                    };
                    let ch = TeamChannel {
                        id: parsed.id.unwrap_or_else(|| next_id("chan")),
                        space_id: parsed.space_id,
                        name: parsed.name,
                        topic: parsed.topic,
                        created_at: now_secs(),
                    };
                    let raw = serde_json::to_string(&ch).unwrap_or_default();
                    if let Err(e) = self.with_db(|c| {
                        c.execute(
                            "INSERT OR REPLACE INTO team_channels (id, space_id, created_at, payload) VALUES (?1, ?2, ?3, ?4)",
                            params![ch.id, ch.space_id, ch.created_at, raw],
                        )
                        .map_err(io::Error::other)?;
                        Ok(())
                    }) {
                        return Response::error(500, &e.to_string());
                    }
                    Response::json(201, &json!({ "channel": ch }))
                }
                _ => Response::error(405, "GET or POST"),
            };
        }

        if path == "/v1/team/threads" {
            return match method {
                "GET" => match self.with_db(|c| {
                    let mut stmt = c
                        .prepare("SELECT payload FROM team_threads ORDER BY created_at ASC LIMIT 500")
                        .map_err(io::Error::other)?;
                    let rows = stmt
                        .query_map([], |r| r.get::<_, String>(0))
                        .map_err(io::Error::other)?;
                    let mut out = Vec::new();
                    for r in rows {
                        if let Ok(m) = serde_json::from_str::<ThreadMessage>(&r.map_err(io::Error::other)?) {
                            out.push(m);
                        }
                    }
                    Ok(out)
                }) {
                    Ok(messages) => Response::json(200, &json!({ "messages": messages })),
                    Err(e) => Response::error(500, &e.to_string()),
                },
                "POST" => {
                    let parsed: PostThreadReq = match serde_json::from_slice(&req.body) {
                        Ok(r) => r,
                        Err(e) => return Response::error(400, &format!("invalid JSON: {e}")),
                    };
                    let id = next_id("tmsg");
                    let msg = ThreadMessage {
                        thread_id: parsed.thread_id.unwrap_or_else(|| id.clone()),
                        id,
                        channel_id: parsed.channel_id,
                        author: parsed.author.unwrap_or_else(|| who.to_string()),
                        author_kind: parsed.author_kind,
                        content: parsed.content,
                        created_at: now_secs(),
                    };
                    if let Err(e) = self.append_message(&msg) {
                        return Response::error(500, &e.to_string());
                    }
                    Response::json(201, &json!({ "message": msg }))
                }
                _ => Response::error(405, "GET or POST"),
            };
        }

        if path == "/v1/team/tasks" {
            return match method {
                "GET" => match self.with_db(|c| {
                    let mut stmt = c
                        .prepare("SELECT payload FROM team_tasks ORDER BY updated_at DESC LIMIT 500")
                        .map_err(io::Error::other)?;
                    let rows = stmt
                        .query_map([], |r| r.get::<_, String>(0))
                        .map_err(io::Error::other)?;
                    let mut out = Vec::new();
                    for r in rows {
                        if let Ok(t) = serde_json::from_str::<BoardTask>(&r.map_err(io::Error::other)?) {
                            out.push(t);
                        }
                    }
                    Ok(out)
                }) {
                    Ok(tasks) => Response::json(200, &json!({ "tasks": tasks })),
                    Err(e) => Response::error(500, &e.to_string()),
                },
                "POST" => {
                    let parsed: UpsertTaskReq = match serde_json::from_slice(&req.body) {
                        Ok(r) => r,
                        Err(e) => return Response::error(400, &format!("invalid JSON: {e}")),
                    };
                    let bearer = req.bearer().map(str::to_owned);
                    match self.upsert_task(who, None, parsed, bearer).await {
                        Ok(task) => Response::json(201, &json!({ "task": task })),
                        Err(e) => Response::error(500, &e.to_string()),
                    }
                }
                _ => Response::error(405, "GET or POST"),
            };
        }

        if let Some(id) = path.strip_prefix("/v1/team/tasks/") {
            let id = id.trim_matches('/');
            if method == "PUT" && !id.is_empty() && !id.contains('/') {
                let parsed: UpsertTaskReq = match serde_json::from_slice(&req.body) {
                    Ok(r) => r,
                    Err(e) => return Response::error(400, &format!("invalid JSON: {e}")),
                };
                let bearer = req.bearer().map(str::to_owned);
                return match self.upsert_task(who, Some(id), parsed, bearer).await {
                    Ok(task) => Response::json(200, &json!({ "task": task })),
                    Err(e) => Response::error(500, &e.to_string()),
                };
            }
        }

        self.forward_cloud(req).await
    }

    async fn forward_cloud(&self, req: Request) -> Response {
        let tok = req
            .bearer()
            .map(str::to_owned)
            .or_else(|| me::tokens().map(|(_, t)| t.access_token));
        let Some(tok) = tok else {
            return Response::error(404, "unknown local /v1/team route (sign in for cloud sync)");
        };
        let url = format!("{}{}", self.cloud_api(), req.target);
        let method = req.method.clone();
        let body = req.body.clone();
        let res = tokio::task::spawn_blocking(move || {
            let r = ureq::request(&method, &url)
                .timeout(Duration::from_secs(15))
                .set("Authorization", &format!("Bearer {tok}"))
                .set("Content-Type", "application/json");
            let resp = if body.is_empty() {
                r.call()
            } else {
                r.send_bytes(&body)
            };
            match resp {
                Ok(ok) => {
                    let status = ok.status();
                    let mut bytes = Vec::new();
                    let _ = std::io::Read::read_to_end(&mut ok.into_reader(), &mut bytes);
                    Response {
                        status,
                        headers: vec![("content-type".into(), "application/json".into())],
                        body: bytes,
                    }
                }
                Err(ureq::Error::Status(code, err_resp)) => {
                    let mut bytes = Vec::new();
                    let _ = std::io::Read::read_to_end(&mut err_resp.into_reader(), &mut bytes);
                    Response {
                        status: code,
                        headers: vec![("content-type".into(), "application/json".into())],
                        body: bytes,
                    }
                }
                Err(e) => Response::error(502, &e.to_string()),
            }
        })
        .await;
        res.unwrap_or_else(|e| Response::error(500, &e.to_string()))
    }
}

impl Subsystem for Team {
    fn name(&self) -> &'static str {
        "team"
    }
    fn prefix(&self) -> &'static str {
        "/v1/team"
    }
    fn resident(&self) -> bool {
        true
    }
    fn start(&self) -> Fut<'_, io::Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn frame<'a>(&'a self, who: &'a str, req: Request) -> Fut<'a, io::Result<Reply>> {
        Box::pin(async move { Ok(Reply::Whole(self.handle(who, req).await)) })
    }
    fn stop(&self) -> Fut<'_, ()> {
        Box::pin(async move {
            *self.port.lock().unwrap() = None;
            if let Some(mut c) = self.child.lock().await.take() {
                proc::terminate(&mut c, Duration::from_secs(5)).await;
            }
        })
    }
}

fn dispatch_board_task(
    cloud_api: &str,
    token: &str,
    queue: &str,
    task: &BoardTask,
) -> Result<String, String> {
    let act_id = format!("team-{}", task.id);
    let body = json!({
        "activityId": act_id,
        "activityType": { "name": "team.task" },
        "taskQueue": queue,
        "heartbeatTimeout": "60s",
        "input": {
            "task_id": task.id,
            "space_id": task.space_id,
            "channel_id": task.channel_id,
            "thread_id": task.thread_id,
            "title": task.title,
            "description": task.description,
            "bot_id": task.bot_id.clone().unwrap_or_else(|| "hanzo".into()),
        },
        "requestId": format!("{}-{}", task.id, task.updated_at),
    });
    ureq::post(&format!("{cloud_api}/v1/workflow/namespaces/gpu-jobs/activities"))
        .timeout(Duration::from_secs(10))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
        .map_err(|e| e.to_string())?;
    Ok(act_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pins::Pins;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[tokio::test]
    async fn team_spaces_threads_and_bot_board_task_execution_work() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for _ in 0..2 {
                if let Ok((mut s, _)) = listener.accept() {
                    let mut buf = [0u8; 4096];
                    let _ = s.read(&mut buf);
                    let body = r#"{"choices":[{"message":{"content":"Pods healthy."}}]}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = s.write_all(resp.as_bytes());
                }
            }
        });

        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(Ledger::open(dir.path().join("ledger")));
        let upstream: Upstream = Arc::new(StdMutex::new(None));
        let team = Team::with_db_path(dir.path().join("team.sqlite"), ledger.clone(), upstream, Some(addr.clone()));
        let pins = Arc::new(Pins::load(&dir.path().join("pins.json")));
        let eng = Arc::new(crate::engine::Engine::new(pins));
        let bot = Bot::with_db_path(dir.path().join("bot.sqlite"), ledger, eng, Some(addr));

        let task = team
            .upsert_task(
                "z/admin",
                None,
                UpsertTaskReq {
                    id: Some("t-1".into()),
                    space_id: "hanzo".into(),
                    channel_id: "eng".into(),
                    thread_id: "thr-1".into(),
                    title: "Verify k3s health".into(),
                    description: "Check pods".into(),
                    status: None,
                    assignee: None,
                    bot_id: Some("dev".into()),
                    dispatch: true,
                    task_queue: None,
                },
                Some("mock-test-token".into()),
            )
            .await
            .unwrap();
        assert_eq!(task.status, "in_progress");

        let res = team
            .execute_team_task(
                "tasks-worker",
                &json!({
                    "task_id": "t-1",
                    "bot_id": "dev",
                    "title": "Verify k3s health",
                    "description": "Check pods",
                }),
                &bot,
                Some("mock-test-token".into()),
            )
            .await
            .unwrap();
        assert_eq!(res["task_id"], "t-1");
        let updated = team.get_task("t-1").unwrap().unwrap();
        assert_eq!(updated.status, "done");
    }
}
