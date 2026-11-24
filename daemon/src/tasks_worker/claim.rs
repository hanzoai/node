// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! Fleet heartbeat, activity claim, busy deferral, and execution dispatch for `TasksWorker`.

use super::{
    rfc3339_now, sanitize_hostname, TasksWorker, WorkerConfig, CLAIM_LEASE_SECS, ENGINE_API,
    FLEET_NS, JOBS_NS, JOB_HEARTBEAT_SECS,
};
use crate::dev_native::{CreateSessionReq, RunTurnReq};
use crate::research::ExecuteResearchReq;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Deserialize)]
struct ClaimedActivity {
    execution: ExecutionRef,
    r#type: TypeRef,
    #[serde(default)]
    input: Value,
    #[serde(rename = "taskQueue", default)]
    #[allow(dead_code)]
    task_queue: String,
}

#[derive(Debug, Deserialize)]
struct ExecutionRef {
    #[serde(rename = "workflowId")]
    workflow_id: String,
    #[serde(rename = "runId")]
    run_id: String,
}

#[derive(Debug, Deserialize)]
struct TypeRef {
    name: String,
}

impl TasksWorker {
    pub async fn register_and_heartbeat(self: &Arc<Self>, token: &str) -> Result<(), String> {
        let cfg = self.cfg.lock().unwrap().clone();
        let (busy_reason, gpu_util) = self.busy_status().await;
        let is_busy = busy_reason.is_some();
        {
            let mut st = self.state.lock().unwrap();
            st.busy = is_busy;
            st.busy_reason = busy_reason.clone();
            st.gpu_util_pct = gpu_util;
        }

        let reg = self.registration_payload(&cfg.worker_id);
        let active_dev = self.dev.active_sessions_count();
        let active_res = self.research.active_runs();
        let gpus_count = self.hw.gpus.len();
        let api = cfg.cloud_api.clone();
        let wid = cfg.worker_id.clone();
        let tok = token.to_string();

        let res = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let auth = format!("Bearer {tok}");
            for ns in [FLEET_NS, JOBS_NS] {
                let _ = ureq::post(&format!("{api}{ENGINE_API}/namespaces"))
                    .timeout(Duration::from_secs(8))
                    .set("Authorization", &auth)
                    .set("Content-Type", "application/json")
                    .send_string(&json!({ "namespaceInfo": { "name": ns } }).to_string());
            }

            let start_body = json!({
                "activityId": wid,
                "runId": wid,
                "activityType": { "name": "fleet.worker" },
                "taskQueue": FLEET_NS,
                "heartbeatTimeout": "120s",
                "input": reg,
            });
            http_json_post(
                &api,
                &auth,
                &format!("{ENGINE_API}/namespaces/{FLEET_NS}/activities"),
                &start_body,
            )?;

            let ts = rfc3339_now();
            let hb_body = json!({
                "details": {
                    "gpus": gpus_count,
                    "ts": ts,
                    "busy": is_busy,
                    "busy_reason": busy_reason,
                    "active_dev_sessions": active_dev,
                    "active_research_runs": active_res,
                    "gpu_util": gpu_util,
                }
            });
            http_json_post(
                &api,
                &auth,
                &format!("{ENGINE_API}/namespaces/{FLEET_NS}/activities/{wid}/{wid}/heartbeat"),
                &hb_body,
            )?;
            Ok(ts)
        })
        .await
        .map_err(|e| e.to_string())?;

        match res {
            Ok(ts) => {
                self.metrics.heartbeats.fetch_add(1, Ordering::Relaxed);
                let mut st = self.state.lock().unwrap();
                st.linked = true;
                st.last_heartbeat = Some(ts);
                st.last_error = None;
                Ok(())
            }
            Err(e) => {
                self.state.lock().unwrap().last_error = Some(e.clone());
                Err(e)
            }
        }
    }

    pub async fn heartbeat_only(self: &Arc<Self>, token: &str) -> Result<(), String> {
        let cfg = self.cfg.lock().unwrap().clone();
        let (busy_reason, gpu_util) = self.busy_status().await;
        let is_busy = busy_reason.is_some();
        {
            let mut st = self.state.lock().unwrap();
            st.busy = is_busy;
            st.busy_reason = busy_reason.clone();
            st.gpu_util_pct = gpu_util;
        }
        let api = cfg.cloud_api.clone();
        let wid = cfg.worker_id.clone();
        let tok = token.to_string();
        let gpus_count = self.hw.gpus.len();
        let active_dev = self.dev.active_sessions_count();
        let active_res = self.research.active_runs();

        let res = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let ts = rfc3339_now();
            let hb_body = json!({
                "details": {
                    "gpus": gpus_count,
                    "ts": ts,
                    "busy": is_busy,
                    "busy_reason": busy_reason,
                    "active_dev_sessions": active_dev,
                    "active_research_runs": active_res,
                    "gpu_util": gpu_util,
                }
            });
            http_json_post(
                &api,
                &format!("Bearer {tok}"),
                &format!("{ENGINE_API}/namespaces/{FLEET_NS}/activities/{wid}/{wid}/heartbeat"),
                &hb_body,
            )?;
            Ok(ts)
        })
        .await
        .map_err(|e| e.to_string())?;

        match res {
            Ok(ts) => {
                self.metrics.heartbeats.fetch_add(1, Ordering::Relaxed);
                let mut st = self.state.lock().unwrap();
                st.linked = true;
                st.last_heartbeat = Some(ts);
                Ok(())
            }
            Err(e) => self.register_and_heartbeat(token).await.or(Err(e)),
        }
    }

    /// Pre-v1.54 schedule workflow bridge: converts `RUNNING` schedule workflows in
    /// `gpu-jobs` into idempotent pull-claimed standalone activities and closes the
    /// parent workflow once the activity finishes. Zero-op on v1.54+ `ActivityType` schedules.
    async fn bridge_running_schedule_workflows(&self, token: &str) {
        let cfg = self.cfg.lock().unwrap().clone();
        let queues = self.claim_queues();
        let api = cfg.cloud_api.clone();
        let tok = token.to_string();

        let _ = tokio::task::spawn_blocking(move || {
            let auth = format!("Bearer {tok}");
            let list_url = format!(
                "{api}{ENGINE_API}/namespaces/{JOBS_NS}/workflows?query=ExecutionStatus%3D%27Running%27"
            );
            let Ok(resp) = ureq::get(&list_url)
                .timeout(Duration::from_secs(8))
                .set("Authorization", &auth)
                .call()
            else {
                return;
            };
            let Ok(body) = serde_json::from_reader::<_, Value>(resp.into_reader()) else {
                return;
            };
            let Some(execs) = body.get("executions").and_then(Value::as_array) else {
                return;
            };
            for wf in execs {
                let tq = wf.get("taskQueue").and_then(Value::as_str).unwrap_or("");
                if !queues.iter().any(|q| q == tq) {
                    continue;
                }
                let wf_type = wf
                    .get("type")
                    .and_then(|t| t.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let mapped_act = match wf_type {
                    "research.bench" | "research.tick" | "hanzod.run" | "gen.run"
                    | "coding.run" | "bot.run" | "team.task" | "fn.run" => wf_type,
                    "ResearchCampaignTickWorkflow" => "research.tick",
                    "FleetDispatchWorkflow" => "hanzod.run",
                    _ => continue,
                };
                let wf_id = wf
                    .get("execution")
                    .and_then(|e| e.get("workflowId"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let run_id = wf
                    .get("execution")
                    .and_then(|e| e.get("runId"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if wf_id.is_empty() || run_id.is_empty() {
                    continue;
                }
                let hist_url = format!(
                    "{api}{ENGINE_API}/namespaces/{JOBS_NS}/workflows/{wf_id}/history?runId={run_id}&pageSize=5"
                );
                let mut wf_input = wf.get("input").cloned().unwrap_or_else(|| json!({}));
                if let Ok(hresp) = ureq::get(&hist_url)
                    .timeout(Duration::from_secs(6))
                    .set("Authorization", &auth)
                    .call()
                {
                    if let Ok(hval) = serde_json::from_reader::<_, Value>(hresp.into_reader()) {
                        if let Some(ev0) = hval
                            .get("events")
                            .and_then(Value::as_array)
                            .and_then(|a| a.first())
                        {
                            if let Some(inp) = ev0.get("attributes").and_then(|at| at.get("input"))
                            {
                                if !inp.is_null() {
                                    wf_input = inp.clone();
                                }
                            }
                        }
                    }
                }
                if !wf_input.is_object() {
                    wf_input = json!({ "payload": wf_input });
                }
                wf_input["_bridge_wf_id"] = json!(wf_id);
                wf_input["_bridge_run_id"] = json!(run_id);

                let act_body = json!({
                    "activityId": wf_id,
                    "activityType": { "name": mapped_act },
                    "taskQueue": tq,
                    "heartbeatTimeout": "60s",
                    "input": wf_input,
                    "requestId": run_id,
                });
                let _ = http_json_post(
                    &api,
                    &auth,
                    &format!("{ENGINE_API}/namespaces/{JOBS_NS}/activities"),
                    &act_body,
                );
            }
        })
        .await;
    }

    fn post_claim_mismatch(&self, input: &Value, worker_id: &str) -> Option<(String, u64)> {
        let Some(obj) = input.as_object() else {
            return None;
        };
        if let Some(target) = obj.get("target_machine").and_then(Value::as_str) {
            let t = sanitize_hostname(target);
            if !t.is_empty() && t != worker_id && t != self.hw.hostname {
                return Some((format!("target_machine_mismatch:{t}"), 0));
            }
        }
        if let Some(req_backend) = obj.get("required_backend").and_then(Value::as_str) {
            let b = req_backend.trim().to_ascii_uppercase();
            if !b.is_empty() && !self.hw.backends.iter().any(|have| have == &b) {
                return Some((format!("backend_mismatch:{b}"), 5));
            }
        }
        if let Some(min_gb) = obj.get("min_memory_gb").and_then(Value::as_u64) {
            if self.hw.memory_bytes > 0 && self.hw.memory_bytes < (min_gb << 30) {
                return Some((format!("insufficient_memory:{min_gb}GB"), 10));
            }
        }
        None
    }

    async fn release_or_let_expire(
        &self,
        api: &str,
        token: &str,
        worker_id: &str,
        wf_id: &str,
        run_id: &str,
        reason: &str,
        delay_secs: u64,
    ) {
        self.metrics.deferred_busy.fetch_add(1, Ordering::Relaxed);
        let api = api.to_string();
        let auth = format!("Bearer {token}");
        let wid = worker_id.to_string();
        let wf = wf_id.to_string();
        let run = run_id.to_string();
        let why = reason.to_string();
        let _ = tokio::task::spawn_blocking(move || {
            let url =
                format!("{api}{ENGINE_API}/namespaces/{JOBS_NS}/activities/{wf}/{run}/release");
            let body = json!({
                "reason": why,
                "identity": wid,
                "delaySeconds": delay_secs,
            });
            let _ = ureq::post(&url)
                .timeout(Duration::from_secs(6))
                .set("Authorization", &auth)
                .set("Content-Type", "application/json")
                .send_string(&body.to_string());
        })
        .await;
    }

    /// Runs one reactive claim-and-execute pass: skips claiming when busy, otherwise
    /// drains ready work across `gpu:<worker_id>`, `hanzod:<hostname>`, and `gpu-jobs`.
    pub async fn poll_and_execute_once(
        self: &Arc<Self>,
        token: &str,
        long_poll_wait_secs: u64,
    ) -> Result<bool, String> {
        self.metrics.polls.fetch_add(1, Ordering::Relaxed);
        let cfg = self.cfg.lock().unwrap().clone();
        if !cfg.enabled {
            return Ok(false);
        }

        let (busy_reason, gpu_util) = self.busy_status().await;
        {
            let mut st = self.state.lock().unwrap();
            st.busy = busy_reason.is_some();
            st.busy_reason = busy_reason.clone();
            st.gpu_util_pct = gpu_util;
        }
        if busy_reason.is_some() {
            self.metrics.busy_skips.fetch_add(1, Ordering::Relaxed);
            return Ok(false);
        }

        self.bridge_running_schedule_workflows(token).await;

        let queues = self.claim_queues();
        let mut claimed: Option<ClaimedActivity> = None;
        for (idx, q) in queues.iter().enumerate() {
            let wait = if idx + 1 == queues.len() {
                long_poll_wait_secs
            } else {
                0
            };
            let api = cfg.cloud_api.clone();
            let tok = token.to_string();
            let wid = cfg.worker_id.clone();
            let q_name = q.clone();
            let got = tokio::task::spawn_blocking(move || {
                claim_one(&api, &tok, &q_name, &wid, CLAIM_LEASE_SECS, wait)
            })
            .await
            .map_err(|e| e.to_string())??;
            if let Some(act) = got {
                claimed = Some(act);
                break;
            }
        }

        let Some(act) = claimed else {
            return Ok(false);
        };
        self.metrics.jobs_claimed.fetch_add(1, Ordering::Relaxed);
        let wf_id = act.execution.workflow_id.clone();
        let run_id = act.execution.run_id.clone();

        if let Some((reason, delay)) = self.post_claim_mismatch(&act.input, &cfg.worker_id) {
            self.release_or_let_expire(
                &cfg.cloud_api,
                token,
                &cfg.worker_id,
                &wf_id,
                &run_id,
                &reason,
                delay,
            )
            .await;
            return Ok(false);
        }

        let allow_busy = act
            .input
            .get("allow_when_busy")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !allow_busy {
            if let (Some(reason), _) = self.busy_status().await {
                self.release_or_let_expire(
                    &cfg.cloud_api,
                    token,
                    &cfg.worker_id,
                    &wf_id,
                    &run_id,
                    &reason,
                    10,
                )
                .await;
                return Ok(false);
            }
        }

        let hb_stop = Arc::new(AtomicBool::new(false));
        let hb_handle = {
            let me = Arc::clone(self);
            let stop = Arc::clone(&hb_stop);
            let tok = token.to_string();
            let api = cfg.cloud_api.clone();
            let wid = cfg.worker_id.clone();
            let wf = wf_id.clone();
            let run = run_id.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs(JOB_HEARTBEAT_SECS));
                tick.tick().await;
                while !stop.load(Ordering::Relaxed) {
                    tick.tick().await;
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let api_c = api.clone();
                    let auth = format!("Bearer {tok}");
                    let wid_c = wid.clone();
                    let wf_c = wf.clone();
                    let run_c = run.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        let _ = http_json_post(
                            &api_c,
                            &auth,
                            &format!(
                                "{ENGINE_API}/namespaces/{JOBS_NS}/activities/{wf_c}/{run_c}/heartbeat"
                            ),
                            &json!({ "details": { "worker": wid_c, "ts": rfc3339_now() } }),
                        );
                    })
                    .await;
                    let _ = me.heartbeat_only(&tok).await;
                }
            })
        };

        let outcome = self
            .execute_claimed(&act.r#type.name, &wf_id, &act.input, &cfg, token)
            .await;
        hb_stop.store(true, Ordering::Relaxed);
        hb_handle.abort();

        let bridge_wf = act
            .input
            .get("_bridge_wf_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let bridge_run = act
            .input
            .get("_bridge_run_id")
            .and_then(Value::as_str)
            .map(str::to_owned);

        let api = cfg.cloud_api.clone();
        let wid = cfg.worker_id.clone();
        let auth = format!("Bearer {token}");
        match outcome {
            Ok(result_val) => {
                self.metrics.jobs_completed.fetch_add(1, Ordering::Relaxed);
                let _ = tokio::task::spawn_blocking(move || {
                    let _ = http_json_post(
                        &api,
                        &auth,
                        &format!(
                            "{ENGINE_API}/namespaces/{JOBS_NS}/activities/{wf_id}/{run_id}/complete"
                        ),
                        &json!({ "result": result_val, "identity": wid }),
                    );
                    if let (Some(bwf), Some(brun)) = (bridge_wf, bridge_run) {
                        let _ = http_json_post(
                            &api,
                            &auth,
                            &format!(
                                "{ENGINE_API}/namespaces/{JOBS_NS}/workflows/{bwf}/terminate?runId={brun}"
                            ),
                            &json!({ "reason": "completed via linked hanzod activity", "identity": wid }),
                        );
                    }
                })
                .await;
            }
            Err(cause) => {
                self.metrics.jobs_failed.fetch_add(1, Ordering::Relaxed);
                let _ = tokio::task::spawn_blocking(move || {
                    let _ = http_json_post(
                        &api,
                        &auth,
                        &format!(
                            "{ENGINE_API}/namespaces/{JOBS_NS}/activities/{wf_id}/{run_id}/fail"
                        ),
                        &json!({ "cause": cause, "identity": wid }),
                    );
                    if let (Some(bwf), Some(brun)) = (bridge_wf, bridge_run) {
                        let _ = http_json_post(
                            &api,
                            &auth,
                            &format!(
                                "{ENGINE_API}/namespaces/{JOBS_NS}/workflows/{bwf}/terminate?runId={brun}"
                            ),
                            &json!({ "reason": format!("activity failed: {cause}"), "identity": wid }),
                        );
                    }
                })
                .await;
            }
        }
        Ok(true)
    }

    async fn execute_claimed(
        &self,
        act_type: &str,
        wf_id: &str,
        input: &Value,
        cfg: &WorkerConfig,
        token: &str,
    ) -> Result<Value, String> {
        match act_type {
            "echo" => Ok(json!({ "echo": input, "worker": cfg.worker_id })),
            "gen.run" | "coding.run" => {
                let prompt = input
                    .get("prompt")
                    .or_else(|| input.get("task"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_default();
                if prompt.trim().is_empty() {
                    return Err("InvalidConfig: prompt or task required".to_string());
                }
                let workspace = input
                    .get("workspace")
                    .or_else(|| input.get("dir"))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let model = input
                    .get("model")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let sid = match input.get("session_id").and_then(Value::as_str) {
                    Some(existing) if !existing.trim().is_empty() => existing.to_string(),
                    _ => {
                        let created = self
                            .dev
                            .native
                            .create_session(
                                "tasks-worker",
                                CreateSessionReq {
                                    label: Some(format!("gen:{wf_id}")),
                                    workspace,
                                    model: model.clone(),
                                    prelude: input
                                        .get("instructions")
                                        .and_then(Value::as_str)
                                        .map(str::to_owned),
                                },
                            )
                            .map_err(|e| e.to_string())?;
                        created.id
                    }
                };
                let turn = self
                    .dev
                    .native
                    .run_turn(
                        "tasks-worker",
                        &sid,
                        RunTurnReq {
                            prompt,
                            model,
                            effort: input
                                .get("effort")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            backend: input
                                .get("tool")
                                .or_else(|| input.get("backend"))
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            ..Default::default()
                        },
                        Some(token.to_string()),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(json!(turn))
            }
            "bot.run" => {
                self.bot
                    .execute_bot_job("tasks-worker", input, Some(token.to_string()))
                    .await
            }
            "team.task" => {
                self.team
                    .execute_team_task("tasks-worker", input, &self.bot, Some(token.to_string()))
                    .await
            }
            "research.bench" | "research.tick" | "hanzod.run" | "fn.run" => {
                let command = input
                    .get("command")
                    .or_else(|| input.get("script"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        if act_type == "research.tick" {
                            let camp = input
                                .get("campaign")
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            if camp.is_empty() {
                                "hanzo-research tick".to_string()
                            } else {
                                format!("hanzo-research tick {camp}")
                            }
                        } else {
                            String::new()
                        }
                    });
                if command.trim().is_empty() {
                    return Err("InvalidConfig: command or script required".to_string());
                }
                let benchmark = input
                    .get("benchmark")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or(act_type)
                    .to_string();
                let artifact_paths = input
                    .get("artifact_paths")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let req = ExecuteResearchReq {
                    id: Some(format!("task_{wf_id}")),
                    hypothesis_id: input
                        .get("hypothesis_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    candidate_id: input
                        .get("candidate_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    benchmark,
                    split: input
                        .get("split")
                        .and_then(Value::as_str)
                        .unwrap_or("test")
                        .to_string(),
                    system: cfg.worker_id.clone(),
                    workspace: input
                        .get("workspace")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    command,
                    artifact_paths,
                    max_compute_seconds: input.get("max_compute_seconds").and_then(Value::as_u64),
                    questions: input.get("questions").and_then(Value::as_i64),
                    sandbox: input
                        .get("sandbox")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    cloud_api: Some(cfg.cloud_api.clone()),
                    project: input
                        .get("project")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    sync_cloud: input
                        .get("sync_cloud")
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                };
                let run = self
                    .research
                    .execute_task("tasks-worker", req, Some(token.to_string()))
                    .await
                    .map_err(|e| e.to_string())?;
                if run.status == "complete" {
                    Ok(json!(run))
                } else {
                    Err(run
                        .error
                        .unwrap_or_else(|| format!("command exited with {:?}", run.exit_code)))
                }
            }
            other => Err(format!("UnsupportedActivity: {other}")),
        }
    }
}

pub(super) fn http_json_post(
    api: &str,
    auth: &str,
    path: &str,
    body: &Value,
) -> Result<Value, String> {
    let url = format!("{api}{path}");
    match ureq::post(&url)
        .timeout(Duration::from_secs(12))
        .set("Authorization", auth)
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
    {
        Ok(resp) => {
            if resp.status() == 204 {
                return Ok(Value::Null);
            }
            Ok(serde_json::from_reader(resp.into_reader()).unwrap_or(Value::Null))
        }
        Err(ureq::Error::Status(code, resp)) => {
            let txt = resp.into_string().unwrap_or_default();
            Err(format!("HTTP {code} on {path}: {txt}"))
        }
        Err(e) => Err(format!("{path}: {e}")),
    }
}

fn claim_one(
    api: &str,
    token: &str,
    queue: &str,
    worker_id: &str,
    lease_secs: u64,
    wait_secs: u64,
) -> Result<Option<ClaimedActivity>, String> {
    let url = format!("{api}{ENGINE_API}/namespaces/{JOBS_NS}/activities/claim");
    let body = json!({
        "taskQueue": queue,
        "identity": worker_id,
        "leaseSeconds": lease_secs,
        "waitSeconds": wait_secs,
    });
    match ureq::post(&url)
        .timeout(Duration::from_secs(wait_secs + 10))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
    {
        Ok(resp) => {
            if resp.status() == 204 {
                return Ok(None);
            }
            let act: ClaimedActivity =
                serde_json::from_reader(resp.into_reader()).map_err(|e| e.to_string())?;
            Ok(Some(act))
        }
        Err(ureq::Error::Status(404, _)) => Ok(None),
        Err(ureq::Error::Status(code, resp)) => {
            let txt = resp.into_string().unwrap_or_default();
            Err(format!("claim {queue} HTTP {code}: {txt}"))
        }
        Err(e) => Err(format!("claim {queue}: {e}")),
    }
}
