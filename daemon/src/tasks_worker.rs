// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! Outbound `hanzoai/tasks` linked-machine worker embedded inside `hanzod`.
//!
//! Registers machine presence in `/v1/workflow/namespaces/fleet/activities` and
//! claims queued work from `/v1/workflow/namespaces/gpu-jobs/activities/claim`
//! reactively via outbound SSE (`/v1/workflow/events`) and in-process `Notify`
//! busy-to-idle transitions — with zero fragile polling timers.

use crate::bot::Bot;
use crate::cluster::ClusterCore;
use crate::dev::Dev;
use crate::engine::Engine;
use crate::me;
use crate::paths;
use crate::research::{ExecuteResearchReq, Research};
use crate::team::Team;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Notify;

const DEFAULT_CLOUD_API: &str = "https://api.hanzo.ai";
const ENGINE_API: &str = "/v1/workflow";
const FLEET_NS: &str = "fleet";
const JOBS_NS: &str = "gpu-jobs";
const HEARTBEAT_SECS: u64 = 30;
const JOB_HEARTBEAT_SECS: u64 = 15;
const CLAIM_LEASE_SECS: u64 = 30;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn rfc3339_now() -> String {
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GpuInventory {
    pub name: String,
    #[serde(rename = "memoryTotal", skip_serializing_if = "String::is_empty", default)]
    pub memory_total: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub arch: String,
    #[serde(default)]
    pub unified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareInventory {
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub cpus: usize,
    pub cpu_model: String,
    pub memory_bytes: u64,
    pub cuda: String,
    pub driver: String,
    pub rocm: String,
    pub gpus: Vec<GpuInventory>,
    pub backends: Vec<String>,
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
struct WorkerLiveState {
    linked: bool,
    sse_connected: bool,
    busy: bool,
    busy_reason: Option<String>,
    gpu_util_pct: Option<f64>,
    last_heartbeat: Option<String>,
    last_error: Option<String>,
}

pub struct TasksWorker {
    cfg: Mutex<WorkerConfig>,
    hw: HardwareInventory,
    state: Mutex<WorkerLiveState>,
    pub metrics: WorkerMetrics,
    dev: Arc<Dev>,
    research: Arc<Research>,
    bot: Arc<Bot>,
    team: Arc<Team>,
    engine: Arc<Engine>,
    cluster: Arc<ClusterCore>,
    idle_notify: Arc<Notify>,
    wake_notify: Arc<Notify>,
    stopped: AtomicBool,
    token_override: Mutex<Option<String>>,
    gpu_lock_path: PathBuf,
    gpu_util_cache: Mutex<Option<(Instant, Option<f64>)>>,
}

pub fn sanitize_hostname(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_dash = false;
    for ch in raw.trim().chars() {
        if ch == '.' {
            break;
        }
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if (ch == '-' || ch == '_') && !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "machine".to_string()
    } else {
        out
    }
}

fn detect_short_hostname() -> String {
    if let Ok(out) = std::process::Command::new("hostname").arg("-s").output() {
        if out.status.success() {
            let s = sanitize_hostname(&String::from_utf8_lossy(&out.stdout));
            if !s.is_empty() {
                return s;
            }
        }
    }
    if let Ok(h) = std::env::var("HOSTNAME") {
        let s = sanitize_hostname(&h);
        if !s.is_empty() {
            return s;
        }
    }
    "machine".to_string()
}

fn companion_hanzo_link_running() -> bool {
    if cfg!(target_os = "linux") {
        if let Ok(out) = std::process::Command::new("systemctl")
            .args(["--user", "is-active", "hanzo-node.service"])
            .output()
        {
            if String::from_utf8_lossy(&out.stdout).trim() == "active" {
                return true;
            }
        }
        if let Ok(entries) = std::fs::read_dir("/proc") {
            let my_pid = std::process::id().to_string();
            for entry in entries.flatten() {
                let fname = entry.file_name();
                let pid_str = fname.to_string_lossy();
                if pid_str == my_pid || !pid_str.chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }
                if let Ok(cmdline) = std::fs::read(entry.path().join("cmdline")) {
                    let parts: Vec<&[u8]> = cmdline.split(|b| *b == 0).filter(|s| !s.is_empty()).collect();
                    if parts.len() >= 2 {
                        let argv0 = String::from_utf8_lossy(parts[0]);
                        let argv1 = String::from_utf8_lossy(parts[1]);
                        if argv0.ends_with("/hanzo") && argv1 == "link" {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

pub fn detect_hardware() -> HardwareInventory {
    let hostname = detect_short_hostname();
    let os = match std::env::consts::OS {
        "macos" => "darwin".to_string(),
        other => other.to_string(),
    };
    let arch = std::process::Command::new("uname")
        .arg("-m")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| std::env::consts::ARCH.to_string());
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);

    let mut cpu_model = String::new();
    let mut memory_bytes: u64 = 0;

    if cfg!(target_os = "macos") {
        if let Ok(o) = std::process::Command::new("sysctl")
            .args(["-n", "machdep.cpu.brand_string"])
            .output()
        {
            cpu_model = String::from_utf8_lossy(&o.stdout).trim().to_string();
        }
        if let Ok(o) = std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
        {
            memory_bytes = String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<u64>()
                .unwrap_or(0);
        }
    } else if cfg!(target_os = "linux") {
        if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
            for line in meminfo.lines() {
                if let Some(rest) = line.strip_prefix("MemTotal:") {
                    if let Some(kb_str) = rest.split_whitespace().next() {
                        if let Ok(kb) = kb_str.parse::<u64>() {
                            memory_bytes = kb * 1024;
                        }
                    }
                    break;
                }
            }
        }
        if let Ok(cpuinfo) = std::fs::read_to_string("/proc/cpuinfo") {
            for line in cpuinfo.lines() {
                if let Some((_, val)) = line.split_once(':') {
                    let key = line.split(':').next().unwrap_or("").trim();
                    if matches!(key, "model name" | "Model name" | "Hardware" | "Processor") {
                        let v = val.trim();
                        if !v.is_empty() {
                            cpu_model = v.to_string();
                            break;
                        }
                    }
                }
            }
        }
        if cpu_model.is_empty() {
            if let Ok(o) = std::process::Command::new("lscpu").output() {
                for line in String::from_utf8_lossy(&o.stdout).lines() {
                    if let Some(rest) = line.strip_prefix("Model name:") {
                        let v = rest.trim();
                        if !v.is_empty() {
                            cpu_model = v.to_string();
                            break;
                        }
                    }
                }
            }
        }
    }

    let mut gpus = Vec::new();
    let mut cuda = String::new();
    let mut driver = String::new();
    let mut rocm = String::new();
    let mut backends = vec!["CPU".to_string()];

    if let Ok(o) = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,memory.total,compute_cap,driver_version",
            "--format=csv,noheader",
        ])
        .output()
    {
        if o.status.success() {
            for line in String::from_utf8_lossy(&o.stdout).lines() {
                let cols: Vec<&str> = line.split(',').map(str::trim).collect();
                if cols.is_empty() || cols[0].is_empty() {
                    continue;
                }
                let name = cols[0].to_string();
                let raw_mem = cols.get(1).copied().unwrap_or("");
                let cap = cols.get(2).copied().unwrap_or("");
                if let Some(drv) = cols.get(3).copied().filter(|s| !s.is_empty()) {
                    driver = drv.to_string();
                }
                let unified = raw_mem.contains("N/A") || raw_mem.is_empty();
                let memory_total = if unified && memory_bytes > 0 {
                    format!("{} MiB", memory_bytes >> 20)
                } else {
                    raw_mem.to_string()
                };
                gpus.push(GpuInventory {
                    name,
                    memory_total,
                    arch: if cap.is_empty() { String::new() } else { format!("sm_{}", cap.replace('.', "")) },
                    unified,
                });
            }
            if !gpus.is_empty() {
                backends.push("CUDA".to_string());
                backends.push("VULKAN".to_string());
                if let Ok(hdr) = std::process::Command::new("nvidia-smi").output() {
                    let txt = String::from_utf8_lossy(&hdr.stdout);
                    if let Some(pos) = txt.find("CUDA Version:") {
                        let after = &txt[pos + "CUDA Version:".len()..];
                        if let Some(ver) = after.split_whitespace().next() {
                            cuda = ver.trim_matches('|').to_string();
                        }
                    }
                }
            }
        }
    }

    if gpus.is_empty() && cfg!(target_os = "macos") && (arch == "arm64" || arch == "aarch64") {
        let name = if cpu_model.is_empty() {
            "Apple Silicon GPU".to_string()
        } else {
            cpu_model.clone()
        };
        gpus.push(GpuInventory {
            name,
            memory_total: if memory_bytes > 0 {
                format!("{} MiB", memory_bytes >> 20)
            } else {
                String::new()
            },
            arch: "metal".to_string(),
            unified: true,
        });
        backends.push("METAL".to_string());
    }

    if gpus.is_empty() && cfg!(target_os = "linux") {
        if let Ok(entries) = std::fs::read_dir("/sys/class/drm") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let s = name.to_string_lossy();
                if !s.starts_with("card") || s.contains('-') {
                    continue;
                }
                let vendor = std::fs::read_to_string(entry.path().join("device/vendor"))
                    .unwrap_or_default();
                if vendor.trim() == "0x1002" {
                    gpus.push(GpuInventory {
                        name: "AMD Radeon / Instinct GPU".to_string(),
                        memory_total: String::new(),
                        arch: "rocm".to_string(),
                        unified: false,
                    });
                    rocm = std::fs::read_to_string("/opt/rocm/.info/version")
                        .ok()
                        .map(|v| v.trim().to_string())
                        .unwrap_or_default();
                    backends.push("ROCM".to_string());
                    backends.push("VULKAN".to_string());
                    break;
                }
            }
        }
    }

    HardwareInventory {
        hostname,
        os,
        arch,
        cpus,
        cpu_model,
        memory_bytes,
        cuda,
        driver,
        rocm,
        gpus,
        backends,
    }
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
            gpu_lock_path,
            gpu_util_cache: Mutex::new(None),
        })
    }

    #[cfg(test)]
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

    fn active_token(&self) -> Option<String> {
        if let Some(ref tok) = *self.token_override.lock().unwrap() {
            if !tok.trim().is_empty() {
                return Some(tok.clone());
            }
        }
        if let Ok(env_tok) = std::env::var("HANZOD_TASKS_TOKEN") {
            if !env_tok.trim().is_empty() {
                return Some(env_tok.trim().to_string());
            }
        }
        let (_, t) = me::tokens()?;
        if t.access_token.trim().is_empty() {
            return None;
        }
        if let Some(exp) = me::expiry(&t.access_token) {
            if exp <= now_secs() as i64 + 30 {
                return None;
            }
        }
        Some(t.access_token)
    }

    pub fn capabilities(&self) -> Vec<String> {
        let mut caps = vec![
            "hanzod.run".to_string(),
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
            tokio::process::Command::new("nvidia-smi")
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

    fn registration_payload(&self, worker_id: &str) -> Value {
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
            http_json_post(&api, &auth, &format!("{ENGINE_API}/namespaces/{FLEET_NS}/activities"), &start_body)?;

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
            Err(e) => {
                // Re-register if the presence record was purged or closed.
                self.register_and_heartbeat(token).await.or(Err(e))
            }
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
                    "research.bench" | "research.tick" | "hanzod.run" | "bot.run" | "team.task" | "fn.run" => wf_type,
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
                            if let Some(inp) = ev0.get("attributes").and_then(|at| at.get("input")) {
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
            let url = format!("{api}{ENGINE_API}/namespaces/{JOBS_NS}/activities/{wf}/{run}/release");
            let body = json!({
                "reason": why,
                "identity": wid,
                "delaySeconds": delay_secs,
            });
            // On pre-v1.54 servers `/release` answers 404/400; never call `/fail` —
            // simply let the 30s claim lease lapse so another worker reclaims it.
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

        // Background lease + fleet presence heartbeat while the job executes.
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
                tick.tick().await; // skip immediate tick
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
                            &format!("{ENGINE_API}/namespaces/{JOBS_NS}/activities/{wf_c}/{run_c}/heartbeat"),
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
                        &format!("{ENGINE_API}/namespaces/{JOBS_NS}/activities/{wf_id}/{run_id}/complete"),
                        &json!({ "result": result_val, "identity": wid }),
                    );
                    if let (Some(bwf), Some(brun)) = (bridge_wf, bridge_run) {
                        let _ = http_json_post(
                            &api,
                            &auth,
                            &format!("{ENGINE_API}/namespaces/{JOBS_NS}/workflows/{bwf}/terminate?runId={brun}"),
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
                        &format!("{ENGINE_API}/namespaces/{JOBS_NS}/activities/{wf_id}/{run_id}/fail"),
                        &json!({ "cause": cause, "identity": wid }),
                    );
                    if let (Some(bwf), Some(brun)) = (bridge_wf, bridge_run) {
                        let _ = http_json_post(
                            &api,
                            &auth,
                            &format!("{ENGINE_API}/namespaces/{JOBS_NS}/workflows/{bwf}/terminate?runId={brun}"),
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
                    Err(run.error.unwrap_or_else(|| {
                        format!("command exited with {:?}", run.exit_code)
                    }))
                }
            }
            other => Err(format!("UnsupportedActivity: {other}")),
        }
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

                if !registered {
                    if me.register_and_heartbeat(&tok).await.is_ok() {
                        registered = true;
                    }
                }

                // Drain all immediately claimable work when idle.
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
    std::fs::write(&tmp, serde_json::to_vec_pretty(&doc).map_err(std::io::Error::other)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

fn http_json_post(api: &str, auth: &str, path: &str, body: &Value) -> Result<Value, String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::Ledger;
    use crate::pins::Pins;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    struct MockTasksServer {
        addr: String,
        queued: Arc<Mutex<Vec<(String, Value)>>>,
        claims: Arc<AtomicU64>,
        releases: Arc<AtomicU64>,
        fails: Arc<AtomicU64>,
        completes: Arc<AtomicU64>,
        fleet_regs: Arc<AtomicU64>,
        fleet_hbs: Arc<AtomicU64>,
        stop: Arc<AtomicBool>,
    }

    impl MockTasksServer {
        fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let addr = format!("http://{}", listener.local_addr().unwrap());
            let queued: Arc<Mutex<Vec<(String, Value)>>> = Arc::new(Mutex::new(Vec::new()));
            let claims = Arc::new(AtomicU64::new(0));
            let releases = Arc::new(AtomicU64::new(0));
            let fails = Arc::new(AtomicU64::new(0));
            let completes = Arc::new(AtomicU64::new(0));
            let fleet_regs = Arc::new(AtomicU64::new(0));
            let fleet_hbs = Arc::new(AtomicU64::new(0));
            let stop = Arc::new(AtomicBool::new(false));

            let q_c = Arc::clone(&queued);
            let cl_c = Arc::clone(&claims);
            let rel_c = Arc::clone(&releases);
            let fl_c = Arc::clone(&fails);
            let cp_c = Arc::clone(&completes);
            let fr_c = Arc::clone(&fleet_regs);
            let fh_c = Arc::clone(&fleet_hbs);
            let st_c = Arc::clone(&stop);

            std::thread::spawn(move || {
                while !st_c.load(Ordering::Relaxed) {
                    let Ok((mut stream, _)) = listener.accept() else {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    };
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                    let mut buf = vec![0u8; 16 * 1024];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        continue;
                    }
                    let raw = String::from_utf8_lossy(&buf[..n]);
                    let (head, body_str) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
                    let first_line = head.lines().next().unwrap_or("");
                    let mut parts = first_line.split_whitespace();
                    let method = parts.next().unwrap_or("");
                    let path = parts.next().unwrap_or("");

                    let (status, resp_body) = if method == "GET" && path.ends_with("/events") {
                        // Return short SSE hello +scheduled event then close cleanly for test.
                        let sse = ": ok\n\nevent: activity.scheduled\ndata: {\"kind\":\"activity.scheduled\"}\n\n";
                        let hdr = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            sse.len(),
                            sse
                        );
                        let _ = stream.write_all(hdr.as_bytes());
                        continue;
                    } else if path.contains("/namespaces/fleet/activities/") && path.ends_with("/heartbeat") {
                        fh_c.fetch_add(1, Ordering::SeqCst);
                        (200, r#"{"status":"ACTIVITY_TASK_STATE_STARTED"}"#.to_string())
                    } else if method == "POST" && path.ends_with("/namespaces/fleet/activities") {
                        fr_c.fetch_add(1, Ordering::SeqCst);
                        (200, r#"{"status":"ACTIVITY_TASK_STATE_SCHEDULED"}"#.to_string())
                    } else if method == "POST" && path.ends_with("/activities/claim") {
                        cl_c.fetch_add(1, Ordering::SeqCst);
                        let req_json: Value = serde_json::from_str(body_str).unwrap_or_default();
                        let want_q = req_json.get("taskQueue").and_then(Value::as_str).unwrap_or("");
                        let mut q = q_c.lock().unwrap();
                        if let Some(pos) = q.iter().position(|(tq, _)| tq == want_q) {
                            let (_, act) = q.remove(pos);
                            (200, act.to_string())
                        } else {
                            (204, String::new())
                        }
                    } else if method == "POST" && path.ends_with("/release") {
                        rel_c.fetch_add(1, Ordering::SeqCst);
                        (200, r#"{"status":"ACTIVITY_TASK_STATE_SCHEDULED"}"#.to_string())
                    } else if method == "POST" && path.ends_with("/complete") {
                        cp_c.fetch_add(1, Ordering::SeqCst);
                        (200, r#"{"status":"ACTIVITY_TASK_STATE_COMPLETED"}"#.to_string())
                    } else if method == "POST" && path.ends_with("/fail") {
                        fl_c.fetch_add(1, Ordering::SeqCst);
                        (200, r#"{"status":"ACTIVITY_TASK_STATE_FAILED"}"#.to_string())
                    } else if method == "GET" && path.contains("/workflows") {
                        (200, r#"{"executions":[]}"#.to_string())
                    } else {
                        (200, r#"{"ok":true}"#.to_string())
                    };

                    let status_text = if status == 204 { "204 No Content" } else { "200 OK" };
                    let reply = format!(
                        "HTTP/1.1 {status_text}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        resp_body.len(),
                        resp_body
                    );
                    let _ = stream.write_all(reply.as_bytes());
                }
            });

            Self {
                addr,
                queued,
                claims,
                releases,
                fails,
                completes,
                fleet_regs,
                fleet_hbs,
                stop,
            }
        }
    }

    impl Drop for MockTasksServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
        }
    }

    fn build_test_worker(dir: &std::path::Path, api: &str) -> (Arc<TasksWorker>, Arc<Research>, Arc<Notify>) {
        let ledger = Arc::new(Ledger::open(dir.join("ledger")));
        let pins = Arc::new(Pins::load(&dir.join("pins.json")));
        let engine = Arc::new(Engine::new(pins));
        let notify = Arc::new(Notify::new());
        let dev = Arc::new(Dev::with_notify(ledger.clone(), Some(engine.clone()), notify.clone()));
        let research = Arc::new(Research::with_db_path_and_notify(
            dir.join("research.sqlite"),
            ledger.clone(),
            notify.clone(),
        ));
        let bot = Arc::new(Bot::with_db_path(dir.join("bot.sqlite"), ledger.clone(), engine.clone(), Some(api.into())));
        let upstream = Arc::new(Mutex::new(None));
        let team = Arc::new(Team::with_db_path(dir.join("team.sqlite"), ledger.clone(), upstream.clone(), Some(api.into())));
        let cluster = ClusterCore::new(ledger, upstream);

        let hw = HardwareInventory {
            hostname: "dgx".into(),
            os: "linux".into(),
            arch: "aarch64".into(),
            cpus: 20,
            cpu_model: "Cortex-X925".into(),
            memory_bytes: 128 << 30,
            cuda: "13.0".into(),
            driver: "580.178".into(),
            rocm: String::new(),
            gpus: vec![GpuInventory {
                name: "NVIDIA GB10".into(),
                memory_total: "131072 MiB".into(),
                arch: "sm_121".into(),
                unified: true,
            }],
            backends: vec!["CPU".into(), "CUDA".into(), "VULKAN".into()],
        };
        let cfg = WorkerConfig {
            enabled: true,
            cloud_api: api.to_string(),
            worker_id: "dgx-hanzod".into(),
            when_idle_only: true,
            targeted_only: false,
            max_gpu_util_pct: 75.0,
            probe_gpu_util: false,
            force_busy_reason: None,
        };
        let worker = TasksWorker::for_test(
            cfg,
            hw,
            dev,
            research.clone(),
            bot,
            team,
            engine,
            cluster,
            notify.clone(),
            "test-jwt-token",
            dir.join("gpu.lock"),
        );
        (worker, research, notify)
    }

    #[tokio::test]
    async fn fleet_registration_and_heartbeat_link_worker() {
        let srv = MockTasksServer::start();
        let dir = tempfile::tempdir().unwrap();
        let (worker, _, _) = build_test_worker(dir.path(), &srv.addr);

        worker.register_and_heartbeat("test-jwt-token").await.unwrap();
        let st = worker.status_json();
        assert_eq!(st["linked"], true);
        assert_eq!(st["worker_id"], "dgx-hanzod");
        assert_eq!(srv.fleet_regs.load(Ordering::SeqCst), 1);
        assert_eq!(srv.fleet_hbs.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn busy_research_run_skips_claim_then_idle_notify_completes_automatically() {
        let srv = MockTasksServer::start();
        let dir = tempfile::tempdir().unwrap();
        let (worker, research, _) = build_test_worker(dir.path(), &srv.addr);

        srv.queued.lock().unwrap().push((
            "gpu:dgx-hanzod".into(),
            json!({
                "execution": { "workflowId": "bench-1", "runId": "run-1" },
                "type": { "name": "research.bench" },
                "taskQueue": "gpu:dgx-hanzod",
                "input": {
                    "benchmark": "arc-mini",
                    "command": "printf '{\"measures\":[{\"metric\":\"score\",\"value\":1.0}]}\\n'",
                    "sandbox": "workspace",
                    "sync_cloud": false
                }
            }),
        ));

        // Hold an active research guard -> machine is busy!
        let busy_guard = research.hold_active_guard();
        let claimed_while_busy = worker
            .poll_and_execute_once("test-jwt-token", 0)
            .await
            .unwrap();
        assert!(!claimed_while_busy);
        assert_eq!(srv.claims.load(Ordering::SeqCst), 0, "must not call /claim when busy");
        assert_eq!(worker.metrics.busy_skips.load(Ordering::SeqCst), 1);

        // Start reactive background worker loop; dropping busy_guard triggers Notify immediately!
        let handle = worker.spawn();
        let t0 = Instant::now();
        drop(busy_guard);

        let deadline = Instant::now() + Duration::from_secs(3);
        while srv.completes.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(srv.completes.load(Ordering::SeqCst), 1);
        assert!(t0.elapsed() < Duration::from_secs(2));
        worker.shutdown();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn backend_mismatch_releases_without_failing() {
        let srv = MockTasksServer::start();
        let dir = tempfile::tempdir().unwrap();
        let (worker, _, _) = build_test_worker(dir.path(), &srv.addr);

        srv.queued.lock().unwrap().push((
            "gpu:dgx-hanzod".into(),
            json!({
                "execution": { "workflowId": "metal-only-1", "runId": "r-metal" },
                "type": { "name": "hanzod.run" },
                "taskQueue": "gpu:dgx-hanzod",
                "input": {
                    "required_backend": "METAL",
                    "command": "echo hi"
                }
            }),
        ));

        let ran = worker
            .poll_and_execute_once("test-jwt-token", 0)
            .await
            .unwrap();
        assert!(!ran);
        assert_eq!(srv.releases.load(Ordering::SeqCst), 1);
        assert_eq!(srv.fails.load(Ordering::SeqCst), 0, "mismatch must yield, never fail");
        assert_eq!(worker.metrics.deferred_busy.load(Ordering::SeqCst), 1);
    }
}
