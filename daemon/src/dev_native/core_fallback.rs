// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! Zero-dependency in-process `dev_core::Session` fallback driver when external
//! coding agent binaries (`dev`, `claude`, `codex`) are absent or `wasm-core` is requested.

use super::{DevNative, RunTurnReq, TurnActionRecord, TurnResult, DEFAULT_CLOUD_API};
use crate::catalog;
use crate::me;
use crate::paths;
use dev_protocol::{
    Answer, Ask, Browse, Call, Cancel, Event, Exec, Git, Message, Op, Outcome, Output, Patch,
    Read, Reply, Turn, Write,
};
use serde_json::{json, Value};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::watch;

pub(super) const MAX_TOOL_ROUNDS: usize = 64;
const MAX_OUTPUT_BYTES: usize = 256 * 1024;
pub(super) const EXEC_TIMEOUT: Duration = Duration::from_secs(600);
const GIT_TIMEOUT: Duration = Duration::from_secs(30);

fn act_rec(id: u64, kind: &str, summary: impl Into<String>, failed: bool, replayed: bool) -> TurnActionRecord {
    TurnActionRecord { id, kind: kind.into(), summary: summary.into(), failed, replayed }
}

impl DevNative {
    pub(super) async fn run_turn_core_fallback(
        &self,
        who: &str,
        session_id: &str,
        req: RunTurnReq,
        request_bearer: Option<String>,
        cancel_rx: watch::Receiver<bool>,
    ) -> io::Result<TurnResult> {
        let mut loaded = self.load_session_full(session_id)?;
        let turn_id = loaded.summary.next_turn_id;
        let next_turn_id = turn_id + 1;
        let workspace = PathBuf::from(&loaded.summary.workspace);

        let mut pending = loaded.session.step(Event::Turn(Turn {
            id: turn_id,
            prompt: req.prompt,
        }));
        self.save_snapshot(session_id, next_turn_id, &loaded.session)?;

        let mut emitted = Vec::new();
        let mut actions_log = Vec::new();
        let mut final_outcome = Outcome::Complete;
        let mut rounds = 0usize;

        while !pending.is_empty() && rounds < MAX_TOOL_ROUNDS {
            if *cancel_rx.borrow() {
                let _ = loaded.session.step(Event::Cancel(Cancel { turn: turn_id }));
                final_outcome = Outcome::Cancelled;
                break;
            }
            rounds += 1;
            let batch = std::mem::take(&mut pending);
            for action in batch {
                match action.op {
                    Op::Model(mut ask) => {
                        if let Some(ref ov) = req.model {
                            ask.model = Some(ov.clone());
                        }
                        let reply = self.call_model(&ask, request_bearer.as_deref()).await?;
                        actions_log.push(act_rec(
                            action.id,
                            "model",
                            ask.model.unwrap_or_else(|| "auto".into()),
                            false,
                            false,
                        ));
                        pending.extend(loaded.session.step(Event::Model(Answer { id: action.id, reply })));
                    }
                    Op::Read(r) => {
                        let (out, rep) = self
                            .exec_dedup(who, session_id, action.id, "read", &r.path, || async {
                                exec_read(&workspace, &r)
                            })
                            .await?;
                        actions_log.push(act_rec(action.id, "read", r.path, out.failed, rep));
                        pending.extend(loaded.session.step(Event::File(out)));
                    }
                    Op::Write(w) => {
                        let (out, rep) = self
                            .exec_dedup(who, session_id, action.id, "write", &w.path, || async {
                                exec_write(&workspace, &w)
                            })
                            .await?;
                        actions_log.push(act_rec(action.id, "write", w.path, out.failed, rep));
                        pending.extend(loaded.session.step(Event::File(out)));
                    }
                    Op::Patch(p) => {
                        let (out, rep) = self
                            .exec_dedup(who, session_id, action.id, "patch", &p.path, || async {
                                exec_patch(&workspace, &p)
                            })
                            .await?;
                        actions_log.push(act_rec(action.id, "patch", p.path, out.failed, rep));
                        pending.extend(loaded.session.step(Event::File(out)));
                    }
                    Op::Exec(e) => {
                        let desc = e.argv.join(" ");
                        let (out, rep) = self
                            .exec_dedup(who, session_id, action.id, "exec", &desc, || async {
                                exec_cmd(&workspace, &e).await
                            })
                            .await?;
                        actions_log.push(act_rec(action.id, "exec", desc, out.failed, rep));
                        pending.extend(loaded.session.step(Event::Exec(out)));
                    }
                    Op::Git(g) => {
                        let desc = format!("git {}", g.argv.join(" "));
                        let (out, rep) = self
                            .exec_dedup(who, session_id, action.id, "git", &desc, || async {
                                exec_git(&workspace, &g).await
                            })
                            .await?;
                        actions_log.push(act_rec(action.id, "git", desc, out.failed, rep));
                        pending.extend(loaded.session.step(Event::Git(out)));
                    }
                    Op::Browse(b) => {
                        let url = b.url.clone();
                        let (out, rep) = self
                            .exec_dedup(who, session_id, action.id, "browse", &url, || async {
                                tokio::task::spawn_blocking(move || exec_browse(&b))
                                    .await
                                    .unwrap_or_else(|e| (e.to_string(), true))
                            })
                            .await?;
                        actions_log.push(act_rec(action.id, "browse", url, out.failed, rep));
                        pending.extend(loaded.session.step(Event::Browse(out)));
                    }
                    Op::Emit(text) => emitted.push(text),
                    Op::Save => {
                        self.save_snapshot(session_id, next_turn_id, &loaded.session)?;
                        self.record_checkpoint(session_id, action.id, &workspace);
                    }
                    Op::Done(done) => {
                        final_outcome = done.outcome;
                        self.save_snapshot(session_id, next_turn_id, &loaded.session)?;
                    }
                }
            }
        }

        self.save_snapshot(session_id, next_turn_id, &loaded.session)?;
        let last_text = emitted.last().cloned().or_else(|| {
            loaded.session.messages().iter().rev().find_map(|m| match m {
                Message::Agent { text: Some(t), .. } => Some(t.clone()),
                _ => None,
            })
        });

        Ok(TurnResult {
            session_id: session_id.to_string(),
            turn_id,
            outcome: match final_outcome {
                Outcome::Complete => "complete".into(),
                Outcome::Cancelled => "interrupted".into(),
            },
            text: last_text,
            emitted,
            actions: actions_log,
            messages: loaded.session.messages().to_vec(),
        })
    }

    pub(super) async fn exec_dedup<F, Fut>(
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
        if let Some((text, failed)) = self.ledger_lookup(session_id, action_id, kind)? {
            return Ok((
                Output {
                    id: action_id,
                    text,
                    failed,
                },
                true,
            ));
        }
        self.ledger.record(
            who,
            &format!("dev.{kind}"),
            &format!("{session_id}#{action_id}: {summary}"),
        );
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

    async fn call_model(&self, ask: &Ask, request_bearer: Option<&str>) -> io::Result<Reply> {
        let chosen_model = ask.model.clone().unwrap_or_else(|| "enso-auto".into());
        let is_local_model =
            chosen_model.starts_with("local:") || catalog::find(&chosen_model).is_some();

        let (endpoint, model_id, bearer) = if is_local_model {
            let clean = chosen_model
                .strip_prefix("local:")
                .unwrap_or(&chosen_model)
                .to_string();
            match self.engine.as_ref().and_then(|e| e.port()) {
                Some(p) => (format!("http://127.0.0.1:{p}/v1/chat/completions"), clean, None),
                None => (
                    format!("http://{}/v1/engine/chat/completions", paths::HTTP_ADDR),
                    clean,
                    None,
                ),
            }
        } else {
            let base = std::env::var("HANZO_API_URL")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_CLOUD_API.into());
            let token = request_bearer.map(str::to_owned).or_else(me::fresh_token);
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
                                    "function": { "name": name, "arguments": args.to_string() }
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
                Message::Tool { id, text, .. } => oai_messages.push(json!({
                    "role": "tool",
                    "tool_call_id": format!("call_{id}"),
                    "content": text,
                })),
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
            let resp = req
                .send_json(payload)
                .map_err(|e| io::Error::other(format!("model request to {endpoint} failed: {e}")))?;
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
                let args_str = fn_obj.get("arguments").and_then(|v| v.as_str()).unwrap_or("{}");
                let args: Value = serde_json::from_str(args_str).unwrap_or(Value::Null);
                if let Some(call) = oai_to_call(name, &args) {
                    calls.push(call);
                }
            }
        }

        Ok(Reply { text, calls })
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

pub(super) fn resolve_inside(workspace: &Path, rel: &str) -> Result<PathBuf, String> {
    if !dev_protocol::inside(rel) {
        return Err(format!("refused: {rel} is outside the workspace"));
    }
    let root = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let joined = root.join(rel);
    if joined.exists() {
        let canon = joined.canonicalize().map_err(|e| format!("resolve {rel}: {e}"))?;
        if !canon.starts_with(&root) {
            return Err(format!("refused: {rel} escapes workspace via symlink"));
        }
        Ok(canon)
    } else {
        Ok(joined)
    }
}

pub(super) fn exec_read(workspace: &Path, r: &Read) -> (String, bool) {
    match resolve_inside(workspace, &r.path)
        .and_then(|p| std::fs::read_to_string(&p).map_err(|e| format!("read {}: {e}", r.path)))
    {
        Ok(text) => (text, false),
        Err(e) => (e, true),
    }
}

pub(super) fn exec_write(workspace: &Path, w: &Write) -> (String, bool) {
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
    if p.diff.contains("@@") {
        let mut child = match crate::proc::bg_std_cmd("git")
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
                format!("git apply failed: {}", String::from_utf8_lossy(&out.stderr).trim()),
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
    let fut = crate::proc::bg_tokio_cmd(prog).args(args).current_dir(&cwd).output();
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
    let fut = crate::proc::bg_tokio_cmd("git").args(&g.argv).current_dir(workspace).output();
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
        Some(body) => ureq::post(&b.url).timeout(Duration::from_secs(20)).send_string(body),
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

pub(super) fn tool_schemas() -> Value {
    let tool = |name: &str, desc: &str, props: Value, req: &[&str]| {
        json!({
            "type": "function",
            "function": {
                "name": name,
                "description": desc,
                "parameters": { "type": "object", "properties": props, "required": req }
            }
        })
    };
    json!([
        tool(
            "read_file",
            "Read a workspace-relative file.",
            json!({ "path": { "type": "string" } }),
            &["path"],
        ),
        tool(
            "write_file",
            "Write full contents to a workspace-relative file.",
            json!({ "path": { "type": "string" }, "text": { "type": "string" } }),
            &["path", "text"],
        ),
        tool(
            "patch_file",
            "Apply a unified diff to a workspace-relative file.",
            json!({ "path": { "type": "string" }, "diff": { "type": "string" } }),
            &["path", "diff"],
        ),
        tool(
            "exec",
            "Run a command inside the workspace.",
            json!({
                "argv": { "type": "array", "items": { "type": "string" } },
                "cwd": { "type": "string" }
            }),
            &["argv"],
        ),
        tool(
            "git",
            "Run a git command inside the workspace.",
            json!({ "argv": { "type": "array", "items": { "type": "string" } } }),
            &["argv"],
        ),
        tool(
            "browse",
            "Fetch an http/https URL (GET, or POST when body is supplied).",
            json!({ "url": { "type": "string" }, "body": { "type": "string" } }),
            &["url"],
        ),
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

pub(super) fn oai_to_call(name: &str, args: &Value) -> Option<Call> {
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
