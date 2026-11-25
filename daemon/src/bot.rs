// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! `/v1/bot`: Unified Hanzo Bot backend inside `hanzod`.
//!
//! Serves bot presets, persistent bot definitions, local/cloud execution runs,
//! native `hanzoai/tasks` schedule registration (`bot-<id>`), and optional lazy
//! proxying to a standalone `hanzo-bot` gateway binary at `/v1/bot/gateway/*`.

use crate::engine::{self, Engine};
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
static BOT_COUNTER: AtomicU64 = AtomicU64::new(1);

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn next_id(prefix: &str) -> String {
    let ts = now_secs() as u64;
    let seq = BOT_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}_{ts:x}_{seq:04x}")
}

#[derive(Debug, Clone, Serialize)]
pub struct BotTemplate {
    pub id: &'static str,
    pub name: &'static str,
    pub role: &'static str,
    pub default_model: &'static str,
    pub instructions: &'static str,
}

pub const TEMPLATES: &[BotTemplate] = &[
    BotTemplate { id: "hanzo", name: "Hanzo", role: "Chief Orchestrator", default_model: "enso", instructions: "Coordinate engineering, research, and operations across linked machines." },
    BotTemplate { id: "vi", name: "Vi", role: "Principal Architect", default_model: "enso", instructions: "Design minimal, high-leverage system architectures and review critical code." },
    BotTemplate { id: "des", name: "Des", role: "Design Systems Lead", default_model: "enso", instructions: "Craft crisp interfaces, typography, and accessible UX flows." },
    BotTemplate { id: "dev", name: "Dev", role: "Autonomous Software Engineer", default_model: "enso", instructions: "Implement tested production code, fix root causes, and verify builds." },
    BotTemplate { id: "einstein", name: "Einstein", role: "Theoretical Research Lead", default_model: "enso", instructions: "Formulate falsifiable hypotheses and mathematical invariants." },
    BotTemplate { id: "feynman", name: "Feynman", role: "First-Principles Verifier", default_model: "enso", instructions: "Deconstruct assumptions, measure empirically, and explain clearly." },
    BotTemplate { id: "leo", name: "Leo", role: "Polymath Builder", default_model: "enso", instructions: "Unify hardware, graphics, simulation, and systems engineering." },
    BotTemplate { id: "maya", name: "Maya", role: "Product & Growth Strategist", default_model: "enso", instructions: "Turn user workflows into clear product loops and adoption." },
    BotTemplate { id: "nora", name: "Nora", role: "Operations & Reliability", default_model: "enso", instructions: "Maintain SLOs, incident playbooks, and fleet health." },
    BotTemplate { id: "creative", name: "Creative", role: "Media & Studio Director", default_model: "enso", instructions: "Direct generative visual, audio, and narrative production." },
    BotTemplate { id: "antje", name: "Antje", role: "Ecosystem & Partnerships", default_model: "enso", instructions: "Align open-source standards, developer docs, and community." },
    BotTemplate { id: "zach", name: "Zach", role: "Founder Mode Reviewer", default_model: "enso", instructions: "Enforce simplicity, velocity, native Rust/Go performance, and zero bloat." },
    BotTemplate { id: "jobs", name: "Jobs", role: "Product Taste Critic", default_model: "enso", instructions: "Eliminate clutter until every interaction is obvious and cohesive." },
    BotTemplate { id: "dario", name: "Dario", role: "Safety & Scaling Researcher", default_model: "enso", instructions: "Evaluate alignment, scaling laws, and empirical benchmark rigor." },
    BotTemplate { id: "altman", name: "Altman", role: "Deployment & Compute Strategist", default_model: "enso", instructions: "Scale compute fleets and ship high-impact capabilities." },
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotRecord {
    pub id: String,
    pub name: String,
    pub template: String,
    pub instructions: String,
    pub model: String,
    #[serde(default)]
    pub workspace: String,
    #[serde(default)]
    pub channels: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule_cron: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule_interval: Option<String>,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Deserialize)]
pub struct UpsertBotReq {
    pub id: Option<String>,
    pub name: Option<String>,
    pub template: Option<String>,
    pub instructions: Option<String>,
    pub model: Option<String>,
    pub workspace: Option<String>,
    pub channels: Option<Vec<String>>,
    pub schedule_cron: Option<String>,
    pub schedule_interval: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotRunRecord {
    pub id: String,
    pub bot_id: String,
    pub prompt: String,
    pub model: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_activity_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Deserialize)]
pub struct RunBotReq {
    #[serde(default)]
    pub prompt: String,
    pub model: Option<String>,
    #[serde(default)]
    pub r#async: bool,
    pub task_queue: Option<String>,
}

pub struct Bot {
    db_path: PathBuf,
    conn: StdMutex<Option<Connection>>,
    ledger: Arc<Ledger>,
    engine: Arc<Engine>,
    gateway_child: Mutex<Option<Child>>,
    gateway_port: StdMutex<Option<u16>>,
    cloud_api_override: Option<String>,
}

impl Bot {
    pub fn new(ledger: Arc<Ledger>, engine: Arc<Engine>) -> Self {
        Self::with_db_path(paths::data().join("bot").join("state.sqlite"), ledger, engine, None)
    }

    pub fn with_db_path(
        db_path: PathBuf,
        ledger: Arc<Ledger>,
        engine: Arc<Engine>,
        cloud_api_override: Option<String>,
    ) -> Self {
        Self {
            db_path,
            conn: StdMutex::new(None),
            ledger,
            engine,
            gateway_child: Mutex::new(None),
            gateway_port: StdMutex::new(None),
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
                 CREATE TABLE IF NOT EXISTS bots (
                     id TEXT PRIMARY KEY,
                     updated_at INTEGER NOT NULL,
                     payload TEXT NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS bot_runs (
                     id TEXT PRIMARY KEY,
                     created_at INTEGER NOT NULL,
                     payload TEXT NOT NULL
                 );",
            )
            .map_err(io::Error::other)?;
            *guard = Some(c);
        }
        f(guard.as_ref().unwrap())
    }

    pub fn list_bots(&self) -> io::Result<Vec<BotRecord>> {
        self.with_db(|c| {
            let mut stmt = c
                .prepare("SELECT payload FROM bots ORDER BY updated_at DESC")
                .map_err(io::Error::other)?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(io::Error::other)?;
            let mut out = Vec::new();
            for row in rows {
                if let Ok(b) = serde_json::from_str::<BotRecord>(&row.map_err(io::Error::other)?) {
                    out.push(b);
                }
            }
            Ok(out)
        })
    }

    pub fn get_bot(&self, id: &str) -> io::Result<Option<BotRecord>> {
        self.with_db(|c| {
            let raw: Option<String> = c
                .query_row("SELECT payload FROM bots WHERE id = ?1", params![id], |r| r.get(0))
                .optional()
                .map_err(io::Error::other)?;
            Ok(raw.and_then(|s| serde_json::from_str(&s).ok()))
        })
    }

    fn save_bot(&self, bot: &BotRecord) -> io::Result<()> {
        let raw = serde_json::to_string(bot).map_err(io::Error::other)?;
        self.with_db(|c| {
            c.execute(
                "INSERT INTO bots (id, updated_at, payload) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET updated_at = excluded.updated_at, payload = excluded.payload",
                params![bot.id, bot.updated_at, raw],
            )
            .map_err(io::Error::other)?;
            Ok(())
        })
    }

    fn save_run(&self, run: &BotRunRecord) -> io::Result<()> {
        let raw = serde_json::to_string(run).map_err(io::Error::other)?;
        self.with_db(|c| {
            c.execute(
                "INSERT INTO bot_runs (id, created_at, payload) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET payload = excluded.payload",
                params![run.id, run.created_at, raw],
            )
            .map_err(io::Error::other)?;
            Ok(())
        })
    }

    pub fn list_runs(&self) -> io::Result<Vec<BotRunRecord>> {
        self.with_db(|c| {
            let mut stmt = c
                .prepare("SELECT payload FROM bot_runs ORDER BY created_at DESC LIMIT 200")
                .map_err(io::Error::other)?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(io::Error::other)?;
            let mut out = Vec::new();
            for row in rows {
                if let Ok(r) = serde_json::from_str::<BotRunRecord>(&row.map_err(io::Error::other)?) {
                    out.push(r);
                }
            }
            Ok(out)
        })
    }

    pub fn health(&self) -> Value {
        let bots = self.list_bots().unwrap_or_default();
        let active = bots.iter().filter(|b| b.enabled).count();
        json!({
            "ok": true,
            "runtime": "hanzod-native",
            "version": env!("CARGO_PKG_VERSION"),
            "active_bots": active,
            "total_bots": bots.len(),
            "gateway_available": proc::find("hanzo-bot").is_some(),
            "tasks_queue": "bot-jobs",
        })
    }

    pub async fn upsert_bot(
        &self,
        who: &str,
        existing_id: Option<&str>,
        req: UpsertBotReq,
        bearer: Option<String>,
    ) -> io::Result<BotRecord> {
        let now = now_secs();
        let prev = match existing_id {
            Some(id) => self.get_bot(id)?,
            None => req.id.as_deref().and_then(|id| self.get_bot(id).ok().flatten()),
        };
        let tpl_id = req
            .template
            .clone()
            .or_else(|| prev.as_ref().map(|p| p.template.clone()))
            .unwrap_or_else(|| "hanzo".to_string());
        let tpl = TEMPLATES.iter().find(|t| t.id == tpl_id.as_str());

        let id = existing_id
            .map(str::to_owned)
            .or_else(|| req.id.filter(|s| !s.trim().is_empty()))
            .or_else(|| prev.as_ref().map(|p| p.id.clone()))
            .unwrap_or_else(|| next_id("bot"));

        let name = req
            .name
            .filter(|s| !s.trim().is_empty())
            .or_else(|| prev.as_ref().map(|p| p.name.clone()))
            .or_else(|| tpl.map(|t| t.name.to_string()))
            .unwrap_or_else(|| id.clone());

        let instructions = req
            .instructions
            .filter(|s| !s.trim().is_empty())
            .or_else(|| prev.as_ref().map(|p| p.instructions.clone()))
            .or_else(|| tpl.map(|t| t.instructions.to_string()))
            .unwrap_or_else(|| "Assist the team accurately and concisely.".to_string());

        let model = req
            .model
            .filter(|s| !s.trim().is_empty())
            .or_else(|| prev.as_ref().map(|p| p.model.clone()))
            .or_else(|| tpl.map(|t| t.default_model.to_string()))
            .unwrap_or_else(|| "enso".to_string());

        let record = BotRecord {
            id: id.clone(),
            name,
            template: tpl_id,
            instructions,
            model,
            workspace: req
                .workspace
                .or_else(|| prev.as_ref().map(|p| p.workspace.clone()))
                .unwrap_or_default(),
            channels: req
                .channels
                .or_else(|| prev.as_ref().map(|p| p.channels.clone()))
                .unwrap_or_default(),
            schedule_cron: req
                .schedule_cron
                .or_else(|| prev.as_ref().and_then(|p| p.schedule_cron.clone()))
                .filter(|s| !s.trim().is_empty()),
            schedule_interval: req
                .schedule_interval
                .or_else(|| prev.as_ref().and_then(|p| p.schedule_interval.clone()))
                .filter(|s| !s.trim().is_empty()),
            enabled: req
                .enabled
                .or_else(|| prev.as_ref().map(|p| p.enabled))
                .unwrap_or(true),
            created_at: prev.as_ref().map(|p| p.created_at).unwrap_or(now),
            updated_at: now,
        };
        self.save_bot(&record)?;
        self.ledger.record(who, "bot.upsert", &record.id);

        if record.schedule_cron.is_some() || record.schedule_interval.is_some() {
            let tok = bearer.or_else(|| me::tokens().map(|(_, t)| t.access_token));
            if let Some(tok) = tok {
                let api = self.cloud_api();
                let bot_clone = record.clone();
                let _ = tokio::task::spawn_blocking(move || sync_bot_schedule(&api, &tok, &bot_clone)).await;
            }
        }
        Ok(record)
    }

    pub async fn run_bot(
        &self,
        who: &str,
        bot_id: &str,
        req: RunBotReq,
        bearer: Option<String>,
    ) -> io::Result<(u16, BotRunRecord)> {
        let bot = self.get_bot(bot_id)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, format!("bot {bot_id} not found"))
        })?;
        let prompt = if req.prompt.trim().is_empty() {
            format!("Execute scheduled check-in for {}.", bot.name)
        } else {
            req.prompt.trim().to_string()
        };
        let model = req
            .model
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| bot.model.clone());
        let run_id = next_id("botrun");

        if req.r#async {
            let tok = bearer
                .or_else(|| me::tokens().map(|(_, t)| t.access_token))
                .unwrap_or_default();
            let api = self.cloud_api();
            let queue = req.task_queue.unwrap_or_else(|| "gpu-jobs".to_string());
            let act_id = run_id.clone();
            let b_id = bot.id.clone();
            let p_clone = prompt.clone();
            let m_clone = model.clone();
            let enq = tokio::task::spawn_blocking(move || {
                enqueue_bot_activity(&api, &tok, &act_id, &queue, &b_id, &p_clone, &m_clone)
            })
            .await
            .map_err(io::Error::other)?;
            let mut rec = BotRunRecord {
                id: run_id.clone(),
                bot_id: bot.id,
                prompt,
                model,
                status: if enq.is_ok() { "queued".into() } else { "faulted".into() },
                output: None,
                task_activity_id: enq.as_ref().ok().cloned(),
                error: enq.err(),
                created_at: now_secs(),
            };
            if rec.task_activity_id.is_none() && rec.status == "queued" {
                rec.task_activity_id = Some(run_id);
            }
            self.save_run(&rec)?;
            return Ok((202, rec));
        }

        let answer = self
            .complete_prompt(&bot.instructions, &prompt, &model, bearer.as_deref())
            .await;
        let rec = match answer {
            Ok(text) => BotRunRecord {
                id: run_id,
                bot_id: bot.id.clone(),
                prompt,
                model,
                status: "complete".into(),
                output: Some(text),
                task_activity_id: None,
                error: None,
                created_at: now_secs(),
            },
            Err(err) => BotRunRecord {
                id: run_id,
                bot_id: bot.id.clone(),
                prompt,
                model,
                status: "faulted".into(),
                output: None,
                task_activity_id: None,
                error: Some(err),
                created_at: now_secs(),
            },
        };
        self.save_run(&rec)?;
        self.ledger
            .record(who, "bot.run", &format!("{} ({})", rec.id, rec.status));
        Ok((200, rec))
    }

    pub async fn execute_bot_job(
        &self,
        who: &str,
        input: &Value,
        bearer: Option<String>,
    ) -> Result<Value, String> {
        let bot_id = input
            .get("bot_id")
            .or_else(|| input.get("botId"))
            .and_then(Value::as_str)
            .unwrap_or("hanzo");
        let prompt = input
            .get("prompt")
            .and_then(Value::as_str)
            .unwrap_or("Run scheduled bot tick.")
            .to_string();
        let model = input.get("model").and_then(Value::as_str).map(str::to_owned);

        // Ensure preset bot exists if invoked by template id.
        if self.get_bot(bot_id).map_err(|e| e.to_string())?.is_none() {
            if TEMPLATES.iter().any(|t| t.id == bot_id) {
                let _ = self
                    .upsert_bot(
                        who,
                        Some(bot_id),
                        UpsertBotReq {
                            id: Some(bot_id.to_string()),
                            name: None,
                            template: Some(bot_id.to_string()),
                            instructions: None,
                            model: model.clone(),
                            workspace: None,
                            channels: None,
                            schedule_cron: None,
                            schedule_interval: None,
                            enabled: Some(true),
                        },
                        bearer.clone(),
                    )
                    .await;
            }
        }

        let (_, run) = self
            .run_bot(
                who,
                bot_id,
                RunBotReq {
                    prompt,
                    model,
                    r#async: false,
                    task_queue: None,
                },
                bearer,
            )
            .await
            .map_err(|e| e.to_string())?;
        if run.status == "complete" {
            Ok(json!(run))
        } else {
            Err(run.error.unwrap_or_else(|| "bot execution failed".into()))
        }
    }

    async fn complete_prompt(
        &self,
        instructions: &str,
        prompt: &str,
        model: &str,
        bearer_override: Option<&str>,
    ) -> Result<String, String> {
        let messages = json!([
            { "role": "system", "content": instructions },
            { "role": "user", "content": prompt },
        ]);
        if let Some(port) = self.engine.port() {
            let body = serde_json::to_vec(&json!({
                "model": model,
                "messages": messages,
                "stream": false,
            }))
            .unwrap_or_default();
            let req = Request {
                method: "POST".into(),
                target: "/v1/chat/completions".into(),
                headers: vec![("content-type".into(), "application/json".into())],
                body,
            };
            if let Ok(resp) = engine::fetch(([127, 0, 0, 1], port).into(), &req).await {
                if resp.status == 200 {
                    if let Ok(v) = serde_json::from_slice::<Value>(&resp.body) {
                        if let Some(txt) = v["choices"][0]["message"]["content"].as_str() {
                            return Ok(txt.to_string());
                        }
                    }
                }
            }
        }

        let tok = bearer_override
            .map(str::to_owned)
            .or_else(|| me::tokens().map(|(_, t)| t.access_token));
        let Some(tok) = tok else {
            return Ok(format!("[local-bot:{model}] acknowledged: {prompt}"));
        };
        let api = self.cloud_api();
        let payload = json!({
            "model": model,
            "messages": messages,
            "stream": false,
        });
        tokio::task::spawn_blocking(move || {
            let resp = ureq::post(&format!("{api}/v1/chat/completions"))
                .timeout(Duration::from_secs(30))
                .set("Authorization", &format!("Bearer {tok}"))
                .set("Content-Type", "application/json")
                .send_string(&payload.to_string())
                .map_err(|e| e.to_string())?;
            let v: Value = serde_json::from_reader(resp.into_reader()).map_err(|e| e.to_string())?;
            Ok(v["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or("")
                .to_string())
        })
        .await
        .map_err(|e| e.to_string())?
    }

    async fn ensure_gateway(&self) -> io::Result<u16> {
        if let Some(port) = *self.gateway_port.lock().unwrap() {
            return Ok(port);
        }
        let mut slot = self.gateway_child.lock().await;
        if let Some(port) = *self.gateway_port.lock().unwrap() {
            return Ok(port);
        }
        let bin = proc::find("hanzo-bot").ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "hanzo-bot gateway binary not installed")
        })?;
        proc::owned_by_us_alone(&bin)?;
        let port = proc::free_port()?;
        let child = proc::bg_tokio_cmd(&bin)
            .args(["gateway", "--host", "127.0.0.1", "--port", &port.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        *slot = Some(child);
        *self.gateway_port.lock().unwrap() = Some(port);
        Ok(port)
    }

    async fn handle(&self, who: &str, req: Request) -> Response {
        let path = req.path().to_string();
        let method = req.method.as_str();

        if path == "/v1/bot" || path == "/v1/bot/health" {
            return if method == "GET" {
                Response::json(200, &self.health())
            } else {
                Response::error(405, "GET only")
            };
        }
        if path == "/v1/bot/templates" {
            return if method == "GET" {
                Response::json(200, &json!({ "templates": TEMPLATES }))
            } else {
                Response::error(405, "GET only")
            };
        }
        if path == "/v1/bot/runs" {
            return if method == "GET" {
                match self.list_runs() {
                    Ok(runs) => Response::json(200, &json!({ "runs": runs })),
                    Err(e) => Response::error(500, &e.to_string()),
                }
            } else {
                Response::error(405, "GET only")
            };
        }
        if path.starts_with("/v1/bot/gateway") {
            return match self.ensure_gateway().await {
                Ok(port) => {
                    let target = req.target.strip_prefix("/v1/bot/gateway").unwrap_or("/");
                    let fwd = Request {
                        target: if target.is_empty() { "/".into() } else { target.into() },
                        ..req
                    };
                    match engine::fetch(([127, 0, 0, 1], port).into(), &fwd).await {
                        Ok(r) => r,
                        Err(e) => Response::error(502, &e.to_string()),
                    }
                }
                Err(e) => Response::error(404, &e.to_string()),
            };
        }
        if path == "/v1/bot/bots" {
            return match method {
                "GET" => match self.list_bots() {
                    Ok(bots) => Response::json(200, &json!({ "bots": bots })),
                    Err(e) => Response::error(500, &e.to_string()),
                },
                "POST" => {
                    let parsed: UpsertBotReq = match serde_json::from_slice(&req.body) {
                        Ok(r) => r,
                        Err(e) => return Response::error(400, &format!("invalid JSON: {e}")),
                    };
                    let bearer = req.bearer().map(str::to_owned);
                    match self.upsert_bot(who, None, parsed, bearer).await {
                        Ok(bot) => Response::json(201, &json!({ "bot": bot })),
                        Err(e) => Response::error(500, &e.to_string()),
                    }
                }
                _ => Response::error(405, "GET or POST /v1/bot/bots"),
            };
        }
        if let Some(rest) = path.strip_prefix("/v1/bot/bots/") {
            let parts: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
            match parts.as_slice() {
                [id] => match method {
                    "GET" => match self.get_bot(id) {
                        Ok(Some(b)) => Response::json(200, &json!({ "bot": b })),
                        Ok(None) => Response::error(404, &format!("bot {id} not found")),
                        Err(e) => Response::error(500, &e.to_string()),
                    },
                    "PUT" => {
                        let parsed: UpsertBotReq = match serde_json::from_slice(&req.body) {
                            Ok(r) => r,
                            Err(e) => return Response::error(400, &format!("invalid JSON: {e}")),
                        };
                        let bearer = req.bearer().map(str::to_owned);
                        match self.upsert_bot(who, Some(id), parsed, bearer).await {
                            Ok(b) => Response::json(200, &json!({ "bot": b })),
                            Err(e) => Response::error(500, &e.to_string()),
                        }
                    }
                    "DELETE" => {
                        let deleted = self
                            .with_db(|c| {
                                let n = c
                                    .execute("DELETE FROM bots WHERE id = ?1", params![id])
                                    .map_err(io::Error::other)?;
                                Ok(n > 0)
                            })
                            .unwrap_or(false);
                        if !deleted {
                            return Response::error(404, &format!("bot {id} not found"));
                        }
                        self.ledger.record(who, "bot.delete", id);
                        Response::json(200, &json!({ "deleted": true, "id": id }))
                    }
                    _ => Response::error(405, "GET, PUT, or DELETE"),
                },
                [id, "run"] if method == "POST" => {
                    let run_req: RunBotReq = if req.body.is_empty() {
                        RunBotReq {
                            prompt: String::new(),
                            model: None,
                            r#async: false,
                            task_queue: None,
                        }
                    } else {
                        match serde_json::from_slice(&req.body) {
                            Ok(r) => r,
                            Err(e) => return Response::error(400, &format!("invalid JSON: {e}")),
                        }
                    };
                    let bearer = req.bearer().map(str::to_owned);
                    match self.run_bot(who, id, run_req, bearer).await {
                        Ok((code, run)) => Response::json(code, &json!({ "run": run })),
                        Err(e) if e.kind() == io::ErrorKind::NotFound => {
                            Response::error(404, &e.to_string())
                        }
                        Err(e) => Response::error(500, &e.to_string()),
                    }
                }
                _ => Response::error(404, "unknown /v1/bot/bots route"),
            }
        } else {
            Response::error(404, "unknown /v1/bot route")
        }
    }
}

impl Subsystem for Bot {
    fn name(&self) -> &'static str {
        "bot"
    }
    fn prefix(&self) -> &'static str {
        "/v1/bot"
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
            *self.gateway_port.lock().unwrap() = None;
            if let Some(mut c) = self.gateway_child.lock().await.take() {
                proc::terminate(&mut c, Duration::from_secs(5)).await;
            }
        })
    }
}

fn sync_bot_schedule(cloud_api: &str, token: &str, bot: &BotRecord) -> Result<(), String> {
    let mut spec = json!({});
    if let Some(ref cron) = bot.schedule_cron {
        spec["cronString"] = json!([cron]);
    }
    if let Some(ref iv) = bot.schedule_interval {
        spec["interval"] = json!([{ "interval": iv }]);
    }
    let body = json!({
        "scheduleId": format!("bot-{}", bot.id),
        "namespace": "gpu-jobs",
        "spec": spec,
        "action": {
            "workflowType": { "name": "bot.run" },
            "activityType": { "name": "bot.run" },
            "taskQueue": "gpu-jobs",
            "input": {
                "bot_id": bot.id,
                "model": bot.model,
                "prompt": format!("Scheduled execution for bot {}", bot.name),
            }
        },
        "state": { "paused": !bot.enabled }
    });
    let _ = ureq::post(&format!("{cloud_api}/v1/workflow/namespaces/gpu-jobs/schedules"))
        .timeout(Duration::from_secs(10))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn enqueue_bot_activity(
    cloud_api: &str,
    token: &str,
    act_id: &str,
    queue: &str,
    bot_id: &str,
    prompt: &str,
    model: &str,
) -> Result<String, String> {
    if token.trim().is_empty() {
        return Err("missing IAM token for async tasks enqueue".into());
    }
    let body = json!({
        "activityId": act_id,
        "activityType": { "name": "bot.run" },
        "taskQueue": queue,
        "heartbeatTimeout": "60s",
        "input": {
            "bot_id": bot_id,
            "prompt": prompt,
            "model": model,
        },
        "requestId": act_id,
    });
    ureq::post(&format!("{cloud_api}/v1/workflow/namespaces/gpu-jobs/activities"))
        .timeout(Duration::from_secs(10))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
        .map_err(|e| e.to_string())?;
    Ok(act_id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pins::Pins;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[tokio::test]
    async fn bot_templates_crud_and_local_run_work() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let body = r#"{"choices":[{"message":{"content":"Reviewed subsystem table cleanly."}}]}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = s.write_all(resp.as_bytes());
            }
        });

        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(Ledger::open(dir.path().join("ledger")));
        let pins = Arc::new(Pins::load(&dir.path().join("pins.json")));
        let eng = Arc::new(Engine::new(pins));
        let bot = Bot::with_db_path(dir.path().join("bot.sqlite"), ledger, eng, Some(addr));

        assert_eq!(TEMPLATES.len(), 15);
        let created = bot
            .upsert_bot(
                "test/user",
                None,
                UpsertBotReq {
                    id: Some("vi-1".into()),
                    name: None,
                    template: Some("vi".into()),
                    instructions: None,
                    model: None,
                    workspace: None,
                    channels: Some(vec!["#eng".into()]),
                    schedule_cron: None,
                    schedule_interval: None,
                    enabled: Some(true),
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(created.name, "Vi");
        assert_eq!(created.bots_channel_preview(), "#eng");

        let (code, run) = bot
            .run_bot(
                "test/user",
                "vi-1",
                RunBotReq {
                    prompt: "Review subsystem table.".into(),
                    model: None,
                    r#async: false,
                    task_queue: None,
                },
                Some("mock-test-token".into()),
            )
            .await
            .unwrap();
        assert_eq!(code, 200);
        assert_eq!(run.status, "complete");
        assert!(run.output.unwrap_or_default().contains("subsystem table"));
    }

    impl BotRecord {
        fn bots_channel_preview(&self) -> &str {
            self.channels.first().map(String::as_str).unwrap_or("")
        }
    }
}
