// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! Outbound `hanzoai/tasks` linked-machine worker embedded inside `hanzod`.
//!
//! Registers machine presence in `/v1/workflow/namespaces/fleet/activities` and
//! claims queued work from `/v1/workflow/namespaces/gpu-jobs/activities/claim`
//! reactively via outbound SSE (`/v1/workflow/events`) and in-process `Notify`
//! busy-to-idle transitions — with zero fragile polling timers.

mod claim;
mod hardware;
#[cfg(test)]
mod tests;

#[cfg(test)]
pub use hardware::GpuInventory;
pub use hardware::{detect_hardware, sanitize_hostname, HardwareInventory};

use crate::bot::Bot;
use crate::cluster::ClusterCore;
use crate::dev::Dev;
use crate::engine::Engine;
use crate::me;
use crate::paths;
use crate::research::Research;
use crate::team::Team;
pub(crate) use hardware::companion_hanzo_link_running;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Notify;

pub(super) const DEFAULT_CLOUD_API: &str = "https://api.hanzo.ai";
pub(super) const ENGINE_API: &str = "/v1/workflow";
pub(super) const FLEET_NS: &str = "fleet";
pub(super) const JOBS_NS: &str = "gpu-jobs";
const HEARTBEAT_SECS: u64 = 30;
pub(super) const JOB_HEARTBEAT_SECS: u64 = 15;
pub(super) const CLAIM_LEASE_SECS: u64 = 30;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub(super) fn rfc3339_now() -> String {
    let secs = now_secs();
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let hh = rem / 3600;
    let mm = (rem % 3600) / 60;
    let ss = rem % 60;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    format!("{year:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerConfig {
    pub enabled: bool,
    pub cloud_api: String,
    pub worker_id: String,
    pub when_idle_only: bool,
    pub targeted_only: bool,
    pub max_gpu_util_pct: f64,
    pub probe_gpu_util: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub force_busy_reason: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct WorkerConfigPatch {
    pub enabled: Option<bool>,
    pub cloud_api: Option<String>,
    pub worker_id: Option<String>,
    pub when_idle_only: Option<bool>,
    pub targeted_only: Option<bool>,
    pub max_gpu_util_pct: Option<f64>,
    pub force_busy_reason: Option<Option<String>>,
}

#[derive(Debug, Default)]
pub struct WorkerMetrics {
    pub polls: AtomicU64,
    pub busy_skips: AtomicU64,
    pub jobs_claimed: AtomicU64,
    pub jobs_completed: AtomicU64,
    pub jobs_failed: AtomicU64,
    pub deferred_busy: AtomicU64,
    pub heartbeats: AtomicU64,
    pub sse_events: AtomicU64,
}

#[derive(Debug, Default, Clone)]
pub(super) struct WorkerLiveState {
    pub linked: bool,
    pub sse_connected: bool,
    pub busy: bool,
    pub busy_reason: Option<String>,
    pub gpu_util_pct: Option<f64>,
    pub last_heartbeat: Option<String>,
    pub last_error: Option<String>,
}

pub struct TasksWorker {
    pub(super) cfg: Mutex<WorkerConfig>,
    pub(super) hw: HardwareInventory,
    pub(super) state: Mutex<WorkerLiveState>,
    pub metrics: WorkerMetrics,
    pub(super) dev: Arc<Dev>,
    pub(super) research: Arc<Research>,
    pub(super) bot: Arc<Bot>,
    pub(super) team: Arc<Team>,
    pub(super) engine: Arc<Engine>,
    pub(super) cluster: Arc<ClusterCore>,
    pub(super) idle_notify: Arc<Notify>,
    pub(super) wake_notify: Arc<Notify>,
    pub(super) stopped: AtomicBool,
    token_override: Mutex<Option<String>>,
    cached_cli_token: Mutex<Option<(String, i64)>>,
    gpu_lock_path: PathBuf,
    gpu_util_cache: Mutex<Option<(Instant, Option<f64>)>>,
}

fn load_worker_config(hw: &HardwareInventory) -> WorkerConfig {
    let file_cfg: Value = std::fs::read(paths::data().join("hanzod.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let tw = file_cfg.get("tasks_worker").cloned().unwrap_or_default();

    let env_enabled = std::env::var("HANZOD_TASKS_WORKER")
        .ok()
        .map(|v| !matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no"));
    let enabled = env_enabled
        .or_else(|| tw.get("enabled").and_then(Value::as_bool))
        .unwrap_or(true);

    let cloud_api = tw
        .get("cloud_api")
        .and_then(Value::as_str)
        .or_else(|| file_cfg.get("cloud_api").and_then(Value::as_str))
        .map(str::to_owned)
        .or_else(|| std::env::var("HANZO_API_URL").ok())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_CLOUD_API.to_string())
        .trim_end_matches('/')
        .to_string();

    let explicit_id = std::env::var("HANZOD_WORKER_ID")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::env::var("HANZO_WORKER_NAME")
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
        .or_else(|| {
            tw.get("worker_id")
                .or_else(|| file_cfg.get("worker_id"))
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned)
        });
    let worker_id = match explicit_id {
        Some(id) => sanitize_hostname(&id),
        None if companion_hanzo_link_running() => format!("{}-hanzod", hw.hostname),
        None => hw.hostname.clone(),
    };

    let when_idle_only = tw
        .get("when_idle_only")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let targeted_only = tw
        .get("targeted_only")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let max_gpu_util_pct = tw
        .get("max_gpu_util_pct")
        .and_then(Value::as_f64)
        .unwrap_or(75.0);

    WorkerConfig {
        enabled,
        cloud_api,
        worker_id,
        when_idle_only,
        targeted_only,
        max_gpu_util_pct,
        probe_gpu_util: true,
        force_busy_reason: None,
    }
}

impl TasksWorker {
    pub fn new(
        dev: Arc<Dev>,
        research: Arc<Research>,
        bot: Arc<Bot>,
        team: Arc<Team>,
        engine: Arc<Engine>,
        cluster: Arc<ClusterCore>,
        idle_notify: Arc<Notify>,
    ) -> Arc<Self> {
        let hw = detect_hardware();
        let cfg = load_worker_config(&hw);
        let gpu_lock_path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".hanzo")
            .join("research")
            .join("controller")
            .join("gpu.lock");
        Arc::new(Self {
            cfg: Mutex::new(cfg),
            hw,
            state: Mutex::new(WorkerLiveState::default()),
            metrics: WorkerMetrics::default(),
            dev,
            research,
            bot,
            team,
            engine,
            cluster,
            idle_notify,
            wake_notify: Arc::new(Notify::new()),
            stopped: AtomicBool::new(false),
            token_override: Mutex::new(None),
            cached_cli_token: Mutex::new(None),
            gpu_lock_path,
            gpu_util_cache: Mutex::new(None),
        })
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub fn for_test(
        cfg: WorkerConfig,
        hw: HardwareInventory,
        dev: Arc<Dev>,
        research: Arc<Research>,
        bot: Arc<Bot>,
        team: Arc<Team>,
        engine: Arc<Engine>,
        cluster: Arc<ClusterCore>,
        idle_notify: Arc<Notify>,
        token: &str,
        gpu_lock_path: PathBuf,
    ) -> Arc<Self> {
        Arc::new(Self {
            cfg: Mutex::new(cfg),
            hw,
            state: Mutex::new(WorkerLiveState::default()),
            metrics: WorkerMetrics::default(),
            dev,
            research,
            bot,
            team,
            engine,
            cluster,
            idle_notify,
            wake_notify: Arc::new(Notify::new()),
            stopped: AtomicBool::new(false),
            token_override: Mutex::new(Some(token.to_string())),
            cached_cli_token: Mutex::new(None),
            gpu_lock_path,
            gpu_util_cache: Mutex::new(None),
        })
    }

    pub fn wake(&self) {
        self.wake_notify.notify_one();
    }

    pub fn shutdown(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.wake_notify.notify_waiters();
        self.idle_notify.notify_waiters();
    }

    pub fn active_token(&self) -> Option<String> {
        if let Some(ref tok) = *self.token_override.lock().unwrap() {
            let trimmed = tok.trim();
            return if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            };
        }
        if let Ok(env_tok) = std::env::var("HANZOD_TASKS_TOKEN") {
            if !env_tok.trim().is_empty() {
                return Some(env_tok.trim().to_string());
            }
        }
        let now = now_secs() as i64;
        if let Some((_, t)) = me::tokens() {
            let tok = t.access_token.trim();
            if !tok.is_empty() && me::expiry(tok).map_or(true, |exp| exp > now + 30) {
                return Some(tok.to_string());
            }
        }
        if let Some((ref tok, exp)) = *self.cached_cli_token.lock().unwrap() {
            if exp > now + 60 {
                return Some(tok.clone());
            }
        }
        let refreshed = me::refresh_token()?;
        let exp = me::expiry(&refreshed).unwrap_or(now + 900);
        *self.cached_cli_token.lock().unwrap() = Some((refreshed.clone(), exp));
        Some(refreshed)
    }

    pub fn capabilities(&self) -> Vec<String> {
        let mut caps = vec![
            "hanzod.run".to_string(),
            "gen.run".to_string(),
            "coding.run".to_string(),
            "research.bench".to_string(),
            "research.tick".to_string(),
            "bot.run".to_string(),
            "team.task".to_string(),
            "fn.run".to_string(),
        ];
        if self.engine.port().is_some() {
            caps.push("engine.serve".to_string());
        }
        if self.cluster.is_ready() {
            caps.push("k3s.ready".to_string());
        }
        caps
    }

    pub fn claim_queues(&self) -> Vec<String> {
        let cfg = self.cfg.lock().unwrap().clone();
        let mut q = vec![format!("gpu:{}", cfg.worker_id)];
        let host_q = format!("hanzod:{}", self.hw.hostname);
        if !q.contains(&host_q) {
            q.push(host_q);
        }
        if !cfg.targeted_only {
            q.push(JOBS_NS.to_string());
        }
        q
    }

    fn gpu_lock_held_externally(&self) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            if !self.gpu_lock_path.is_file() {
                return false;
            }
            let Ok(f) = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&self.gpu_lock_path)
            else {
                return false;
            };
            let rc = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if rc == 0 {
                unsafe {
                    libc::flock(f.as_raw_fd(), libc::LOCK_UN);
                }
                false
            } else {
                true
            }
        }
        #[cfg(not(unix))]
        {
            false
        }
    }

    async fn sample_gpu_util(&self, enabled: bool) -> Option<f64> {
        if !enabled || !self.hw.backends.iter().any(|b| b == "CUDA") {
            return None;
        }
        if let Some((at, cached)) = *self.gpu_util_cache.lock().unwrap() {
            if at.elapsed() < Duration::from_secs(2) {
                return cached;
            }
        }
        let out = tokio::time::timeout(
            Duration::from_secs(2),
            crate::proc::bg_tokio_cmd("nvidia-smi")
                .args(["--query-gpu=utilization.gpu", "--format=csv,noheader,nounits"])
                .kill_on_drop(true)
                .output(),
        )
        .await
        .ok()
        .and_then(|r| r.ok())
        .filter(|o| o.status.success());

        let parsed = out.and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter_map(|l| l.trim().parse::<f64>().ok())
                .fold(None, |acc: Option<f64>, v| Some(acc.map_or(v, |m| m.max(v))))
        });
        *self.gpu_util_cache.lock().unwrap() = Some((Instant::now(), parsed));
        parsed
    }

    pub async fn busy_status(&self) -> (Option<String>, Option<f64>) {
        let cfg = self.cfg.lock().unwrap().clone();
        if let Some(forced) = cfg.force_busy_reason.filter(|s| !s.trim().is_empty()) {
            return (Some(forced), None);
        }
        if !cfg.when_idle_only {
            let util = self.sample_gpu_util(cfg.probe_gpu_util).await;
            return (None, util);
        }
        if self.dev.active_sessions_count() > 0 {
            return (Some("active_dev_session".into()), None);
        }
        if self.research.active_runs() > 0 {
            return (Some("active_research_run".into()), None);
        }
        if self.gpu_lock_held_externally() {
            return (Some("local_gpu_lock_held".into()), None);
        }
        let util = self.sample_gpu_util(cfg.probe_gpu_util).await;
        if let Some(u) = util {
            if u > cfg.max_gpu_util_pct {
                return (Some("gpu_utilization_high".into()), Some(u));
            }
        }
        (None, util)
    }

    pub fn status_json(&self) -> Value {
        let cfg = self.cfg.lock().unwrap().clone();
        let st = self.state.lock().unwrap().clone();
        json!({
            "ok": true,
            "enabled": cfg.enabled,
            "linked": st.linked,
            "sse_connected": st.sse_connected,
            "worker_id": cfg.worker_id,
            "hostname": self.hw.hostname,
            "cloud_api": cfg.cloud_api,
            "queues": self.claim_queues(),
            "capabilities": self.capabilities(),
            "os": self.hw.os,
            "arch": self.hw.arch,
            "cpus": self.hw.cpus,
            "cpu_model": self.hw.cpu_model,
            "memory": self.hw.memory_bytes,
            "gpus": self.hw.gpus,
            "backends": self.hw.backends,
            "busy": st.busy,
            "busy_reason": st.busy_reason,
            "gpu_util_pct": st.gpu_util_pct,
            "when_idle_only": cfg.when_idle_only,
            "targeted_only": cfg.targeted_only,
            "max_gpu_util_pct": cfg.max_gpu_util_pct,
            "active_dev_sessions": self.dev.active_sessions_count(),
            "active_research_runs": self.research.active_runs(),
            "last_heartbeat": st.last_heartbeat,
            "last_error": st.last_error,
            "metrics": {
                "polls": self.metrics.polls.load(Ordering::Relaxed),
                "busy_skips": self.metrics.busy_skips.load(Ordering::Relaxed),
                "jobs_claimed": self.metrics.jobs_claimed.load(Ordering::Relaxed),
                "jobs_completed": self.metrics.jobs_completed.load(Ordering::Relaxed),
                "jobs_failed": self.metrics.jobs_failed.load(Ordering::Relaxed),
                "deferred_busy": self.metrics.deferred_busy.load(Ordering::Relaxed),
                "heartbeats": self.metrics.heartbeats.load(Ordering::Relaxed),
                "sse_events": self.metrics.sse_events.load(Ordering::Relaxed),
            }
        })
    }

    pub fn summary_json(&self) -> Value {
        let cfg = self.cfg.lock().unwrap().clone();
        let st = self.state.lock().unwrap().clone();
        json!({
            "enabled": cfg.enabled,
            "linked": st.linked,
            "sse_connected": st.sse_connected,
            "worker_id": cfg.worker_id,
            "queues": self.claim_queues(),
            "busy": st.busy,
            "busy_reason": st.busy_reason,
            "capabilities": self.capabilities(),
        })
    }

    pub fn apply_patch(&self, patch: WorkerConfigPatch, persist: bool) -> std::io::Result<Value> {
        {
            let mut cfg = self.cfg.lock().unwrap();
            if let Some(en) = patch.enabled {
                cfg.enabled = en;
            }
            if let Some(api) = patch.cloud_api.filter(|s| !s.trim().is_empty()) {
                cfg.cloud_api = api.trim_end_matches('/').to_string();
            }
            if let Some(id) = patch.worker_id.filter(|s| !s.trim().is_empty()) {
                cfg.worker_id = sanitize_hostname(&id);
            }
            if let Some(idle) = patch.when_idle_only {
                cfg.when_idle_only = idle;
            }
            if let Some(targeted) = patch.targeted_only {
                cfg.targeted_only = targeted;
            }
            if let Some(max_u) = patch.max_gpu_util_pct {
                cfg.max_gpu_util_pct = max_u.clamp(1.0, 100.0);
            }
            if let Some(force) = patch.force_busy_reason {
                cfg.force_busy_reason = force;
            }
            if persist {
                persist_worker_config(&cfg)?;
            }
        }
        self.wake();
        Ok(self.status_json())
    }

    pub(super) fn registration_payload(&self, worker_id: &str) -> Value {
        let mut reg = json!({
            "hostname": worker_id,
            "os": self.hw.os,
            "arch": self.hw.arch,
            "cpus": self.hw.cpus,
            "cpuModel": self.hw.cpu_model,
            "memory": self.hw.memory_bytes,
            "version": format!("hanzod/{}", env!("CARGO_PKG_VERSION")),
            "jobQueue": JOBS_NS,
            "gpus": self.hw.gpus,
            "capabilities": self.capabilities(),
        });
        if !self.hw.cuda.is_empty() {
            reg["cuda"] = json!(self.hw.cuda);
        }
        if !self.hw.driver.is_empty() {
            reg["driver"] = json!(self.hw.driver);
        }
        if !self.hw.rocm.is_empty() {
            reg["rocm"] = json!(self.hw.rocm);
        }
        if let Some(port) = self.engine.port() {
            reg["engine"] = json!({
                "url": format!("http://127.0.0.1:{port}"),
                "apis": ["openai"],
                "status": "ready"
            });
        }
        reg
    }

    fn spawn_sse_listener(self: &Arc<Self>) {
        let me = Arc::clone(self);
        std::thread::Builder::new()
            .name("hanzod-tasks-sse".into())
            .spawn(move || {
                while !me.stopped.load(Ordering::Relaxed) {
                    let cfg = me.cfg.lock().unwrap().clone();
                    let Some(tok) = me.active_token() else {
                        std::thread::sleep(Duration::from_secs(5));
                        continue;
                    };
                    if !cfg.enabled {
                        std::thread::sleep(Duration::from_secs(3));
                        continue;
                    }
                    let url = format!("{}{ENGINE_API}/events", cfg.cloud_api);
                    let req = ureq::get(&url)
                        .set("Authorization", &format!("Bearer {tok}"))
                        .set("Accept", "text/event-stream")
                        .timeout(Duration::from_secs(300));
                    match req.call() {
                        Ok(resp) => {
                            me.state.lock().unwrap().sse_connected = true;
                            me.wake();
                            let reader = BufReader::new(resp.into_reader());
                            for line in reader.lines() {
                                if me.stopped.load(Ordering::Relaxed) {
                                    break;
                                }
                                let Ok(l) = line else {
                                    break;
                                };
                                if let Some(ev_name) = l.strip_prefix("event:") {
                                    let kind = ev_name.trim();
                                    if matches!(
                                        kind,
                                        "activity.scheduled"
                                            | "activity.deferred"
                                            | "activity.retrying"
                                            | "activity.timedout"
                                            | "workflow.started"
                                            | "schedule.triggered"
                                    ) {
                                        me.metrics.sse_events.fetch_add(1, Ordering::Relaxed);
                                        me.wake();
                                    }
                                }
                            }
                            me.state.lock().unwrap().sse_connected = false;
                        }
                        Err(_) => {
                            me.state.lock().unwrap().sse_connected = false;
                        }
                    }
                    if !me.stopped.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_secs(3));
                    }
                }
            })
            .ok();
    }

    pub fn spawn(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        self.spawn_sse_listener();
        let me = Arc::clone(self);
        tokio::spawn(async move {
            let mut hb_tick = tokio::time::interval(Duration::from_secs(HEARTBEAT_SECS));
            let mut registered = false;

            while !me.stopped.load(Ordering::Relaxed) {
                let enabled = me.cfg.lock().unwrap().enabled;
                if !enabled {
                    me.state.lock().unwrap().linked = false;
                    registered = false;
                    tokio::select! {
                        _ = me.wake_notify.notified() => {}
                        _ = hb_tick.tick() => {}
                    }
                    continue;
                }

                let Some(tok) = me.active_token() else {
                    me.state.lock().unwrap().linked = false;
                    registered = false;
                    tokio::select! {
                        _ = me.wake_notify.notified() => {}
                        _ = hb_tick.tick() => {}
                    }
                    continue;
                };

                if !registered && me.register_and_heartbeat(&tok).await.is_ok() {
                    registered = true;
                }

                while !me.stopped.load(Ordering::Relaxed) {
                    let sse_up = me.state.lock().unwrap().sse_connected;
                    let wait_secs = if sse_up { 0 } else { 2 };
                    match me.poll_and_execute_once(&tok, wait_secs).await {
                        Ok(true) => continue,
                        _ => break,
                    }
                }

                tokio::select! {
                    _ = me.wake_notify.notified() => {}
                    _ = me.idle_notify.notified() => {}
                    _ = hb_tick.tick() => {
                        if let Some(t) = me.active_token() {
                            let _ = me.heartbeat_only(&t).await;
                        }
                    }
                }
            }
        })
    }
}

fn persist_worker_config(cfg: &WorkerConfig) -> std::io::Result<()> {
    let path = paths::data().join("hanzod.json");
    let mut doc: Value = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| json!({}));
    if !doc.is_object() {
        doc = json!({});
    }
    doc["tasks_worker"] = json!({
        "enabled": cfg.enabled,
        "cloud_api": cfg.cloud_api,
        "worker_id": cfg.worker_id,
        "when_idle_only": cfg.when_idle_only,
        "targeted_only": cfg.targeted_only,
        "max_gpu_util_pct": cfg.max_gpu_util_pct,
    });
    std::fs::create_dir_all(paths::data())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(
        &tmp,
        serde_json::to_vec_pretty(&doc).map_err(std::io::Error::other)?,
    )?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}
