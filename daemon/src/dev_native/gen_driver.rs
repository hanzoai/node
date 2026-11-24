// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! Primary in-process `hanzo-gen` driver (`hanzo_gen::embedded::run_embedded_turn`)
//! for `DevNative`, unifying Desktop, CLI, Bot, Team, and `/v1/agent/coding` on
//! the canonical `hanzo-gen` harness.

use super::{DevNative, RunTurnReq, TurnActionRecord, TurnResult, DEFAULT_CLOUD_API};
use crate::catalog;
use crate::me;
use crate::paths;
use dev_protocol::{Answer, Cancel, Event, Op, Reply, Turn};
use hanzo_gen::backend::BackendKind;
use hanzo_gen::embedded::{run_embedded_turn, EmbeddedTurnRequest, StaticHost, TokenFn};
use hanzo_gen::{Host, Root};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::watch;

fn split_principal(who: &str) -> (String, String) {
    if let Some((owner, name)) = who.split_once('/') {
        let o = owner.trim();
        let n = name.trim();
        if !o.is_empty() && !n.is_empty() {
            return (o.to_string(), n.to_string());
        }
    }
    ("hanzo".to_string(), "agent".to_string())
}

fn parse_backend_override(raw: Option<&str>) -> Option<BackendKind> {
    let s = raw?.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("gen") {
        return None;
    }
    BackendKind::parse(s).ok()
}

fn combine_instructions(prelude: Option<&str>, extra: Option<&str>) -> Option<String> {
    let p = prelude.map(str::trim).filter(|s| !s.is_empty());
    let e = extra.map(str::trim).filter(|s| !s.is_empty());
    match (p, e) {
        (Some(a), Some(b)) => Some(format!("{a}\n\n{b}")),
        (Some(a), None) => Some(a.to_string()),
        (None, Some(b)) => Some(b.to_string()),
        (None, None) => None,
    }
}

impl DevNative {
    pub(super) fn build_gen_host(
        &self,
        who: &str,
        root: &Root,
        chosen_model: Option<&str>,
        request_bearer: Option<String>,
    ) -> (Arc<dyn Host>, Option<String>) {
        let model_trimmed = chosen_model.map(str::trim).filter(|s| !s.is_empty());
        if let Some(raw) = model_trimmed.filter(|m| m.starts_with("http://") || m.starts_with("https://")) {
            let (url_part, frag_model) = match raw.split_once('#') {
                Some((u, m)) => (u.trim().trim_end_matches('/'), Some(m.trim()).filter(|s| !s.is_empty())),
                None => (raw.trim_end_matches('/'), None),
            };
            let host = StaticHost::local(url_part)
                .with_hanzo_key(std::env::var("HANZO_API_KEY").ok());
            return (Arc::new(host), frag_model.map(String::from));
        }

        let is_local = model_trimmed.is_some_and(|m| {
            m.starts_with("local:") || catalog::find(m).is_some()
        });

        if is_local {
            let clean_model = model_trimmed.map(|m| {
                m.strip_prefix("local:").unwrap_or(m).to_string()
            });
            let engine_base_url = std::env::var("HANZOD_LOCAL_ENGINE_URL")
                .ok()
                .map(|s| s.trim().trim_end_matches('/').to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| match self.engine.as_ref().and_then(|e| e.port()) {
                    Some(port) => format!("http://127.0.0.1:{port}"),
                    None => format!("http://{}/v1/engine", paths::HTTP_ADDR),
                });
            return (Arc::new(StaticHost::local(engine_base_url)), clean_model);
        }

        let cloud_api = std::env::var("HANZO_API_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_CLOUD_API.to_string());
        let (owner, name) = me::effective_identity()
            .map(|id| (id.owner, id.name))
            .unwrap_or_else(|| split_principal(who));

        let req_tok = request_bearer
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && !me::is_worker_only_jwt(s));
        let token_fn: TokenFn =
            Arc::new(move || req_tok.clone().or_else(me::fresh_inference_token));

        let refresh_cmd = me::refresh_command(hanzo_gen::exe::find_for_root(root, "hanzo"));

        let host = StaticHost::cloud(cloud_api, owner.clone(), name, token_fn)
            .with_org(Some(owner))
            .with_refresh_cmd(refresh_cmd)
            .with_hanzo_key(std::env::var("HANZO_API_KEY").ok());

        (Arc::new(host), model_trimmed.map(String::from))
    }

    pub(super) async fn run_turn_gen(
        &self,
        who: &str,
        session_id: &str,
        req: RunTurnReq,
        request_bearer: Option<String>,
        root: Root,
        cancel_rx: watch::Receiver<bool>,
    ) -> io::Result<TurnResult> {
        let mut loaded = self.load_session_full(session_id)?;
        let turn_id = loaded.summary.next_turn_id;
        let next_turn_id = turn_id + 1;
        let workspace = PathBuf::from(&loaded.summary.workspace);
        if !workspace.exists() {
            let _ = std::fs::create_dir_all(&workspace);
        }

        let effective_model = req.model.clone().or_else(|| loaded.summary.model.clone());
        let (host, clean_model) =
            self.build_gen_host(who, &root, effective_model.as_deref(), request_bearer);

        // Advance `dev_core::Session` with the user prompt so mid-turn session
        // reads immediately reflect the user's message.
        let initial_actions = loaded.session.step(Event::Turn(Turn {
            id: turn_id,
            prompt: req.prompt.clone(),
        }));
        let model_ask_id = initial_actions
            .iter()
            .find_map(|a| matches!(a.op, Op::Model(_)).then_some(a.id));
        self.save_snapshot_and_backend(
            session_id,
            next_turn_id,
            &loaded.session,
            loaded.backend_session_id.as_deref(),
        )?;

        // Prepend session prelude on fresh turns (and per-turn instructions whenever supplied).
        let session_prelude = if loaded.backend_session_id.is_none() {
            combine_instructions(loaded.prelude.as_deref(), req.instructions.as_deref())
        } else {
            combine_instructions(None, req.instructions.as_deref())
        };

        let mut gen_req = EmbeddedTurnRequest::new(root, workspace.clone(), req.prompt.clone());
        gen_req.backend = parse_backend_override(req.backend.as_deref());
        gen_req.model = clean_model;
        gen_req.effort = req.effort.clone();
        gen_req.instructions = session_prelude;
        gen_req.resume_backend_session = loaded.backend_session_id.clone();
        gen_req.link_session_id = req.link_session_id.clone();
        gen_req.mcp = true;
        gen_req.cancel = Some(cancel_rx);

        let res = run_embedded_turn(host, gen_req)
            .await
            .map_err(|e| io::Error::other(format!("hanzo-gen turn failed: {e:#}")))?;

        let merged_backend_sid = res
            .backend_session_id
            .clone()
            .or(loaded.backend_session_id.clone());
        let reply_text = res.text.clone().or_else(|| res.error.clone());

        if res.status == "interrupted" && reply_text.is_none() {
            let _ = loaded.session.step(Event::Cancel(Cancel { turn: turn_id }));
        } else if let Some(ask_id) = model_ask_id {
            let _ = loaded.session.step(Event::Model(Answer {
                id: ask_id,
                reply: Reply {
                    text: reply_text.clone(),
                    calls: Vec::new(),
                },
            }));
        }

        self.save_snapshot_and_backend(
            session_id,
            next_turn_id,
            &loaded.session,
            merged_backend_sid.as_deref(),
        )?;

        let mut next_aid = self.next_ledger_action_id(session_id, loaded.session.next_id())?;
        let mut actions_log = Vec::with_capacity(res.actions.len());
        for act in &res.actions {
            let aid = next_aid;
            next_aid += 1;
            let kind = if act.name.is_empty() {
                act.kind.clone()
            } else {
                format!("{}:{}", act.kind, act.name)
            };
            let summary = if act.summary.is_empty() {
                act.name.clone()
            } else {
                act.summary.clone()
            };
            self.ledger.record(
                who,
                &format!("gen.{}", act.kind),
                &format!("{session_id}#{aid}: {} {summary}", act.name),
            );
            self.ledger_record(session_id, aid, &kind, &summary, &summary, act.failed)?;
            actions_log.push(TurnActionRecord {
                id: aid,
                kind,
                summary,
                failed: act.failed,
                replayed: false,
            });
        }

        self.record_checkpoint(session_id, turn_id, &workspace);

        let outcome = if res.ok {
            "complete".to_string()
        } else if res.status == "interrupted" {
            "interrupted".to_string()
        } else {
            "error".to_string()
        };

        Ok(TurnResult {
            session_id: session_id.to_string(),
            turn_id,
            outcome,
            text: reply_text,
            emitted: res.messages,
            actions: actions_log,
            messages: loaded.session.messages().to_vec(),
        })
    }
}
