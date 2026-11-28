// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! Unit and state-machine conformance tests for `/v1/lifecycle`.

use super::project::{discover_project, resolve_app_slug, DetectedRepo};
use super::reconciler::{
    AppActionReq, AppExecOutcome, AppProcessObs, BootOutcome, EnvironmentActionReq,
    LifecycleExecutor, Observation, Reconciler, ResetReq, RuntimeActionReq, SandboxActionReq,
    ServiceActionReq,
};
use super::state::{
    default_service_endpoint, AppState, BackoffPolicy, LifecycleState, PlatformState,
    RuntimeComponents, RuntimeState, SandboxState, StateStore, StorePaths, DEFAULT_CLOUD_IMAGE,
    STRICT_CLOUD_IMAGE,
};
use super::{Lifecycle, SystemExecutor};
use crate::auth::Verifier;
use crate::cluster::ClusterCore;
use crate::daemon::{Daemon, Via};
use crate::dev::Dev;
use crate::engine::Engine;
use crate::ledger::Ledger;
use crate::pins::Pins;
use crate::table::{Fut, Reply, Table};
use crate::zap::Request;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct MockWorld {
    runtime_up: bool,
    backend: String,
    cpus: u32,
    memory_mb: u64,
    disk_mb: u64,
    boot_calls: usize,
    drain_stop_calls: usize,
    resize_calls: usize,
    running_services: BTreeSet<String>,
    service_starts: BTreeMap<String, usize>,
    service_in_place_reconciles: BTreeMap<String, usize>,
    running_apps: BTreeSet<String>,
    app_crashes: BTreeMap<String, String>,
    next_pid: u32,
    running_sandboxes: BTreeSet<String>,
    app_logs: BTreeMap<String, Vec<String>>,
    image_cache_warm: bool,
    volumes_preserved: bool,
    model_weights_preserved: bool,
}

struct MockExecutor {
    world: Mutex<MockWorld>,
}

impl MockExecutor {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            world: Mutex::new(MockWorld {
                next_pid: 42000,
                image_cache_warm: true,
                volumes_preserved: true,
                model_weights_preserved: true,
                ..MockWorld::default()
            }),
        })
    }

    fn kill_app(&self, slug: &str, reason: &str) {
        let mut w = self.world.lock().unwrap();
        w.running_apps.remove(slug);
        w.app_crashes.insert(slug.to_string(), reason.to_string());
    }

    fn kill_service(&self, name: &str) {
        let mut w = self.world.lock().unwrap();
        w.running_services.remove(name);
    }

    fn snapshot(&self) -> (usize, usize, usize, usize, BTreeSet<String>, BTreeSet<String>) {
        let w = self.world.lock().unwrap();
        let cloud_starts = *w.service_starts.get("cloud").unwrap_or(&0);
        let cloud_reconciles = *w.service_in_place_reconciles.get("cloud").unwrap_or(&0);
        (
            w.boot_calls,
            w.drain_stop_calls,
            cloud_starts,
            cloud_reconciles,
            w.running_services.clone(),
            w.running_apps.clone(),
        )
    }
}

fn ready_components(backend: &str) -> RuntimeComponents {
    RuntimeComponents {
        vm: if backend == "managed-vm" { "running" } else { "native" }.to_string(),
        k3s: "ready".to_string(),
        containerd: "running".to_string(),
        visor: "ready".to_string(),
        vfs: "ready".to_string(),
        networking: "ready".to_string(),
    }
}

impl LifecycleExecutor for MockExecutor {
    fn observe<'a>(&'a self, _state: &'a LifecycleState) -> Fut<'a, Observation> {
        Box::pin(async move {
            let w = self.world.lock().unwrap();
            let mut app_processes = BTreeMap::new();
            for slug in &w.running_apps {
                app_processes.insert(
                    slug.clone(),
                    AppProcessObs { alive: true, pid: Some(w.next_pid), exit_reason: None },
                );
            }
            for (slug, reason) in &w.app_crashes {
                app_processes.insert(
                    slug.clone(),
                    AppProcessObs {
                        alive: false,
                        pid: None,
                        exit_reason: Some(reason.clone()),
                    },
                );
            }
            Observation {
                runtime_running: Some(w.runtime_up),
                components: Some(if w.runtime_up {
                    ready_components(&w.backend)
                } else {
                    RuntimeComponents::default()
                }),
                cloud_running: Some(w.running_services.contains("cloud")),
                cloud_endpoint: w.running_services.contains("cloud").then(|| "http://127.0.0.1:8000".to_string()),
                engine_running: Some(w.running_services.contains("engine")),
                engine_endpoint: w.running_services.contains("engine").then(|| "http://127.0.0.1:3690/v1/engine".to_string()),
                dev_status: "Ready",
                dev_detail: Some("native harness (hanzo-gen)".to_string()),
                app_processes,
            }
        })
    }

    fn boot_runtime<'a>(&'a self, runtime: &'a RuntimeState) -> Fut<'a, io::Result<BootOutcome>> {
        Box::pin(async move {
            let mut w = self.world.lock().unwrap();
            w.runtime_up = true;
            w.backend = runtime.backend.clone();
            w.cpus = runtime.cpus;
            w.memory_mb = runtime.memory_mb;
            w.disk_mb = runtime.disk_mb;
            w.boot_calls += 1;
            w.image_cache_warm = true;
            w.volumes_preserved = true;
            w.model_weights_preserved = true;
            Ok(BootOutcome {
                actual: "running".to_string(),
                components: ready_components(&runtime.backend),
            })
        })
    }

    fn drain_and_stop_runtime<'a>(&'a self, _state: &'a LifecycleState) -> Fut<'a, io::Result<()>> {
        Box::pin(async move {
            let mut w = self.world.lock().unwrap();
            w.drain_stop_calls += 1;
            w.runtime_up = false;
            w.running_services.clear();
            w.running_apps.clear();
            w.running_sandboxes.clear();
            Ok(())
        })
    }

    fn resize_runtime<'a>(
        &'a self,
        runtime: &'a RuntimeState,
        was_running: bool,
    ) -> Fut<'a, io::Result<BootOutcome>> {
        Box::pin(async move {
            let mut w = self.world.lock().unwrap();
            w.resize_calls += 1;
            w.cpus = runtime.cpus;
            w.memory_mb = runtime.memory_mb;
            w.disk_mb = runtime.disk_mb;
            if was_running {
                w.runtime_up = true;
                Ok(BootOutcome { actual: "running".to_string(), components: ready_components(&runtime.backend) })
            } else {
                Ok(BootOutcome { actual: "stopped".to_string(), components: RuntimeComponents::default() })
            }
        })
    }

    fn reconcile_platform<'a>(
        &'a self,
        runtime: &'a RuntimeState,
        platform: &'a PlatformState,
    ) -> Fut<'a, io::Result<BTreeMap<String, (String, Option<String>)>>> {
        Box::pin(async move {
            let mut out = BTreeMap::new();
            for (name, svc) in &platform.services {
                let res = self.reconcile_service(name, &svc.desired, false, runtime, platform).await?;
                out.insert(name.clone(), res);
            }
            Ok(out)
        })
    }

    fn reconcile_service<'a>(
        &'a self,
        name: &'a str,
        desired: &'a str,
        force_restart: bool,
        _runtime: &'a RuntimeState,
        _platform: &'a PlatformState,
    ) -> Fut<'a, io::Result<(String, Option<String>)>> {
        Box::pin(async move {
            let mut w = self.world.lock().unwrap();
            if desired == "stopped" {
                w.running_services.remove(name);
                return Ok(("stopped".to_string(), None));
            }
            if w.running_services.contains(name) && !force_restart {
                *w.service_in_place_reconciles.entry(name.to_string()).or_insert(0) += 1;
            } else {
                w.running_services.insert(name.to_string());
                *w.service_starts.entry(name.to_string()).or_insert(0) += 1;
            }
            Ok(("running".to_string(), default_service_endpoint(name)))
        })
    }

    fn reconcile_app<'a>(&'a self, app: &'a AppState, desired: &'a str) -> Fut<'a, io::Result<AppExecOutcome>> {
        Box::pin(async move {
            let mut w = self.world.lock().unwrap();
            if desired == "stopped" {
                w.running_apps.remove(&app.slug);
                w.app_crashes.remove(&app.slug);
                w.app_logs.entry(app.slug.clone()).or_default().push(format!("stopped {}", app.namespace));
                Ok(AppExecOutcome { actual: "stopped".to_string(), pid: None })
            } else {
                w.next_pid += 1;
                let pid = w.next_pid;
                w.running_apps.insert(app.slug.clone());
                w.app_crashes.remove(&app.slug);
                w.app_logs.entry(app.slug.clone()).or_default().push(format!("started {} on {:?} pid={pid}", app.namespace, app.port));
                Ok(AppExecOutcome { actual: "running".to_string(), pid: Some(pid) })
            }
        })
    }

    fn read_app_logs<'a>(&'a self, app: &'a AppState, tail: usize) -> Fut<'a, io::Result<Vec<String>>> {
        Box::pin(async move {
            let w = self.world.lock().unwrap();
            let lines = w.app_logs.get(&app.slug).cloned().unwrap_or_default();
            Ok(lines[lines.len().saturating_sub(tail)..].to_vec())
        })
    }

    fn reconcile_sandbox<'a>(&'a self, sandbox: &'a SandboxState, desired: &'a str) -> Fut<'a, io::Result<String>> {
        Box::pin(async move {
            let mut w = self.world.lock().unwrap();
            if desired == "stopped" {
                w.running_sandboxes.remove(&sandbox.name);
                Ok("stopped".to_string())
            } else {
                w.running_sandboxes.insert(sandbox.name.clone());
                Ok("running".to_string())
            }
        })
    }

    fn reset<'a>(&'a self, all: bool, paths: &'a StorePaths) -> Fut<'a, io::Result<()>> {
        Box::pin(async move {
            {
                let mut w = self.world.lock().unwrap();
                w.runtime_up = false;
                w.running_services.clear();
                w.running_apps.clear();
                w.running_sandboxes.clear();
                if all {
                    w.image_cache_warm = false;
                    w.volumes_preserved = false;
                    w.model_weights_preserved = false;
                }
            }
            if paths.up_dir.exists() {
                let _ = std::fs::remove_dir_all(&paths.up_dir);
            }
            if paths.kubeconfig_file.exists() {
                let _ = std::fs::remove_file(&paths.kubeconfig_file);
            }
            if all && paths.vm_dir.exists() {
                let _ = std::fs::remove_dir_all(&paths.vm_dir);
            }
            Ok(())
        })
    }
}

fn dead_probe_port() -> std::net::SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap()
}

fn make_harness(root: &Path) -> (Arc<Reconciler>, Arc<MockExecutor>, StorePaths) {
    let paths = StorePaths::for_root(root, dead_probe_port());
    let store = StateStore::new(paths.clone());
    let exec = MockExecutor::new();
    let ledger = Arc::new(Ledger::open(root.join("ledger")));
    (Arc::new(Reconciler::new(store, exec.clone(), ledger)), exec, paths)
}

fn create_app_repo(parent: &Path, folder: &str, manifest_file: &str, content: &str) -> String {
    let dir = parent.join(folder);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(manifest_file), content).unwrap();
    dir.display().to_string()
}

fn rt_req(action: &str) -> RuntimeActionReq {
    RuntimeActionReq { action: action.into(), ..Default::default() }
}

#[test]
fn project_discovery_distinguishes_cloud_platform_and_user_manifests() {
    let tmp = tempfile::tempdir().unwrap();

    let cloud_dir = tmp.path().join("cloud");
    std::fs::create_dir_all(cloud_dir.join("cmd/cloud-host")).unwrap();
    std::fs::write(cloud_dir.join("go.mod"), "module github.com/hanzoai/cloud\n").unwrap();
    assert!(matches!(discover_project(&cloud_dir), Some(DetectedRepo::CloudPlatform { .. })));

    let solar_dir = tmp.path().join("solar-crm");
    std::fs::create_dir_all(&solar_dir).unwrap();
    std::fs::write(solar_dir.join("hanzo.toml"), "[project]\nslug = \"solar-crm\"\nport = 3100\n").unwrap();
    match discover_project(&solar_dir) {
        Some(DetectedRepo::UserApp(app)) => {
            assert_eq!(app.slug, "solar-crm");
            assert_eq!(app.name, "Solar CRM");
            assert_eq!(app.namespace, "hanzo-app-solar-crm");
            assert_eq!(app.manifest_kind, "hanzo.toml");
            assert_eq!(app.preferred_port, Some(3100));
        }
        other => panic!("expected UserApp, got {other:?}"),
    }

    let acme_dir = tmp.path().join("acme-web");
    std::fs::create_dir_all(&acme_dir).unwrap();
    std::fs::write(acme_dir.join("package.json"), r#"{"name": "@acme/acme-web", "version": "1.0.0"}"#).unwrap();
    match discover_project(&acme_dir) {
        Some(DetectedRepo::UserApp(app)) => {
            assert_eq!((app.slug.as_str(), app.name.as_str(), app.manifest_kind.as_str()), ("acme-web", "Acme Web", "node"));
        }
        other => panic!("expected node UserApp, got {other:?}"),
    }
}

#[tokio::test]
async fn start_boots_infrastructure_only_keeping_cloud_and_apps_stopped() {
    let tmp = tempfile::tempdir().unwrap();
    let (rec, exec, paths) = make_harness(tmp.path());

    let st = rec.runtime_action("hanzo/z", rt_req("start")).await.unwrap();
    assert_eq!(st["runtime"]["desired"], "running");
    assert_eq!(st["runtime"]["actual"], "running");
    assert_eq!(st["runtime"]["components"]["k3s"], "ready");
    assert_eq!(st["runtime"]["components"]["containerd"], "running");
    assert_eq!(st["platform"]["desired"], "down");
    assert_eq!(st["platform"]["services"]["cloud"]["actual"], "stopped");
    assert_eq!(st["platform"]["services"]["engine"]["actual"], "stopped");

    let (boots, _, cloud_starts, _, running_svcs, running_apps) = exec.snapshot();
    assert_eq!((boots, cloud_starts), (1, 0));
    assert!(running_svcs.is_empty() && running_apps.is_empty());

    assert!(paths.primary_file.is_file() && paths.mirror_file.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&paths.primary_file).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(&paths.mirror_file).unwrap().permissions().mode() & 0o777, 0o600);
    }
}

#[tokio::test]
async fn up_down_multi_project_isolation_and_idempotent_cloud_reconcile() {
    let tmp = tempfile::tempdir().unwrap();
    let (rec, exec, _paths) = make_harness(tmp.path());

    let solar_dir = create_app_repo(tmp.path(), "solar-crm", "hanzo.toml", "[project]\nslug = \"solar-crm\"\nport = 15100\n");
    let acme_dir = create_app_repo(tmp.path(), "acme-web", "hanzo.yaml", "name: Acme Web\nslug: acme-web\nport: 15101\n");

    let st1 = rec
        .environment_action("hanzo/z", EnvironmentActionReq { action: "up".into(), project_dir: Some(solar_dir.clone()), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(st1["runtime"]["actual"], "running");
    assert_eq!(st1["platform"]["cloud_image"], DEFAULT_CLOUD_IMAGE);
    assert_eq!(st1["platform"]["services"]["cloud"]["actual"], "running");
    assert_eq!(st1["workloads"]["applications"]["solar-crm"]["actual"], "running");
    assert_eq!(st1["workloads"]["applications"]["solar-crm"]["namespace"], "hanzo-app-solar-crm");

    let st2 = rec
        .environment_action("hanzo/z", EnvironmentActionReq { action: "up".into(), project_dir: Some(acme_dir.clone()), ..Default::default() })
        .await
        .unwrap();
    let (boots, _, cloud_starts, cloud_in_place, _, running_apps) = exec.snapshot();
    assert_eq!((boots, cloud_starts, cloud_in_place), (1, 1, 1));
    assert!(running_apps.contains("solar-crm") && running_apps.contains("acme-web"));
    assert_ne!(st2["workloads"]["applications"]["solar-crm"]["port"], st2["workloads"]["applications"]["acme-web"]["port"]);

    // `down` inside solar-crm stops ONLY solar-crm; acme-web, Cloud, and warm k3s keep running!
    let st3 = rec
        .environment_action("hanzo/z", EnvironmentActionReq { action: "down".into(), project_dir: Some(solar_dir), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(st3["workloads"]["applications"]["solar-crm"]["actual"], "stopped");
    assert_eq!(st3["workloads"]["applications"]["acme-web"]["actual"], "running");
    assert_eq!(st3["platform"]["services"]["cloud"]["actual"], "running");
    assert_eq!(st3["runtime"]["components"]["k3s"], "ready");

    // Global `down` stops Cloud while preserving warm k3s, caches, PVCs, model weights, and unrelated `acme-web`!
    let st4 = rec
        .environment_action("hanzo/z", EnvironmentActionReq { action: "down".into(), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(st4["platform"]["desired"], "down");
    assert_eq!(st4["platform"]["services"]["cloud"]["actual"], "stopped");
    assert_eq!(st4["runtime"]["actual"], "running");
    assert_eq!(st4["runtime"]["components"]["k3s"], "ready");
    assert_eq!(st4["caches"]["image_cache_warm"], true);
    assert_eq!(st4["caches"]["persistent_volumes_preserved"], true);
    assert_eq!(st4["caches"]["model_weights_preserved"], true);
    assert_eq!(st4["workloads"]["applications"]["acme-web"]["actual"], "running");
}

#[tokio::test]
async fn ai_only_and_cloud_only_modes_and_strict_registry_upgrade() {
    let tmp = tempfile::tempdir().unwrap();
    let (rec, _exec, _paths) = make_harness(tmp.path());
    let solar_dir = create_app_repo(tmp.path(), "solar-crm", "hanzo.config.ts", "export default {\n  name: 'Solar CRM',\n  slug: 'solar-crm',\n  port: 3200,\n}\n");

    let st_ai = rec
        .environment_action("hanzo/z", EnvironmentActionReq { action: "up".into(), mode: Some("ai-only".into()), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(st_ai["runtime"]["actual"], "running");
    assert_eq!(st_ai["platform"]["services"]["engine"]["actual"], "running");
    assert_eq!(st_ai["platform"]["services"]["cloud"]["actual"], "stopped");
    assert_eq!(st_ai["platform"]["services"]["datastore"]["actual"], "stopped");

    let st_cloud = rec
        .environment_action(
            "hanzo/z",
            EnvironmentActionReq {
                action: "up".into(),
                mode: Some("cloud-only".into()),
                project_dir: Some(solar_dir),
                strict_registry: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(st_cloud["platform"]["cloud_image"], STRICT_CLOUD_IMAGE);
    assert_eq!(st_cloud["platform"]["services"]["cloud"]["actual"], "running");
    assert!(st_cloud["workloads"]["applications"].as_object().unwrap().is_empty());
}

#[tokio::test]
async fn restart_stop_and_resize_preserve_and_restore_workloads() {
    let tmp = tempfile::tempdir().unwrap();
    let (rec, exec, _paths) = make_harness(tmp.path());
    let solar_dir = create_app_repo(tmp.path(), "solar-crm", "Cargo.toml", "[package]\nname = \"solar-crm\"\nversion = \"0.1.0\"\n");

    rec.environment_action("hanzo/z", EnvironmentActionReq { action: "up".into(), project_dir: Some(solar_dir), ..Default::default() }).await.unwrap();
    rec.sandbox_action("hanzo/z", SandboxActionReq { action: "start".into(), name: Some("sbx-1".into()), image: None, runtime: Some("visor".into()) }).await.unwrap();

    // 1. Live `restart` restores Cloud + solar-crm + sbx-1
    let st_restarted = rec.runtime_action("hanzo/z", rt_req("restart")).await.unwrap();
    assert_eq!(st_restarted["runtime"]["actual"], "running");
    assert_eq!(st_restarted["platform"]["services"]["cloud"]["actual"], "running");
    assert_eq!(st_restarted["workloads"]["applications"]["solar-crm"]["actual"], "running");
    assert_eq!(st_restarted["workloads"]["sandboxes"]["sbx-1"]["actual"], "running");

    // 2. `stop` snapshots desired workloads, drains everything, stops k3s/VM, keeps hanzod alive
    let st_stopped = rec.runtime_action("hanzo/z", rt_req("stop")).await.unwrap();
    assert_eq!(st_stopped["daemon"]["running"], true);
    assert_eq!(st_stopped["runtime"]["actual"], "stopped");
    assert_eq!(st_stopped["runtime"]["components"]["k3s"], "stopped");
    assert_eq!(st_stopped["platform"]["services"]["cloud"]["actual"], "stopped");
    assert_eq!(st_stopped["workloads"]["applications"]["solar-crm"]["actual"], "stopped");
    assert!(st_stopped["runtime"]["saved_desired_snapshot"].is_object());
    assert!(exec.snapshot().4.is_empty() && exec.snapshot().5.is_empty());

    // 3. `restart` after `stop` restores saved_desired_snapshot
    let st_restored = rec
        .runtime_action("hanzo/z", RuntimeActionReq { action: "restart".into(), backend: Some("managed-vm".into()), cpus: Some(8), memory_mb: Some(8192), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(st_restored["runtime"]["cpus"], 8);
    assert_eq!(st_restored["platform"]["services"]["cloud"]["actual"], "running");
    assert_eq!(st_restored["workloads"]["applications"]["solar-crm"]["actual"], "running");

    // 4. `resize` updates VM spec while preserving running workloads
    let st_resized = rec
        .runtime_action("hanzo/z", RuntimeActionReq { action: "resize".into(), cpus: Some(12), memory_mb: Some(16384), disk_mb: Some(65536), ..Default::default() })
        .await
        .unwrap();
    assert_eq!((st_resized["runtime"]["cpus"].as_u64(), st_resized["runtime"]["memory_mb"].as_u64(), st_resized["runtime"]["disk_mb"].as_u64()), (Some(12), Some(16384), Some(65536)));
    assert_eq!(st_resized["workloads"]["applications"]["solar-crm"]["actual"], "running");
}

#[tokio::test]
async fn service_app_prefix_sandbox_ps_doctor_reset_external_sync_and_http_probes() {
    let tmp = tempfile::tempdir().unwrap();
    let (rec, _exec, paths) = make_harness(tmp.path());
    let solar_dir = create_app_repo(tmp.path(), "solar-crm", "hanzo.toml", "[project]\nname = \"Solar CRM\"\nslug = \"solar-crm\"\n");

    rec.app_action("hanzo/z", AppActionReq { action: "start".into(), project_dir: Some(solar_dir), ..Default::default() }).await.unwrap();

    // Prefix match `"solar"` -> `"solar-crm"`
    let stopped = rec.app_action("hanzo/z", AppActionReq { action: "stop".into(), name: Some("solar".into()), ..Default::default() }).await.unwrap();
    assert_eq!((stopped["slug"].as_str(), stopped["application"]["actual"].as_str()), (Some("solar-crm"), Some("stopped")));

    let started = rec.app_action("hanzo/z", AppActionReq { action: "start".into(), name: Some("solar".into()), ..Default::default() }).await.unwrap();
    assert_eq!(started["application"]["actual"], "running");

    let logs = rec.app_action("hanzo/z", AppActionReq { action: "logs".into(), name: Some("solar".into()), tail: Some(10), ..Default::default() }).await.unwrap();
    assert!(logs["logs"].as_array().unwrap().len() >= 2);

    let svc_start = rec.service_action("hanzo/z", ServiceActionReq { action: "start".into(), name: Some("analytics".into()) }).await.unwrap();
    assert_eq!(svc_start["service"]["actual"], "running");
    let svc_stop = rec.service_action("hanzo/z", ServiceActionReq { action: "stop".into(), name: Some("analytics".into()) }).await.unwrap();
    assert_eq!(svc_stop["service"]["actual"], "stopped");

    let ps = rec.ps_json().await;
    let rows = ps["rows"].as_array().unwrap();
    let names: Vec<&str> = rows.iter().filter_map(|r| r["component"].as_str()).collect();
    for expected in ["Hanzo Runtime", "k3s", "Hanzo Cloud", "Hanzo Engine", "Solar CRM", "Analytics", "Insights", "Hanzo Dev"] {
        assert!(names.contains(&expected), "missing '{expected}' in {names:?}");
    }
    assert_eq!(rows.iter().find(|r| r["component"] == "k3s").unwrap()["status"], "Ready");
    assert_eq!(rows.iter().find(|r| r["component"] == "Solar CRM").unwrap()["status"], "Running");

    let doc = rec.doctor_json().await;
    assert_eq!((doc["ok"].as_bool(), doc["healthy"].as_bool(), doc["drift_detected"].as_bool()), (Some(true), Some(true), Some(false)));

    // Verify Daemon local_probe allows GET /v1/lifecycle{,/status,/ps,/doctor} over HTTP without bearer!
    let sub: Arc<dyn crate::table::Subsystem> = Arc::new(Lifecycle::with_reconciler(rec.clone()));
    let daemon = Daemon { table: Arc::new(Table::new(vec![sub])), verifier: Arc::new(Verifier::new("https://hanzo.ai".into())) };
    for probe_path in ["/v1/lifecycle", "/v1/lifecycle/status", "/v1/lifecycle/ps", "/v1/lifecycle/doctor"] {
        let (reply, _) = daemon.handle(Via::Http, Request { method: "GET".into(), target: probe_path.into(), headers: vec![], body: vec![] }).await;
        match reply {
            Reply::Whole(resp) => {
                assert_eq!(resp.status, 200);
                let v: Value = serde_json::from_slice(&resp.body).unwrap();
                assert_eq!(v["ok"], true);
            }
            _ => panic!("expected whole reply for {probe_path}"),
        }
    }

    // Test `reset --all` removes ~/.hanzo/up, ~/.kube/hanzo.yaml, and ~/.hanzo/vm
    std::fs::create_dir_all(&paths.up_dir).unwrap();
    std::fs::write(paths.up_dir.join("state"), "ready").unwrap();
    std::fs::create_dir_all(paths.kubeconfig_file.parent().unwrap()).unwrap();
    std::fs::write(&paths.kubeconfig_file, "apiVersion: v1\n").unwrap();
    std::fs::create_dir_all(&paths.vm_dir).unwrap();
    std::fs::write(paths.vm_dir.join("checkpoint.bin"), b"ckpt").unwrap();

    let after_reset = rec.reset_action("hanzo/z", ResetReq { all: true }).await.unwrap();
    assert_eq!(after_reset["runtime"]["actual"], "stopped");
    assert!(!paths.up_dir.exists() && !paths.kubeconfig_file.exists() && !paths.vm_dir.exists());

    // External kubeconfig + live k3s listener synchronization check
    let k3s_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let ext_paths = StorePaths::for_root(&tmp.path().join("ext"), k3s_listener.local_addr().unwrap());
    std::fs::create_dir_all(ext_paths.kubeconfig_file.parent().unwrap()).unwrap();
    std::fs::write(&ext_paths.kubeconfig_file, "apiVersion: v1\n").unwrap();
    let ext_rec = Reconciler::new(StateStore::new(ext_paths), MockExecutor::new(), Arc::new(Ledger::open(tmp.path().join("ext-ledger"))));
    let ext_st = ext_rec.status_json().await;
    assert_eq!(ext_st["runtime"]["actual"], "running");
    assert_eq!(ext_st["runtime"]["components"]["k3s"], "ready");

    // Prefix ambiguity check
    let mut map = BTreeMap::new();
    for s in ["solar-crm", "solar-erp"] {
        map.insert(s.to_string(), AppState {
            slug: s.to_string(), name: s.to_string(), project_dir: "/tmp".into(),
            namespace: format!("hanzo-app-{s}"), manifest_kind: "hanzo.toml".into(),
            desired: "stopped".into(), actual: "stopped".into(), command: Vec::new(), port: None, endpoints: vec![], pid: None, restart_count: 0, last_exit: None, updated_at: 0,
        });
    }
    assert!(resolve_app_slug(&map, "solar").is_err());
    assert_eq!(resolve_app_slug(&map, "solar-crm").unwrap(), "solar-crm");
}

#[tokio::test]
async fn killed_workload_reconciles_with_bounded_backoff() {
    let policy = BackoffPolicy {
        initial: Duration::from_millis(150),
        max: Duration::from_millis(360),
        reset_after: Duration::from_millis(550),
    };
    assert_eq!(policy.delay_for(0), Duration::from_millis(150));
    assert_eq!(policy.delay_for(1), Duration::from_millis(300));
    assert_eq!(policy.delay_for(2), Duration::from_millis(360));
    assert_eq!(policy.delay_for(5), Duration::from_millis(360));

    let tmp = tempfile::tempdir().unwrap();
    let paths = StorePaths::for_root(tmp.path(), dead_probe_port());
    let exec = MockExecutor::new();
    let ledger = Arc::new(Ledger::open(tmp.path().join("ledger")));
    let rec = Arc::new(Reconciler::with_backoff(
        StateStore::new(paths),
        exec.clone(),
        ledger,
        policy,
    ));

    let solar_dir = create_app_repo(
        tmp.path(),
        "solar-crm",
        "hanzo.toml",
        "[project]\nname = \"Solar CRM\"\nslug = \"solar-crm\"\n",
    );
    let started = rec
        .environment_action(
            "hanzo/z",
            EnvironmentActionReq {
                action: "up".into(),
                project_dir: Some(solar_dir),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let pid0 = started["workloads"]["applications"]["solar-crm"]["pid"].as_u64();
    assert!(pid0.is_some());

    // Simulate unexpected app process crash (`desired == "running"`, child gone)
    exec.kill_app("solar-crm", "SIGKILL (9)");
    let r1 = rec.reconcile_once("supervisor").await.unwrap();
    assert!(r1["recovered"].as_array().unwrap().is_empty());
    assert_eq!(r1["deferred"].as_array().unwrap()[0]["target"], "app:solar-crm");
    assert_eq!(r1["failures"]["app:solar-crm"], 1);

    // Still inside `initial` (150ms) backoff window -> restart stays deferred
    tokio::time::sleep(Duration::from_millis(15)).await;
    let r2 = rec.reconcile_once("supervisor").await.unwrap();
    assert!(r2["recovered"].as_array().unwrap().is_empty());

    let st_crashed = rec.status_json().await;
    assert_eq!(st_crashed["workloads"]["applications"]["solar-crm"]["actual"], "crashed");
    assert_eq!(st_crashed["workloads"]["applications"]["solar-crm"]["last_exit"], "SIGKILL (9)");
    let doc = rec.doctor_json().await;
    assert_eq!(doc["drift_detected"], true);
    let ps = rec.ps_json().await;
    assert_eq!(
        ps["rows"].as_array().unwrap().iter().find(|r| r["component"] == "Solar CRM").unwrap()["status"],
        "Crashed"
    );

    // Past 150ms backoff window -> app reconciles back to running with new PID
    tokio::time::sleep(Duration::from_millis(165)).await;
    let r3 = rec.reconcile_once("supervisor").await.unwrap();
    assert_eq!(r3["recovered"], serde_json::json!(["app:solar-crm"]));
    let st_rec1 = rec.status_json().await;
    assert_eq!(st_rec1["workloads"]["applications"]["solar-crm"]["actual"], "running");
    assert_eq!(st_rec1["workloads"]["applications"]["solar-crm"]["restart_count"], 1);
    let pid1 = st_rec1["workloads"]["applications"]["solar-crm"]["pid"].as_u64();
    assert!(pid1.is_some() && pid1 != pid0);

    // Second crash doubles backoff to 300ms
    exec.kill_app("solar-crm", "exit code 137");
    let r4 = rec.reconcile_once("supervisor").await.unwrap();
    assert_eq!(r4["failures"]["app:solar-crm"], 2);
    tokio::time::sleep(Duration::from_millis(20)).await;
    let r5 = rec.reconcile_once("supervisor").await.unwrap();
    assert!(r5["recovered"].as_array().unwrap().is_empty());
    tokio::time::sleep(Duration::from_millis(300)).await;
    let r6 = rec.reconcile_once("supervisor").await.unwrap();
    assert_eq!(r6["recovered"], serde_json::json!(["app:solar-crm"]));

    // Repeated crashes cap strictly at `max = 360ms`
    for expected_failures in 3u32..=4u32 {
        exec.kill_app("solar-crm", "SIGTERM");
        let arm = rec.reconcile_once("supervisor").await.unwrap();
        assert_eq!(arm["failures"]["app:solar-crm"], expected_failures);
        let retry_ms = arm["deferred"][0]["retry_in_ms"].as_u64().unwrap();
        assert!(retry_ms <= 360, "backoff exceeded cap: {retry_ms}ms");
        tokio::time::sleep(Duration::from_millis(380)).await;
        let rec_done = rec.reconcile_once("supervisor").await.unwrap();
        assert_eq!(rec_done["recovered"], serde_json::json!(["app:solar-crm"]));
    }

    // Remaining healthy >= `reset_after` (550ms) resets consecutive failure count back to 0
    tokio::time::sleep(Duration::from_millis(580)).await;
    let after_reset = rec.reconcile_once("supervisor").await.unwrap();
    assert_eq!(after_reset["failures"]["app:solar-crm"], 0);

    // Also verify killed platform service recovers via HTTP/Socket `POST /v1/lifecycle/reconcile`
    exec.kill_service("cloud");
    let sub: Arc<dyn crate::table::Subsystem> = Arc::new(Lifecycle::with_reconciler(rec.clone()));
    let daemon = Daemon { table: Arc::new(Table::new(vec![sub])), verifier: Arc::new(Verifier::new("https://hanzo.ai".into())) };
    let _ = daemon.handle(Via::Socket, Request { method: "POST".into(), target: "/v1/lifecycle/reconcile".into(), headers: vec![], body: vec![] }).await;
    tokio::time::sleep(Duration::from_millis(175)).await;
    let (reply, _) = daemon.handle(Via::Socket, Request { method: "POST".into(), target: "/v1/lifecycle/reconcile".into(), headers: vec![], body: vec![] }).await;
    match reply {
        Reply::Whole(resp) => {
            assert_eq!(resp.status, 200);
            let v: Value = serde_json::from_slice(&resp.body).unwrap();
            assert!(v["recovered"].as_array().unwrap().iter().any(|x| x == "service:cloud"));
        }
        _ => panic!("expected whole reply from POST /v1/lifecycle/reconcile"),
    }

    // Explicit stop clears desired state so reconcile does NOT restart intentionally stopped workloads
    rec.app_action("hanzo/z", AppActionReq { action: "stop".into(), name: Some("solar-crm".into()), ..Default::default() }).await.unwrap();
    let idle = rec.reconcile_once("supervisor").await.unwrap();
    assert!(idle["recovered"].as_array().unwrap().is_empty());
    let final_st = rec.status_json().await;
    assert_eq!(final_st["workloads"]["applications"]["solar-crm"]["actual"], "stopped");
}

#[tokio::test]
async fn system_executor_recovers_killed_os_child_process() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = StorePaths::for_root(tmp.path(), dead_probe_port());
    let ledger = Arc::new(Ledger::open(tmp.path().join("ledger")));
    let pins = Arc::new(Pins::load(&tmp.path().join("pins.json")));
    let upstream: crate::cloud::Upstream = Arc::new(Mutex::new(None));
    let engine = Arc::new(Engine::new(pins));
    let cluster = ClusterCore::new(ledger.clone(), upstream.clone());
    let dev = Arc::new(Dev::new(ledger.clone(), Some(engine.clone())));
    let sys_exec = SystemExecutor::new(cluster, engine, upstream, dev, paths.clone());

    let policy = BackoffPolicy {
        initial: Duration::from_millis(80),
        max: Duration::from_millis(240),
        reset_after: Duration::from_secs(5),
    };
    let rec = Arc::new(Reconciler::with_backoff(
        StateStore::new(paths.clone()),
        sys_exec.clone(),
        ledger,
        policy,
    ));

    let cmd_toml = if cfg!(target_os = "windows") {
        "[project]\nname = \"Sleeper App\"\nslug = \"sleeper-app\"\ncommand = [\"cmd.exe\", \"/C\", \"ping\", \"-n\", \"30\", \"127.0.0.1\"]\n"
    } else {
        "[project]\nname = \"Sleeper App\"\nslug = \"sleeper-app\"\ncommand = [\"sh\", \"-c\", \"sleep 30\"]\n"
    };
    let app_dir = create_app_repo(tmp.path(), "sleeper-app", "hanzo.toml", cmd_toml);

    let started = rec
        .app_action(
            "hanzo/z",
            AppActionReq { action: "start".into(), project_dir: Some(app_dir), ..Default::default() },
        )
        .await
        .unwrap();
    let pid1 = started["application"]["pid"].as_u64().expect("OS child PID");
    assert_eq!(started["application"]["actual"], "running");

    // Kill the real OS child process unexpectedly
    assert!(sys_exec.kill_app_child("sleeper-app").await);

    // First reconcile observes exit, marks crashed, and arms bounded backoff
    let arm = rec.reconcile_once("supervisor").await.unwrap();
    assert_eq!(arm["deferred"].as_array().unwrap()[0]["target"], "app:sleeper-app");
    let crashed = rec.status_json().await;
    assert_eq!(crashed["workloads"]["applications"]["sleeper-app"]["actual"], "crashed");
    assert!(crashed["workloads"]["applications"]["sleeper-app"]["pid"].is_null());

    // After backoff expires, real OS child respawns under a fresh PID
    tokio::time::sleep(Duration::from_millis(115)).await;
    let recovered = rec.reconcile_once("supervisor").await.unwrap();
    assert_eq!(recovered["recovered"], serde_json::json!(["app:sleeper-app"]));
    let post = rec.status_json().await;
    let pid2 = post["workloads"]["applications"]["sleeper-app"]["pid"].as_u64().expect("respawned OS child PID");
    assert_ne!(pid1, pid2);
    assert_eq!(post["workloads"]["applications"]["sleeper-app"]["restart_count"], 1);
    assert_eq!(post["workloads"]["applications"]["sleeper-app"]["actual"], "running");

    let logs = rec
        .app_action("hanzo/z", AppActionReq { action: "logs".into(), name: Some("sleeper-app".into()), tail: Some(20), ..Default::default() })
        .await
        .unwrap();
    let log_lines: Vec<&str> = logs["logs"].as_array().unwrap().iter().filter_map(|v| v.as_str()).collect();
    assert!(log_lines.iter().any(|l| l.contains("exited unexpectedly")));

    // Cleanly terminate respawned child process
    rec.app_action("hanzo/z", AppActionReq { action: "stop".into(), name: Some("sleeper-app".into()), ..Default::default() }).await.unwrap();
}
