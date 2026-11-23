// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! `/v1/cluster` & `/v1/k3s`: Automated local/edge Kubernetes (`k3s`), unified
//! `hanzo-cloud`, and local `tasksd` orchestrator inside `hanzod`.

use crate::cloud::Upstream;
use crate::ledger::Ledger;
use crate::paths;
use crate::proc;
use crate::table::{Fut, Reply, Subsystem};
use crate::zap::{Request, Response};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

#[derive(Debug, Deserialize, Default)]
pub struct ClusterUpReq {
    pub mode: Option<String>, // "auto" | "vm-k3s" | "native-k3s" | "cloud-host" | "tasksd"
    pub cpus: Option<u32>,
    pub memory_mb: Option<u64>,
    pub cloud_image: Option<String>,
    pub join_server: Option<String>,
    pub join_token: Option<String>,
    pub upstream_port: Option<u16>,
}

pub struct ClusterCore {
    ledger: Arc<Ledger>,
    upstream: Upstream,
    k3s_child: Mutex<Option<Child>>,
    tasksd_child: Mutex<Option<Child>>,
    state_label: StdMutex<String>,
    driver_label: StdMutex<String>,
}

impl ClusterCore {
    pub fn new(ledger: Arc<Ledger>, upstream: Upstream) -> Arc<Self> {
        Arc::new(Self {
            ledger,
            upstream,
            k3s_child: Mutex::new(None),
            tasksd_child: Mutex::new(None),
            state_label: StdMutex::new("stopped".into()),
            driver_label: StdMutex::new("none".into()),
        })
    }

    fn kubeconfig_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".kube")
            .join("hanzo.yaml")
    }

    fn tasksd_socket_path() -> PathBuf {
        paths::data().join("tasks.zap.sock")
    }

    fn find_bin(name: &str) -> Option<PathBuf> {
        if let Some(p) = proc::find(name) {
            return Some(p);
        }
        let home = dirs::home_dir().unwrap_or_default();
        for cand in [
            home.join(".local").join("bin").join(name),
            home.join(".cargo").join("bin").join(name),
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

    fn pid_alive(path: &Path) -> bool {
        let Ok(txt) = std::fs::read_to_string(path) else {
            return false;
        };
        let Ok(pid) = txt.trim().parse::<i32>() else {
            return false;
        };
        #[cfg(unix)]
        {
            pid > 0 && unsafe { libc::kill(pid, 0) == 0 }
        }
        #[cfg(not(unix))]
        {
            let _ = pid;
            false
        }
    }

    fn detect_external_up() -> (Option<&'static str>, Option<&'static str>, bool) {
        let home = dirs::home_dir().unwrap_or_default();
        let up_dir = home.join(".hanzo").join("up");
        let attestation = up_dir.join("measure.json").is_file();
        if Self::pid_alive(&up_dir.join("vm.pid")) || Self::pid_alive(&up_dir.join("supervisor.pid")) {
            let st = std::fs::read_to_string(up_dir.join("state")).unwrap_or_default();
            let state = if st.trim() == "ready" || Self::kubeconfig_path().is_file() {
                "ready"
            } else {
                "booting"
            };
            return (Some(state), Some("hanzo-vm-k3s"), attestation);
        }
        if Self::kubeconfig_path().is_file()
            && std::net::TcpStream::connect_timeout(
                &"127.0.0.1:6443".parse().unwrap(),
                Duration::from_millis(120),
            )
            .is_ok()
        {
            return (Some("external"), Some("native-k3s"), attestation);
        }
        (None, None, attestation)
    }

    pub fn is_ready(&self) -> bool {
        let cur = self.state_label.lock().unwrap().clone();
        if cur == "ready" || cur == "external" {
            return true;
        }
        matches!(Self::detect_external_up().0, Some("ready" | "external"))
    }

    pub async fn status(&self) -> Value {
        let mut k3s_running = false;
        if let Some(child) = self.k3s_child.lock().await.as_mut() {
            k3s_running = matches!(child.try_wait(), Ok(None));
        }
        let mut tasksd_running = false;
        if let Some(child) = self.tasksd_child.lock().await.as_mut() {
            tasksd_running = matches!(child.try_wait(), Ok(None));
        }
        if !tasksd_running && Self::tasksd_socket_path().exists() {
            tasksd_running = true;
        }

        let (ext_state, ext_driver, attestation) = Self::detect_external_up();
        let (state, driver) = if k3s_running {
            let s = self.state_label.lock().unwrap().clone();
            let d = self.driver_label.lock().unwrap().clone();
            (if s == "stopped" { "booting".into() } else { s }, d)
        } else if let (Some(s), Some(d)) = (ext_state, ext_driver) {
            (s.to_string(), d.to_string())
        } else if self.upstream.lock().unwrap().is_some() {
            ("ready".into(), "cloud-host".into())
        } else {
            ("stopped".into(), "none".into())
        };

        let kubeconfig = Self::kubeconfig_path();
        let cloud_up = self.upstream.lock().unwrap().map(|a| a.to_string());
        json!({
            "ok": true,
            "state": state,
            "driver": driver,
            "kubeconfig": kubeconfig.display().to_string(),
            "kubeconfig_exists": kubeconfig.is_file(),
            "k3s_api": "https://127.0.0.1:6443",
            "cloud_upstream": cloud_up,
            "tasksd_running": tasksd_running,
            "tasksd_socket": Self::tasksd_socket_path().display().to_string(),
            "attestation_present": attestation,
            "capabilities": {
                "vm_k3s": Self::find_bin("hanzo").is_some() && crate::research::discover_hanzo_vm().is_some(),
                "native_k3s": Self::find_bin("k3s").is_some(),
                "cloud_host": Self::find_bin("hanzo-cloud").is_some(),
                "tasksd": Self::find_bin("tasksd").is_some(),
                "kubectl": Self::find_bin("kubectl").is_some(),
            }
        })
    }

    async fn ensure_tasksd(&self) -> io::Result<bool> {
        let mut slot = self.tasksd_child.lock().await;
        if let Some(cur) = slot.as_mut() {
            if cur.try_wait()?.is_none() {
                return Ok(true);
            }
        }
        let Some(bin) = Self::find_bin("tasksd") else {
            return Ok(false);
        };
        proc::owned_by_us_alone(&bin)?;
        let sock = Self::tasksd_socket_path();
        let data_dir = paths::data().join("tasks-data");
        std::fs::create_dir_all(&data_dir)?;
        let _ = std::fs::remove_file(&sock);
        let child = Command::new(&bin)
            .args([
                "--zap",
                &sock.display().to_string(),
                "--data",
                &data_dir.display().to_string(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        *slot = Some(child);
        Ok(true)
    }

    async fn up(self: &Arc<Self>, who: &str, req: ClusterUpReq) -> Response {
        let _ = self.ensure_tasksd().await;
        let requested = req.mode.as_deref().unwrap_or("auto");

        if requested == "tasksd" {
            return Response::json(200, &self.status().await);
        }

        let mut k3s_slot = self.k3s_child.lock().await;
        if let Some(cur) = k3s_slot.as_mut() {
            if matches!(cur.try_wait(), Ok(None)) {
                drop(k3s_slot);
                return Response::json(200, &self.status().await);
            }
        }

        let chosen = match requested {
            "auto" => {
                if Self::find_bin("hanzo").is_some() && crate::research::discover_hanzo_vm().is_some() {
                    "vm-k3s"
                } else if Self::find_bin("k3s").is_some() {
                    "native-k3s"
                } else if Self::find_bin("hanzo-cloud").is_some() {
                    "cloud-host"
                } else {
                    "none"
                }
            }
            other => other,
        };

        match chosen {
            "vm-k3s" => {
                let Some(hanzo_bin) = Self::find_bin("hanzo") else {
                    return Response::error(404, "hanzo CLI binary not found for vm-k3s");
                };
                if let Err(e) = proc::owned_by_us_alone(&hanzo_bin) {
                    return Response::error(403, &e.to_string());
                }
                let mut cmd = Command::new(&hanzo_bin);
                cmd.arg("up");
                if let Some(cpus) = req.cpus {
                    cmd.args(["--cpus", &cpus.to_string()]);
                }
                if let Some(mem) = req.memory_mb {
                    cmd.args(["--memory", &mem.to_string()]);
                }
                if let Some(ref img) = req.cloud_image {
                    cmd.args(["--image", img]);
                }
                cmd.stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .kill_on_drop(true);
                match cmd.spawn() {
                    Ok(child) => {
                        *k3s_slot = Some(child);
                        *self.state_label.lock().unwrap() = "booting".into();
                        *self.driver_label.lock().unwrap() = "hanzo-vm-k3s".into();
                        self.ledger.record(who, "cluster.up", "hanzo-vm-k3s");
                    }
                    Err(e) => return Response::error(500, &format!("spawn hanzo up: {e}")),
                }
            }
            "native-k3s" => {
                let Some(k3s_bin) = Self::find_bin("k3s") else {
                    return Response::error(404, "k3s binary is not installed on this machine");
                };
                if let Err(e) = proc::owned_by_us_alone(&k3s_bin) {
                    return Response::error(403, &e.to_string());
                }
                let k3s_dir = paths::data().join("k3s");
                let _ = std::fs::create_dir_all(&k3s_dir);
                let kube = Self::kubeconfig_path();
                if let Some(p) = kube.parent() {
                    let _ = std::fs::create_dir_all(p);
                }
                let mut cmd = Command::new(&k3s_bin);
                if let Some(ref server_url) = req.join_server {
                    cmd.args([
                        "agent",
                        "--server",
                        server_url,
                        "--data-dir",
                        &k3s_dir.display().to_string(),
                    ]);
                    // Never place join_token in argv or logs; pass via environment only.
                    if let Some(ref tok) = req.join_token {
                        cmd.env("K3S_TOKEN", tok);
                    }
                } else {
                    cmd.args([
                        "server",
                        "--data-dir",
                        &k3s_dir.display().to_string(),
                        "--write-kubeconfig",
                        &kube.display().to_string(),
                        "--https-listen-port",
                        "6443",
                    ]);
                }
                cmd.stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .kill_on_drop(true);
                match cmd.spawn() {
                    Ok(child) => {
                        *k3s_slot = Some(child);
                        *self.state_label.lock().unwrap() = "booting".into();
                        *self.driver_label.lock().unwrap() = "native-k3s".into();
                        self.ledger.record(who, "cluster.up", "native-k3s");
                    }
                    Err(e) => return Response::error(500, &format!("spawn k3s: {e}")),
                }
            }
            "cloud-host" => {
                let Some(cloud_bin) = Self::find_bin("hanzo-cloud") else {
                    return Response::error(404, "hanzo-cloud binary is not installed");
                };
                if let Err(e) = proc::owned_by_us_alone(&cloud_bin) {
                    return Response::error(403, &e.to_string());
                }
                let port = req.upstream_port.or_else(|| proc::free_port().ok()).unwrap_or(8000);
                let mut cmd = Command::new(&cloud_bin);
                cmd.env("PORT", port.to_string())
                    .env("TASKS_ADDR", Self::tasksd_socket_path().display().to_string())
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .kill_on_drop(true);
                match cmd.spawn() {
                    Ok(child) => {
                        *k3s_slot = Some(child);
                        *self.state_label.lock().unwrap() = "booting".into();
                        *self.driver_label.lock().unwrap() = "cloud-host".into();
                        self.spawn_upstream_watcher(port);
                        self.ledger
                            .record(who, "cluster.up", &format!("cloud-host 127.0.0.1:{port}"));
                    }
                    Err(e) => return Response::error(500, &format!("spawn hanzo-cloud: {e}")),
                }
            }
            _ => {
                return Response::error(
                    404,
                    "no local cluster runtime installed (install `hanzo` + `hanzo-vm`, `k3s`, or `hanzo-cloud`)",
                );
            }
        }
        drop(k3s_slot);
        if let Some(port) = req.upstream_port {
            self.spawn_upstream_watcher(port);
        }
        Response::json(202, &self.status().await)
    }

    fn spawn_upstream_watcher(self: &Arc<Self>, port: u16) {
        let me = Arc::clone(self);
        tokio::spawn(async move {
            let addr: SocketAddr = ([127, 0, 0, 1], port).into();
            let probe_req = Request {
                method: "GET".into(),
                target: "/healthz".into(),
                headers: vec![],
                body: vec![],
            };
            let up = proc::until(Duration::from_secs(90), || async {
                matches!(crate::engine::fetch(addr, &probe_req).await, Ok(r) if r.status < 500)
            })
            .await;
            if up {
                *me.upstream.lock().unwrap() = Some(addr);
                *me.state_label.lock().unwrap() = "ready".into();
            }
        });
    }

    pub async fn down(&self, who: &str) -> Response {
        if let Some(mut c) = self.k3s_child.lock().await.take() {
            proc::terminate(&mut c, Duration::from_secs(10)).await;
        }
        if let Some(mut t) = self.tasksd_child.lock().await.take() {
            proc::terminate(&mut t, Duration::from_secs(5)).await;
        }
        *self.upstream.lock().unwrap() = None;
        *self.state_label.lock().unwrap() = "stopped".into();
        *self.driver_label.lock().unwrap() = "none".into();
        self.ledger.record(who, "cluster.down", "stopped");
        Response::json(200, &self.status().await)
    }

    async fn workloads(&self) -> Response {
        let kube = Self::kubeconfig_path();
        if !kube.is_file() {
            return Response::json(
                200,
                &json!({ "ready": false, "items": [], "reason": "kubeconfig not present" }),
            );
        }
        let Some(kubectl) = Self::find_bin("kubectl") else {
            return Response::json(
                200,
                &json!({ "ready": false, "items": [], "reason": "kubectl not installed" }),
            );
        };
        let out = tokio::time::timeout(
            Duration::from_secs(5),
            Command::new(&kubectl)
                .args([
                    "--kubeconfig",
                    &kube.display().to_string(),
                    "get",
                    "pods,svc",
                    "-n",
                    "hanzo",
                    "-o",
                    "json",
                ])
                .kill_on_drop(true)
                .output(),
        )
        .await;
        match out {
            Ok(Ok(o)) if o.status.success() => {
                let parsed: Value = serde_json::from_slice(&o.stdout).unwrap_or_else(|_| json!({}));
                Response::json(200, &json!({ "ready": true, "workloads": parsed }))
            }
            Ok(Ok(o)) => Response::json(
                200,
                &json!({
                    "ready": false,
                    "items": [],
                    "reason": String::from_utf8_lossy(&o.stderr).trim().to_string(),
                }),
            ),
            _ => Response::json(
                200,
                &json!({ "ready": false, "items": [], "reason": "kubectl probe timed out" }),
            ),
        }
    }

    async fn handle(self: &Arc<Self>, base: &str, who: &str, req: Request) -> Response {
        let path = req.path().to_string();
        let sub = path.strip_prefix(base).unwrap_or("").trim_matches('/');
        match (req.method.as_str(), sub) {
            ("GET", "" | "health") => Response::json(200, &self.status().await),
            ("GET", "workloads") => self.workloads().await,
            ("POST", "up") => {
                let up_req: ClusterUpReq = if req.body.is_empty() {
                    ClusterUpReq::default()
                } else {
                    match serde_json::from_slice(&req.body) {
                        Ok(r) => r,
                        Err(e) => return Response::error(400, &format!("invalid JSON: {e}")),
                    }
                };
                self.up(who, up_req).await
            }
            ("POST", "down") => self.down(who).await,
            _ => Response::error(404, "use GET /health, GET /workloads, POST /up, or POST /down"),
        }
    }
}

pub struct ClusterSub {
    pub core: Arc<ClusterCore>,
    pub prefix: &'static str,
    pub name: &'static str,
}

impl Subsystem for ClusterSub {
    fn name(&self) -> &'static str {
        self.name
    }
    fn prefix(&self) -> &'static str {
        self.prefix
    }
    fn resident(&self) -> bool {
        true
    }
    fn start(&self) -> Fut<'_, io::Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn frame<'a>(&'a self, who: &'a str, req: Request) -> Fut<'a, io::Result<Reply>> {
        Box::pin(async move { Ok(Reply::Whole(self.core.handle(self.prefix, who, req).await)) })
    }
    fn stop(&self) -> Fut<'_, ()> {
        Box::pin(async move {
            let _ = self.core.down("hanzod").await;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cluster_and_k3s_health_report_capabilities_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(Ledger::open(dir.path().join("ledger")));
        let upstream: Upstream = Arc::new(StdMutex::new(None));
        let core = ClusterCore::new(ledger, upstream);
        let st = core.status().await;
        assert_eq!(st["ok"], true);
        assert!(st.get("capabilities").is_some());
    }
}
