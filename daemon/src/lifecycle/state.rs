// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! Persistent 3-layer desired-state store, request DTOs, and `ps`/`doctor` views.
//!
//! Persists atomically (`0600`) to `<data>/lifecycle.json` and mirrors/reads
//! `~/.hanzo/lifecycle/state.json`, while inspecting `~/.hanzo/up/` and
//! `~/.kube/hanzo.yaml` so CLI/VM runs and `hanzod` stay synchronized.

use crate::paths;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const DEFAULT_CLOUD_IMAGE: &str = "ghcr.io/hanzoai/cloud-oss:main";
pub const STRICT_CLOUD_IMAGE: &str = "oci.hanzo.ai/hanzoai/cloud-oss:main";
pub const DEFAULT_SANDBOX_IMAGE: &str = "ghcr.io/hanzoai/sandbox:latest";

pub const PLATFORM_SERVICES: [&str; 9] = [
    "cloud", "iam", "engine", "datastore", "analytics", "insights", "storage", "commerce", "search",
];

static TMP_SEQ: AtomicU64 = AtomicU64::new(1);

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn default_backend() -> String {
    if cfg!(target_os = "linux") {
        "native-linux".to_string()
    } else {
        "managed-vm".to_string()
    }
}

pub fn default_cpus() -> u32 { 4 }
pub fn default_memory_mb() -> u64 { 4096 }
pub fn default_disk_mb() -> u64 { 32768 }
pub fn default_cloud_image() -> String { DEFAULT_CLOUD_IMAGE.to_string() }
fn default_version() -> u32 { 1 }
fn default_list_action() -> String { "list".to_string() }

pub fn effective_cloud_image(current: &str, strict_registry: bool) -> String {
    let trimmed = current.trim();
    if strict_registry {
        if trimmed.is_empty() || trimmed == DEFAULT_CLOUD_IMAGE {
            return STRICT_CLOUD_IMAGE.to_string();
        }
        return trimmed.to_string();
    }
    if trimmed.is_empty() || trimmed == STRICT_CLOUD_IMAGE {
        return DEFAULT_CLOUD_IMAGE.to_string();
    }
    trimmed.to_string()
}

pub fn canonical_service_name(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "cloud" | "hanzo-cloud" | "cloud-oss" => Some("cloud"),
        "iam" | "hanzo-iam" | "casdoor" => Some("iam"),
        "engine" | "hanzo-engine" | "ai" => Some("engine"),
        "datastore" | "clickhouse" | "db" => Some("datastore"),
        "analytics" | "umami" => Some("analytics"),
        "insights" | "posthog" => Some("insights"),
        "storage" | "minio" | "s3" => Some("storage"),
        "commerce" | "hanzo-commerce" => Some("commerce"),
        "search" | "meilisearch" => Some("search"),
        _ => None,
    }
}

pub fn default_service_endpoint(name: &str) -> Option<String> {
    let ep = match name {
        "cloud" => "http://127.0.0.1:8000",
        "iam" => "http://127.0.0.1:8001",
        "engine" => "http://127.0.0.1:3690/v1/engine",
        "datastore" => "tcp://127.0.0.1:9000",
        "analytics" => "http://127.0.0.1:8085",
        "insights" => "http://127.0.0.1:8086",
        "storage" => "http://127.0.0.1:9001",
        "commerce" => "http://127.0.0.1:8002",
        "search" => "http://127.0.0.1:7700",
        _ => return None,
    };
    Some(ep.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeComponents {
    pub vm: String,
    pub k3s: String,
    pub containerd: String,
    pub visor: String,
    pub vfs: String,
    pub networking: String,
}

impl Default for RuntimeComponents {
    fn default() -> Self {
        Self {
            vm: "stopped".to_string(),
            k3s: "stopped".to_string(),
            containerd: "stopped".to_string(),
            visor: "stopped".to_string(),
            vfs: "stopped".to_string(),
            networking: "stopped".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesiredSnapshot {
    pub platform_desired: String,
    pub platform_mode: String,
    #[serde(default)]
    pub running_services: Vec<String>,
    #[serde(default)]
    pub running_apps: Vec<String>,
    #[serde(default)]
    pub running_sandboxes: Vec<String>,
    pub captured_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeState {
    pub desired: String,
    pub actual: String,
    #[serde(default = "default_backend")]
    pub backend: String,
    #[serde(default = "default_cpus")]
    pub cpus: u32,
    #[serde(default = "default_memory_mb")]
    pub memory_mb: u64,
    #[serde(default = "default_disk_mb")]
    pub disk_mb: u64,
    #[serde(default)]
    pub strict_registry: bool,
    #[serde(default)]
    pub components: RuntimeComponents,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_desired_snapshot: Option<DesiredSnapshot>,
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            desired: "stopped".to_string(),
            actual: "stopped".to_string(),
            backend: default_backend(),
            cpus: default_cpus(),
            memory_mb: default_memory_mb(),
            disk_mb: default_disk_mb(),
            strict_registry: false,
            components: RuntimeComponents::default(),
            saved_desired_snapshot: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceState {
    pub desired: String,
    pub actual: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}

impl Default for ServiceState {
    fn default() -> Self {
        Self { desired: "stopped".to_string(), actual: "stopped".to_string(), endpoint: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformState {
    pub desired: String,
    pub mode: String,
    #[serde(default)]
    pub no_ui: bool,
    #[serde(default = "default_cloud_image")]
    pub cloud_image: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iam_issuer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_base_url: Option<String>,
    #[serde(default = "default_platform_services")]
    pub services: BTreeMap<String, ServiceState>,
}

pub fn default_platform_services() -> BTreeMap<String, ServiceState> {
    PLATFORM_SERVICES.into_iter().map(|n| (n.to_string(), ServiceState::default())).collect()
}

impl Default for PlatformState {
    fn default() -> Self {
        Self {
            desired: "down".to_string(),
            mode: "full".to_string(),
            no_ui: false,
            cloud_image: default_cloud_image(),
            staged: None,
            iam_issuer: None,
            ai_base_url: None,
            services: default_platform_services(),
        }
    }
}

impl PlatformState {
    pub fn ensure_canonical_services(&mut self) {
        for name in PLATFORM_SERVICES {
            self.services.entry(name.to_string()).or_default();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackoffPolicy {
    pub initial: Duration,
    pub max: Duration,
    pub reset_after: Duration,
}

impl Default for BackoffPolicy {
    fn default() -> Self {
        Self {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(30),
            reset_after: Duration::from_secs(60),
        }
    }
}

impl BackoffPolicy {
    /// Bounded exponential backoff: `min(initial * 2^attempt, max)`.
    pub fn delay_for(&self, attempt: u32) -> Duration {
        let factor = 1u32.checked_shl(attempt.min(16)).unwrap_or(u32::MAX);
        self.initial.saturating_mul(factor).min(self.max)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppState {
    pub slug: String,
    pub name: String,
    pub project_dir: String,
    pub namespace: String,
    pub manifest_kind: String,
    pub desired: String,
    pub actual: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default)]
    pub endpoints: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default)]
    pub restart_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_exit: Option<String>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxState {
    pub name: String,
    pub image: String,
    pub desired: String,
    pub actual: String,
    pub runtime: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachePreservation {
    pub image_cache_warm: bool,
    pub persistent_volumes_preserved: bool,
    pub model_weights_preserved: bool,
    pub kube_system_preserved: bool,
}

impl Default for CachePreservation {
    fn default() -> Self {
        Self {
            image_cache_warm: true,
            persistent_volumes_preserved: true,
            model_weights_preserved: true,
            kube_system_preserved: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleState {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub runtime: RuntimeState,
    #[serde(default)]
    pub platform: PlatformState,
    #[serde(default)]
    pub applications: BTreeMap<String, AppState>,
    #[serde(default)]
    pub sandboxes: BTreeMap<String, SandboxState>,
    #[serde(default)]
    pub caches: CachePreservation,
    #[serde(default = "now_unix")]
    pub updated_at: i64,
}

impl Default for LifecycleState {
    fn default() -> Self {
        Self {
            version: 1,
            runtime: RuntimeState::default(),
            platform: PlatformState::default(),
            applications: BTreeMap::new(),
            sandboxes: BTreeMap::new(),
            caches: CachePreservation::default(),
            updated_at: now_unix(),
        }
    }
}

impl LifecycleState {
    pub fn capture_desired_snapshot(&self) -> Option<DesiredSnapshot> {
        let running_services: Vec<String> = self
            .platform
            .services
            .iter()
            .filter(|(_, s)| s.desired == "running")
            .map(|(k, _)| k.clone())
            .collect();
        let running_apps: Vec<String> = self
            .applications
            .iter()
            .filter(|(_, a)| a.desired == "running")
            .map(|(k, _)| k.clone())
            .collect();
        let running_sandboxes: Vec<String> = self
            .sandboxes
            .iter()
            .filter(|(_, sb)| sb.desired == "running")
            .map(|(k, _)| k.clone())
            .collect();
        if self.platform.desired != "up"
            && running_services.is_empty()
            && running_apps.is_empty()
            && running_sandboxes.is_empty()
        {
            return None;
        }
        Some(DesiredSnapshot {
            platform_desired: self.platform.desired.clone(),
            platform_mode: self.platform.mode.clone(),
            running_services,
            running_apps,
            running_sandboxes,
            captured_at: now_unix(),
        })
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RuntimeActionReq {
    pub action: String,
    pub backend: Option<String>,
    pub cpus: Option<u32>,
    pub memory_mb: Option<u64>,
    pub disk_mb: Option<u64>,
    pub strict_registry: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EnvironmentActionReq {
    pub action: String,
    pub mode: Option<String>,
    pub project_dir: Option<String>,
    pub scope: Option<String>,
    pub no_ui: Option<bool>,
    pub cloud_image: Option<String>,
    pub strict_registry: Option<bool>,
    pub staged: Option<String>,
    pub iam_issuer: Option<String>,
    pub ai_base_url: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ServiceActionReq {
    #[serde(default = "default_list_action")]
    pub action: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AppActionReq {
    #[serde(default = "default_list_action")]
    pub action: String,
    pub name: Option<String>,
    pub project_dir: Option<String>,
    pub tail: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SandboxActionReq {
    #[serde(default = "default_list_action")]
    pub action: String,
    pub name: Option<String>,
    pub image: Option<String>,
    pub runtime: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ResetReq {
    #[serde(default)]
    pub all: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PsRow {
    pub component: String,
    pub layer: String,
    pub desired: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorCheck {
    pub name: String,
    pub layer: String,
    pub status: String,
    pub message: String,
}

pub fn build_ps_rows(st: &LifecycleState, dev_status: &str, dev_detail: Option<String>) -> Vec<PsRow> {
    let mut rows = Vec::new();
    let rt_status = match st.runtime.actual.as_str() {
        "running" => "Running",
        "starting" => "Starting",
        "degraded" => "Degraded",
        _ => "Stopped",
    };
    rows.push(PsRow {
        component: "Hanzo Runtime".to_string(),
        layer: "infrastructure".to_string(),
        desired: st.runtime.desired.clone(),
        status: rt_status.to_string(),
        namespace: None,
        endpoint: None,
        detail: Some(format!("{} ({} vCPU, {} MiB)", st.runtime.backend, st.runtime.cpus, st.runtime.memory_mb)),
    });

    let k3s_status = match st.runtime.components.k3s.as_str() {
        "ready" => "Ready",
        "starting" => "Starting",
        _ => "Stopped",
    };
    rows.push(PsRow {
        component: "k3s".to_string(),
        layer: "infrastructure".to_string(),
        desired: st.runtime.desired.clone(),
        status: k3s_status.to_string(),
        namespace: Some("kube-system".to_string()),
        endpoint: (k3s_status == "Ready").then(|| "https://127.0.0.1:6443".to_string()),
        detail: Some(format!("containerd={}", st.runtime.components.containerd)),
    });

    for (svc_key, label) in [("cloud", "Hanzo Cloud"), ("engine", "Hanzo Engine")] {
        let svc = st.platform.services.get(svc_key).cloned().unwrap_or_default();
        rows.push(PsRow {
            component: label.to_string(),
            layer: "platform".to_string(),
            desired: svc.desired,
            status: if svc.actual == "running" { "Running" } else { "Stopped" }.to_string(),
            namespace: Some("hanzo".to_string()),
            endpoint: svc.endpoint,
            detail: (svc_key == "cloud").then(|| st.platform.cloud_image.clone()),
        });
    }

    for app in st.applications.values() {
        let status = match app.actual.as_str() {
            "running" => "Running",
            "crashed" => "Crashed",
            _ => "Stopped",
        };
        rows.push(PsRow {
            component: app.name.clone(),
            layer: "application".to_string(),
            desired: app.desired.clone(),
            status: status.to_string(),
            namespace: Some(app.namespace.clone()),
            endpoint: app.endpoints.first().cloned(),
            detail: Some(format!("slug={} kind={}", app.slug, app.manifest_kind)),
        });
    }

    for (svc_key, label) in [("analytics", "Analytics"), ("insights", "Insights")] {
        let svc = st.platform.services.get(svc_key).cloned().unwrap_or_default();
        rows.push(PsRow {
            component: label.to_string(),
            layer: "platform".to_string(),
            desired: svc.desired,
            status: if svc.actual == "running" { "Running" } else { "Stopped" }.to_string(),
            namespace: Some("hanzo".to_string()),
            endpoint: svc.endpoint,
            detail: None,
        });
    }

    for sb in st.sandboxes.values() {
        rows.push(PsRow {
            component: format!("Sandbox ({})", sb.name),
            layer: "workload".to_string(),
            desired: sb.desired.clone(),
            status: if sb.actual == "running" { "Running" } else { "Stopped" }.to_string(),
            namespace: Some("hanzo-sandboxes".to_string()),
            endpoint: None,
            detail: Some(format!("runtime={} image={}", sb.runtime, sb.image)),
        });
    }

    rows.push(PsRow {
        component: "Hanzo Dev".to_string(),
        layer: "workload".to_string(),
        desired: "running".to_string(),
        status: dev_status.to_string(),
        namespace: None,
        endpoint: Some("http://127.0.0.1:3690/v1/dev".to_string()),
        detail: dev_detail,
    });
    rows
}

pub fn format_ps_table(rows: &[PsRow]) -> String {
    let mut out = format!("{:<22} {:<16} {:<10} {}\n", "COMPONENT", "LAYER", "STATUS", "ENDPOINT");
    for r in rows {
        out.push_str(&format!(
            "{:<22} {:<16} {:<10} {}\n",
            r.component,
            r.layer,
            r.status,
            r.endpoint.as_deref().unwrap_or("-")
        ));
    }
    out
}

pub fn build_doctor_checks(st: &LifecycleState, paths: &StorePaths) -> Vec<DoctorCheck> {
    let store_synced = paths.primary_file.is_file() == paths.mirror_file.is_file();
    let rt_ok = (st.runtime.desired == "stopped" && st.runtime.actual == "stopped")
        || (st.runtime.desired == "running" && st.runtime.actual == "running");
    let expected_default = effective_cloud_image(DEFAULT_CLOUD_IMAGE, st.runtime.strict_registry);
    let disallowed_org = ["hanzo", "-inc/"].concat();
    let private_leak = st.platform.cloud_image.contains(&disallowed_org);
    let app_drift = st.applications.values().any(|a| a.desired != a.actual);

    vec![
        DoctorCheck {
            name: "state_store".to_string(),
            layer: "daemon".to_string(),
            status: if store_synced { "ok" } else { "warn" }.to_string(),
            message: format!("primary={} mirror={}", paths.primary_file.display(), paths.mirror_file.display()),
        },
        DoctorCheck {
            name: "runtime_reconcile".to_string(),
            layer: "infrastructure".to_string(),
            status: if rt_ok { "ok" } else { "warn" }.to_string(),
            message: format!("desired={} actual={} backend={} k3s={}", st.runtime.desired, st.runtime.actual, st.runtime.backend, st.runtime.components.k3s),
        },
        DoctorCheck {
            name: "oss_registry_policy".to_string(),
            layer: "platform".to_string(),
            status: if private_leak { "fail" } else { "ok" }.to_string(),
            message: format!("cloud_image={} strict_registry={} canonical_default={expected_default}", st.platform.cloud_image, st.runtime.strict_registry),
        },
        DoctorCheck {
            name: "workload_isolation".to_string(),
            layer: "application".to_string(),
            status: if app_drift { "warn" } else { "ok" }.to_string(),
            message: format!("registered_applications={}", st.applications.len()),
        },
    ]
}

#[derive(Debug, Clone)]
pub struct StorePaths {
    pub primary_file: PathBuf,
    pub mirror_file: PathBuf,
    pub up_dir: PathBuf,
    pub kubeconfig_file: PathBuf,
    pub vm_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub k3s_api_addr: SocketAddr,
}

impl StorePaths {
    pub fn system() -> Self {
        let data = paths::data();
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let hanzo_home = home.join(".hanzo");
        Self {
            primary_file: data.join("lifecycle.json"),
            mirror_file: hanzo_home.join("lifecycle").join("state.json"),
            up_dir: hanzo_home.join("up"),
            kubeconfig_file: home.join(".kube").join("hanzo.yaml"),
            vm_dir: hanzo_home.join("vm"),
            logs_dir: data.join("lifecycle").join("logs"),
            k3s_api_addr: SocketAddr::from(([127, 0, 0, 1], 6443)),
        }
    }

    #[cfg(test)]
    pub fn for_root(root: &Path, k3s_api_addr: SocketAddr) -> Self {
        let data = root.join("data");
        let home = root.join("home");
        let hanzo_home = home.join(".hanzo");
        Self {
            primary_file: data.join("lifecycle.json"),
            mirror_file: hanzo_home.join("lifecycle").join("state.json"),
            up_dir: hanzo_home.join("up"),
            kubeconfig_file: home.join(".kube").join("hanzo.yaml"),
            vm_dir: hanzo_home.join("vm"),
            logs_dir: data.join("lifecycle").join("logs"),
            k3s_api_addr,
        }
    }

    pub fn inspect_external(&self) -> ExternalUpProbe {
        let vm_alive = pid_file_alive(&self.up_dir.join("vm.pid"))
            || pid_file_alive(&self.up_dir.join("supervisor.pid"));
        let up_state_label = std::fs::read_to_string(self.up_dir.join("state"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let kubeconfig_exists = self.kubeconfig_file.is_file();
        let k3s_tcp_reachable =
            TcpStream::connect_timeout(&self.k3s_api_addr, Duration::from_millis(80)).is_ok();
        ExternalUpProbe { vm_alive, up_state_label, kubeconfig_exists, k3s_tcp_reachable }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ExternalUpProbe {
    pub vm_alive: bool,
    pub up_state_label: Option<String>,
    pub kubeconfig_exists: bool,
    pub k3s_tcp_reachable: bool,
}

pub fn pid_file_alive(path: &Path) -> bool {
    let Ok(txt) = std::fs::read_to_string(path) else { return false };
    let Ok(pid) = txt.trim().parse::<i32>() else { return false };
    #[cfg(unix)]
    {
        pid > 0 && unsafe { libc::kill(pid, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        pid > 0
    }
}

#[derive(Debug, Clone)]
pub struct StateStore {
    pub paths: StorePaths,
}

impl StateStore {
    pub fn new(paths: StorePaths) -> Self {
        Self { paths }
    }

    pub fn load(&self) -> LifecycleState {
        let primary = read_state_file(&self.paths.primary_file);
        let mirror = read_state_file(&self.paths.mirror_file);
        let mut chosen = match (primary, mirror) {
            (Some(p), Some(m)) if m.updated_at > p.updated_at => m,
            (Some(p), _) => p,
            (None, Some(m)) => m,
            (None, None) => LifecycleState::default(),
        };
        chosen.platform.ensure_canonical_services();
        chosen.platform.cloud_image =
            effective_cloud_image(&chosen.platform.cloud_image, chosen.runtime.strict_registry);
        chosen
    }

    pub fn save(&self, state: &mut LifecycleState) -> io::Result<()> {
        state.platform.ensure_canonical_services();
        state.updated_at = now_unix();
        let bytes = serde_json::to_vec_pretty(state).map_err(io::Error::other)?;
        write_atomic_0600(&self.paths.primary_file, &bytes)?;
        write_atomic_0600(&self.paths.mirror_file, &bytes)?;
        Ok(())
    }
}

fn read_state_file(path: &Path) -> Option<LifecycleState> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

pub fn write_atomic_0600(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("state.json");
    let tmp_path = path.with_file_name(format!(".{file_name}.tmp.{pid}.{seq}"));

    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    {
        let mut f = opts.open(&tmp_path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp_path, path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}
