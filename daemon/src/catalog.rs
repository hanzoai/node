//! `/v1/engine/catalog`: the Zen models this machine can run, which are on disk,
//! and which one the engine serves. Answered by hanzod itself, so it works
//! before the engine has ever started.
//!
//!   GET  /v1/engine/catalog          models, installed or not, with the memory-sized pick
//!   POST /v1/engine/catalog/pull     {"id"}: download one into the shared Hugging Face cache
//!   POST /v1/engine/catalog/select   {"id"}: serve it (the engine restarts onto it)
//!
//! Weights live in one cache, `<hub>/models--<org>--<repo>/`, the layout the
//! engine reads, so a model pulled here is never copied and one already pulled
//! by `hf` is already installed. A download is checked against the SHA-256 the
//! host publishes for the file before it is linked in.

use crate::engine::Engine;
use crate::ledger::Ledger;
use crate::table::{Fut, Reply, Subsystem};
use crate::zap::{Request, Response};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const GIB: u64 = 1 << 30;

/// Where weights are fetched from. One host; the engine never reaches it.
const HOST: &str = "https://huggingface.co";

pub struct Entry {
    pub id: &'static str,
    pub name: &'static str,
    pub repo: &'static str,
    pub file: &'static str,
    pub bytes: u64,
    /// The least RAM, in GiB, the machine should have to run it comfortably.
    pub memory: u64,
}

/// Smallest first. Each is a GGUF the shipped engine loads on Metal.
pub const MODELS: [Entry; 2] = [
    Entry { id: "zen-eco-3b-agent", name: "Zen Eco 3B Agent", repo: "zenlm/zen-eco-4b-agent-gguf", file: "zen-eco-3b-agent-q4_k_m.gguf", bytes: 1_929_902_464, memory: 8 },
    Entry { id: "zen-eco-3b-agent-q8", name: "Zen Eco 3B Agent (8-bit)", repo: "zenlm/zen-eco-4b-agent-gguf", file: "zen-eco-3b-agent-q8_0.gguf", bytes: 3_285_475_712, memory: 16 },
];

pub fn find(id: &str) -> Option<&'static Entry> {
    MODELS.iter().find(|m| m.id == id)
}

/// Physical memory in bytes.
pub fn memory() -> u64 {
    #[cfg(target_os = "macos")]
    {
        let mut v: u64 = 0;
        let mut n = std::mem::size_of::<u64>();
        let name = std::ffi::CString::new("hw.memsize").unwrap();
        let rc = unsafe { libc::sysctlbyname(name.as_ptr(), &mut v as *mut u64 as *mut _, &mut n, std::ptr::null_mut(), 0) };
        if rc == 0 {
            return v;
        }
        0
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|s| s.lines().find_map(|l| l.strip_prefix("MemTotal:")?.trim().strip_suffix("kB")?.trim().parse::<u64>().ok()))
            .map(|kb| kb * 1024)
            .unwrap_or(0)
    }
}

/// The largest model the memory runs comfortably, else the smallest.
pub fn recommended(memory_bytes: u64) -> &'static Entry {
    MODELS.iter().rev().find(|m| memory_bytes >= m.memory * GIB).unwrap_or(&MODELS[0])
}

/// The Hugging Face hub cache, as `hf` and the engine resolve it.
pub fn hub() -> PathBuf {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    env("HF_HUB_CACHE")
        .or_else(|| env("HF_HOME").map(|h| h.join("hub")))
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".cache").join("huggingface").join("hub"))
}

fn repo_dir(hub: &Path, repo: &str) -> PathBuf {
    hub.join(format!("models--{}", repo.replace('/', "--")))
}

/// The file's path in the cache, when it is there and whole.
pub fn installed(hub: &Path, e: &Entry) -> Option<PathBuf> {
    let dir = repo_dir(hub, e.repo);
    let commit = std::fs::read_to_string(dir.join("refs").join("main")).ok()?;
    let p = dir.join("snapshots").join(commit.trim()).join(e.file);
    (std::fs::metadata(&p).ok()?.len() == e.bytes).then_some(p)
}

#[derive(Clone, Default)]
struct Pull {
    state: &'static str,
    done: u64,
    total: u64,
    error: Option<String>,
}

pub struct Catalog {
    pub engine: Arc<Engine>,
    pub ledger: Arc<Ledger>,
    pulls: Arc<Mutex<HashMap<&'static str, Pull>>>,
}

impl Catalog {
    pub fn new(engine: Arc<Engine>, ledger: Arc<Ledger>) -> Catalog {
        Catalog { engine, ledger, pulls: Arc::new(Mutex::new(HashMap::new())) }
    }

    fn list(&self) -> Response {
        let hub = hub();
        let mem = memory();
        let selected = self.engine.model();
        let pulls = self.pulls.lock().unwrap();
        let models: Vec<Value> = MODELS
            .iter()
            .map(|m| {
                let p = pulls.get(m.id).cloned().unwrap_or_default();
                json!({
                    "id": m.id, "name": m.name, "bytes": m.bytes, "memory_gb": m.memory,
                    "installed": installed(&hub, m).is_some(),
                    "selected": selected.map(|s| s.id) == Some(m.id),
                    "pull": { "state": if p.state.is_empty() { "idle" } else { p.state }, "done": p.done, "total": p.total, "error": p.error },
                })
            })
            .collect();
        Response::json(200, &json!({ "memory_bytes": mem, "recommended": recommended(mem).id, "selected": selected.map(|s| s.id), "models": models }))
    }

    fn pull(&self, who: &str, id: &str) -> Response {
        let Some(e) = find(id) else { return Response::error(404, "no such model") };
        {
            let mut pulls = self.pulls.lock().unwrap();
            if pulls.get(e.id).is_some_and(|p| p.state == "pulling") {
                return Response::json(200, &json!({ "id": e.id, "state": "pulling" }));
            }
            pulls.insert(e.id, Pull { state: "pulling", total: e.bytes, ..Pull::default() });
        }
        self.ledger.record(who, "pull", &format!("{} from {}", e.id, e.repo));
        let pulls = self.pulls.clone();
        std::thread::spawn(move || {
            let result = download(&hub(), e, &|done| {
                if let Some(p) = pulls.lock().unwrap().get_mut(e.id) {
                    p.done = done;
                }
            });
            let mut pulls = pulls.lock().unwrap();
            let p = pulls.entry(e.id).or_default();
            match result {
                Ok(()) => {
                    p.state = "done";
                    p.done = p.total;
                    p.error = None;
                }
                Err(err) => {
                    p.state = "failed";
                    p.error = Some(err.to_string());
                }
            }
        });
        Response::json(200, &json!({ "id": e.id, "state": "pulling" }))
    }
}

/// Fetch `e` into the cache. The first request is not followed: its headers
/// name the commit and the file's SHA-256, and the redirect target is the bytes.
fn download(hub: &Path, e: &Entry, progress: &dyn Fn(u64)) -> io::Result<()> {
    let other = |m: String| io::Error::other(m);
    let agent = ureq::AgentBuilder::new().redirects(0).timeout_connect(Duration::from_secs(15)).timeout_read(Duration::from_secs(60)).build();
    let url = format!("{HOST}/{}/resolve/main/{}", e.repo, e.file);
    let head = agent.get(&url).call().map_err(|x| other(format!("{url}: {x}")))?;
    let commit = head.header("x-repo-commit").ok_or_else(|| other("the host named no commit".into()))?.to_string();
    let etag = head.header("x-linked-etag").unwrap_or("").trim_matches('"').to_string();
    if etag.len() != 64 || !etag.bytes().all(|b| b.is_ascii_hexdigit()) || commit.contains(['/', '.']) {
        return Err(other("the host named no SHA-256 for the file".into()));
    }
    let dir = repo_dir(hub, e.repo);
    let blob = dir.join("blobs").join(&etag);
    if !blob.is_file() {
        let part = dir.join("blobs").join(format!("{etag}.incomplete"));
        std::fs::create_dir_all(dir.join("blobs"))?;
        let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0).min(e.bytes);
        let target = head.header("location").ok_or_else(|| other("the host named no location".into()))?;
        let mut req = agent.get(target);
        if have > 0 {
            req = req.set("Range", &format!("bytes={have}-"));
        }
        let resp = req.call().map_err(|x| other(format!("download: {x}")))?;
        let resume = resp.status() == 206;
        let mut out = std::fs::OpenOptions::new().create(true).write(true).append(resume).truncate(!resume).open(&part)?;
        let mut done = if resume { have } else { 0 };
        let mut body = resp.into_reader();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = body.read(&mut buf)?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])?;
            done += n as u64;
            progress(done);
        }
        out.flush()?;
        drop(out);
        if std::fs::metadata(&part)?.len() != e.bytes {
            return Err(other("the download ended short; pull again to resume".into()));
        }
        if sha256_hex(&part)? != etag {
            let _ = std::fs::remove_file(&part);
            return Err(other("the download did not match the host's SHA-256".into()));
        }
        std::fs::rename(&part, &blob)?;
    }
    let snap = dir.join("snapshots").join(&commit);
    std::fs::create_dir_all(&snap)?;
    let link = snap.join(e.file);
    let _ = std::fs::remove_file(&link);
    #[cfg(unix)]
    std::os::unix::fs::symlink(Path::new("../../blobs").join(&etag), &link)?;
    std::fs::create_dir_all(dir.join("refs"))?;
    std::fs::write(dir.join("refs").join("main"), &commit)
}

fn sha256_hex(p: &Path) -> io::Result<String> {
    let mut f = std::fs::File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn id_of(req: &Request) -> Option<String> {
    serde_json::from_slice::<Value>(&req.body).ok()?.get("id")?.as_str().map(str::to_owned)
}

impl Subsystem for Catalog {
    fn name(&self) -> &'static str {
        "catalog"
    }
    fn prefix(&self) -> &'static str {
        "/v1/engine/catalog"
    }
    fn resident(&self) -> bool {
        true
    }
    fn start(&self) -> Fut<'_, io::Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn frame<'a>(&'a self, who: &'a str, req: Request) -> Fut<'a, io::Result<Reply>> {
        Box::pin(async move {
            let r = match (req.method.as_str(), req.path()) {
                ("GET", "/v1/engine/catalog") => self.list(),
                ("POST", "/v1/engine/catalog/pull") => match id_of(&req) {
                    Some(id) => self.pull(who, &id),
                    None => Response::error(400, "body must be {\"id\": \"<model>\"}"),
                },
                ("POST", "/v1/engine/catalog/select") => match id_of(&req).as_deref().and_then(find) {
                    Some(e) if installed(&hub(), e).is_some() => {
                        self.ledger.record(who, "select", e.id);
                        match self.engine.select(e.id).await {
                            Ok(()) => Response::json(200, &json!({ "selected": e.id })),
                            Err(x) => Response::error(502, &x.to_string()),
                        }
                    }
                    Some(_) => Response::error(409, "that model is not installed; pull it first"),
                    None => Response::error(404, "no such model"),
                },
                _ => Response::error(405, "GET the catalog, POST pull or select"),
            };
            Ok(Reply::Whole(r))
        })
    }
    fn stop(&self) -> Fut<'_, ()> {
        Box::pin(async {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pick_follows_memory() {
        assert_eq!(recommended(0).id, "zen-eco-3b-agent");
        assert_eq!(recommended(8 * GIB).id, "zen-eco-3b-agent");
        assert_eq!(recommended(16 * GIB).id, "zen-eco-3b-agent-q8");
        assert_eq!(recommended(64 * GIB).id, "zen-eco-3b-agent-q8");
    }

    #[test]
    fn a_model_is_installed_only_when_its_file_is_whole() {
        let d = tempfile::tempdir().unwrap();
        let e = &MODELS[0];
        assert!(installed(d.path(), e).is_none());
        let dir = repo_dir(d.path(), e.repo);
        std::fs::create_dir_all(dir.join("snapshots/abc")).unwrap();
        std::fs::create_dir_all(dir.join("refs")).unwrap();
        std::fs::write(dir.join("refs/main"), "abc").unwrap();
        std::fs::write(dir.join("snapshots/abc").join(e.file), b"short").unwrap();
        assert!(installed(d.path(), e).is_none());
        let f = std::fs::File::create(dir.join("snapshots/abc").join(e.file)).unwrap();
        f.set_len(e.bytes).unwrap();
        assert!(installed(d.path(), e).is_some());
    }
}
