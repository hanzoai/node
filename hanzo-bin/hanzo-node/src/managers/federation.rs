//! Federation manager — opts the node into a `hanzo-federation` lab.
//!
//! Decomplected:
//! * **Config** ([`FederationConfig`]) is a pure value parsed once from
//!   `hanzo.toml`'s `[federation]` table (or env). No I/O at construction.
//! * **Role election** ([`FederationConfig::resolve_role`]) is a function
//!   over the lab + this node's `hostname`. Picks coordinator if our
//!   declared `nic_gbps * tflops_hint` is the maximum in the lab. Falls
//!   back to worker otherwise. Deterministic — no probing.
//! * **Lifecycle** ([`FederationManager`]) owns the spawned task and
//!   tears it down on `shutdown()`. Mounting `/v1/federation/*` happens
//!   inside the spawned coordinator process — workers expose no routes.
//!
//! When the `federation-runtime` cargo feature is **off** (default), the
//! `hanzo-federation` crate is not linked. The manager still loads and
//! validates config, but `start()` only logs that the runtime is not
//! linked. This lets the node compile and run while the federation
//! transport/coordinator modules are still under construction.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// Aggregation strategy for delta soup. Matches `hanzo_federation::Lab.aggregation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Aggregation {
    Mean,
    Median,
    ByzantineRobust,
}

impl Default for Aggregation {
    fn default() -> Self {
        Aggregation::ByzantineRobust
    }
}

impl Aggregation {
    pub fn as_str(self) -> &'static str {
        match self {
            Aggregation::Mean => "mean",
            Aggregation::Median => "median",
            Aggregation::ByzantineRobust => "byzantine_robust",
        }
    }
}

/// Desired role; `Auto` defers to lab election.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DesiredRole {
    Auto,
    Coordinator,
    Worker,
}

impl Default for DesiredRole {
    fn default() -> Self {
        DesiredRole::Auto
    }
}

/// Resolved role after election.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedRole {
    Coordinator,
    Worker,
}

/// Parsed `[federation]` block from `hanzo.toml`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FederationConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_lab_path")]
    pub lab: String,
    #[serde(default)]
    pub role: DesiredRole,
    #[serde(default)]
    pub coordinator_url: String,
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default = "default_sync_interval")]
    pub sync_interval_steps: u32,
    #[serde(default)]
    pub aggregation: Aggregation,
}

fn default_lab_path() -> String {
    "/etc/hanzo/lab.yaml".to_string()
}
fn default_bind() -> String {
    "0.0.0.0:8443".to_string()
}
fn default_sync_interval() -> u32 {
    8
}

impl Default for FederationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            lab: default_lab_path(),
            role: DesiredRole::Auto,
            coordinator_url: String::new(),
            bind: default_bind(),
            sync_interval_steps: default_sync_interval(),
            aggregation: Aggregation::default(),
        }
    }
}

impl FederationConfig {
    /// Load from the `[federation]` table of `hanzo.toml`, expanding `$VAR`
    /// references in `lab`. Returns a disabled default if the file or
    /// section is absent — federation is strictly opt-in.
    pub fn load_from(path: &std::path::Path) -> Self {
        let raw = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => return Self::default(),
        };
        let value: toml::Value = match toml::from_str(&raw) {
            Ok(v) => v,
            Err(_) => return Self::default(),
        };
        let Some(table) = value.get("federation") else {
            return Self::default();
        };
        let mut cfg: FederationConfig = match table.clone().try_into() {
            Ok(c) => c,
            Err(e) => {
                log::warn!("[federation] failed to parse [federation] in {}: {e}", path.display());
                return Self::default();
            }
        };
        cfg.lab = expand_env(&cfg.lab);
        cfg
    }

    /// Resolve the working role.
    ///
    /// * Explicit `role = "coordinator" | "worker"` short-circuits.
    /// * `role = "auto"` requires a `lab.yaml`. We pick coordinator if
    ///   this `hostname` matches the lab node with the highest
    ///   `nic_gbps * tflops_hint` product (tie-broken by name).
    ///   Otherwise worker.
    ///
    /// Returns `(role, coordinator_url)`. For workers in `auto` mode we
    /// synthesise a coordinator URL from the elected coordinator's host
    /// and the bind port; the user can override via `coordinator_url`.
    pub fn resolve_role(&self, hostname: &str) -> (ResolvedRole, Option<String>) {
        match self.role {
            DesiredRole::Coordinator => (ResolvedRole::Coordinator, None),
            DesiredRole::Worker => {
                let url = if self.coordinator_url.is_empty() {
                    None
                } else {
                    Some(self.coordinator_url.clone())
                };
                (ResolvedRole::Worker, url)
            }
            DesiredRole::Auto => self.elect(hostname),
        }
    }

    fn elect(&self, hostname: &str) -> (ResolvedRole, Option<String>) {
        // Auto-mode needs a lab to read; if absent, fall back to worker
        // pointing at the configured coordinator_url (or nothing).
        let lab_path = PathBuf::from(&self.lab);
        let Ok(raw) = std::fs::read_to_string(&lab_path) else {
            log::warn!(
                "[federation] role=auto but lab.yaml missing at {}; defaulting to worker",
                lab_path.display()
            );
            return (
                ResolvedRole::Worker,
                if self.coordinator_url.is_empty() {
                    None
                } else {
                    Some(self.coordinator_url.clone())
                },
            );
        };
        let expanded = expand_env(&raw);
        let Ok(parsed) = serde_yaml::from_str::<LabShape>(&expanded) else {
            log::warn!("[federation] lab.yaml at {} failed to parse", lab_path.display());
            return (ResolvedRole::Worker, None);
        };
        // Score = nic_gbps * tflops_hint. Higher = stronger candidate.
        let best = parsed
            .nodes
            .iter()
            .max_by(|a, b| {
                let sa = (a.nic_gbps.unwrap_or(10) as u64) * (a.tflops_hint.unwrap_or(10) as u64);
                let sb = (b.nic_gbps.unwrap_or(10) as u64) * (b.tflops_hint.unwrap_or(10) as u64);
                sa.cmp(&sb).then(a.name.cmp(&b.name))
            });
        match best {
            Some(node) if matches_hostname(&node.name, &node.host, hostname) => {
                (ResolvedRole::Coordinator, None)
            }
            Some(node) => {
                let url = if self.coordinator_url.is_empty() {
                    // synthesise from the elected coordinator host + bind port
                    let port = self.bind.rsplit(':').next().unwrap_or("8443");
                    Some(format!("http://{}:{}", node.host, port))
                } else {
                    Some(self.coordinator_url.clone())
                };
                (ResolvedRole::Worker, url)
            }
            None => (ResolvedRole::Worker, None),
        }
    }
}

/// Minimal lab shape we read for election. Avoids depending on the
/// federation crate's types so the manager compiles either way.
#[derive(Debug, Deserialize)]
struct LabShape {
    nodes: Vec<LabNode>,
}

#[derive(Debug, Deserialize)]
struct LabNode {
    name: String,
    host: String,
    #[serde(default)]
    nic_gbps: Option<u32>,
    #[serde(default, alias = "tflops_hint")]
    tflops_hint: Option<u32>,
}

fn matches_hostname(name: &str, host: &str, hostname: &str) -> bool {
    name.eq_ignore_ascii_case(hostname)
        || host.eq_ignore_ascii_case(hostname)
        || host
            .split('.')
            .next()
            .map(|h| h.eq_ignore_ascii_case(hostname))
            .unwrap_or(false)
}

/// Expand `${VAR}` against the process environment; unset → empty string.
fn expand_env(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'$' && bytes[i + 1] == b'{' {
            if let Some(end) = bytes[i + 2..].iter().position(|&b| b == b'}') {
                let var = &text[i + 2..i + 2 + end];
                let valid =
                    !var.is_empty() && var.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
                if valid {
                    out.push_str(&std::env::var(var).unwrap_or_default());
                    i += 2 + end + 1;
                    continue;
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Owns the spawned coordinator/worker task. Drop or call `shutdown()`
/// to stop. Idempotent: `shutdown()` after `Drop` is a no-op.
pub struct FederationManager {
    pub config: FederationConfig,
    pub role: Option<ResolvedRole>,
    handle: Option<JoinHandle<()>>,
    stop_tx: Option<oneshot::Sender<()>>,
}

impl std::fmt::Debug for FederationManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FederationManager")
            .field("config", &self.config)
            .field("role", &self.role)
            .field("running", &self.handle.is_some())
            .finish()
    }
}

impl FederationManager {
    /// Construct from a parsed config. Does **not** start the task.
    pub fn new(config: FederationConfig) -> Self {
        Self {
            config,
            role: None,
            handle: None,
            stop_tx: None,
        }
    }

    /// Spawn the coordinator or worker if enabled. Safe to call once.
    ///
    /// Federation runtime linkage is behind the `federation-runtime`
    /// cargo feature. While the `hanzo-federation` crate is still being
    /// built out, this logs and returns successfully so the rest of the
    /// node keeps running.
    pub fn start(&mut self) {
        if !self.config.enabled {
            log::debug!("[federation] disabled in config; skipping");
            return;
        }
        if self.handle.is_some() {
            log::warn!("[federation] start() called twice; ignoring");
            return;
        }

        let hostname = hostname();
        let (role, coordinator_url) = self.config.resolve_role(&hostname);
        self.role = Some(role);

        log::info!(
            "[federation] enabled lab={} hostname={} role={:?} bind={} sync={} agg={}",
            self.config.lab,
            hostname,
            role,
            self.config.bind,
            self.config.sync_interval_steps,
            self.config.aggregation.as_str(),
        );

        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        self.stop_tx = Some(stop_tx);

        let cfg = self.config.clone();
        let handle = tokio::spawn(async move {
            run_federation(cfg, role, coordinator_url, stop_rx).await;
        });
        self.handle = Some(handle);
    }

    /// Graceful shutdown. Sends a stop signal and awaits the task.
    pub async fn shutdown(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.handle.take() {
            // 5s grace; abort otherwise.
            match tokio::time::timeout(Duration::from_secs(5), handle).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) if e.is_cancelled() => {}
                Ok(Err(e)) => log::warn!("[federation] task joined with error: {e}"),
                Err(_) => log::warn!("[federation] shutdown timeout; task left detached"),
            }
        }
    }
}

impl Drop for FederationManager {
    fn drop(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

/// Cross-platform hostname lookup. `HANZO_HOSTNAME` env wins so tests
/// and containerised deploys can override.
fn hostname() -> String {
    if let Ok(h) = std::env::var("HANZO_HOSTNAME") {
        if !h.is_empty() {
            return h;
        }
    }
    // libc::gethostname avoids an extra dep; falls back to "localhost".
    let mut buf = [0u8; 256];
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut _, buf.len()) };
    if rc != 0 {
        return "localhost".to_string();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

// ---- runtime stub vs real linkage --------------------------------------

#[cfg(feature = "federation-runtime")]
async fn run_federation(
    cfg: FederationConfig,
    role: ResolvedRole,
    coordinator_url: Option<String>,
    stop_rx: oneshot::Receiver<()>,
) {
    use hanzo_federation::coordinator::Coordinator;
    use hanzo_federation::topology::Lab;
    use hanzo_federation::worker::{Worker, WorkerConfig};

    let lab = match Lab::from_yaml(&cfg.lab) {
        Ok(l) => l,
        Err(e) => {
            log::error!("[federation] cannot load lab {}: {e}", cfg.lab);
            return;
        }
    };

    match role {
        ResolvedRole::Coordinator => {
            let bind: std::net::SocketAddr = match cfg.bind.parse() {
                Ok(s) => s,
                Err(e) => {
                    log::error!("[federation] bad bind addr {}: {e}", cfg.bind);
                    return;
                }
            };
            let coord = match Coordinator::new(lab) {
                Ok(c) => c,
                Err(e) => {
                    log::error!("[federation] coordinator init failed: {e}");
                    return;
                }
            };
            log::info!("[federation] coordinator serving on {bind} (lab={})", cfg.lab);
            let serve = coord.serve(bind);
            tokio::select! {
                _ = stop_rx => log::info!("[federation] coordinator shutdown requested"),
                res = serve => {
                    if let Err(e) = res {
                        log::error!("[federation] coordinator exited: {e}");
                    }
                }
            }
        }
        ResolvedRole::Worker => {
            // Without a hanzod-hosted training model we can't drive real
            // step/params/apply closures yet. Stand up the transport so
            // operators can verify connectivity (healthz + topology) and
            // park until shutdown. When the model side lands, swap to
            // `worker.run(step, params, apply, data)`.
            let url = coordinator_url.unwrap_or_else(|| format!("http://{}", cfg.bind));
            let name = hostname();
            let secret = lab.secrets().get(&name).cloned();
            let wcfg = WorkerConfig {
                coordinator_url: url.clone(),
                worker_name: name.clone(),
                secret,
                steps_per_round: cfg.sync_interval_steps,
                total_rounds: u32::MAX,
            };
            let worker = Worker::new(wcfg);
            log::info!(
                "[federation] worker name={name} coordinator={url} lab={} (transport-only stub)",
                cfg.lab
            );
            // Best-effort liveness probe — log but do not abort if it fails;
            // the coordinator may come up later.
            if let Err(e) = worker.client().healthz().await {
                log::warn!("[federation] coordinator healthz failed: {e}");
            }
            let _ = stop_rx.await;
            log::info!("[federation] worker shutdown requested");
        }
    }
}

#[cfg(not(feature = "federation-runtime"))]
async fn run_federation(
    cfg: FederationConfig,
    role: ResolvedRole,
    coordinator_url: Option<String>,
    stop_rx: oneshot::Receiver<()>,
) {
    log::warn!(
        "[federation] enabled but hanzo-federation crate not yet linked \
         (cargo feature `federation-runtime` is off). lab={} role={:?} bind={} url={:?}",
        cfg.lab,
        role,
        cfg.bind,
        coordinator_url,
    );
    // Park until shutdown so the manager's lifecycle still works.
    let _ = stop_rx.await;
    log::info!("[federation] placeholder task exiting");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn defaults_are_disabled() {
        let c = FederationConfig::default();
        assert!(!c.enabled);
        assert_eq!(c.bind, "0.0.0.0:8443");
        assert_eq!(c.sync_interval_steps, 8);
        assert_eq!(c.aggregation, Aggregation::ByzantineRobust);
    }

    #[test]
    fn load_missing_file_returns_default() {
        let c = FederationConfig::load_from(std::path::Path::new("/does/not/exist.toml"));
        assert!(!c.enabled);
    }

    #[test]
    fn load_parses_federation_block() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            tmp,
            r#"
[node]
ip = "0.0.0.0"

[federation]
enabled = true
lab = "/tmp/lab.yaml"
role = "coordinator"
bind = "0.0.0.0:9000"
sync_interval_steps = 16
aggregation = "median"
"#
        )
        .unwrap();
        let c = FederationConfig::load_from(tmp.path());
        assert!(c.enabled);
        assert_eq!(c.lab, "/tmp/lab.yaml");
        assert_eq!(c.role, DesiredRole::Coordinator);
        assert_eq!(c.bind, "0.0.0.0:9000");
        assert_eq!(c.sync_interval_steps, 16);
        assert_eq!(c.aggregation, Aggregation::Median);
    }

    #[test]
    fn role_explicit_short_circuits() {
        let mut c = FederationConfig::default();
        c.role = DesiredRole::Coordinator;
        assert_eq!(c.resolve_role("anything").0, ResolvedRole::Coordinator);

        c.role = DesiredRole::Worker;
        c.coordinator_url = "http://co:8443".to_string();
        let (r, url) = c.resolve_role("anything");
        assert_eq!(r, ResolvedRole::Worker);
        assert_eq!(url.as_deref(), Some("http://co:8443"));
    }

    #[test]
    fn role_auto_elects_highest_score_as_coordinator() {
        let dir = tempfile::tempdir().unwrap();
        let lab_path = dir.path().join("lab.yaml");
        std::fs::write(
            &lab_path,
            r#"
nodes:
  - name: spark
    host: spark.lan
    nic_gbps: 200
    tflops_hint: 31
  - name: m1
    host: m1.lan
    nic_gbps: 10
    tflops_hint: 5
"#,
        )
        .unwrap();
        let mut c = FederationConfig::default();
        c.lab = lab_path.to_string_lossy().into_owned();
        c.role = DesiredRole::Auto;

        let (r, _) = c.resolve_role("spark");
        assert_eq!(r, ResolvedRole::Coordinator);

        let (r, url) = c.resolve_role("m1");
        assert_eq!(r, ResolvedRole::Worker);
        assert_eq!(url.as_deref(), Some("http://spark.lan:8443"));
    }

    #[test]
    fn role_auto_missing_lab_falls_back_to_worker() {
        let mut c = FederationConfig::default();
        c.lab = "/does/not/exist/lab.yaml".to_string();
        c.role = DesiredRole::Auto;
        let (r, _) = c.resolve_role("anyhost");
        assert_eq!(r, ResolvedRole::Worker);
    }

    #[test]
    fn manager_disabled_is_noop() {
        let mut mgr = FederationManager::new(FederationConfig::default());
        mgr.start();
        assert!(mgr.handle.is_none());
        assert!(mgr.role.is_none());
    }
}
