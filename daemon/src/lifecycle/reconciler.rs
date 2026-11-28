// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! Desired-state reconciler and 3-layer lifecycle state machine.

use super::backoff::BackoffTracker;
use super::project::{
    canonical_dir, discover_project, find_app_by_dir, resolve_app_slug, DetectedRepo, DiscoveredApp,
};
pub use super::state::{
    AppActionReq, EnvironmentActionReq, ResetReq, RuntimeActionReq, SandboxActionReq,
    ServiceActionReq,
};
use super::state::{
    build_doctor_checks, build_ps_rows, canonical_service_name, default_service_endpoint,
    effective_cloud_image, format_ps_table, now_unix, AppState, BackoffPolicy, CachePreservation,
    LifecycleState, PlatformState, RuntimeComponents, RuntimeState, SandboxState, StateStore,
    StorePaths, DEFAULT_SANDBOX_IMAGE, PLATFORM_SERVICES,
};
use crate::ledger::Ledger;
use crate::table::Fut;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Default)]
pub struct AppProcessObs {
    pub alive: bool,
    pub pid: Option<u32>,
    pub exit_reason: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Observation {
    pub runtime_running: Option<bool>,
    pub components: Option<RuntimeComponents>,
    pub cloud_running: Option<bool>,
    pub cloud_endpoint: Option<String>,
    pub engine_running: Option<bool>,
    pub engine_endpoint: Option<String>,
    pub dev_status: &'static str,
    pub dev_detail: Option<String>,
    pub app_processes: BTreeMap<String, AppProcessObs>,
}

#[derive(Debug, Clone)]
pub struct BootOutcome {
    pub actual: String,
    pub components: RuntimeComponents,
}

#[derive(Debug, Clone)]
pub struct AppExecOutcome {
    pub actual: String,
    pub pid: Option<u32>,
}

pub trait LifecycleExecutor: Send + Sync {
    fn observe<'a>(&'a self, state: &'a LifecycleState) -> Fut<'a, Observation>;
    fn boot_runtime<'a>(&'a self, runtime: &'a RuntimeState) -> Fut<'a, io::Result<BootOutcome>>;
    fn drain_and_stop_runtime<'a>(&'a self, state: &'a LifecycleState) -> Fut<'a, io::Result<()>>;
    fn resize_runtime<'a>(
        &'a self,
        runtime: &'a RuntimeState,
        was_running: bool,
    ) -> Fut<'a, io::Result<BootOutcome>>;
    fn reconcile_platform<'a>(
        &'a self,
        runtime: &'a RuntimeState,
        platform: &'a PlatformState,
    ) -> Fut<'a, io::Result<BTreeMap<String, (String, Option<String>)>>>;
    fn reconcile_service<'a>(
        &'a self,
        name: &'a str,
        desired: &'a str,
        force_restart: bool,
        runtime: &'a RuntimeState,
        platform: &'a PlatformState,
    ) -> Fut<'a, io::Result<(String, Option<String>)>>;
    fn reconcile_app<'a>(
        &'a self,
        app: &'a AppState,
        desired: &'a str,
    ) -> Fut<'a, io::Result<AppExecOutcome>>;
    fn read_app_logs<'a>(
        &'a self,
        app: &'a AppState,
        tail: usize,
    ) -> Fut<'a, io::Result<Vec<String>>>;
    fn reconcile_sandbox<'a>(
        &'a self,
        sandbox: &'a SandboxState,
        desired: &'a str,
    ) -> Fut<'a, io::Result<String>>;
    fn reset<'a>(&'a self, all: bool, paths: &'a StorePaths) -> Fut<'a, io::Result<()>>;
}

pub struct Reconciler {
    pub store: StateStore,
    pub(super) state: Mutex<LifecycleState>,
    pub(super) executor: Arc<dyn LifecycleExecutor>,
    pub(super) ledger: Arc<Ledger>,
    pub(super) backoff: BackoffTracker,
}

impl Reconciler {
    pub fn new(store: StateStore, executor: Arc<dyn LifecycleExecutor>, ledger: Arc<Ledger>) -> Self {
        Self::with_backoff(store, executor, ledger, BackoffPolicy::default())
    }

    pub fn with_backoff(
        store: StateStore,
        executor: Arc<dyn LifecycleExecutor>,
        ledger: Arc<Ledger>,
        policy: BackoffPolicy,
    ) -> Self {
        let initial = store.load();
        Self {
            store,
            state: Mutex::new(initial),
            executor,
            ledger,
            backoff: BackoffTracker::new(policy),
        }
    }

    pub(super) async fn sync_locked(&self, st: &mut LifecycleState) -> bool {
        let mut drifted = false;
        let ext = self.store.paths.inspect_external();
        let obs = self.executor.observe(st).await;

        if ext.vm_alive || (ext.kubeconfig_exists && ext.k3s_tcp_reachable) {
            st.runtime.actual = "running".to_string();
            st.runtime.components.vm = if ext.vm_alive { "running" } else { "native" }.to_string();
            st.runtime.components.k3s = if ext.k3s_tcp_reachable
                || ext.kubeconfig_exists
                || ext.up_state_label.as_deref() == Some("ready")
            {
                "ready"
            } else {
                "starting"
            }
            .to_string();
            st.runtime.components.containerd = "running".to_string();
            st.runtime.components.networking = "ready".to_string();
            if st.runtime.components.vfs == "stopped" {
                st.runtime.components.vfs = "ready".to_string();
            }
        } else if let Some(running) = obs.runtime_running {
            if running {
                st.runtime.actual = "running".to_string();
            } else {
                if st.runtime.actual != "stopped" {
                    drifted = true;
                }
                st.runtime.actual = "stopped".to_string();
            }
            if let Some(comp) = obs.components {
                st.runtime.components = comp;
            }
        }

        if let Some(cloud_up) = obs.cloud_running {
            if let Some(svc) = st.platform.services.get_mut("cloud") {
                if cloud_up {
                    svc.actual = "running".to_string();
                    if obs.cloud_endpoint.is_some() {
                        svc.endpoint = obs.cloud_endpoint;
                    }
                } else if svc.actual == "running" {
                    svc.actual = "stopped".to_string();
                    svc.endpoint = None;
                    drifted = true;
                }
            }
        }
        if let Some(eng_up) = obs.engine_running {
            if let Some(svc) = st.platform.services.get_mut("engine") {
                if eng_up {
                    svc.actual = "running".to_string();
                    if obs.engine_endpoint.is_some() {
                        svc.endpoint = obs.engine_endpoint;
                    }
                } else if svc.actual == "running" {
                    svc.actual = "stopped".to_string();
                    svc.endpoint = None;
                    drifted = true;
                }
            }
        }

        for (slug, proc_obs) in &obs.app_processes {
            if let Some(app) = st.applications.get_mut(slug) {
                if proc_obs.alive {
                    app.pid = proc_obs.pid.or(app.pid);
                    if app.desired == "running" {
                        app.actual = "running".to_string();
                    }
                } else if app.actual == "running" {
                    app.actual = "crashed".to_string();
                    app.pid = None;
                    app.last_exit = proc_obs.exit_reason.clone();
                    app.updated_at = now_unix();
                    drifted = true;
                }
            }
        }

        drifted
    }

    pub(super) async fn ensure_runtime_running_locked(
        &self,
        who: &str,
        st: &mut LifecycleState,
    ) -> Result<(), (u16, String)> {
        if st.runtime.desired == "running" && st.runtime.actual == "running" {
            return Ok(());
        }
        st.runtime.desired = "running".to_string();
        st.runtime.actual = "starting".to_string();
        let boot = self
            .executor
            .boot_runtime(&st.runtime)
            .await
            .map_err(|e| (502, format!("runtime start failed: {e}")))?;
        st.runtime.actual = boot.actual;
        st.runtime.components = boot.components;
        st.caches = CachePreservation::default();
        self.ledger.record(who, "lifecycle.runtime.start", &st.runtime.backend);
        Ok(())
    }

    pub async fn status_json(&self) -> Value {
        let mut st = self.state.lock().await;
        self.sync_locked(&mut st).await;
        let obs = self.executor.observe(&st).await;
        let rows = build_ps_rows(&st, obs.dev_status, obs.dev_detail);
        json!({
            "ok": true,
            "daemon": {
                "name": "hanzod",
                "running": true,
                "controller": "desired-state-reconciler",
            },
            "runtime": st.runtime,
            "platform": st.platform,
            "workloads": {
                "applications": st.applications,
                "sandboxes": st.sandboxes,
            },
            "caches": st.caches,
            "ps": rows,
            "updated_at": st.updated_at,
        })
    }

    pub async fn ps_json(&self) -> Value {
        let mut st = self.state.lock().await;
        self.sync_locked(&mut st).await;
        let obs = self.executor.observe(&st).await;
        let rows = build_ps_rows(&st, obs.dev_status, obs.dev_detail);
        let table = format_ps_table(&rows);
        json!({ "ok": true, "rows": rows, "table": table })
    }

    pub async fn doctor_json(&self) -> Value {
        let mut st = self.state.lock().await;
        self.sync_locked(&mut st).await;
        let checks = build_doctor_checks(&st, &self.store.paths);
        let drift_detected = checks.iter().any(|c| c.status != "ok");
        let healthy = !checks.iter().any(|c| c.status == "fail");
        let summary = if !drift_detected {
            "all lifecycle layers synchronized and healthy"
        } else if healthy {
            "desired-state drift or warnings detected; run `hanzo start` or `hanzo up` to reconcile"
        } else {
            "one or more lifecycle checks failed"
        };
        json!({
            "ok": true,
            "healthy": healthy,
            "drift_detected": drift_detected,
            "summary": summary,
            "checks": checks,
        })
    }

    pub async fn runtime_action(
        &self,
        who: &str,
        req: RuntimeActionReq,
    ) -> Result<Value, (u16, String)> {
        let mut st = self.state.lock().await;
        self.sync_locked(&mut st).await;

        if let Some(ref b) = req.backend {
            match b.as_str() {
                "native-linux" | "managed-vm" => st.runtime.backend = b.clone(),
                other => return Err((400, format!("unsupported backend '{other}'"))),
            }
        }
        if let Some(cpus) = req.cpus {
            if cpus == 0 {
                return Err((400, "cpus must be >= 1".to_string()));
            }
            st.runtime.cpus = cpus;
        }
        if let Some(mem) = req.memory_mb {
            if mem < 256 {
                return Err((400, "memory_mb must be >= 256".to_string()));
            }
            st.runtime.memory_mb = mem;
        }
        if let Some(disk) = req.disk_mb {
            if disk < 1024 {
                return Err((400, "disk_mb must be >= 1024".to_string()));
            }
            st.runtime.disk_mb = disk;
        }
        if let Some(strict) = req.strict_registry {
            st.runtime.strict_registry = strict;
            st.platform.cloud_image = effective_cloud_image(&st.platform.cloud_image, strict);
        }

        match req.action.trim() {
            "start" => {
                self.backoff.clear_key("runtime");
                self.ensure_runtime_running_locked(who, &mut st).await?;
            }
            "stop" => {
                if let Some(snap) = st.capture_desired_snapshot() {
                    st.runtime.saved_desired_snapshot = Some(snap);
                }
                st.runtime.desired = "stopped".to_string();
                st.runtime.actual = "stopping".to_string();
                self.executor
                    .drain_and_stop_runtime(&st)
                    .await
                    .map_err(|e| (502, format!("runtime stop failed: {e}")))?;
                mark_all_stopped_after_drain(&mut st);
                self.backoff.clear_all();
                self.ledger.record(who, "lifecycle.runtime.stop", "drained-and-stopped");
            }
            "restart" => {
                let snapshot = st
                    .capture_desired_snapshot()
                    .or_else(|| st.runtime.saved_desired_snapshot.clone());
                st.runtime.saved_desired_snapshot = snapshot.clone();

                st.runtime.actual = "stopping".to_string();
                self.executor
                    .drain_and_stop_runtime(&st)
                    .await
                    .map_err(|e| (502, format!("runtime stop during restart failed: {e}")))?;
                mark_all_stopped_after_drain(&mut st);
                self.ensure_runtime_running_locked(who, &mut st).await?;

                if let Some(snap) = snapshot {
                    st.platform.desired = snap.platform_desired.clone();
                    st.platform.mode = snap.platform_mode.clone();
                    for (name, svc) in st.platform.services.iter_mut() {
                        svc.desired = if snap.running_services.contains(name) {
                            "running"
                        } else {
                            "stopped"
                        }
                        .to_string();
                    }
                    let outcomes = self
                        .executor
                        .reconcile_platform(&st.runtime, &st.platform)
                        .await
                        .map_err(|e| (502, format!("platform restore failed: {e}")))?;
                    apply_service_outcomes(&mut st.platform, outcomes);

                    for slug in &snap.running_apps {
                        let maybe_app = st.applications.get_mut(slug).map(|a| {
                            a.desired = "running".to_string();
                            a.clone()
                        });
                        if let Some(app_clone) = maybe_app {
                            let out = self
                                .executor
                                .reconcile_app(&app_clone, "running")
                                .await
                                .map_err(|e| (502, format!("app '{slug}' restore failed: {e}")))?;
                            if let Some(a) = st.applications.get_mut(slug) {
                                a.actual = out.actual;
                                a.pid = out.pid;
                                a.updated_at = now_unix();
                            }
                        }
                    }

                    for sb_name in &snap.running_sandboxes {
                        let maybe_sb = st.sandboxes.get_mut(sb_name).map(|s| {
                            s.desired = "running".to_string();
                            s.clone()
                        });
                        if let Some(sb_clone) = maybe_sb {
                            let actual = self
                                .executor
                                .reconcile_sandbox(&sb_clone, "running")
                                .await
                                .map_err(|e| (502, format!("sandbox '{sb_name}' restore failed: {e}")))?;
                            if let Some(s) = st.sandboxes.get_mut(sb_name) {
                                s.actual = actual;
                            }
                        }
                    }
                    st.runtime.saved_desired_snapshot = None;
                }
                self.ledger.record(who, "lifecycle.runtime.restart", &st.runtime.backend);
            }
            "resize" => {
                let was_running = st.runtime.actual == "running";
                let boot = self
                    .executor
                    .resize_runtime(&st.runtime, was_running)
                    .await
                    .map_err(|e| (502, format!("runtime resize failed: {e}")))?;
                if was_running {
                    st.runtime.actual = boot.actual;
                    st.runtime.components = boot.components;
                }
                self.ledger.record(
                    who,
                    "lifecycle.runtime.resize",
                    &format!("cpus={} mem={}MB disk={}MB", st.runtime.cpus, st.runtime.memory_mb, st.runtime.disk_mb),
                );
            }
            other => return Err((400, format!("unknown runtime action '{other}'"))),
        }

        self.store.save(&mut st).map_err(|e| (500, format!("persist lifecycle state: {e}")))?;
        drop(st);
        Ok(self.status_json().await)
    }

    pub async fn environment_action(
        &self,
        who: &str,
        req: EnvironmentActionReq,
    ) -> Result<Value, (u16, String)> {
        let mut st = self.state.lock().await;
        self.sync_locked(&mut st).await;

        if let Some(strict) = req.strict_registry {
            st.runtime.strict_registry = strict;
        }
        if let Some(ref img) = req.cloud_image {
            st.platform.cloud_image = effective_cloud_image(img, st.runtime.strict_registry);
        } else {
            st.platform.cloud_image =
                effective_cloud_image(&st.platform.cloud_image, st.runtime.strict_registry);
        }
        if let Some(no_ui) = req.no_ui {
            st.platform.no_ui = no_ui;
        }
        if req.staged.is_some() {
            st.platform.staged = req.staged.clone();
        }
        if req.iam_issuer.is_some() {
            st.platform.iam_issuer = req.iam_issuer.clone();
        }
        if req.ai_base_url.is_some() {
            st.platform.ai_base_url = req.ai_base_url.clone();
        }

        match req.action.trim() {
            "up" => {
                let mode = req.mode.as_deref().unwrap_or("full").trim();
                if !matches!(mode, "full" | "ai-only" | "cloud-only") {
                    return Err((400, format!("unsupported environment mode '{mode}'")));
                }
                self.ensure_runtime_running_locked(who, &mut st).await?;
                st.platform.desired = "up".to_string();
                st.platform.mode = mode.to_string();

                if mode == "ai-only" {
                    if let Some(eng) = st.platform.services.get_mut("engine") {
                        eng.desired = "running".to_string();
                    }
                } else {
                    for name in PLATFORM_SERVICES {
                        if let Some(svc) = st.platform.services.get_mut(name) {
                            svc.desired = "running".to_string();
                        }
                    }
                }

                let outcomes = self
                    .executor
                    .reconcile_platform(&st.runtime, &st.platform)
                    .await
                    .map_err(|e| (502, format!("platform reconcile failed: {e}")))?;
                apply_service_outcomes(&mut st.platform, outcomes);

                if mode == "full" {
                    if let Some(ref raw_dir) = req.project_dir {
                        if let Some(DetectedRepo::UserApp(disc)) = discover_project(Path::new(raw_dir.trim())) {
                            let slug = upsert_discovered_app(&mut st, disc);
                            let app_clone = st.applications.get(&slug).cloned().unwrap();
                            let out = self
                                .executor
                                .reconcile_app(&app_clone, "running")
                                .await
                                .map_err(|e| (502, format!("start app '{slug}' failed: {e}")))?;
                            if let Some(app) = st.applications.get_mut(&slug) {
                                app.actual = out.actual;
                                app.pid = out.pid;
                                app.updated_at = now_unix();
                            }
                        }
                    }
                }
                self.ledger.record(who, "lifecycle.environment.up", mode);
            }
            "down" => {
                let scope = req.scope.as_deref().unwrap_or("auto").trim();
                if !matches!(scope, "auto" | "project" | "all") {
                    return Err((400, format!("unsupported down scope '{scope}'")));
                }

                let targeted_app_slug = if scope != "all" {
                    req.project_dir.as_deref().and_then(|raw_dir| {
                        let p = Path::new(raw_dir.trim());
                        find_app_by_dir(&st.applications, p).or_else(|| match discover_project(p) {
                            Some(DetectedRepo::UserApp(disc)) => Some(disc.slug),
                            _ => None,
                        })
                    })
                } else {
                    None
                };

                if let Some(slug) = targeted_app_slug {
                    if let Some(mut app_clone) = st.applications.get(&slug).cloned() {
                        app_clone.desired = "stopped".to_string();
                        let out = self
                            .executor
                            .reconcile_app(&app_clone, "stopped")
                            .await
                            .map_err(|e| (502, format!("stop app '{slug}' failed: {e}")))?;
                        if let Some(app) = st.applications.get_mut(&slug) {
                            app.desired = "stopped".to_string();
                            app.actual = out.actual;
                            app.pid = None;
                            app.updated_at = now_unix();
                        }
                    }
                    self.backoff.clear_key(&format!("app:{slug}"));
                    self.ledger.record(who, "lifecycle.environment.down", &format!("project:{slug}"));
                } else if scope == "project" {
                    return Err((400, "scope 'project' requires a user application project_dir".to_string()));
                } else {
                    st.platform.desired = "down".to_string();
                    for name in PLATFORM_SERVICES {
                        self.backoff.clear_key(&format!("service:{name}"));
                    }
                    for svc in st.platform.services.values_mut() {
                        svc.desired = "stopped".to_string();
                    }
                    let outcomes = self
                        .executor
                        .reconcile_platform(&st.runtime, &st.platform)
                        .await
                        .map_err(|e| (502, format!("platform down failed: {e}")))?;
                    apply_service_outcomes(&mut st.platform, outcomes);

                    if scope == "all" {
                        let slugs: Vec<String> = st.applications.keys().cloned().collect();
                        for slug in slugs {
                            if let Some(app_clone) = st.applications.get(&slug).cloned() {
                                let out = self
                                    .executor
                                    .reconcile_app(&app_clone, "stopped")
                                    .await
                                    .map_err(|e| (502, format!("stop app '{slug}' failed: {e}")))?;
                                if let Some(app) = st.applications.get_mut(&slug) {
                                    app.desired = "stopped".to_string();
                                    app.actual = out.actual;
                                    app.pid = None;
                                    app.updated_at = now_unix();
                                }
                            }
                            self.backoff.clear_key(&format!("app:{slug}"));
                        }
                        let sb_names: Vec<String> = st.sandboxes.keys().cloned().collect();
                        for name in sb_names {
                            if let Some(sb_clone) = st.sandboxes.get(&name).cloned() {
                                let actual = self
                                    .executor
                                    .reconcile_sandbox(&sb_clone, "stopped")
                                    .await
                                    .map_err(|e| (502, format!("stop sandbox '{name}' failed: {e}")))?;
                                if let Some(sb) = st.sandboxes.get_mut(&name) {
                                    sb.desired = "stopped".to_string();
                                    sb.actual = actual;
                                }
                            }
                        }
                    }
                    st.caches = CachePreservation::default();
                    self.ledger.record(who, "lifecycle.environment.down", scope);
                }
            }
            other => return Err((400, format!("unknown environment action '{other}'"))),
        }

        self.store.save(&mut st).map_err(|e| (500, format!("persist lifecycle state: {e}")))?;
        drop(st);
        Ok(self.status_json().await)
    }

    pub async fn service_action(
        &self,
        who: &str,
        req: ServiceActionReq,
    ) -> Result<Value, (u16, String)> {
        let mut st = self.state.lock().await;
        self.sync_locked(&mut st).await;

        let action = req.action.trim();
        if action == "list" {
            return Ok(json!({
                "ok": true,
                "platform_desired": st.platform.desired,
                "services": st.platform.services,
            }));
        }

        let raw_name = req.name.as_deref().ok_or_else(|| (400, "service 'name' is required".to_string()))?;
        let canon = canonical_service_name(raw_name)
            .ok_or_else(|| (400, format!("unknown platform service '{raw_name}'")))?;

        let (desired, force_restart) = match action {
            "start" => ("running", false),
            "stop" => ("stopped", false),
            "restart" => ("running", true),
            other => return Err((400, format!("unknown service action '{other}'"))),
        };

        self.backoff.clear_key(&format!("service:{canon}"));
        if desired == "running" {
            self.ensure_runtime_running_locked(who, &mut st).await?;
        }
        if let Some(svc) = st.platform.services.get_mut(canon) {
            svc.desired = desired.to_string();
        }
        let (actual, endpoint) = self
            .executor
            .reconcile_service(canon, desired, force_restart, &st.runtime, &st.platform)
            .await
            .map_err(|e| (502, format!("service '{canon}' {action} failed: {e}")))?;
        if let Some(svc) = st.platform.services.get_mut(canon) {
            svc.actual = actual;
            svc.endpoint = (svc.actual == "running")
                .then(|| endpoint.or_else(|| default_service_endpoint(canon)))
                .flatten();
        }
        let any_running = st.platform.services.values().any(|s| s.desired == "running");
        st.platform.desired = if any_running { "up" } else { "down" }.to_string();

        self.ledger.record(who, &format!("lifecycle.service.{action}"), canon);
        self.store.save(&mut st).map_err(|e| (500, format!("persist lifecycle state: {e}")))?;
        Ok(json!({
            "ok": true,
            "name": canon,
            "service": st.platform.services.get(canon).cloned(),
            "services": st.platform.services,
        }))
    }

    pub async fn app_action(&self, who: &str, req: AppActionReq) -> Result<Value, (u16, String)> {
        let mut st = self.state.lock().await;
        self.sync_locked(&mut st).await;

        let action = req.action.trim();
        if action == "list" {
            return Ok(json!({ "ok": true, "applications": st.applications }));
        }

        let slug = if let Some(ref raw_dir) = req.project_dir {
            let p = Path::new(raw_dir.trim());
            if let Some(existing) = find_app_by_dir(&st.applications, p) {
                existing
            } else {
                match discover_project(p) {
                    Some(DetectedRepo::UserApp(disc)) => upsert_discovered_app(&mut st, disc),
                    Some(DetectedRepo::CloudPlatform { .. }) => {
                        return Err((400, "project_dir is the Hanzo Cloud platform repo".to_string()))
                    }
                    None => return Err((400, format!("no application manifest in '{}'", p.display()))),
                }
            }
        } else if let Some(ref name) = req.name {
            resolve_app_slug(&st.applications, name).map_err(|e| (404, e))?
        } else {
            return Err((400, "application 'name' or 'project_dir' is required".to_string()));
        };

        match action {
            "start" => {
                self.backoff.clear_key(&format!("app:{slug}"));
                self.ensure_runtime_running_locked(who, &mut st).await?;
                if let Some(app) = st.applications.get_mut(&slug) {
                    app.desired = "running".to_string();
                    app.last_exit = None;
                }
                let app_clone = st.applications.get(&slug).cloned().unwrap();
                let out = self
                    .executor
                    .reconcile_app(&app_clone, "running")
                    .await
                    .map_err(|e| (502, format!("app '{slug}' start failed: {e}")))?;
                if let Some(app) = st.applications.get_mut(&slug) {
                    app.actual = out.actual;
                    app.pid = out.pid;
                    app.updated_at = now_unix();
                }
                self.ledger.record(who, "lifecycle.app.start", &slug);
            }
            "stop" => {
                self.backoff.clear_key(&format!("app:{slug}"));
                if let Some(app) = st.applications.get_mut(&slug) {
                    app.desired = "stopped".to_string();
                }
                let app_clone = st.applications.get(&slug).cloned().unwrap();
                let out = self
                    .executor
                    .reconcile_app(&app_clone, "stopped")
                    .await
                    .map_err(|e| (502, format!("app '{slug}' stop failed: {e}")))?;
                if let Some(app) = st.applications.get_mut(&slug) {
                    app.actual = out.actual;
                    app.pid = None;
                    app.updated_at = now_unix();
                }
                self.ledger.record(who, "lifecycle.app.stop", &slug);
            }
            "logs" => {
                let app_clone = st.applications.get(&slug).cloned().unwrap();
                let lines = self
                    .executor
                    .read_app_logs(&app_clone, req.tail.unwrap_or(100).min(2000))
                    .await
                    .map_err(|e| (500, format!("read logs for '{slug}': {e}")))?;
                return Ok(json!({
                    "ok": true,
                    "slug": slug,
                    "application": app_clone,
                    "logs": lines,
                }));
            }
            other => return Err((400, format!("unknown app action '{other}'"))),
        }

        self.store.save(&mut st).map_err(|e| (500, format!("persist lifecycle state: {e}")))?;
        Ok(json!({
            "ok": true,
            "slug": slug,
            "application": st.applications.get(&slug).cloned(),
            "applications": st.applications,
        }))
    }

    pub async fn sandbox_action(
        &self,
        who: &str,
        req: SandboxActionReq,
    ) -> Result<Value, (u16, String)> {
        let mut st = self.state.lock().await;
        self.sync_locked(&mut st).await;

        let action = req.action.trim();
        if action == "list" {
            return Ok(json!({ "ok": true, "sandboxes": st.sandboxes }));
        }

        let name = req
            .name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| (400, "sandbox 'name' is required".to_string()))?
            .to_string();

        match action {
            "start" => {
                self.ensure_runtime_running_locked(who, &mut st).await?;
                let default_rt = if matches!(st.runtime.components.visor.as_str(), "ready" | "available") {
                    "visor"
                } else if st.runtime.backend == "managed-vm" {
                    "vm"
                } else {
                    "runc"
                };
                let chosen_rt = req.runtime.as_deref().unwrap_or(default_rt);
                if !matches!(chosen_rt, "visor" | "vm" | "runc") {
                    return Err((400, format!("invalid sandbox runtime '{chosen_rt}'")));
                }
                let entry = st.sandboxes.entry(name.clone()).or_insert_with(|| SandboxState {
                    name: name.clone(),
                    image: req.image.clone().unwrap_or_else(|| DEFAULT_SANDBOX_IMAGE.to_string()),
                    desired: "running".to_string(),
                    actual: "stopped".to_string(),
                    runtime: chosen_rt.to_string(),
                });
                entry.desired = "running".to_string();
                if let Some(ref img) = req.image {
                    entry.image = img.clone();
                }
                if req.runtime.is_some() {
                    entry.runtime = chosen_rt.to_string();
                }
                let sb_clone = entry.clone();
                let actual = self
                    .executor
                    .reconcile_sandbox(&sb_clone, "running")
                    .await
                    .map_err(|e| (502, format!("sandbox '{name}' start failed: {e}")))?;
                if let Some(sb) = st.sandboxes.get_mut(&name) {
                    sb.actual = actual;
                }
                self.ledger.record(who, "lifecycle.sandbox.start", &name);
            }
            "stop" => {
                let Some(existing) = st.sandboxes.get_mut(&name) else {
                    return Err((404, format!("sandbox '{name}' is not registered")));
                };
                existing.desired = "stopped".to_string();
                let sb_clone = existing.clone();
                let actual = self
                    .executor
                    .reconcile_sandbox(&sb_clone, "stopped")
                    .await
                    .map_err(|e| (502, format!("sandbox '{name}' stop failed: {e}")))?;
                if let Some(sb) = st.sandboxes.get_mut(&name) {
                    sb.actual = actual;
                }
                self.ledger.record(who, "lifecycle.sandbox.stop", &name);
            }
            other => return Err((400, format!("unknown sandbox action '{other}'"))),
        }

        self.store.save(&mut st).map_err(|e| (500, format!("persist lifecycle state: {e}")))?;
        Ok(json!({
            "ok": true,
            "name": name,
            "sandbox": st.sandboxes.get(&name).cloned(),
            "sandboxes": st.sandboxes,
        }))
    }

    pub async fn reset_action(&self, who: &str, req: ResetReq) -> Result<Value, (u16, String)> {
        let mut st = self.state.lock().await;
        let _ = self.executor.drain_and_stop_runtime(&st).await;
        self.executor
            .reset(req.all, &self.store.paths)
            .await
            .map_err(|e| (500, format!("lifecycle reset failed: {e}")))?;

        let mut fresh = LifecycleState::default();
        if req.all {
            fresh.caches.image_cache_warm = false;
            fresh.caches.persistent_volumes_preserved = false;
            fresh.caches.model_weights_preserved = false;
            fresh.caches.kube_system_preserved = false;
        }
        *st = fresh;
        self.backoff.clear_all();
        self.store.save(&mut st).map_err(|e| (500, format!("persist reset state: {e}")))?;
        self.ledger.record(who, "lifecycle.reset", if req.all { "all" } else { "state-and-runtime" });
        drop(st);
        Ok(self.status_json().await)
    }
}

fn mark_all_stopped_after_drain(st: &mut LifecycleState) {
    st.runtime.actual = "stopped".to_string();
    st.runtime.components = RuntimeComponents::default();
    st.platform.desired = "down".to_string();
    for svc in st.platform.services.values_mut() {
        svc.desired = "stopped".to_string();
        svc.actual = "stopped".to_string();
        svc.endpoint = None;
    }
    for app in st.applications.values_mut() {
        app.desired = "stopped".to_string();
        app.actual = "stopped".to_string();
        app.pid = None;
    }
    for sb in st.sandboxes.values_mut() {
        sb.desired = "stopped".to_string();
        sb.actual = "stopped".to_string();
    }
}

fn apply_service_outcomes(
    platform: &mut PlatformState,
    outcomes: BTreeMap<String, (String, Option<String>)>,
) {
    for (name, (actual, ep)) in outcomes {
        if let Some(svc) = platform.services.get_mut(&name) {
            svc.actual = actual;
            svc.endpoint = (svc.actual == "running")
                .then(|| ep.or_else(|| default_service_endpoint(&name)))
                .flatten();
        }
    }
}

fn upsert_discovered_app(st: &mut LifecycleState, disc: DiscoveredApp) -> String {
    let slug = disc.slug.clone();
    let occupied: BTreeSet<u16> = st
        .applications
        .iter()
        .filter(|(k, _)| *k != &slug)
        .filter_map(|(_, a)| a.port)
        .collect();
    let existing_port = st.applications.get(&slug).and_then(|a| a.port);
    let mut disc_with_port = disc;
    disc_with_port.preferred_port = existing_port.or(disc_with_port.preferred_port);
    let (port, endpoints) = disc_with_port.assign_endpoints(&occupied);
    let dir_str = canonical_dir(&disc_with_port.project_dir).display().to_string();

    let entry = st.applications.entry(slug.clone()).or_insert_with(|| AppState {
        slug: slug.clone(),
        name: disc_with_port.name.clone(),
        project_dir: dir_str.clone(),
        namespace: disc_with_port.namespace.clone(),
        manifest_kind: disc_with_port.manifest_kind.clone(),
        desired: "running".to_string(),
        actual: "stopped".to_string(),
        command: disc_with_port.command.clone(),
        port: Some(port),
        endpoints: endpoints.clone(),
        pid: None,
        restart_count: 0,
        last_exit: None,
        updated_at: now_unix(),
    });
    entry.name = disc_with_port.name;
    entry.project_dir = dir_str;
    entry.namespace = disc_with_port.namespace;
    entry.manifest_kind = disc_with_port.manifest_kind;
    entry.command = disc_with_port.command;
    entry.desired = "running".to_string();
    entry.port = Some(port);
    entry.endpoints = endpoints;
    entry.updated_at = now_unix();
    slug
}
