// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

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
                    let sse = ": ok\n\nevent: activity.scheduled\ndata: {\"kind\":\"activity.scheduled\"}\n\n";
                    let hdr = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        sse.len(),
                        sse
                    );
                    let _ = stream.write_all(hdr.as_bytes());
                    continue;
                } else if path.contains("/namespaces/fleet/activities/")
                    && path.ends_with("/heartbeat")
                {
                    fh_c.fetch_add(1, Ordering::SeqCst);
                    (200, r#"{"status":"ACTIVITY_TASK_STATE_STARTED"}"#.to_string())
                } else if method == "POST" && path.ends_with("/namespaces/fleet/activities") {
                    fr_c.fetch_add(1, Ordering::SeqCst);
                    (200, r#"{"status":"ACTIVITY_TASK_STATE_SCHEDULED"}"#.to_string())
                } else if method == "POST" && path.ends_with("/activities/claim") {
                    cl_c.fetch_add(1, Ordering::SeqCst);
                    let req_json: Value = serde_json::from_str(body_str).unwrap_or_default();
                    let want_q = req_json
                        .get("taskQueue")
                        .and_then(Value::as_str)
                        .unwrap_or("");
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

                let status_text = if status == 204 {
                    "204 No Content"
                } else {
                    "200 OK"
                };
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

fn build_test_worker(
    dir: &std::path::Path,
    api: &str,
) -> (Arc<TasksWorker>, Arc<Research>, Arc<Notify>) {
    let ledger = Arc::new(Ledger::open(dir.join("ledger")));
    let pins = Arc::new(Pins::load(&dir.join("pins.json")));
    let engine = Arc::new(Engine::new(pins));
    let notify = Arc::new(Notify::new());
    let dev = Arc::new(Dev::with_notify(
        ledger.clone(),
        Some(engine.clone()),
        notify.clone(),
    ));
    let research = Arc::new(Research::with_db_path_and_notify(
        dir.join("research.sqlite"),
        ledger.clone(),
        notify.clone(),
    ));
    let bot = Arc::new(Bot::with_db_path(
        dir.join("bot.sqlite"),
        ledger.clone(),
        engine.clone(),
        Some(api.into()),
    ));
    let upstream = Arc::new(Mutex::new(None));
    let team = Arc::new(Team::with_db_path(
        dir.join("team.sqlite"),
        ledger.clone(),
        upstream.clone(),
        Some(api.into()),
    ));
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
    assert!(
        st["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str() == Some("gen.run")),
        "capabilities must include gen.run"
    );
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

    let busy_guard = research.hold_active_guard();
    let claimed_while_busy = worker
        .poll_and_execute_once("test-jwt-token", 0)
        .await
        .unwrap();
    assert!(!claimed_while_busy);
    assert_eq!(
        srv.claims.load(Ordering::SeqCst),
        0,
        "must not call /claim when busy"
    );
    assert_eq!(worker.metrics.busy_skips.load(Ordering::SeqCst), 1);

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
    assert_eq!(
        srv.fails.load(Ordering::SeqCst),
        0,
        "mismatch must yield, never fail"
    );
    assert_eq!(worker.metrics.deferred_busy.load(Ordering::SeqCst), 1);
}
