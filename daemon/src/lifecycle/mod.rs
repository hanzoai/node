// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! `/v1/lifecycle`: Canonical 3-Layer Hanzo Lifecycle Controller & Desired-State Reconciler.
//!
//! Separates and reconciles:
//! 1. Infrastructure (`Hanzo Runtime` — `k3s`, `containerd`, `hanzo-vm`, visor, VFS, networking)
//! 2. Platform (`Hanzo Cloud` — OSS Cloud, IAM, Engine, Datastore, Analytics, Insights, Storage, Commerce, Search)
//! 3. Workloads / Applications (`hanzo-app-<slug>` multi-project isolation + Sandboxes + Dev)

pub mod project;
pub mod reconciler;
pub mod state;
#[cfg(test)]
mod tests;

use crate::cloud::Upstream;
use crate::cluster::ClusterCore;
use crate::dev::Dev;
use crate::engine::Engine;
use crate::ledger::Ledger;
use crate::proc;
use crate::table::{Fut, Reply, Subsystem};
use crate::zap::{Request, Response};
use reconciler::{
    AppActionReq, AppExecOutcome, BootOutcome, EnvironmentActionReq, LifecycleExecutor,
    Observation, Reconciler, ResetReq, RuntimeActionReq, SandboxActionReq, ServiceActionReq,
};
use serde::de::DeserializeOwned;
use serde_json::json;
use state::{
    default_service_endpoint, write_atomic_0600, AppState, LifecycleState, PlatformState,
    RuntimeComponents, RuntimeState, SandboxState, StateStore, StorePaths,
};
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Child;
use tokio::sync::Mutex;

pub struct SystemExecutor {
    cluster: Arc<ClusterCore>,
    engine: Arc<Engine>,
    upstream: Upstream,
    dev: Arc<Dev>,
    paths: StorePaths,
    app_children: Mutex<BTreeMap<String, Child>>,
}

impl SystemExecutor {
    pub fn new(
        cluster: Arc<ClusterCore>,
        engine: Arc<Engine>,
        upstream: Upstream,
        dev: Arc<Dev>,
        paths: StorePaths,
    ) -> Arc<Self> {
        Arc::new(Self {
            cluster,
            engine,
            upstream,
            dev,
            paths,
            app_children: Mutex::new(BTreeMap::new()),
        })
    }

    fn find_host_bin(name: &str) -> Option<PathBuf> {
        if let Some(p) = proc::find(name) {
            return Some(p);
        }
        let home = dirs::home_dir().unwrap_or_default();
        for cand in [
            home.join(".local").join("bin").join(name),
            PathBuf::from("/usr/local/bin").join(name),
            PathBuf::from("/usr/bin").join(name),
            PathBuf::from("/opt/homebrew/bin").join(name),
        ] {
            if cand.is_file() {
                return Some(cand);
            }
        }
        None
    }

    fn containerd_socket_present() -> bool {
        [
            "/run/k3s/containerd/containerd.sock",
            "/run/containerd/containerd.sock",
        ]
        .iter()
        .any(|p| Path::new(p).exists())
    }

    fn visor_status() -> String {
        if crate::research::discover_hanzo_vm().is_some() {
            "available".to_string()
        } else {
            "stopped".to_string()
        }
    }

    fn append_app_log(&self, slug: &str, line: &str) {
        let _ = std::fs::create_dir_all(&self.paths.logs_dir);
        let log_file = self.paths.logs_dir.join(format!("{slug}.log"));
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_file)
        {
            let _ = writeln!(f, "[{}] {line}", state::now_unix());
        }
    }

    async fn kubectl_if_ready(&self, args: &[&str]) -> bool {
        let ext = self.paths.inspect_external();
        if !ext.kubeconfig_exists || !ext.k3s_tcp_reachable {
            return false;
        }
        let Some(kubectl) = Self::find_host_bin("kubectl") else {
            return false;
        };
        let kube = self.paths.kubeconfig_file.display().to_string();
        let res = tokio::time::timeout(
            Duration::from_secs(8),
            proc::bg_tokio_cmd(&kubectl)
                .arg("--kubeconfig")
                .arg(&kube)
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .status(),
        )
        .await;
        matches!(res, Ok(Ok(st)) if st.success())
    }
}

impl LifecycleExecutor for SystemExecutor {
    fn observe<'a>(&'a self, _state: &'a LifecycleState) -> Fut<'a, Observation> {
        Box::pin(async move {
            let ext = self.paths.inspect_external();
            let runtime_running = if ext.vm_alive
                || (ext.kubeconfig_exists && ext.k3s_tcp_reachable)
                || self.cluster.is_ready()
            {
                Some(true)
            } else {
                None
            };

            let components = runtime_running.map(|_| RuntimeComponents {
                vm: if ext.vm_alive {
                    "running".to_string()
                } else {
                    "native".to_string()
                },
                k3s: if ext.k3s_tcp_reachable || self.cluster.is_ready() {
                    "ready".to_string()
                } else {
                    "starting".to_string()
                },
                containerd: "running".to_string(),
                visor: Self::visor_status(),
                vfs: "ready".to_string(),
                networking: "ready".to_string(),
            });

            let upstream_addr = *self.upstream.lock().unwrap();
            let cloud_sock = crate::paths::cloud_socket().exists();
            let (cloud_running, cloud_endpoint) = match upstream_addr {
                Some(addr) => (Some(true), Some(format!("http://{addr}"))),
                None if cloud_sock => (Some(true), default_service_endpoint("cloud")),
                None => (None, None),
            };

            let eng_port = self.engine.port();
            let (engine_running, engine_endpoint) = match eng_port {
                Some(p) => (Some(true), Some(format!("http://127.0.0.1:{p}/v1"))),
                None => (None, None),
            };

            let active_dev = self.dev.active_sessions_count();
            let (dev_status, dev_detail) = if active_dev > 0 {
                (
                    "Running",
                    Some(format!(
                        "{active_dev} active session(s) ({})",
                        crate::dev_native::HARNESS_NAME
                    )),
                )
            } else {
                (
                    "Ready",
                    Some(format!("native harness ({})", crate::dev_native::HARNESS_NAME)),
                )
            };

            Observation {
                runtime_running,
                components,
                cloud_running,
                cloud_endpoint,
                engine_running,
                engine_endpoint,
                dev_status,
                dev_detail,
            }
        })
    }

    fn boot_runtime<'a>(&'a self, runtime: &'a RuntimeState) -> Fut<'a, io::Result<BootOutcome>> {
        Box::pin(async move {
            let ext = self.paths.inspect_external();
            if ext.vm_alive || (ext.kubeconfig_exists && ext.k3s_tcp_reachable) {
                return Ok(BootOutcome {
                    actual: "running".to_string(),
                    components: RuntimeComponents {
                        vm: if ext.vm_alive {
                            "running".to_string()
                        } else {
                            "native".to_string()
                        },
                        k3s: "ready".to_string(),
                        containerd: "running".to_string(),
                        visor: Self::visor_status(),
                        vfs: "ready".to_string(),
                        networking: "ready".to_string(),
                    },
                });
            }

            #[cfg(unix)]
            let is_root = unsafe { libc::geteuid() == 0 };
            #[cfg(not(unix))]
            let is_root = false;

            if runtime.backend == "native-linux" && is_root {
                if let Some(k3s_bin) = Self::find_host_bin("k3s") {
                    if proc::owned_by_us_alone(&k3s_bin).is_ok() {
                        let k3s_dir = crate::paths::data().join("k3s");
                        let _ = std::fs::create_dir_all(&k3s_dir);
                        if let Some(parent) = self.paths.kubeconfig_file.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = proc::bg_tokio_cmd(&k3s_bin)
                            .args([
                                "server",
                                "--data-dir",
                                &k3s_dir.display().to_string(),
                                "--write-kubeconfig",
                                &self.paths.kubeconfig_file.display().to_string(),
                                "--https-listen-port",
                                "6443",
                            ])
                            .stdin(Stdio::null())
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .spawn();
                    }
                }
            }

            let recheck = self.paths.inspect_external();
            let containerd_up = Self::containerd_socket_present() || recheck.k3s_tcp_reachable;
            let k3s_ready = recheck.k3s_tcp_reachable && recheck.kubeconfig_exists;

            Ok(BootOutcome {
                actual: if k3s_ready || containerd_up {
                    "running".to_string()
                } else {
                    "degraded".to_string()
                },
                components: RuntimeComponents {
                    vm: if runtime.backend == "managed-vm" {
                        if recheck.vm_alive {
                            "running".to_string()
                        } else {
                            "stopped".to_string()
                        }
                    } else {
                        "native".to_string()
                    },
                    k3s: if k3s_ready {
                        "ready".to_string()
                    } else {
                        "stopped".to_string()
                    },
                    containerd: if containerd_up {
                        "running".to_string()
                    } else {
                        "stopped".to_string()
                    },
                    visor: Self::visor_status(),
                    vfs: "ready".to_string(),
                    networking: "ready".to_string(),
                },
            })
        })
    }

    fn drain_and_stop_runtime<'a>(&'a self, state: &'a LifecycleState) -> Fut<'a, io::Result<()>> {
        Box::pin(async move {
            let drained = {
                let mut apps = self.app_children.lock().await;
                std::mem::take(&mut *apps)
            };
            for (slug, mut child) in drained {
                proc::terminate(&mut child, Duration::from_secs(5)).await;
                self.append_app_log(&slug, "drained by runtime stop");
            }

            for app in state.applications.values() {
                let _ = self
                    .kubectl_if_ready(&[
                        "-n",
                        &app.namespace,
                        "scale",
                        "deploy",
                        "--all",
                        "--replicas=0",
                    ])
                    .await;
            }
            let _ = self
                .kubectl_if_ready(&["-n", "hanzo", "scale", "deploy", "--all", "--replicas=0"])
                .await;

            self.engine.stop().await;
            let _ = self.cluster.down("lifecycle").await;

            #[cfg(unix)]
            for pid_name in ["vm.pid", "supervisor.pid"] {
                let pf = self.paths.up_dir.join(pid_name);
                if let Ok(txt) = std::fs::read_to_string(&pf) {
                    if let Ok(pid) = txt.trim().parse::<i32>() {
                        if pid > 1 {
                            unsafe {
                                libc::kill(pid, libc::SIGTERM);
                            }
                        }
                    }
                }
            }
            Ok(())
        })
    }

    fn resize_runtime<'a>(
        &'a self,
        runtime: &'a RuntimeState,
        was_running: bool,
    ) -> Fut<'a, io::Result<BootOutcome>> {
        Box::pin(async move {
            let spec_doc = json!({
                "backend": runtime.backend,
                "cpus": runtime.cpus,
                "memory_mb": runtime.memory_mb,
                "disk_mb": runtime.disk_mb,
                "updated_at": state::now_unix(),
            });
            let bytes = serde_json::to_vec_pretty(&spec_doc).map_err(io::Error::other)?;
            write_atomic_0600(&self.paths.vm_dir.join("spec.json"), &bytes)?;
            if was_running {
                self.boot_runtime(runtime).await
            } else {
                Ok(BootOutcome {
                    actual: "stopped".to_string(),
                    components: RuntimeComponents::default(),
                })
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
                let res = self
                    .reconcile_service(name, &svc.desired, false, runtime, platform)
                    .await?;
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
        platform: &'a PlatformState,
    ) -> Fut<'a, io::Result<(String, Option<String>)>> {
        Box::pin(async move {
            if desired == "stopped" {
                if name == "engine" {
                    self.engine.stop().await;
                } else {
                    let _ = self
                        .kubectl_if_ready(&[
                            "-n",
                            "hanzo",
                            "scale",
                            "deploy",
                            name,
                            "--replicas=0",
                        ])
                        .await;
                    if name == "cloud" {
                        *self.upstream.lock().unwrap() = None;
                    }
                }
                return Ok(("stopped".to_string(), None));
            }

            if name == "engine" {
                if force_restart {
                    self.engine.stop().await;
                }
                if self.engine.port().is_none() {
                    let _ = self.engine.start().await;
                }
                let ep = self
                    .engine
                    .port()
                    .map(|p| format!("http://127.0.0.1:{p}/v1"))
                    .or_else(|| default_service_endpoint("engine"));
                return Ok(("running".to_string(), ep));
            }

            let _ = self.kubectl_if_ready(&["create", "namespace", "hanzo"]).await;
            if force_restart {
                let _ = self
                    .kubectl_if_ready(&["-n", "hanzo", "rollout", "restart", "deploy", name])
                    .await;
            } else if name == "cloud" {
                let set_img = format!("cloud={}", platform.cloud_image);
                let _ = self
                    .kubectl_if_ready(&["-n", "hanzo", "set", "image", "deploy/cloud", &set_img])
                    .await;
            }
            let ep = if name == "cloud" {
                self.upstream
                    .lock()
                    .unwrap()
                    .map(|a| format!("http://{a}"))
                    .or_else(|| default_service_endpoint("cloud"))
            } else {
                default_service_endpoint(name)
            };
            Ok(("running".to_string(), ep))
        })
    }

    fn reconcile_app<'a>(
        &'a self,
        app: &'a AppState,
        desired: &'a str,
    ) -> Fut<'a, io::Result<AppExecOutcome>> {
        Box::pin(async move {
            if desired == "stopped" {
                if let Some(mut child) = self.app_children.lock().await.remove(&app.slug) {
                    proc::terminate(&mut child, Duration::from_secs(5)).await;
                }
                let _ = self
                    .kubectl_if_ready(&[
                        "-n",
                        &app.namespace,
                        "scale",
                        "deploy",
                        "--all",
                        "--replicas=0",
                    ])
                    .await;
                self.append_app_log(
                    &app.slug,
                    &format!("stopped application in namespace {}", app.namespace),
                );
                return Ok(AppExecOutcome {
                    actual: "stopped".to_string(),
                    pid: None,
                });
            }

            let _ = self
                .kubectl_if_ready(&["create", "namespace", &app.namespace])
                .await;
            self.append_app_log(
                &app.slug,
                &format!(
                    "reconciled running in namespace {} ({})",
                    app.namespace,
                    app.endpoints.first().map(String::as_str).unwrap_or("-")
                ),
            );
            Ok(AppExecOutcome {
                actual: "running".to_string(),
                pid: app.pid,
            })
        })
    }

    fn read_app_logs<'a>(
        &'a self,
        app: &'a AppState,
        tail: usize,
    ) -> Fut<'a, io::Result<Vec<String>>> {
        Box::pin(async move {
            let log_file = self.paths.logs_dir.join(format!("{}.log", app.slug));
            let Ok(raw) = std::fs::read_to_string(log_file) else {
                return Ok(Vec::new());
            };
            let all: Vec<String> = raw.lines().map(str::to_string).collect();
            let start = all.len().saturating_sub(tail);
            Ok(all[start..].to_vec())
        })
    }

    fn reconcile_sandbox<'a>(
        &'a self,
        sandbox: &'a SandboxState,
        desired: &'a str,
    ) -> Fut<'a, io::Result<String>> {
        Box::pin(async move {
            if desired == "stopped" {
                let _ = self
                    .kubectl_if_ready(&[
                        "-n",
                        "hanzo-sandboxes",
                        "delete",
                        "pod",
                        &sandbox.name,
                        "--ignore-not-found=true",
                    ])
                    .await;
                Ok("stopped".to_string())
            } else {
                let _ = self
                    .kubectl_if_ready(&["create", "namespace", "hanzo-sandboxes"])
                    .await;
                Ok("running".to_string())
            }
        })
    }

    fn reset<'a>(&'a self, all: bool, paths: &'a StorePaths) -> Fut<'a, io::Result<()>> {
        Box::pin(async move {
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

pub struct Lifecycle {
    pub reconciler: Arc<Reconciler>,
}

impl Lifecycle {
    pub fn new(
        cluster: Arc<ClusterCore>,
        engine: Arc<Engine>,
        upstream: Upstream,
        dev: Arc<Dev>,
        ledger: Arc<Ledger>,
    ) -> Self {
        let store_paths = StorePaths::system();
        let executor = SystemExecutor::new(cluster, engine, upstream, dev, store_paths.clone());
        let store = StateStore::new(store_paths);
        Self {
            reconciler: Arc::new(Reconciler::new(store, executor, ledger)),
        }
    }

    #[cfg(test)]
    pub fn with_reconciler(reconciler: Arc<Reconciler>) -> Self {
        Self { reconciler }
    }

    async fn handle(&self, who: &str, req: Request) -> Response {
        let path = req.path().to_string();
        let sub = path
            .strip_prefix("/v1/lifecycle")
            .unwrap_or("")
            .trim_matches('/');

        match (req.method.as_str(), sub) {
            ("GET", "" | "status") => Response::json(200, &self.reconciler.status_json().await),
            ("GET", "ps") => Response::json(200, &self.reconciler.ps_json().await),
            ("GET", "doctor") => Response::json(200, &self.reconciler.doctor_json().await),
            ("POST", "runtime") => match parse_json::<RuntimeActionReq>(&req.body) {
                Ok(r) => into_response(self.reconciler.runtime_action(who, r).await),
                Err(resp) => resp,
            },
            ("POST", "environment") => match parse_json::<EnvironmentActionReq>(&req.body) {
                Ok(r) => into_response(self.reconciler.environment_action(who, r).await),
                Err(resp) => resp,
            },
            ("GET", "service" | "services") => into_response(
                self.reconciler
                    .service_action(who, ServiceActionReq::default())
                    .await,
            ),
            ("POST", "service" | "services") => {
                let parsed = if req.body.is_empty() {
                    Ok(ServiceActionReq::default())
                } else {
                    parse_json::<ServiceActionReq>(&req.body)
                };
                match parsed {
                    Ok(r) => into_response(self.reconciler.service_action(who, r).await),
                    Err(resp) => resp,
                }
            }
            ("GET", "app" | "apps") => into_response(
                self.reconciler
                    .app_action(who, AppActionReq::default())
                    .await,
            ),
            ("POST", "app" | "apps") => {
                let parsed = if req.body.is_empty() {
                    Ok(AppActionReq::default())
                } else {
                    parse_json::<AppActionReq>(&req.body)
                };
                match parsed {
                    Ok(r) => into_response(self.reconciler.app_action(who, r).await),
                    Err(resp) => resp,
                }
            }
            ("GET", "sandbox" | "sandboxes") => into_response(
                self.reconciler
                    .sandbox_action(who, SandboxActionReq::default())
                    .await,
            ),
            ("POST", "sandbox" | "sandboxes") => {
                let parsed = if req.body.is_empty() {
                    Ok(SandboxActionReq::default())
                } else {
                    parse_json::<SandboxActionReq>(&req.body)
                };
                match parsed {
                    Ok(r) => into_response(self.reconciler.sandbox_action(who, r).await),
                    Err(resp) => resp,
                }
            }
            ("POST", "reset") => {
                let parsed = if req.body.is_empty() {
                    Ok(ResetReq::default())
                } else {
                    parse_json::<ResetReq>(&req.body)
                };
                match parsed {
                    Ok(r) => into_response(self.reconciler.reset_action(who, r).await),
                    Err(resp) => resp,
                }
            }
            _ => Response::error(
                404,
                "use GET /v1/lifecycle/{status,ps,doctor} or POST /v1/lifecycle/{runtime,environment,service,app,sandbox,reset}",
            ),
        }
    }
}

fn parse_json<T: DeserializeOwned>(body: &[u8]) -> Result<T, Response> {
    serde_json::from_slice::<T>(body)
        .map_err(|e| Response::error(400, &format!("invalid JSON body: {e}")))
}

fn into_response(res: Result<serde_json::Value, (u16, String)>) -> Response {
    match res {
        Ok(v) => Response::json(200, &v),
        Err((status, msg)) => Response::error(status, &msg),
    }
}

impl Subsystem for Lifecycle {
    fn name(&self) -> &'static str {
        "lifecycle"
    }
    fn prefix(&self) -> &'static str {
        "/v1/lifecycle"
    }
    fn resident(&self) -> bool {
        true
    }
    fn start(&self) -> Fut<'_, io::Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn frame<'a>(&'a self, who: &'a str, req: Request) -> Fut<'a, io::Result<Reply>> {
        Box::pin(async move { Ok(Reply::Whole(self.handle(who, req).await)) })
    }
    fn stop(&self) -> Fut<'_, ()> {
        Box::pin(async {})
    }
}
