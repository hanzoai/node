// SPDX-License-Identifier: Apache-2.0
//! The light, open BYO-GPU claim loop.
//!
//! This is the canonical Rust port of what the Go `hanzo gpu connect` CLI used
//! to do (now deprecated-for-desktop). The GPU owner runs only Hanzo Desktop;
//! the desktop manages hanzod (this loop) + hanzo-engine + a local studio
//! (ComfyUI). When the owner turns "Share GPU" on, [`run`] registers the machine
//! in the org's cloud fleet, heartbeats, and claims `studio.render` jobs.
//!
//! Protocol (all under the cloud tasks engine, one bearer, org derived
//! server-side from the token — never sent as a header):
//!
//! * register: `POST /v1/tasks/namespaces` (fleet, gpu-jobs) then
//!   `POST /v1/tasks/namespaces/fleet/activities` with `activityId==runId==identity`
//!   and the presence [`Registration`] as `input`.
//! * heartbeat (30s): `POST /v1/tasks/namespaces/fleet/activities/{id}/{id}/heartbeat`.
//! * claim (2s poll): `POST /v1/tasks/namespaces/gpu-jobs/activities/claim`
//!   (204 == empty queue).
//! * complete/fail: `POST …/gpu-jobs/activities/{wf}/{run}/{complete|fail}`.
//!
//! `studio.render` forwards `input.prompt` (a ComfyUI graph) to the local studio
//! `/prompt`, polls `/history/{id}`, then — the piece the Go worker never did —
//! fetches each finished image from the local `/view` and uploads it to the cloud
//! studio's token-authorized `/upload/output`, so it lands in the org's gallery
//! with NO S3/rclone credentials on the box. The owner's session token is the
//! only credential, and it lives only in memory.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

const FLEET_NS: &str = "fleet";
pub const DEFAULT_JOBS_NS: &str = "gpu-jobs";
pub const DEFAULT_CLOUD_URL: &str = "https://api.hanzo.ai";
pub const DEFAULT_COMFYUI_URL: &str = "http://127.0.0.1:8188";
pub const DEFAULT_STUDIO_URL: &str = "https://studio.hanzo.ai";

const HEARTBEAT_EVERY: Duration = Duration::from_secs(30);
const CLAIM_POLL: Duration = Duration::from_secs(2);
const CLAIM_LEASE_SECS: u64 = 120;
const RENDER_DEADLINE: Duration = Duration::from_secs(600);

pub const CAP_STUDIO_RENDER: &str = "studio.render";
pub const CAP_ENGINE_SERVE: &str = "engine.serve";

/// Runtime configuration. Assembled by the desktop (or the standalone bin from
/// env). `token` is the owner's IAM session bearer — held in memory only.
#[derive(Clone, Debug)]
pub struct FleetConfig {
    pub cloud_url: String,
    pub token: String,
    pub jobs_ns: String,
    pub comfyui_url: String,
    pub studio_url: String,
    /// Machine key; defaults to the sanitized hostname.
    pub hostname: Option<String>,
    /// Optional local hanzo-engine to advertise (engine.serve capability). This
    /// is a *presence advertisement* — the gateway calls the engine directly;
    /// no engine job is ever claimed.
    pub engine: Option<EngineConfig>,
}

impl FleetConfig {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            cloud_url: DEFAULT_CLOUD_URL.to_string(),
            token: token.into(),
            jobs_ns: DEFAULT_JOBS_NS.to_string(),
            comfyui_url: DEFAULT_COMFYUI_URL.to_string(),
            studio_url: DEFAULT_STUDIO_URL.to_string(),
            hostname: None,
            engine: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct EngineConfig {
    /// Local URL to probe hanzo-engine (e.g. http://127.0.0.1:36900).
    pub probe_url: String,
    /// Public URL to advertise to the gateway (defaults to `probe_url`).
    pub advertise_url: Option<String>,
}

// --- wire shapes (camelCase to match the cloud tasks/visor contract) ---------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GpuInfo {
    pub name: String,
    #[serde(rename = "memoryTotal", default, skip_serializing_if = "String::is_empty")]
    pub memory_total: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EngineAdvertisement {
    pub url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub apis: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    pub status: String,
}

/// The fleet presence activity's `input`. Byte-compatible with the cloud's
/// `clients/visor/fleet.go` `fleetRegistration` — edit both in lockstep.
#[derive(Clone, Debug, Serialize)]
pub struct Registration {
    pub hostname: String,
    pub os: String,
    pub version: String,
    #[serde(rename = "jobQueue")]
    pub job_queue: String,
    pub gpus: Vec<GpuInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<EngineAdvertisement>,
}

#[derive(Clone, Debug, Deserialize)]
struct ClaimedActivity {
    #[serde(default)]
    execution: Execution,
    #[serde(default, rename = "type")]
    ty: ActivityType,
    #[serde(default)]
    input: Value,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct Execution {
    #[serde(rename = "workflowId", default)]
    workflow_id: String,
    #[serde(rename = "runId", default)]
    run_id: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct ActivityType {
    #[serde(default)]
    name: String,
}

/// Lower-case and reduce to `[a-z0-9-]` so a hostname is a stable machine key
/// and a valid tasks path segment. Mirrors the Go `sanitizeID`.
pub fn sanitize_id(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for ch in s.trim().to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "worker".to_string()
    } else {
        trimmed
    }
}

/// Resolve this machine's hostname (Linux `/proc`, `HOSTNAME` env, else "worker").
pub fn detect_hostname() -> String {
    if let Ok(h) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
        let h = h.trim();
        if !h.is_empty() {
            return h.to_string();
        }
    }
    std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.trim().is_empty())
        .unwrap_or_else(|| "worker".to_string())
}

/// Query `nvidia-smi` for accelerators, degrading to an empty list (CPU-only)
/// when nvidia-smi is absent or errors. Mirrors the Go `detectGPUs`.
pub fn detect_gpus() -> Vec<GpuInfo> {
    let out = match std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=name,memory.total", "--format=csv,noheader"])
        .output()
    {
        Ok(o) if o.status.success() => o.stdout,
        _ => return Vec::new(),
    };
    String::from_utf8_lossy(&out)
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let (name, mem) = line.split_once(',').unwrap_or((line, ""));
            let mem = mem.trim();
            Some(GpuInfo {
                name: name.trim().to_string(),
                // nvidia-smi reports "[N/A]" for unified-memory parts (e.g. GB10).
                memory_total: if mem == "[N/A]" { String::new() } else { mem.to_string() },
            })
        })
        .collect()
}

/// A connected worker. Cheap to clone (Arc'd HTTP client + shared config).
#[derive(Clone)]
pub struct Worker {
    http: reqwest::Client,
    cfg: Arc<FleetConfig>,
    identity: String,
    hostname: String,
    gpus: Vec<GpuInfo>,
    engine: Option<EngineAdvertisement>,
}

impl Worker {
    pub fn new(cfg: FleetConfig) -> Result<Self> {
        let hostname = cfg
            .hostname
            .clone()
            .unwrap_or_else(detect_hostname);
        let identity = sanitize_id(&hostname);
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .context("build http client")?;
        Ok(Self {
            http,
            identity,
            hostname,
            gpus: detect_gpus(),
            engine: None,
            cfg: Arc::new(cfg),
        })
    }

    pub fn identity(&self) -> &str {
        &self.identity
    }

    fn capabilities(&self) -> Vec<String> {
        let mut caps = vec![CAP_STUDIO_RENDER.to_string()];
        if self.engine.is_some() {
            caps.push(CAP_ENGINE_SERVE.to_string());
        }
        caps
    }

    fn registration(&self) -> Registration {
        Registration {
            hostname: self.hostname.clone(),
            os: std::env::consts::OS.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            job_queue: self.cfg.jobs_ns.clone(),
            gpus: self.gpus.clone(),
            capabilities: self.capabilities(),
            engine: self.engine.clone(),
        }
    }

    /// One authed JSON request. Returns `(status, body)`; a 204 yields
    /// `(204, Null)` — the empty-queue signal the claim loop reads.
    async fn call(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<(u16, Value)> {
        let url = format!("{}{}", self.cfg.cloud_url.trim_end_matches('/'), path);
        let mut req = self
            .http
            .request(method.clone(), &url)
            .bearer_auth(&self.cfg.token)
            .header("Accept", "application/json")
            .header("User-Agent", concat!("hanzo-fleet-worker/", env!("CARGO_PKG_VERSION")));
        if let Some(b) = &body {
            req = req.json(b);
        }
        let resp = req.send().await.with_context(|| format!("{method} {path}"))?;
        let status = resp.status().as_u16();
        if status == 204 {
            return Ok((204, Value::Null));
        }
        let raw = resp.text().await.unwrap_or_default();
        if status / 100 != 2 {
            return Err(anyhow!("{method} {path}: HTTP {status}: {}", server_message(&raw)));
        }
        let val: Value = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&raw).with_context(|| format!("{method} {path}: decode"))?
        };
        Ok((status, val))
    }

    /// Ensure fleet + jobs namespaces exist, then write the presence record
    /// (activityId==runId==identity; a reconnect overwrites any prior row).
    pub async fn register(&self) -> Result<()> {
        for ns in [FLEET_NS, self.cfg.jobs_ns.as_str()] {
            self.call(
                reqwest::Method::POST,
                "/v1/tasks/namespaces",
                Some(json!({ "namespaceInfo": { "name": ns } })),
            )
            .await
            .with_context(|| format!("ensure namespace {ns}"))?;
        }
        self.call(
            reqwest::Method::POST,
            &format!("/v1/tasks/namespaces/{FLEET_NS}/activities"),
            Some(json!({
                "activityId": self.identity,
                "runId": self.identity,
                "activityType": { "name": "fleet.worker" },
                "taskQueue": FLEET_NS,
                "heartbeatTimeout": "120s",
                "input": self.registration(),
            })),
        )
        .await?;
        Ok(())
    }

    pub async fn heartbeat(&self) -> Result<()> {
        let path = format!(
            "/v1/tasks/namespaces/{FLEET_NS}/activities/{id}/{id}/heartbeat",
            id = self.identity
        );
        self.call(
            reqwest::Method::POST,
            &path,
            Some(json!({ "details": { "gpus": self.gpus.len(), "ts": chrono::Utc::now().to_rfc3339() } })),
        )
        .await?;
        Ok(())
    }

    /// Complete the presence activity so the fleet reader drops this machine.
    pub async fn disconnect(&self) -> Result<()> {
        let path = format!(
            "/v1/tasks/namespaces/{FLEET_NS}/activities/{id}/{id}/complete",
            id = self.identity
        );
        self.call(
            reqwest::Method::POST,
            &path,
            Some(json!({ "result": { "disconnected": true }, "identity": self.identity })),
        )
        .await?;
        Ok(())
    }

    /// Claim the next job (if any), run it, and report the terminal result.
    /// Returns `true` when a job was handled (for logging), `false` on an empty
    /// queue.
    pub async fn claim_and_run(&self, cancel: &CancellationToken) -> Result<bool> {
        let (code, body) = self
            .call(
                reqwest::Method::POST,
                &format!("/v1/tasks/namespaces/{}/activities/claim", self.cfg.jobs_ns),
                Some(json!({
                    "taskQueue": self.cfg.jobs_ns,
                    "identity": self.identity,
                    "leaseSeconds": CLAIM_LEASE_SECS,
                })),
            )
            .await?;
        if code == 204 {
            return Ok(false);
        }
        let act: ClaimedActivity = serde_json::from_value(body).context("decode claimed activity")?;
        let (wf, run) = (act.execution.workflow_id.clone(), act.execution.run_id.clone());
        log::info!("claimed job {wf} (type {})", act.ty.name);

        let result = match act.ty.name.as_str() {
            "echo" => Ok(json!({ "echo": act.input })),
            CAP_STUDIO_RENDER => self.run_studio_render(act.input, cancel).await,
            other => Err(anyhow!("no handler for job type {other:?}")),
        };

        match result {
            Ok(res) => {
                self.call(
                    reqwest::Method::POST,
                    &self.act_path(&wf, &run, "complete"),
                    Some(json!({ "result": res, "identity": self.identity })),
                )
                .await
                .with_context(|| format!("complete {wf}"))?;
                log::info!("job {wf} completed");
            }
            Err(e) => {
                let cause = format!("{e:#}");
                let _ = self
                    .call(
                        reqwest::Method::POST,
                        &self.act_path(&wf, &run, "fail"),
                        Some(json!({ "cause": cause, "identity": self.identity })),
                    )
                    .await;
                log::warn!("job {wf} failed: {cause}");
            }
        }
        Ok(true)
    }

    fn act_path(&self, wf: &str, run: &str, verb: &str) -> String {
        format!("/v1/tasks/namespaces/{}/activities/{wf}/{run}/{verb}", self.cfg.jobs_ns)
    }

    /// Forward `input.prompt` to the local studio `/prompt`, poll `/history/{id}`
    /// until it completes, then fetch each output from `/view` and upload it to
    /// the cloud studio gallery with the owner's token.
    async fn run_studio_render(&self, input: Value, cancel: &CancellationToken) -> Result<Value> {
        let prompt = input
            .get("prompt")
            .filter(|p| !p.is_null())
            .ok_or_else(|| anyhow!("studio.render: input needs a `prompt` graph"))?;

        let comfy = self.cfg.comfyui_url.trim_end_matches('/');
        let submit = self
            .http
            .post(format!("{comfy}/prompt"))
            .json(&json!({ "prompt": prompt }))
            .send()
            .await
            .with_context(|| format!("studio.render: POST /prompt (is the local studio server on {comfy}?)"))?;
        let submit_status = submit.status();
        let submit_body = submit.text().await.unwrap_or_default();
        if !submit_status.is_success() {
            return Err(anyhow!(
                "studio.render: /prompt HTTP {}: {}",
                submit_status.as_u16(),
                server_message(&submit_body)
            ));
        }
        let prompt_id = serde_json::from_str::<Value>(&submit_body)
            .ok()
            .and_then(|v| v.get("prompt_id").and_then(Value::as_str).map(str::to_string))
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("studio.render: no prompt_id in /prompt response"))?;

        // Poll history until the prompt shows up (completed).
        let deadline = tokio::time::Instant::now() + RENDER_DEADLINE;
        let entry = loop {
            tokio::select! {
                _ = cancel.cancelled() => return Err(anyhow!("studio.render: cancelled")),
                _ = tokio::time::sleep(CLAIM_POLL) => {}
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(anyhow!("studio.render: prompt {prompt_id} did not complete within the deadline"));
            }
            let hist = match self.http.get(format!("{comfy}/history/{prompt_id}")).send().await {
                Ok(r) => r,
                Err(_) => continue,
            };
            let hist: Value = match hist.json().await {
                Ok(v) => v,
                Err(_) => continue,
            };
            if let Some(e) = hist.get(&prompt_id) {
                break e.clone();
            }
        };

        let outputs = collect_outputs(&entry);
        // Upload each finished image to the org gallery with the owner's token.
        let mut uploaded: Vec<Value> = Vec::new();
        for out in &outputs {
            match self.upload_output(comfy, &out.subfolder, &out.filename).await {
                Ok(name) => uploaded.push(json!({ "filename": name, "subfolder": out.subfolder })),
                Err(e) => log::warn!("studio.render: upload {}: {e:#}", out.filename),
            }
        }
        Ok(json!({
            "promptId": prompt_id,
            "outputs": outputs.iter().map(|o| join_rel(&o.subfolder, &o.filename)).collect::<Vec<_>>(),
            "uploaded": uploaded,
        }))
    }

    /// Fetch one rendered image from the local studio `/view` and POST it to the
    /// cloud studio's token-authorized `/upload/output`. Returns the stored name.
    async fn upload_output(&self, comfy: &str, subfolder: &str, filename: &str) -> Result<String> {
        let bytes = self
            .http
            .get(format!("{comfy}/view"))
            .query(&[("filename", filename), ("subfolder", subfolder), ("type", "output")])
            .send()
            .await
            .context("GET /view")?
            .error_for_status()
            .context("GET /view status")?
            .bytes()
            .await
            .context("read /view body")?;

        let part = reqwest::multipart::Part::bytes(bytes.to_vec())
            .file_name(filename.to_string())
            .mime_str(guess_mime(filename))?;
        let form = reqwest::multipart::Form::new()
            .part("image", part)
            .text("type", "output")
            .text("subfolder", subfolder.to_string());

        let url = format!("{}/upload/output", self.cfg.studio_url.trim_end_matches('/'));
        let resp = self
            .http
            .post(url)
            .bearer_auth(&self.cfg.token)
            .multipart(form)
            .send()
            .await
            .context("POST /upload/output")?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!("/upload/output HTTP {}: {}", status.as_u16(), server_message(&body)));
        }
        let name = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| v.get("name").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| filename.to_string());
        Ok(name)
    }

    /// Probe the local hanzo-engine and set the presence engine advertisement.
    /// Best-effort: a probe failure advertises `status:"unreachable"`.
    pub async fn refresh_engine(&mut self) {
        let Some(ec) = self.cfg.engine.clone() else {
            return;
        };
        let advertise = ec.advertise_url.clone().unwrap_or_else(|| ec.probe_url.clone());
        let mut adv = EngineAdvertisement {
            url: advertise,
            apis: vec!["openai".into(), "anthropic".into()],
            models: Vec::new(),
            status: "unreachable".into(),
        };
        if let Ok(resp) = self.http.get(format!("{}/v1/models", ec.probe_url.trim_end_matches('/'))).send().await {
            if resp.status().is_success() {
                if let Ok(v) = resp.json::<Value>().await {
                    if let Some(data) = v.get("data").and_then(Value::as_array) {
                        adv.models = data
                            .iter()
                            .filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_string))
                            .collect();
                    }
                    adv.status = "ready".into();
                }
            }
        }
        self.engine = Some(adv);
    }
}

struct Output {
    subfolder: String,
    filename: String,
}

/// Pull output image names from a ComfyUI history entry. Mirrors the Go
/// `collectOutputs`.
fn collect_outputs(entry: &Value) -> Vec<Output> {
    let mut files = Vec::new();
    let Some(outputs) = entry.get("outputs").and_then(Value::as_object) else {
        return files;
    };
    for node in outputs.values() {
        let Some(images) = node.get("images").and_then(Value::as_array) else {
            continue;
        };
        for img in images {
            let filename = img.get("filename").and_then(Value::as_str).unwrap_or("").to_string();
            if filename.is_empty() {
                continue;
            }
            let subfolder = img.get("subfolder").and_then(Value::as_str).unwrap_or("").to_string();
            files.push(Output { subfolder, filename });
        }
    }
    files
}

fn join_rel(subfolder: &str, filename: &str) -> String {
    if subfolder.is_empty() {
        filename.to_string()
    } else {
        format!("{}/{}", subfolder.trim_end_matches('/'), filename)
    }
}

fn guess_mime(filename: &str) -> &'static str {
    match filename.rsplit('.').next().map(str::to_ascii_lowercase).as_deref() {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        _ => "application/octet-stream",
    }
}

/// Extract a human server error message from a JSON error body, else the raw text.
fn server_message(raw: &str) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(raw) {
        for key in ["message", "error", "detail"] {
            if let Some(s) = v.get(key).and_then(Value::as_str) {
                return s.to_string();
            }
        }
    }
    let t = raw.trim();
    if t.len() > 300 {
        format!("{}…", &t[..300])
    } else {
        t.to_string()
    }
}

/// Run the fleet loop until `cancel` fires. Registers, heartbeats every 30s,
/// polls for jobs every 2s, and cleanly marks the machine offline on shutdown.
///
/// This is the single entry point for both mount points: the standalone
/// `hanzo-fleet-worker` bin and the in-daemon (hanzod) background task.
pub async fn run(cfg: FleetConfig, cancel: CancellationToken) -> Result<()> {
    let mut worker = Worker::new(cfg)?;
    worker.refresh_engine().await;

    worker.register().await.context("register")?;
    log::info!(
        "connected {:?} to {} — {}; claiming {} jobs, heartbeat {}s",
        worker.hostname,
        worker.cfg.cloud_url,
        describe_gpus(&worker.gpus),
        worker.cfg.jobs_ns,
        HEARTBEAT_EVERY.as_secs()
    );

    // Heartbeat once immediately so the machine reports online without waiting.
    let _ = worker.heartbeat().await;

    let mut hb = tokio::time::interval(HEARTBEAT_EVERY);
    hb.tick().await; // consume the immediate first tick
    let mut poll = tokio::time::interval(CLAIM_POLL);
    poll.tick().await;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                log::info!("stopping; marking {:?} offline", worker.hostname);
                let _ = worker.disconnect().await;
                return Ok(());
            }
            _ = hb.tick() => {
                if let Err(e) = worker.heartbeat().await {
                    // A failed heartbeat usually means the cloud rolled/restarted
                    // and dropped our presence activity (the fleet store is not yet
                    // durable across restarts). Re-register so the machine comes
                    // back online on its own instead of silently going dark.
                    log::warn!("heartbeat: {e:#}; re-registering");
                    match worker.register().await {
                        Ok(()) => {
                            let _ = worker.heartbeat().await;
                            log::info!("re-registered {:?} after cloud restart", worker.hostname);
                        }
                        Err(re) => log::warn!("re-register: {re:#}"),
                    }
                }
            }
            _ = poll.tick() => {
                if let Err(e) = worker.claim_and_run(&cancel).await {
                    log::warn!("claim: {e:#}");
                }
            }
        }
    }
}

fn describe_gpus(gpus: &[GpuInfo]) -> String {
    if gpus.is_empty() {
        return "CPU-only".to_string();
    }
    gpus.iter()
        .map(|g| {
            if g.memory_total.is_empty() {
                g.name.clone()
            } else {
                format!("{} ({})", g.name, g.memory_total)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_matches_go() {
        assert_eq!(sanitize_id("Spark"), "spark");
        assert_eq!(sanitize_id("  My_Box.01 "), "my-box-01");
        assert_eq!(sanitize_id("---"), "worker");
        assert_eq!(sanitize_id(""), "worker");
    }

    #[test]
    fn collect_outputs_shape() {
        let entry = json!({
            "outputs": {
                "9": { "images": [ { "filename": "a.png", "subfolder": "" },
                                   { "filename": "b.png", "subfolder": "sub" } ] }
            }
        });
        let outs = collect_outputs(&entry);
        assert_eq!(outs.len(), 2);
        assert_eq!(join_rel(&outs[0].subfolder, &outs[0].filename), "a.png");
        assert_eq!(join_rel(&outs[1].subfolder, &outs[1].filename), "sub/b.png");
    }

    #[test]
    fn registration_is_camel_case() {
        let r = Registration {
            hostname: "spark".into(),
            os: "linux".into(),
            version: "1.1.20".into(),
            job_queue: "gpu-jobs".into(),
            gpus: vec![GpuInfo { name: "NVIDIA GB10".into(), memory_total: String::new() }],
            capabilities: vec![CAP_STUDIO_RENDER.into()],
            engine: None,
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["jobQueue"], "gpu-jobs");
        assert_eq!(v["gpus"][0]["name"], "NVIDIA GB10");
        assert!(v["gpus"][0].get("memoryTotal").is_none()); // skipped when empty
        assert!(v.get("engine").is_none());
    }

    #[test]
    fn guess_mime_by_ext() {
        assert_eq!(guess_mime("x.png"), "image/png");
        assert_eq!(guess_mime("x.JPG"), "image/jpeg");
        assert_eq!(guess_mime("x.bin"), "application/octet-stream");
    }
}
