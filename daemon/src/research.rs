// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! `/v1/research`: Native Hanzo Dō research & candidate execution subsystem in `hanzod`
//! (HIP-1400, HIP-1401, HIP-1403, HIP-1404).
//!
//! Executes authorized scientific experiments and evolutionary candidates locally
//! (or via linked-desktop HIP-1350 relay), preferring hardware-isolated `hanzo-vm`
//! microVM execution when installed, enforcing pre-registered `max_compute_seconds`
//! budgets, content-addressing artifacts with SHA-256, persisting durable run
//! state in WAL SQLite across restarts, and synchronizing completed Evidence +
//! Run records to Hanzo Cloud (`/v1/research/{artifacts,runs}`).

use crate::ledger::Ledger;
use crate::me;
use crate::paths;
use crate::table::{Fut, Reply, Subsystem};
use crate::zap::{Request, Response};
use base64::Engine as _;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::process::Command;

pub const STANDARD_JSON: &str = include_str!("../../hanzo-standard.json");

const DEFAULT_CLOUD_API: &str = "https://api.hanzo.ai";
const DEFAULT_MAX_COMPUTE_SECONDS: u64 = 600;
const MAX_CAPTURE_BYTES: usize = 2 * 1024 * 1024;

static RUN_COUNTER: AtomicU64 = AtomicU64::new(1);

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn rfc3339_utc(secs: u64) -> String {
    // Civil-from-days algorithm (Howard Hinnant, public domain) — zero extra crate needed.
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

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let out = h.finalize();
    let mut hex = String::with_capacity(64);
    for b in out {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{b:02x}");
    }
    hex
}

fn next_run_id() -> String {
    let ts = now_secs();
    let seq = RUN_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("run_{ts:x}_{seq:04x}")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeasureEntry {
    #[serde(default = "default_category")]
    pub category: String,
    pub metric: String,
    pub value: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lo: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hi: Option<f64>,
}

fn default_category() -> String {
    "all".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactDigest {
    pub path: String,
    pub kind: String,
    pub sha256: String,
    pub r#ref: String,
    pub bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalResearchRun {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hypothesis_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_id: Option<String>,
    pub benchmark: String,
    pub split: String,
    pub system: String,
    pub workspace: String,
    pub command: String,
    pub sandbox: String,
    pub status: String,
    pub completion: String,
    pub questions: i64,
    pub answered: i64,
    pub exit_code: Option<i32>,
    pub commit_sha: String,
    pub started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    pub duration_ms: u64,
    pub measures: Vec<MeasureEntry>,
    pub artifacts: Vec<ArtifactDigest>,
    pub cloud_synced: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ExecuteResearchReq {
    pub id: Option<String>,
    pub hypothesis_id: Option<String>,
    pub candidate_id: Option<String>,
    pub benchmark: String,
    #[serde(default = "default_split")]
    pub split: String,
    #[serde(default = "default_system")]
    pub system: String,
    pub workspace: Option<String>,
    pub command: String,
    #[serde(default)]
    pub artifact_paths: Vec<String>,
    pub max_compute_seconds: Option<u64>,
    pub questions: Option<i64>,
    pub sandbox: Option<String>,
    pub cloud_api: Option<String>,
    pub project: Option<String>,
    #[serde(default)]
    pub sync_cloud: bool,
}

fn default_split() -> String {
    "test".to_string()
}

fn default_system() -> String {
    "hanzod-local".to_string()
}

pub fn discover_hanzo_vm() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("HANZO_VM_BIN") {
        let p = PathBuf::from(explicit.trim());
        if p.is_file() {
            return Some(p);
        }
    }
    let home = dirs::home_dir().unwrap_or_default();
    let names = if cfg!(windows) {
        vec!["hanzo-vm.exe", "hanzo-vm"]
    } else {
        vec!["hanzo-vm"]
    };
    for dir in [
        paths::bin(),
        home.join(".local").join("bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/opt/homebrew/bin"),
    ] {
        for name in &names {
            let cand = dir.join(name);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    if let Ok(path_var) = std::env::var("PATH") {
        for entry in std::env::split_paths(&path_var) {
            for name in &names {
                let cand = entry.join(name);
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }
    None
}

pub struct Research {
    db_path: PathBuf,
    conn: Mutex<Option<Connection>>,
    ledger: Arc<Ledger>,
}

impl Research {
    pub fn new(ledger: Arc<Ledger>) -> Self {
        Self::with_db_path(paths::data().join("research").join("runs.sqlite"), ledger)
    }

    pub fn with_db_path(db_path: PathBuf, ledger: Arc<Ledger>) -> Self {
        Self {
            db_path,
            conn: Mutex::new(None),
            ledger,
        }
    }

    fn with_db<T>(&self, f: impl FnOnce(&Connection) -> io::Result<T>) -> io::Result<T> {
        let mut guard = self.conn.lock().unwrap();
        if guard.is_none() {
            if let Some(parent) = self.db_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let c = Connection::open(&self.db_path).map_err(io::Error::other)?;
            c.execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA synchronous = NORMAL;
                 CREATE TABLE IF NOT EXISTS research_runs (
                     id TEXT PRIMARY KEY,
                     seq INTEGER NOT NULL,
                     payload TEXT NOT NULL
                 );",
            )
            .map_err(io::Error::other)?;
            *guard = Some(c);
        }
        f(guard.as_ref().unwrap())
    }

    fn save_run(&self, run: &LocalResearchRun) -> io::Result<()> {
        let payload = serde_json::to_string(run).map_err(io::Error::other)?;
        self.with_db(|c| {
            let seq: i64 = c
                .query_row(
                    "SELECT COALESCE(
                        (SELECT seq FROM research_runs WHERE id = ?1),
                        (SELECT COALESCE(MAX(seq), 0) + 1 FROM research_runs)
                     )",
                    params![run.id],
                    |r| r.get(0),
                )
                .map_err(io::Error::other)?;
            c.execute(
                "INSERT INTO research_runs (id, seq, payload) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET payload = excluded.payload",
                params![run.id, seq, payload],
            )
            .map_err(io::Error::other)?;
            Ok(())
        })
    }

    pub fn list_runs(&self) -> io::Result<Vec<LocalResearchRun>> {
        self.with_db(|c| {
            let mut stmt = c
                .prepare("SELECT payload FROM research_runs ORDER BY seq DESC LIMIT 500")
                .map_err(io::Error::other)?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(io::Error::other)?;
            let mut out = Vec::new();
            for row in rows {
                let raw = row.map_err(io::Error::other)?;
                if let Ok(parsed) = serde_json::from_str::<LocalResearchRun>(&raw) {
                    out.push(parsed);
                }
            }
            Ok(out)
        })
    }

    pub fn get_run(&self, id: &str) -> io::Result<Option<LocalResearchRun>> {
        self.with_db(|c| {
            let raw: Option<String> = c
                .query_row(
                    "SELECT payload FROM research_runs WHERE id = ?1",
                    params![id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(io::Error::other)?;
            Ok(raw.and_then(|s| serde_json::from_str(&s).ok()))
        })
    }

    pub fn health(&self) -> Value {
        let vm_bin = discover_hanzo_vm();
        let runs = self.list_runs().unwrap_or_default();
        let running = runs.iter().filter(|r| r.status == "running").count();
        json!({
            "ok": true,
            "sandbox": if vm_bin.is_some() { "hanzo-vm" } else { "workspace" },
            "hanzo_vm_available": vm_bin.is_some(),
            "hanzo_vm_bin": vm_bin.map(|p| p.display().to_string()),
            "runs_total": runs.len(),
            "runs_running": running,
        })
    }

    pub async fn execute(
        &self,
        who: &str,
        req: ExecuteResearchReq,
        bearer_override: Option<String>,
    ) -> io::Result<LocalResearchRun> {
        let benchmark = req.benchmark.trim().to_string();
        let command = req.command.trim().to_string();
        if benchmark.is_empty() || command.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "benchmark and command are required",
            ));
        }

        let run_id = req
            .id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(next_run_id);

        let workspace = match req.workspace.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(ws) => PathBuf::from(ws),
            None => self
                .db_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| paths::data().join("research"))
                .join("workspaces")
                .join(&run_id),
        };
        std::fs::create_dir_all(&workspace)?;

        let commit_sha = detect_git_head(&workspace).await;
        let max_secs = req
            .max_compute_seconds
            .unwrap_or(DEFAULT_MAX_COMPUTE_SECONDS)
            .clamp(1, 86_400);
        let questions = req.questions.unwrap_or(1).max(1);

        let vm_bin = if req.sandbox.as_deref() == Some("workspace") {
            None
        } else {
            discover_hanzo_vm()
        };
        let sandbox_kind = if vm_bin.is_some() {
            "hanzo-vm".to_string()
        } else {
            "workspace".to_string()
        };

        let started_ts = now_secs();
        let mut record = LocalResearchRun {
            id: run_id.clone(),
            hypothesis_id: req.hypothesis_id.clone(),
            candidate_id: req.candidate_id.clone(),
            benchmark: benchmark.clone(),
            split: req.split.trim().to_string(),
            system: req.system.trim().to_string(),
            workspace: workspace.display().to_string(),
            command: command.clone(),
            sandbox: sandbox_kind.clone(),
            status: "running".to_string(),
            completion: "partial".to_string(),
            questions,
            answered: 0,
            exit_code: None,
            commit_sha: commit_sha.clone(),
            started_at: rfc3339_utc(started_ts),
            ended_at: None,
            duration_ms: 0,
            measures: Vec::new(),
            artifacts: Vec::new(),
            cloud_synced: false,
            error: None,
        };
        self.save_run(&record)?;
        self.ledger.record(
            who,
            "research.start",
            &format!("{} ({sandbox_kind}) on {benchmark}", record.id),
        );

        let t0 = Instant::now();
        let exec_outcome = run_command_bounded(&workspace, &command, vm_bin.as_deref(), max_secs).await;
        let duration_ms = t0.elapsed().as_millis() as u64;
        let ended_str = rfc3339_utc(now_secs());

        let mut uploaded_blobs: Vec<(ArtifactDigest, Vec<u8>)> = Vec::new();
        match exec_outcome {
            Ok((code, stdout, stderr, actual_sandbox)) => {
                record.sandbox = actual_sandbox;
                record.exit_code = Some(code);
                record.duration_ms = duration_ms;
                record.ended_at = Some(ended_str);

                // Always content-address the execution transcript as a report artifact.
                let mut log_bytes = Vec::new();
                log_bytes.extend_from_slice(&stdout);
                if !stderr.is_empty() {
                    log_bytes.extend_from_slice(b"\n--- stderr ---\n");
                    log_bytes.extend_from_slice(&stderr);
                }
                if log_bytes.is_empty() {
                    log_bytes.extend_from_slice(format!("exit_code={code}\n").as_bytes());
                }
                let log_sha = sha256_hex(&log_bytes);
                let log_art = ArtifactDigest {
                    path: "execution.log".to_string(),
                    kind: "report".to_string(),
                    r#ref: format!("sha256:{log_sha}"),
                    sha256: log_sha,
                    bytes: log_bytes.len(),
                };
                record.artifacts.push(log_art.clone());
                uploaded_blobs.push((log_art, log_bytes));

                // Hash declared output artifacts inside the workspace.
                for rel in &req.artifact_paths {
                    if let Some((art, bytes)) = hash_workspace_artifact(&workspace, rel) {
                        record.artifacts.push(art.clone());
                        uploaded_blobs.push((art, bytes));
                    }
                }

                let (mut parsed_measures, parsed_answered) = parse_stdout_measures(&stdout);
                parsed_measures.push(MeasureEntry {
                    category: "all".to_string(),
                    metric: "duration_ms".to_string(),
                    value: duration_ms as f64,
                    lo: None,
                    hi: None,
                });
                record.measures = parsed_measures;

                if code == 0 {
                    record.status = "complete".to_string();
                    record.answered = parsed_answered.unwrap_or(record.questions);
                    record.completion = if record.answered >= record.questions {
                        "complete".to_string()
                    } else {
                        "partial".to_string()
                    };
                } else {
                    record.status = "faulted".to_string();
                    record.answered = parsed_answered.unwrap_or(0);
                    record.completion = "partial".to_string();
                    record.error = Some(format!("command exited with status {code}"));
                }
            }
            Err(err_msg) => {
                record.status = "faulted".to_string();
                record.completion = "partial".to_string();
                record.duration_ms = duration_ms;
                record.ended_at = Some(ended_str);
                record.error = Some(err_msg);
            }
        }

        if req.sync_cloud {
            let bearer = bearer_override
                .or_else(|| me::tokens().map(|(_, t)| t.access_token));
            if let Some(tok) = bearer {
                let api = req
                    .cloud_api
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(DEFAULT_CLOUD_API)
                    .trim_end_matches('/')
                    .to_string();
                let proj = req.project.clone();
                let run_clone = record.clone();
                let sync_res = tokio::task::spawn_blocking(move || {
                    sync_to_cloud(&api, &tok, proj.as_deref(), &run_clone, &uploaded_blobs)
                })
                .await
                .map_err(io::Error::other)?;
                match sync_res {
                    Ok(()) => record.cloud_synced = true,
                    Err(e) => {
                        let prev = record.error.take().unwrap_or_default();
                        record.error = Some(if prev.is_empty() {
                            format!("cloud sync failed: {e}")
                        } else {
                            format!("{prev}; cloud sync failed: {e}")
                        });
                    }
                }
            }
        }

        self.save_run(&record)?;
        self.ledger.record(
            who,
            "research.finish",
            &format!("{} status={} synced={}", record.id, record.status, record.cloud_synced),
        );
        Ok(record)
    }

    async fn handle(&self, who: &str, req: Request) -> Response {
        let path = req.path().to_string();
        let method = req.method.as_str();

        if method == "GET" && path == "/v1/research/standard" {
            let v: Value = serde_json::from_str(STANDARD_JSON).unwrap_or_else(|_| json!({}));
            return Response::json(200, &v);
        }
        if method == "GET" && (path == "/v1/research/health" || path == "/v1/research") {
            return Response::json(200, &self.health());
        }
        if path == "/v1/research/runs" {
            return match method {
                "GET" => match self.list_runs() {
                    Ok(data) => Response::json(200, &json!({ "total": data.len(), "data": data })),
                    Err(e) => Response::error(500, &e.to_string()),
                },
                "POST" => {
                    let exec_req: ExecuteResearchReq = match serde_json::from_slice(&req.body) {
                        Ok(r) => r,
                        Err(e) => return Response::error(400, &format!("invalid JSON body: {e}")),
                    };
                    let bearer = req.bearer().map(str::to_owned);
                    match self.execute(who, exec_req, bearer).await {
                        Ok(run) => Response::json(200, &json!(run)),
                        Err(e) if e.kind() == io::ErrorKind::InvalidInput => {
                            Response::error(422, &e.to_string())
                        }
                        Err(e) => Response::error(500, &e.to_string()),
                    }
                }
                _ => Response::error(405, "GET or POST /v1/research/runs"),
            };
        }
        if let Some(id) = path.strip_prefix("/v1/research/runs/") {
            let id = id.trim_matches('/');
            if method == "GET" && !id.is_empty() && !id.contains('/') {
                return match self.get_run(id) {
                    Ok(Some(run)) => Response::json(200, &json!(run)),
                    Ok(None) => Response::error(404, &format!("run {id} not found")),
                    Err(e) => Response::error(500, &e.to_string()),
                };
            }
        }
        Response::error(404, "unknown /v1/research route")
    }
}

impl Subsystem for Research {
    fn name(&self) -> &'static str {
        "research"
    }
    fn prefix(&self) -> &'static str {
        "/v1/research"
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

async fn detect_git_head(workspace: &Path) -> String {
    let out = Command::new("git")
        .arg("rev-parse")
        .arg("HEAD")
        .current_dir(workspace)
        .output()
        .await;
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => String::new(),
    }
}

async fn run_command_bounded(
    workspace: &Path,
    command: &str,
    vm_bin: Option<&Path>,
    max_secs: u64,
) -> Result<(i32, Vec<u8>, Vec<u8>, String), String> {
    if let Some(vm) = vm_bin {
        let vm_try = tokio::time::timeout(
            Duration::from_secs(max_secs),
            Command::new(vm)
                .arg("exec")
                .arg("--workdir")
                .arg(workspace)
                .arg("--")
                .arg("sh")
                .arg("-lc")
                .arg(command)
                .kill_on_drop(true)
                .output(),
        )
        .await;
        match vm_try {
            Ok(Ok(out)) if out.status.code() != Some(2) => {
                let code = out.status.code().unwrap_or(1);
                let mut so = out.stdout;
                let mut se = out.stderr;
                so.truncate(MAX_CAPTURE_BYTES);
                se.truncate(MAX_CAPTURE_BYTES);
                return Ok((code, so, se, "hanzo-vm".to_string()));
            }
            Err(_) => {
                return Err(format!(
                    "budget exceeded: execution timed out after {max_secs}s inside hanzo-vm"
                ));
            }
            _ => {
                // Fall through cleanly if hanzo-vm CLI does not expose `exec` subcommand on this host.
            }
        }
    }

    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("powershell");
        c.arg("-NoProfile").arg("-Command").arg(command);
        c
    } else {
        let mut c = Command::new("sh");
        c.arg("-lc").arg(command);
        c
    };
    cmd.current_dir(workspace).kill_on_drop(true);

    match tokio::time::timeout(Duration::from_secs(max_secs), cmd.output()).await {
        Ok(Ok(out)) => {
            let code = out.status.code().unwrap_or(1);
            let mut so = out.stdout;
            let mut se = out.stderr;
            so.truncate(MAX_CAPTURE_BYTES);
            se.truncate(MAX_CAPTURE_BYTES);
            Ok((code, so, se, "workspace".to_string()))
        }
        Ok(Err(e)) => Err(format!("spawn failed: {e}")),
        Err(_) => Err(format!(
            "budget exceeded: execution timed out after {max_secs}s (HIP-1403 §2.2)"
        )),
    }
}

fn hash_workspace_artifact(workspace: &Path, rel: &str) -> Option<(ArtifactDigest, Vec<u8>)> {
    let trimmed = rel.trim().trim_start_matches(['/', '\\']);
    if trimmed.is_empty() || trimmed.contains("..") {
        return None;
    }
    let full = workspace.join(trimmed);
    let bytes = std::fs::read(&full).ok()?;
    if bytes.is_empty() || bytes.len() > 16 * 1024 * 1024 {
        return None;
    }
    let sha = sha256_hex(&bytes);
    let kind = if trimmed.ends_with(".png") {
        "snapshot"
    } else {
        "report"
    };
    Some((
        ArtifactDigest {
            path: trimmed.to_string(),
            kind: kind.to_string(),
            r#ref: format!("sha256:{sha}"),
            sha256: sha,
            bytes: bytes.len(),
        },
        bytes,
    ))
}

fn parse_stdout_measures(stdout: &[u8]) -> (Vec<MeasureEntry>, Option<i64>) {
    let text = String::from_utf8_lossy(stdout);
    let mut measures = Vec::new();
    let mut answered = None;
    for line in text.lines() {
        let trimmed = line.trim();
        let json_part = if let Some(rest) = trimmed.strip_prefix("HANZO_MEASURES:") {
            rest.trim()
        } else if trimmed.starts_with('{') && trimmed.ends_with('}') {
            trimmed
        } else {
            continue;
        };
        if let Ok(arr) = serde_json::from_str::<Vec<MeasureEntry>>(json_part) {
            measures.extend(arr);
        } else if let Ok(obj) = serde_json::from_str::<Value>(json_part) {
            if let Some(m_arr) = obj.get("measures") {
                if let Ok(parsed) = serde_json::from_value::<Vec<MeasureEntry>>(m_arr.clone()) {
                    measures.extend(parsed);
                }
            }
            if let Some(a) = obj.get("answered").and_then(Value::as_i64) {
                answered = Some(a);
            }
        }
    }
    (measures, answered)
}

fn sync_to_cloud(
    cloud_api: &str,
    bearer: &str,
    project: Option<&str>,
    run: &LocalResearchRun,
    blobs: &[(ArtifactDigest, Vec<u8>)],
) -> Result<(), String> {
    let auth_header = format!("Bearer {bearer}");
    for (art, bytes) in blobs {
        let payload = json!({
            "kind": art.kind,
            "run_id": run.id,
            "sha256": art.sha256,
            "git_sha": run.commit_sha,
            "content": base64::engine::general_purpose::STANDARD.encode(bytes),
        });
        let mut req = ureq::post(&format!("{cloud_api}/v1/research/artifacts"))
            .timeout(Duration::from_secs(15))
            .set("Authorization", &auth_header)
            .set("Content-Type", "application/json");
        if let Some(p) = project {
            req = req.set("X-Project-Id", p);
        }
        req.send_string(&payload.to_string())
            .map_err(|e| format!("artifact {}: {e}", art.path))?;
    }

    let mut notes = format!("sandbox={}", run.sandbox);
    if let Some(h) = &run.hypothesis_id {
        notes.push_str(&format!(" hypothesis_id={h}"));
    }
    if let Some(c) = &run.candidate_id {
        notes.push_str(&format!(" candidate_id={c}"));
    }
    let facts_digest = run
        .artifacts
        .first()
        .map(|a| a.r#ref.clone())
        .unwrap_or_default();

    let run_payload = json!({
        "runs": [{
            "id": run.id,
            "benchmark": run.benchmark,
            "split": run.split,
            "system": run.system,
            "commit": run.commit_sha,
            "questions": run.questions,
            "answered": run.answered,
            "when": run.started_at,
            "ended": run.ended_at.clone().unwrap_or_default(),
            "facts_digest": facts_digest,
            "measures": run.measures,
            "notes": notes,
        }]
    });
    let mut req = ureq::post(&format!("{cloud_api}/v1/research/runs"))
        .timeout(Duration::from_secs(15))
        .set("Authorization", &auth_header)
        .set("Content-Type", "application/json");
    if let Some(p) = project {
        req = req.set("X-Project-Id", p);
    }
    req.send_string(&run_payload.to_string())
        .map_err(|e| format!("runs: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn embedded_standard_manifest_satisfies_hip_1400() {
        let doc: Value = serde_json::from_str(STANDARD_JSON).expect("valid hanzo-standard.json");
        assert_eq!(doc["spec_version"], "1.0.0");
        assert_eq!(doc["component"], "daemon/hanzod");
        let hips = doc["implemented_hips"].as_array().expect("implemented_hips");
        assert!(hips.iter().any(|h| h["hip"] == "1400"));
        assert!(hips.iter().any(|h| h["hip"] == "1401"));
        assert!(hips.iter().any(|h| h["hip"] == "1403"));
    }

    #[tokio::test]
    async fn durable_execution_hashes_artifacts_enforces_budget_and_syncs_cloud() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("runs.sqlite");
        let ledger = Arc::new(Ledger::open(dir.path().join("ledger")));
        let research = Research::with_db_path(db.clone(), ledger.clone());

        // Start a mock Cloud /v1/research HTTP server capturing artifact & run uploads.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let received = Arc::new(Mutex::new(Vec::<String>::new()));
        let rec_clone = received.clone();
        let srv = tokio::spawn(async move {
            for _ in 0..3 {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 65536];
                let n = sock.read(&mut buf).await.unwrap();
                let text = String::from_utf8_lossy(&buf[..n]).to_string();
                let first_line = text.lines().next().unwrap_or("").to_string();
                rec_clone.lock().unwrap().push(first_line);
                sock.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 12\r\nConnection: close\r\n\r\n{\"recorded\":1}",
                )
                .await
                .unwrap();
            }
        });

        let ws = dir.path().join("ws1");
        let run = research
            .execute(
                "alice",
                ExecuteResearchReq {
                    id: Some("exp-do-001".into()),
                    hypothesis_id: Some("sha256:hyp123".into()),
                    candidate_id: Some("sha256:cand456".into()),
                    benchmark: "swe-bench-verified".into(),
                    split: "test".into(),
                    system: "gen-local".into(),
                    workspace: Some(ws.display().to_string()),
                    command: "printf 'result-bytes' > out.txt && echo 'HANZO_MEASURES: [{\"category\":\"all\",\"metric\":\"pass@1\",\"value\":0.84}]'".into(),
                    artifact_paths: vec!["out.txt".into()],
                    max_compute_seconds: Some(15),
                    questions: Some(50),
                    sandbox: Some("workspace".into()),
                    cloud_api: Some(format!("http://{addr}")),
                    project: Some("do-lab".into()),
                    sync_cloud: true,
                },
                Some("test-bearer-token".into()),
            )
            .await
            .unwrap();

        srv.await.unwrap();

        assert_eq!(run.status, "complete");
        assert_eq!(run.completion, "complete");
        assert_eq!(run.answered, 50);
        assert!(run.cloud_synced, "expected cloud sync to succeed: {:?}", run.error);
        assert_eq!(run.artifacts.len(), 2);
        assert!(run.artifacts.iter().any(|a| a.path == "out.txt" && a.r#ref.starts_with("sha256:")));
        assert!(run.measures.iter().any(|m| m.metric == "pass@1" && (m.value - 0.84).abs() < 1e-9));

        let hits = received.lock().unwrap().clone();
        assert_eq!(hits.len(), 3);
        assert!(hits[0].starts_with("POST /v1/research/artifacts "));
        assert!(hits[2].starts_with("POST /v1/research/runs "));

        // Re-open Research on the same SQLite file to prove restart durability!
        let reopened = Research::with_db_path(db, ledger.clone());
        let persisted = reopened.get_run("exp-do-001").unwrap().expect("persisted across restart");
        assert_eq!(persisted.artifacts, run.artifacts);
        assert!(persisted.cloud_synced);

        // Verify HIP-1403 §2.2 budget timeout enforcement!
        let timed_out = reopened
            .execute(
                "alice",
                ExecuteResearchReq {
                    id: Some("exp-timeout".into()),
                    hypothesis_id: None,
                    candidate_id: None,
                    benchmark: "swe-bench-verified".into(),
                    split: "test".into(),
                    system: "gen-local".into(),
                    workspace: None,
                    command: "sleep 5".into(),
                    artifact_paths: vec![],
                    max_compute_seconds: Some(1),
                    questions: Some(10),
                    sandbox: Some("workspace".into()),
                    cloud_api: None,
                    project: None,
                    sync_cloud: false,
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(timed_out.status, "faulted");
        assert!(timed_out.error.as_deref().unwrap_or("").contains("budget exceeded"));
    }
}
