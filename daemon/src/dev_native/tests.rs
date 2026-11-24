// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

use super::core_fallback::{
    exec_read, exec_write, oai_to_call, tool_schemas, EXEC_TIMEOUT, MAX_TOOL_ROUNDS,
};
use super::*;
use dev_protocol::{Config, Read, Write};
use rusqlite::{params, Connection};
use std::sync::atomic::AtomicU64;
use std::time::Duration;
use tempfile::tempdir;

#[cfg(unix)]
fn write_executable(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    loop {
        match std::process::Command::new(&p)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(mut c) => {
                let _ = c.kill();
                let _ = c.wait();
                return p;
            }
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => panic!("spawn stub {}: {e}", p.display()),
        }
    }
}

#[test]
fn sqlite_session_lifecycle_and_snapshot_restore() {
    let dir = tempdir().unwrap();
    let ledger = Arc::new(Ledger::open(dir.path().join("ledger")));
    let dev = DevNative::with_db_path(dir.path().join("dev.sqlite"), ledger, None);

    let created = dev
        .create_session(
            "hanzo/zach",
            CreateSessionReq {
                label: Some("test-session".into()),
                workspace: Some(dir.path().display().to_string()),
                model: Some("zen-max".into()),
                prelude: Some("Be concise.".into()),
            },
        )
        .unwrap();
    assert_eq!(created.runtime, "hanzo-gen");
    assert_eq!(created.status, "active");

    let health = dev.health(false);
    assert_eq!(health["runtime"], "hanzo-gen");
    assert_eq!(health["harness"], "hanzo-gen");
    assert_eq!(health["harness_version"], "0.2.0");

    let list = dev.list_sessions().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, created.id);

    let suspended = dev.set_status("hanzo/zach", &created.id, "suspended").unwrap();
    assert_eq!(suspended.status, "suspended");

    let resumed = dev.set_status("hanzo/zach", &created.id, "active").unwrap();
    assert_eq!(resumed.status, "active");

    assert!(dev.delete_session("hanzo/zach", &created.id).unwrap());
    assert!(dev.list_sessions().unwrap().is_empty());
}

#[test]
fn sqlite_migration_adds_backend_session_id_without_data_loss() {
    let dir = tempdir().unwrap();
    let db_file = dir.path().join("legacy.sqlite");
    {
        // Create legacy pre-v0.5.0 schema WITHOUT `backend_session_id`.
        let c = Connection::open(&db_file).unwrap();
        c.execute_batch(
            "CREATE TABLE sessions (
                 id TEXT PRIMARY KEY,
                 label TEXT NOT NULL,
                 workspace TEXT NOT NULL,
                 model TEXT,
                 prelude TEXT,
                 status TEXT NOT NULL DEFAULT 'active',
                 next_turn_id INTEGER NOT NULL DEFAULT 1,
                 snapshot BLOB NOT NULL,
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL
             );",
        )
        .unwrap();
        let snap = dev_core::Session::new(Config {
            model: Some("enso".into()),
            prelude: None,
        })
        .snapshot()
        .unwrap();
        c.execute(
            "INSERT INTO sessions (id, label, workspace, model, prelude, status, next_turn_id, snapshot, created_at, updated_at)
             VALUES ('dev_legacy_1', 'legacy-work', '/tmp/ws', 'enso', NULL, 'active', 2, ?1, 1000, 2000)",
            params![snap],
        )
        .unwrap();
    }

    let ledger = Arc::new(Ledger::open(dir.path().join("ledger")));
    let dev = DevNative::with_db_path(db_file, ledger, None);
    let sessions = dev.list_sessions().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "dev_legacy_1");
    assert_eq!(sessions[0].label, "legacy-work");
    assert_eq!(sessions[0].backend_session_id, None);
}

#[cfg(unix)]
#[tokio::test]
async fn embedded_hanzo_gen_multi_turn_resume_and_live_interrupt() {
    let root_dir = tempdir().unwrap();
    let ws_dir = tempdir().unwrap();
    let bin_dir = root_dir.path().join(".local").join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let marker = ws_dir.path().join("turn3_started.flag");
    let inv_log = ws_dir.path().join("invocations.log");

    write_executable(&bin_dir, "hanzo-mcp", "exit 0\n");
    let dev_script = format!(
        r#"printf 'ARGS:%s\n' "$*" >> "{log}"
case "$*" in
  *"exec resume --json"*"th_hanzod_77"*"Block for interrupt"*)
    echo '{{"type":"thread.started","thread_id":"th_hanzod_77"}}'
    echo '{{"type":"item.completed","item":{{"id":"m3","type":"agent_message","text":"Starting long refactor..."}}}}'
    touch "{marker}"
    exec sleep 30
    ;;
  *"exec resume --json"*"th_hanzod_77"*)
    echo '{{"type":"thread.started","thread_id":"th_hanzod_77"}}'
    echo '{{"type":"item.completed","item":{{"id":"m2","type":"agent_message","text":"Turn 2 resumed th_hanzod_77."}}}}'
    echo '{{"type":"turn.completed","usage":{{"input_tokens":40,"cached_input_tokens":10,"output_tokens":15}}}}'
    ;;
  *"exec --json"*)
    echo '{{"type":"thread.started","thread_id":"th_hanzod_77"}}'
    echo '{{"type":"item.completed","item":{{"id":"t1","type":"mcp_tool_call","server":"hanzo","tool":"fs_read","arguments":{{"path":"Cargo.toml"}},"status":"completed"}}}}'
    echo '{{"type":"item.completed","item":{{"id":"m1","type":"agent_message","text":"Turn 1 complete via hanzo-gen."}}}}'
    echo '{{"type":"turn.completed","usage":{{"input_tokens":90,"cached_input_tokens":20,"output_tokens":30}}}}'
    ;;
  *)
    echo '{{"type":"error","message":"unexpected args"}}'
    exit 1
    ;;
esac
"#,
        log = inv_log.display(),
        marker = marker.display()
    );
    write_executable(&bin_dir, "dev", &dev_script);

    let ledger = Arc::new(Ledger::open(root_dir.path().join("ledger")));
    let dev = Arc::new(DevNative::with_db_and_root(
        root_dir.path().join("state.sqlite"),
        root_dir.path().to_path_buf(),
        ledger,
        None,
    ));

    let sess = dev
        .create_session(
            "hanzo/zach",
            CreateSessionReq {
                label: Some("gen-parity".into()),
                workspace: Some(ws_dir.path().display().to_string()),
                model: Some("local:qwen3-coder".into()),
                prelude: Some("Follow repo style.".into()),
            },
        )
        .unwrap();

    // Turn 1: fresh embedded hanzo-gen turn
    let t1 = dev
        .run_turn(
            "hanzo/zach",
            &sess.id,
            RunTurnReq {
                prompt: "Inspect Cargo.toml".into(),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(t1.outcome, "complete");
    assert_eq!(t1.text.as_deref(), Some("Turn 1 complete via hanzo-gen."));
    assert_eq!(t1.actions.len(), 1);
    assert_eq!(t1.actions[0].kind, "tool_call:hanzo/fs_read");

    let detail1 = dev.get_session_detail(&sess.id).unwrap();
    assert_eq!(detail1["backend_session_id"], "th_hanzod_77");
    assert_eq!(detail1["messages"].as_array().unwrap().len(), 2);

    // Turn 2: resumes exact `th_hanzod_77` thread
    let t2 = dev
        .run_turn(
            "hanzo/zach",
            &sess.id,
            RunTurnReq {
                prompt: "Continue phase 2".into(),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(t2.outcome, "complete");
    assert_eq!(t2.text.as_deref(), Some("Turn 2 resumed th_hanzod_77."));

    // Turn 3: live cancellation via `set_status(..., "interrupted")`
    let dev_cancel = dev.clone();
    let sid_cancel = sess.id.clone();
    let marker_wait = marker.clone();
    tokio::spawn(async move {
        for _ in 0..200 {
            if marker_wait.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
        let _ = dev_cancel.set_status("hanzo/zach", &sid_cancel, "interrupted");
    });

    let started = std::time::Instant::now();
    let t3 = dev
        .run_turn(
            "hanzo/zach",
            &sess.id,
            RunTurnReq {
                prompt: "Block for interrupt".into(),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "interrupt must stop live turn promptly"
    );
    assert_eq!(t3.outcome, "interrupted");
    assert_eq!(
        dev.get_session_summary(&sess.id)
            .unwrap()
            .backend_session_id
            .as_deref(),
        Some("th_hanzod_77"),
        "backend_session_id must survive interruption"
    );
}

#[test]
fn core_fallback_constants_and_browse_tool_schema() {
    assert_eq!(MAX_TOOL_ROUNDS, 64);
    assert_eq!(EXEC_TIMEOUT, Duration::from_secs(600));
    let schemas = tool_schemas();
    let names: Vec<&str> = schemas
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s.pointer("/function/name").and_then(Value::as_str))
        .collect();
    assert!(names.contains(&"browse"), "tool_schemas must include browse: {names:?}");
    let parsed = oai_to_call("browse", &json!({ "url": "https://hanzo.ai" }));
    assert!(parsed.is_some());
}

#[test]
fn workspace_confinement_blocks_escapes_and_allows_relative() {
    let dir = tempdir().unwrap();
    let (msg, failed) = exec_read(dir.path(), &Read { path: "../../etc/passwd".into() });
    assert!(failed);
    assert!(msg.contains("outside"));

    let (_, w_failed) = exec_write(
        dir.path(),
        &Write {
            path: "notes/hello.txt".into(),
            text: "hanzo-gen-hanzod".into(),
        },
    );
    assert!(!w_failed);

    let (text, r_failed) = exec_read(dir.path(), &Read { path: "notes/hello.txt".into() });
    assert!(!r_failed);
    assert_eq!(text, "hanzo-gen-hanzod");
}

#[tokio::test]
async fn action_ledger_deduplicates_effects() {
    let dir = tempdir().unwrap();
    let ledger = Arc::new(Ledger::open(dir.path().join("ledger")));
    let dev = DevNative::with_db_path(dir.path().join("dev.sqlite"), ledger, None);
    let s = dev
        .create_session(
            "hanzo/zach",
            CreateSessionReq {
                label: None,
                workspace: Some(dir.path().display().to_string()),
                model: None,
                prelude: None,
            },
        )
        .unwrap();

    let count = Arc::new(AtomicU64::new(0));
    let c1 = count.clone();
    let (out1, rep1) = dev
        .exec_dedup("hanzo/zach", &s.id, 42, "exec", "echo hi", || async move {
            c1.fetch_add(1, Ordering::SeqCst);
            ("hi\n".into(), false)
        })
        .await
        .unwrap();
    assert!(!rep1);
    assert_eq!(out1.text, "hi\n");

    let c2 = count.clone();
    let (out2, rep2) = dev
        .exec_dedup("hanzo/zach", &s.id, 42, "exec", "echo hi", || async move {
            c2.fetch_add(1, Ordering::SeqCst);
            ("should-not-run".into(), false)
        })
        .await
        .unwrap();
    assert!(rep2);
    assert_eq!(out2.text, "hi\n");
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[test]
fn orchestrator_decide_callable_in_process() {
    let dir = tempdir().unwrap();
    let ledger = Arc::new(Ledger::open(dir.path().join("ledger")));
    let dev = DevNative::with_db_path(dir.path().join("dev.sqlite"), ledger, None);
    let raw = RawValue::from_string(r#"{"unknown":true}"#.into()).unwrap();
    assert!(dev.decide("nonexistentRule", &raw).unwrap().is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn agent_coding_subsystem_parity_start_steer_sessions_and_changes() {
    use crate::coding::{CodingCore, CodingSub};
    use crate::dev::Dev;

    let root_dir = tempdir().unwrap();
    let ws_dir = tempdir().unwrap();
    let bin_dir = root_dir.path().join(".local").join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    // Initialize a real git repo in ws_dir so `/changes` reports branch + commits + diffstat.
    for argv in [
        &["init", "-b", "main"][..],
        &["config", "user.email", "z@hanzo.ai"][..],
        &["config", "user.name", "Z"][..],
    ] {
        let st = std::process::Command::new("git")
            .args(argv)
            .current_dir(ws_dir.path())
            .status()
            .unwrap();
        assert!(st.success());
    }
    std::fs::write(ws_dir.path().join("README.md"), "# demo\n").unwrap();
    for argv in [&["add", "."][..], &["commit", "-m", "init"][..]] {
        assert!(std::process::Command::new("git")
            .args(argv)
            .current_dir(ws_dir.path())
            .status()
            .unwrap()
            .success());
    }

    let hold_flag = ws_dir.path().join("hold_turn1.flag");
    write_executable(&bin_dir, "hanzo-mcp", "exit 0\n");
    let script = format!(
        r#"case "$*" in
  *"exec resume --json"*"th_code_11"*)
    echo '{{"type":"thread.started","thread_id":"th_code_11"}}'
    printf 'steered\n' > "{ws}/steered.txt"
    echo '{{"type":"item.completed","item":{{"id":"c1","type":"command_execution","command":"write steered.txt","aggregated_output":"ok","exit_code":0,"status":"completed"}}}}'
    echo '{{"type":"item.completed","item":{{"id":"m2","type":"agent_message","text":"Steered turn finished."}}}}'
    echo '{{"type":"turn.completed","usage":{{"input_tokens":55,"cached_input_tokens":0,"output_tokens":18}}}}'
    ;;
  *"exec --json"*)
    echo '{{"type":"thread.started","thread_id":"th_code_11"}}'
    echo '{{"type":"item.completed","item":{{"id":"m1","type":"agent_message","text":"Initial coding run in progress..."}}}}'
    touch "{hold}"
    exec sleep 30
    ;;
esac
"#,
        ws = ws_dir.path().display(),
        hold = hold_flag.display()
    );
    write_executable(&bin_dir, "dev", &script);

    let ledger = Arc::new(Ledger::open(root_dir.path().join("ledger")));
    let idle = Arc::new(Notify::new());
    let native = Arc::new(DevNative::with_db_and_root(
        root_dir.path().join("state.sqlite"),
        root_dir.path().to_path_buf(),
        ledger.clone(),
        None,
    ));
    let dev_sub = Arc::new(Dev::with_native(ledger.clone(), native.clone(), idle));
    let core = CodingCore::new(dev_sub, ledger);
    let agent_sub = CodingSub { core: core.clone(), prefix: "/v1/agent", name: "agent" };
    let agents_sub = CodingSub { core, prefix: "/v1/agents", name: "agents" };

    async fn call_sub(sub: &crate::coding::CodingSub, method: &str, target: &str, body: Value) -> crate::zap::Response {
        use crate::table::{Reply as SubReply, Subsystem};
        let req = crate::zap::Request {
            method: method.into(),
            target: target.into(),
            headers: vec![],
            body: if body.is_null() { vec![] } else { serde_json::to_vec(&body).unwrap() },
        };
        match sub.frame("hanzo/zach", req).await.unwrap() {
            SubReply::Whole(r) => r,
            SubReply::Frames(_) => panic!("expected Whole response"),
        }
    }

    // 1. POST /v1/agent/coding -> 202 Accepted
    let r_start = call_sub(
        &agent_sub,
        "POST",
        "/v1/agent/coding",
        json!({
            "prompt": "Implement unified gen subsystem",
            "dir": ws_dir.path().display().to_string(),
            "model": "local:qwen3-coder",
            "tool": "gen",
        }),
    )
    .await;
    assert_eq!(r_start.status, 202);
    let start_json: Value = serde_json::from_slice(&r_start.body).unwrap();
    let sid = start_json["id"].as_str().unwrap().to_string();
    assert_eq!(start_json["sessionId"], sid);
    assert_eq!(start_json["status"], "running");
    assert_eq!(start_json["harness"], "hanzo-gen");
    assert_eq!(start_json["branch"], "main");

    // Wait until turn 1 discloses `th_code_11` and enters hold
    for _ in 0..200 {
        if hold_flag.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    assert!(hold_flag.exists(), "turn 1 should be running");

    // 2. GET /v1/agents/sessions while running -> status == "running"
    let r_list_run = call_sub(&agents_sub, "GET", "/v1/agents/sessions", Value::Null).await;
    assert_eq!(r_list_run.status, 200);
    let list_run: Value = serde_json::from_slice(&r_list_run.body).unwrap();
    assert_eq!(list_run["sessions"][0]["status"], "running");
    assert_eq!(list_run["sessions"][0]["kind"], "coding");

    // 3. POST /v1/agent/coding/:id/steer -> interrupts turn 1 & resumes `th_code_11`
    let r_steer = call_sub(
        &agent_sub,
        "POST",
        &format!("/v1/agent/coding/{sid}/steer"),
        json!({ "text": "Pivot and write steered.txt", "model": "local:qwen3-coder" }),
    )
    .await;
    assert_eq!(r_steer.status, 202);

    // Wait for steered turn to finish
    for _ in 0..200 {
        if !native.is_session_running(&sid) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!native.is_session_running(&sid));

    // 4. GET /v1/agents/sessions/:id -> done + recentEvents + backendSessionId
    let r_det = call_sub(
        &agents_sub,
        "GET",
        &format!("/v1/agents/sessions/{sid}"),
        Value::Null,
    )
    .await;
    assert_eq!(r_det.status, 200);
    let det: Value = serde_json::from_slice(&r_det.body).unwrap();
    assert_eq!(det["status"], "done");
    assert_eq!(det["backendSessionId"], "th_code_11");
    assert!(!det["recentEvents"].as_array().unwrap().is_empty());

    // 5. GET /v1/agent/coding/:id/changes -> branch + commits + diffstat (`steered.txt`)
    let r_chg = call_sub(
        &agent_sub,
        "GET",
        &format!("/v1/agent/coding/{sid}/changes"),
        Value::Null,
    )
    .await;
    assert_eq!(r_chg.status, 200);
    let chg: Value = serde_json::from_slice(&r_chg.body).unwrap();
    assert_eq!(chg["branch"], "main");
    assert!(!chg["commits"].as_array().unwrap().is_empty());
    assert!(chg["diffstat"].as_str().unwrap().contains("steered.txt"), "{chg}");
}

#[test]
fn source_files_under_1000_lines() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root");
    let mut stack = vec![workspace_root.to_path_buf()];
    let mut violations = Vec::new();
    let mut checked = 0usize;

    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if path.is_dir() {
                if matches!(name_str.as_ref(), "target" | ".git" | "node_modules") {
                    continue;
                }
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                checked += 1;
                let content = std::fs::read_to_string(&path).unwrap();
                let lines = content.lines().count();
                if lines > 1_000 {
                    violations.push(format!("{} ({lines} lines > 1000)", path.display()));
                }
            }
        }
    }

    assert!(checked >= 20, "expected workspace .rs files, checked {checked}");
    assert!(
        violations.is_empty(),
        "every .rs file in hanzoai/node must be <= 1,000 lines:\n{}",
        violations.join("\n")
    );
}
