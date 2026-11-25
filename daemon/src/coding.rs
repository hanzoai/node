// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! 1:1 Cloud-compatible `/v1/agent` & `/v1/agents` coding subsystem inside `hanzod`.
//!
//! Serves identical wire envelopes to `https://api.hanzo.ai` on loopback
//! (`http://127.0.0.1:3690`) and ZAP socket so CLI (`hanzo --cloud`), Desktop,
//! and Mobile (`lib/sessions.ts` & `lib/gen.ts`) operate seamlessly against both:
//! - `POST   /v1/agent/coding`               -> `202 Accepted` async `hanzo-gen` turn
//! - `POST   /v1/agent/coding/:session/steer` -> `202 Accepted` interrupt + resume turn
//! - `GET    /v1/agent/coding/:session/changes` -> branch, commits, and git diffstat
//! - `GET    /v1/agent/sessions` & `/v1/agents/sessions` -> Cloud session roster
//! - `GET    /v1/agent/sessions/:id` & `/v1/agents/sessions/:id` -> detail + `recentEvents`
//! - `DELETE /v1/agent/sessions/:id` & `/v1/agents/sessions/:id` -> delete session

use crate::dev::Dev;
use crate::dev_native::{
    CreateSessionReq, DevNative, RunTurnReq, SessionSummary, HARNESS_NAME, HARNESS_VERSION,
};
use crate::ledger::Ledger;
use crate::table::{Fut, Reply, Subsystem};
use crate::zap::{Request, Response};
use dev_protocol::Message;
use serde::Deserialize;
use serde_json::{json, Value};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct StartCodingReq {
    prompt: Option<String>,
    task: Option<String>,
    repo: Option<String>,
    dir: Option<String>,
    workspace: Option<String>,
    #[allow(dead_code)]
    base: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    developer_instructions: Option<String>,
    instructions: Option<String>,
    tool: Option<String>,
    after: Option<String>,
    runtime: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct SteerCodingReq {
    prompt: Option<String>,
    text: Option<String>,
    message: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    instructions: Option<String>,
    runtime: Option<String>,
}

pub struct CodingCore {
    pub dev: Arc<Dev>,
    pub ledger: Arc<Ledger>,
}

impl CodingCore {
    pub fn new(dev: Arc<Dev>, ledger: Arc<Ledger>) -> Arc<Self> {
        Arc::new(Self { dev, ledger })
    }

    fn native(&self) -> &Arc<DevNative> {
        &self.dev.native
    }
}

pub struct CodingSub {
    pub core: Arc<CodingCore>,
    pub prefix: &'static str,
    pub name: &'static str,
}

fn map_cloud_status(native: &DevNative, s: &SessionSummary) -> &'static str {
    if native.is_session_running(&s.id) {
        "running"
    } else if s.status == "suspended" || s.status == "paused" {
        "paused"
    } else if s.status == "error" {
        "error"
    } else {
        "done"
    }
}

fn normalize_tool(raw: Option<&str>) -> Result<Option<String>, String> {
    let t = raw.unwrap_or("").trim().to_ascii_lowercase();
    match t.as_str() {
        "" | "gen" => Ok(None),
        "dev" => Ok(Some("dev".into())),
        "claude" | "claude-code" | "cc" => Ok(Some("claude".into())),
        "codex" => Ok(Some("codex".into())),
        other => Err(format!(
            "unsupported coding tool '{other}' (expected gen, dev, claude, or codex)"
        )),
    }
}

fn first_line_title(prompt: &str) -> String {
    let line = prompt.lines().find(|l| !l.trim().is_empty()).unwrap_or(prompt).trim();
    let mut out: String = line.chars().take(72).collect();
    if line.chars().count() > 72 {
        out.push('…');
    }
    out
}

fn git_stdout(cwd: &Path, args: &[&str]) -> Option<String> {
    if !cwd.join(".git").exists() && !cwd.is_dir() {
        return None;
    }
    crate::proc::bg_std_cmd("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

fn spawn_background_turn(
    native: Arc<DevNative>,
    who: String,
    session_id: String,
    req: RunTurnReq,
    bearer: Option<String>,
) {
    let (guard, cancel_rx) = native.acquire_turn_slot(&session_id);
    tokio::spawn(async move {
        let _guard = guard;
        let _ = native
            .run_turn_with_cancel(&who, &session_id, req, bearer, cancel_rx)
            .await;
    });
}

impl CodingCore {
    async fn start_coding(&self, who: &str, req: Request) -> Response {
        let parsed: StartCodingReq = match serde_json::from_slice(&req.body) {
            Ok(p) => p,
            Err(e) => return Response::error(400, &format!("invalid JSON: {e}")),
        };
        let prompt = parsed
            .prompt
            .or(parsed.task)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let Some(prompt) = prompt else {
            return Response::error(400, "prompt is required");
        };
        let backend = match normalize_tool(parsed.tool.as_deref()) {
            Ok(b) => b,
            Err(msg) => return Response::error(400, &msg),
        };

        let explicit_ws = parsed
            .dir
            .or(parsed.workspace)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let instructions = parsed
            .instructions
            .or(parsed.developer_instructions)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let reused = parsed
            .after
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(|aid| self.native().get_session_summary(aid).ok());

        let (sid, ws) = match reused {
            Some(existing) => {
                let ws = explicit_ws.unwrap_or(existing.workspace);
                (existing.id, ws)
            }
            None => {
                let ws = explicit_ws.unwrap_or_else(|| {
                    std::env::current_dir()
                        .unwrap_or_else(|_| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
                        .display()
                        .to_string()
                });
                let created = match self.native().create_session(
                    who,
                    CreateSessionReq {
                        label: Some(first_line_title(&prompt)),
                        workspace: Some(ws.clone()),
                        model: parsed.model.clone(),
                        prelude: instructions.clone(),
                    },
                ) {
                    Ok(c) => c,
                    Err(e) => return Response::error(500, &e.to_string()),
                };
                (created.id, ws)
            }
        };

        let ws_path = Path::new(&ws);
        let branch = git_stdout(ws_path, &["rev-parse", "--abbrev-ref", "HEAD"]);
        let repo = parsed
            .repo
            .filter(|s| !s.trim().is_empty())
            .or_else(|| git_stdout(ws_path, &["config", "--get", "remote.origin.url"]));

        self.ledger
            .record(who, "agent.coding.start", &format!("{sid} ({ws})"));

        let bearer = req.bearer().map(str::to_owned);
        spawn_background_turn(
            self.native().clone(),
            who.to_string(),
            sid.clone(),
            RunTurnReq {
                prompt,
                model: parsed.model,
                runtime: parsed.runtime,
                backend,
                effort: parsed.effort,
                instructions: None,
                link_session_id: None,
            },
            bearer,
        );

        Response::json(
            202,
            &json!({
                "id": sid,
                "sessionId": sid,
                "status": "running",
                "tool": "gen",
                "harness": HARNESS_NAME,
                "workspace": ws,
                "repo": repo,
                "branch": branch,
            }),
        )
    }

    async fn steer_coding(&self, who: &str, session_id: &str, req: Request) -> Response {
        let parsed: SteerCodingReq = match serde_json::from_slice(&req.body) {
            Ok(p) => p,
            Err(e) => return Response::error(400, &format!("invalid JSON: {e}")),
        };
        let prompt = parsed
            .prompt
            .or(parsed.text)
            .or(parsed.message)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let Some(prompt) = prompt else {
            return Response::error(400, "prompt, text, or message is required");
        };

        if let Err(e) = self.native().set_status(who, session_id, "interrupted") {
            return if e.kind() == io::ErrorKind::NotFound {
                Response::error(404, &e.to_string())
            } else {
                Response::error(500, &e.to_string())
            };
        }

        // Wait briefly for any active turn on `session_id` to settle and persist its
        // `backend_session_id` before spawning the steered continuation turn.
        for _ in 0..150 {
            if !self.native().is_session_running(session_id) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        self.ledger.record(who, "agent.coding.steer", session_id);
        let bearer = req.bearer().map(str::to_owned);
        spawn_background_turn(
            self.native().clone(),
            who.to_string(),
            session_id.to_string(),
            RunTurnReq {
                prompt,
                model: parsed.model,
                runtime: parsed.runtime,
                backend: None,
                effort: parsed.effort,
                instructions: parsed.instructions,
                link_session_id: None,
            },
            bearer,
        );

        Response::json(
            202,
            &json!({
                "id": session_id,
                "sessionId": session_id,
                "status": "running",
                "steered": true,
                "tool": "gen",
                "harness": HARNESS_NAME,
            }),
        )
    }

    fn coding_changes(&self, session_id: &str) -> Response {
        let summary = match self.native().get_session_summary(session_id) {
            Ok(s) => s,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Response::error(404, &e.to_string()),
            Err(e) => return Response::error(500, &e.to_string()),
        };
        let ws = Path::new(&summary.workspace);
        let branch = git_stdout(ws, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
        let commits: Vec<String> = git_stdout(ws, &["log", "-n", "20", "--oneline"])
            .map(|s| s.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect())
            .unwrap_or_default();
        let diff_stat = git_stdout(ws, &["diff", "--stat"]).unwrap_or_default();
        let short_status = git_stdout(ws, &["status", "--short"]).unwrap_or_default();
        let diffstat = match (diff_stat.is_empty(), short_status.is_empty()) {
            (false, false) => format!("{diff_stat}\n{short_status}"),
            (false, true) => diff_stat,
            (true, false) => short_status,
            (true, true) => String::new(),
        };

        Response::json(
            200,
            &json!({
                "sessionId": session_id,
                "branch": branch,
                "commits": commits,
                "diffstat": diffstat,
            }),
        )
    }

    fn list_sessions(&self) -> Response {
        let rows = match self.native().list_sessions() {
            Ok(r) => r,
            Err(e) => return Response::error(500, &e.to_string()),
        };
        let sessions: Vec<Value> = rows
            .iter()
            .map(|s| {
                json!({
                    "id": s.id,
                    "sessionId": s.id,
                    "kind": "coding",
                    "title": s.label,
                    "status": map_cloud_status(self.native(), s),
                    "workspace": s.workspace,
                    "model": s.model,
                    "harness": HARNESS_NAME,
                    "createdAt": s.created_at,
                    "updatedAt": s.updated_at,
                })
            })
            .collect();
        Response::json(200, &json!({ "sessions": sessions }))
    }

    fn session_detail(&self, id: &str) -> Response {
        let summary = match self.native().get_session_summary(id) {
            Ok(s) => s,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Response::error(404, &e.to_string()),
            Err(e) => return Response::error(500, &e.to_string()),
        };
        let detail = match self.native().get_session_detail(id) {
            Ok(d) => d,
            Err(e) => return Response::error(500, &e.to_string()),
        };
        let actions = self.native().list_action_events(id).unwrap_or_default();
        let messages_val = detail.get("messages").cloned().unwrap_or_else(|| json!([]));
        let parsed_msgs: Vec<Message> = serde_json::from_value(messages_val.clone()).unwrap_or_default();

        let mut seq = 0u64;
        let mut recent_events = Vec::new();
        for m in &parsed_msgs {
            let (role, text) = match m {
                Message::User(t) => ("user", t.as_str()),
                Message::Agent { text: Some(t), .. } => ("assistant", t.as_str()),
                _ => continue,
            };
            seq += 1;
            recent_events.push(json!({ "seq": seq, "kind": "message", "payload": { "role": role, "text": text } }));
        }
        for a in &actions {
            seq += 1;
            recent_events.push(json!({
                "seq": seq,
                "kind": "tool_call",
                "at": a.completed_at,
                "payload": { "actionId": a.action_id, "name": a.kind, "summary": a.summary, "failed": a.failed },
            }));
        }

        Response::json(
            200,
            &json!({
                "id": summary.id,
                "sessionId": summary.id,
                "kind": "coding",
                "title": summary.label,
                "status": map_cloud_status(self.native(), &summary),
                "workspace": summary.workspace,
                "model": summary.model,
                "harness": HARNESS_NAME,
                "backendSessionId": summary.backend_session_id,
                "createdAt": summary.created_at,
                "updatedAt": summary.updated_at,
                "messages": messages_val,
                "recentEvents": recent_events,
                "checkpoints": detail.get("checkpoints").cloned().unwrap_or_else(|| json!([])),
            }),
        )
    }
}

impl Subsystem for CodingSub {
    fn name(&self) -> &'static str {
        self.name
    }
    fn prefix(&self) -> &'static str {
        self.prefix
    }
    fn resident(&self) -> bool {
        true
    }
    fn start(&self) -> Fut<'_, io::Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn frame<'a>(&'a self, who: &'a str, req: Request) -> Fut<'a, io::Result<Reply>> {
        Box::pin(async move {
            let path = req.path().to_string();
            let sub = path.strip_prefix(self.prefix).unwrap_or("");
            let method = req.method.as_str();

            let resp = match (method, sub) {
                ("GET", "" | "/health") => Response::json(
                    200,
                    &json!({
                        "ok": true,
                        "subsystem": self.name,
                        "harness": HARNESS_NAME,
                        "harness_version": HARNESS_VERSION,
                        "active_turns": self.core.native().active_turns(),
                    }),
                ),
                ("POST", "/coding") => self.core.start_coding(who, req).await,
                ("GET", "/sessions") => self.core.list_sessions(),
                _ => {
                    let parts: Vec<&str> = sub.split('/').filter(|s| !s.is_empty()).collect();
                    match (method, parts.as_slice()) {
                        ("POST", ["coding", sid, "steer"]) => {
                            self.core.steer_coding(who, sid, req).await
                        }
                        ("GET", ["coding", sid, "changes"]) => self.core.coding_changes(sid),
                        ("GET", ["sessions", sid]) => self.core.session_detail(sid),
                        ("DELETE", ["sessions", sid]) => match self.core.native().delete_session(who, sid) {
                            Ok(true) => Response::json(200, &json!({ "deleted": true, "id": sid })),
                            Ok(false) => Response::error(404, &format!("session {sid} not found")),
                            Err(e) => Response::error(500, &e.to_string()),
                        },
                        _ => Response::error(404, "unknown /v1/agent coding or sessions route"),
                    }
                }
            };
            Ok(Reply::Whole(resp))
        })
    }
    fn stop(&self) -> Fut<'_, ()> {
        Box::pin(async {})
    }
}
