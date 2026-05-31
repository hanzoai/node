//! Native local-model scanner.
//!
//! Scans the local filesystem for models installed by common local-LLM tools
//! and returns a unified list of records whose JSON shape matches the desktop
//! frontend's `ScanLocalModelsResponse` element type (see
//! `hanzo-message-ts/src/api/local-models.ts`). Two extra keys (`source`,
//! `path`) are appended for richer UI/debugging; the frontend reads structurally
//! and ignores unknown keys.
//!
//! Sources scanned (each best-effort; a missing directory yields no records and
//! never errors):
//!   - Ollama:      `~/.ollama/models/manifests/**`  (+ blob config metadata)
//!   - HuggingFace: `~/.cache/huggingface/hub/models--*`
//!   - LM Studio:   `~/.lmstudio/models/**`, `~/.cache/lm-studio/models/**`
//!   - Hanzo:       `~/.hanzo/models/**`, `~/.hanzo/llm/**`
//!
//! Everything here uses only `std::fs`, `serde_json` and `chrono`. The home
//! directory is resolved via `$HOME` so no extra crate dependency is required.
//! No function panics; all I/O failures degrade to empty contributions.

use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Nested `details` object — mirrors `ScanLocalModelsResponse[number].details`.
/// Every field is always emitted (empty string / empty vec when unknown) so the
/// frontend never sees `undefined`.
#[derive(Debug, Clone, Serialize)]
pub struct ModelDetails {
    pub format: String,
    pub family: String,
    pub families: Vec<String>,
    pub parameter_size: String,
    pub quantization_level: String,
    pub parent_model: String,
}

impl Default for ModelDetails {
    fn default() -> Self {
        ModelDetails {
            format: String::new(),
            family: String::new(),
            families: Vec::new(),
            parameter_size: String::new(),
            quantization_level: String::new(),
            parent_model: String::new(),
        }
    }
}

/// One discovered model. Serializes to exactly the frontend record shape plus
/// the extra `source` and `path` keys.
#[derive(Debug, Clone, Serialize)]
pub struct LocalModelRecord {
    pub model: String,
    pub name: String,
    pub digest: String,
    pub modified_at: String,
    pub size: u64,
    pub port_used: String,
    pub details: ModelDetails,
    /// Origin of this record: "ollama" | "huggingface" | "lmstudio" | "hanzo".
    pub source: String,
    /// Absolute path to the on-disk artifact (manifest / snapshot dir / gguf).
    pub path: String,
}

/// Public entry point. Scans every known source and returns deduplicated,
/// sorted JSON records. Never panics, never errors.
pub fn scan_all_local_models() -> Vec<serde_json::Value> {
    let home = match home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };

    let mut records: Vec<LocalModelRecord> = Vec::new();

    // Each scanner is fully self-contained and swallows its own errors.
    records.extend(scan_ollama(&home));
    records.extend(scan_huggingface(&home));
    records.extend(scan_lmstudio(&home));
    records.extend(scan_hanzo(&home));

    // Dedupe on the canonical `model` id (stable: keep first occurrence) and
    // sort for deterministic output.
    let mut seen: HashSet<String> = HashSet::new();
    records.retain(|r| seen.insert(r.model.clone()));
    records.sort_by(|a, b| a.model.cmp(&b.model));

    records
        .into_iter()
        .filter_map(|r| serde_json::to_value(r).ok())
        .collect()
}

/// Resolve the user's home directory from `$HOME` (no extra dep). Falls back to
/// `USERPROFILE` for Windows-y environments.
fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .or_else(|| std::env::var("USERPROFILE").ok())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

/// RFC-3339 string from a file/dir's mtime; empty string on any failure.
fn mtime_rfc3339(path: &Path) -> String {
    match fs::metadata(path).and_then(|m| m.modified()) {
        Ok(t) => system_time_rfc3339(t),
        Err(_) => String::new(),
    }
}

fn system_time_rfc3339(t: SystemTime) -> String {
    let dt: DateTime<Utc> = t.into();
    dt.to_rfc3339()
}

/// Collect every regular file under `root` up to `max_depth` directory levels
/// (depth 0 = direct children). Best-effort; unreadable dirs are skipped.
fn collect_files(root: &Path, max_depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_files_inner(root, max_depth, &mut out);
    out
}

fn collect_files_inner(dir: &Path, depth_left: usize, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // `file_type` does not traverse symlinks; resolve via metadata for files.
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if ft.is_dir() {
            if depth_left > 0 {
                collect_files_inner(&path, depth_left - 1, out);
            }
        } else {
            out.push(path);
        }
    }
}

/// Sum file sizes for a set of paths, following symlinks (HF blobs are
/// symlinks). Unreadable entries contribute 0.
fn sum_file_sizes(paths: &[PathBuf]) -> u64 {
    paths
        .iter()
        .filter_map(|p| fs::metadata(p).ok())
        .map(|m| m.len())
        .sum()
}

/// True if a path's lowercase extension is a model weight file.
fn is_weight_file(path: &Path) -> bool {
    matches!(
        ext_lower(path).as_deref(),
        Some("gguf") | Some("safetensors") | Some("bin") | Some("pt") | Some("pth")
    )
}

fn ext_lower(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

fn file_stem_str(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string()
}

/// Extract a parameter-size token like `0.6b`, `7B`, `494m` from a name.
/// Returns the normalized form (digits + lowercase unit), or "" if none.
fn parse_param_size(name: &str) -> String {
    let bytes = name.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_ascii_digit() {
            let start = i;
            // consume digits and at most one dot
            let mut seen_dot = false;
            while i < bytes.len() {
                let cc = bytes[i] as char;
                if cc.is_ascii_digit() {
                    i += 1;
                } else if cc == '.' && !seen_dot {
                    seen_dot = true;
                    i += 1;
                } else {
                    break;
                }
            }
            if i < bytes.len() {
                let unit = bytes[i] as char;
                if matches!(unit, 'b' | 'B' | 'm' | 'M') {
                    // Ensure the unit is a boundary (next char not alphanumeric),
                    // to avoid matching things like "32bit" or "8bits"? Allow it
                    // anyway for "0.6b". Reject when followed by more letters that
                    // would make it a word (e.g. "base").
                    let after_ok = match bytes.get(i + 1) {
                        None => true,
                        Some(&n) => {
                            let nc = n as char;
                            !nc.is_ascii_alphabetic()
                        }
                    };
                    if after_ok {
                        let num = &name[start..i];
                        return format!("{}{}", num, unit.to_ascii_uppercase());
                    }
                }
            }
            // not a size; continue scanning after the consumed digits
        } else {
            i += 1;
        }
    }
    String::new()
}

/// Extract a quantization token like `Q4_K_M`, `Q8_0`, `IQ3_XS`, `F16` from a
/// gguf-style filename. Returns "" if none found.
fn parse_quant(name: &str) -> String {
    // Tokenize on common separators and look for quant-shaped tokens.
    for token in name.split(|c: char| c == '-' || c == '.' || c == '_' || c == ' ') {
        let up = token.to_ascii_uppercase();
        let b = up.as_bytes();
        // Q<digit>... or IQ<digit>...
        if (up.starts_with('Q') && b.len() >= 2 && (b[1] as char).is_ascii_digit())
            || (up.starts_with("IQ") && b.len() >= 3 && (b[2] as char).is_ascii_digit())
        {
            return up;
        }
        // F16 / F32 / BF16 / FP16 / FP8
        if matches!(up.as_str(), "F16" | "F32" | "BF16" | "FP16" | "FP8" | "FP32") {
            return up;
        }
    }
    String::new()
}

// ---------------------------------------------------------------------------
// Source A — Ollama
// ---------------------------------------------------------------------------

/// `~/.ollama/models/manifests/<registry>/<namespace>/<model>/<tag>` where each
/// leaf is a manifest JSON file. The matching blobs live in
/// `~/.ollama/models/blobs/sha256-<hex>`.
fn scan_ollama(home: &Path) -> Vec<LocalModelRecord> {
    let models_dir = home.join(".ollama").join("models");
    let manifests_dir = models_dir.join("manifests");
    let blobs_dir = models_dir.join("blobs");

    if !manifests_dir.is_dir() {
        return Vec::new();
    }

    // Manifests live at depth registry/namespace/model/tag => up to 4 dir levels
    // below `manifests/`. Allow a little extra slack for nested namespaces.
    let files = collect_files(&manifests_dir, 6);

    let mut out = Vec::new();
    for manifest_path in files {
        if let Some(rec) = parse_ollama_manifest(&manifests_dir, &manifest_path, &blobs_dir) {
            out.push(rec);
        }
    }
    out
}

fn parse_ollama_manifest(
    manifests_root: &Path,
    manifest_path: &Path,
    blobs_dir: &Path,
) -> Option<LocalModelRecord> {
    let bytes = fs::read(manifest_path).ok()?;
    let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;

    // Build model id from path components relative to `manifests/`.
    let rel = manifest_path.strip_prefix(manifests_root).ok()?;
    let comps: Vec<String> = rel
        .components()
        .filter_map(|c| c.as_os_str().to_str().map(|s| s.to_string()))
        .collect();
    // Expect at least <registry> <namespace> <model> <tag>.
    if comps.len() < 4 {
        return None;
    }
    let tag = comps.last().cloned().unwrap_or_default();
    let model_name = comps[comps.len() - 2].clone();
    let namespace = comps[comps.len() - 3].clone();
    let registry = comps[0].clone();

    let model_id = if registry == "registry.ollama.ai" && namespace == "library" {
        // Collapse the canonical library namespace: `qwen2.5:0.5b`.
        format!("{}:{}", model_name, tag)
    } else if registry == "registry.ollama.ai" {
        format!("{}/{}:{}", namespace, model_name, tag)
    } else {
        // e.g. hf.co/<ns>/<model>:<tag>
        format!("{}/{}/{}:{}", registry, namespace, model_name, tag)
    };

    // Sum layer sizes; capture the model-layer digest and config digest.
    let mut size: u64 = 0;
    let mut model_digest = String::new();
    if let Some(layers) = json.get("layers").and_then(|l| l.as_array()) {
        for layer in layers {
            if let Some(sz) = layer.get("size").and_then(|s| s.as_u64()) {
                size = size.saturating_add(sz);
            }
            let media = layer.get("mediaType").and_then(|m| m.as_str()).unwrap_or("");
            if media == "application/vnd.ollama.image.model" {
                if let Some(d) = layer.get("digest").and_then(|d| d.as_str()) {
                    model_digest = strip_sha_prefix(d);
                }
            }
        }
    }

    let config_digest = json
        .get("config")
        .and_then(|c| c.get("digest"))
        .and_then(|d| d.as_str())
        .map(|s| s.to_string());

    // Fall back to config digest if no model layer digest was found.
    if model_digest.is_empty() {
        if let Some(cd) = &config_digest {
            model_digest = strip_sha_prefix(cd);
        }
    }

    // Read the config blob for details metadata.
    let mut details = ModelDetails::default();
    if let Some(cd) = &config_digest {
        let blob_file = sha_ref_to_blob_path(blobs_dir, cd);
        if let Ok(cfg_bytes) = fs::read(&blob_file) {
            if let Ok(cfg) = serde_json::from_slice::<serde_json::Value>(&cfg_bytes) {
                details.format = cfg
                    .get("model_format")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                details.family = cfg
                    .get("model_family")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if let Some(fams) = cfg.get("model_families").and_then(|v| v.as_array()) {
                    details.families = fams
                        .iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect();
                }
                // In the Ollama *config blob*, `model_type` is the parameter
                // count string (e.g. "494.03M") — NOT an architecture name.
                details.parameter_size = cfg
                    .get("model_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                details.quantization_level = cfg
                    .get("file_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
            }
        }
    }
    if details.format.is_empty() {
        details.format = "gguf".to_string();
    }
    if details.parameter_size.is_empty() {
        details.parameter_size = parse_param_size(&model_id);
    }

    Some(LocalModelRecord {
        model: model_id.clone(),
        name: model_id,
        digest: model_digest,
        modified_at: mtime_rfc3339(manifest_path),
        size,
        port_used: String::new(),
        details,
        source: "ollama".to_string(),
        path: manifest_path.to_string_lossy().to_string(),
    })
}

fn strip_sha_prefix(d: &str) -> String {
    d.strip_prefix("sha256:")
        .or_else(|| d.strip_prefix("sha256-"))
        .unwrap_or(d)
        .to_string()
}

/// Translate a manifest digest reference (`sha256:<hex>`) to the on-disk blob
/// filename (`sha256-<hex>`).
fn sha_ref_to_blob_path(blobs_dir: &Path, digest_ref: &str) -> PathBuf {
    let file_name = digest_ref.replacen("sha256:", "sha256-", 1);
    blobs_dir.join(file_name)
}

// ---------------------------------------------------------------------------
// Source B — HuggingFace hub cache
// ---------------------------------------------------------------------------

/// `~/.cache/huggingface/hub/models--<org>--<repo>` with the active snapshot in
/// `snapshots/<commit>/` (pointed at by `refs/main`). Files in the snapshot are
/// symlinks into `../../blobs/<sha>`.
fn scan_huggingface(home: &Path) -> Vec<LocalModelRecord> {
    let hub = home.join(".cache").join("huggingface").join("hub");
    if !hub.is_dir() {
        return Vec::new();
    }

    let entries = match fs::read_dir(&hub) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let dir_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if !dir_name.starts_with("models--") {
            continue;
        }
        if let Some(rec) = parse_huggingface_repo(&path, dir_name) {
            out.push(rec);
        }
    }
    out
}

fn parse_huggingface_repo(repo_dir: &Path, dir_name: &str) -> Option<LocalModelRecord> {
    // models--zenlm--zen-nano-0.6b -> zenlm/zen-nano-0.6b
    let trimmed = dir_name.strip_prefix("models--").unwrap_or(dir_name);
    let model_id = trimmed.replace("--", "/");

    // Resolve the active snapshot.
    let snapshots_dir = repo_dir.join("snapshots");
    let commit = resolve_hf_commit(repo_dir, &snapshots_dir);
    let snapshot_dir = match &commit {
        Some(c) => snapshots_dir.join(c),
        None => return None,
    };
    if !snapshot_dir.is_dir() {
        return None;
    }

    // Gather snapshot files (symlinks). Snapshots are usually flat but some
    // repos nest subfolders; recurse a couple levels to be safe.
    let files = collect_files(&snapshot_dir, 3);

    // Sum sizes of weight files; if none found, sum everything resolved.
    let weight_files: Vec<PathBuf> = files.iter().filter(|p| is_weight_file(p)).cloned().collect();
    let size = if weight_files.is_empty() {
        sum_file_sizes(&files)
    } else {
        sum_file_sizes(&weight_files)
    };

    // Determine format.
    let has_safetensors = files
        .iter()
        .any(|p| ext_lower(p).as_deref() == Some("safetensors"));
    let has_gguf = files.iter().any(|p| ext_lower(p).as_deref() == Some("gguf"));
    let format = if has_safetensors {
        "safetensors".to_string()
    } else if has_gguf {
        "gguf".to_string()
    } else {
        String::new()
    };

    // Read config.json for family / architectures / quantization.
    let mut details = ModelDetails {
        format,
        ..Default::default()
    };
    let config_path = snapshot_dir.join("config.json");
    if let Ok(cfg_bytes) = fs::read(&config_path) {
        if let Ok(cfg) = serde_json::from_slice::<serde_json::Value>(&cfg_bytes) {
            if let Some(mt) = cfg.get("model_type").and_then(|v| v.as_str()) {
                details.family = mt.to_string();
                details.families = vec![mt.to_string()];
            }
            if details.families.is_empty() {
                if let Some(arch) = cfg.get("architectures").and_then(|v| v.as_array()) {
                    details.families = arch
                        .iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect();
                    if details.family.is_empty() {
                        if let Some(first) = details.families.first() {
                            details.family = first.clone();
                        }
                    }
                }
            }
            // quantization_config may be an object describing the quant method.
            if let Some(qc) = cfg.get("quantization_config") {
                if !qc.is_null() {
                    if let Some(method) = qc.get("quant_method").and_then(|v| v.as_str()) {
                        details.quantization_level = method.to_string();
                    } else if let Some(s) = qc.as_str() {
                        details.quantization_level = s.to_string();
                    }
                }
            }
        }
    }

    // Parameter size from the repo/dir name (e.g. "0.6b", "35B").
    details.parameter_size = parse_param_size(trimmed);

    // Quantization hints from the repo name (e.g. "-FP8", "-Q4_K_M").
    if details.quantization_level.is_empty() {
        let q = parse_quant(trimmed);
        if !q.is_empty() {
            details.quantization_level = q;
        }
    }

    Some(LocalModelRecord {
        model: model_id.clone(),
        name: model_id,
        digest: commit.unwrap_or_default(),
        modified_at: mtime_rfc3339(&snapshot_dir),
        size,
        port_used: String::new(),
        details,
        source: "huggingface".to_string(),
        path: snapshot_dir.to_string_lossy().to_string(),
    })
}

/// Read `refs/main` for the active commit; fall back to the newest snapshot dir.
fn resolve_hf_commit(repo_dir: &Path, snapshots_dir: &Path) -> Option<String> {
    let ref_main = repo_dir.join("refs").join("main");
    if let Ok(content) = fs::read_to_string(&ref_main) {
        let c = content.trim().to_string();
        if !c.is_empty() {
            return Some(c);
        }
    }
    // Fallback: newest directory under snapshots/.
    let entries = fs::read_dir(snapshots_dir).ok()?;
    let mut best: Option<(SystemTime, String)> = None;
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        let name = match p.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let mtime = fs::metadata(&p)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        match &best {
            Some((bt, _)) if *bt >= mtime => {}
            _ => best = Some((mtime, name)),
        }
    }
    best.map(|(_, name)| name)
}

// ---------------------------------------------------------------------------
// Source C — LM Studio
// ---------------------------------------------------------------------------

/// `~/.lmstudio/models/<publisher>/<repo>/<file>.gguf` (and the older
/// `~/.cache/lm-studio/models/...`). Discover `*.gguf` files.
fn scan_lmstudio(home: &Path) -> Vec<LocalModelRecord> {
    let roots = [
        home.join(".lmstudio").join("models"),
        home.join(".cache").join("lm-studio").join("models"),
    ];

    let mut out = Vec::new();
    for root in roots.iter() {
        if !root.is_dir() {
            continue;
        }
        for file in collect_files(root, 6) {
            if ext_lower(&file).as_deref() != Some("gguf") {
                continue;
            }
            out.push(gguf_record_from_path(root, &file, "lmstudio"));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Source D — Hanzo
// ---------------------------------------------------------------------------

/// Forward-looking: scan `~/.hanzo/models/**` and `~/.hanzo/llm/**` for local
/// weight files so engine-downloaded models surface in the UI. Also honors the
/// `NATIVE_MODEL_PATH` / `RERANKER_MODEL_PATH` env vars if they point at files.
fn scan_hanzo(home: &Path) -> Vec<LocalModelRecord> {
    let roots = [
        home.join(".hanzo").join("models"),
        home.join(".hanzo").join("llm"),
    ];

    let mut out = Vec::new();
    for root in roots.iter() {
        if !root.is_dir() {
            continue;
        }
        for file in collect_files(root, 6) {
            if !is_weight_file(&file) {
                continue;
            }
            out.push(gguf_record_from_path(root, &file, "hanzo"));
        }
    }

    // Explicit engine model paths from the environment.
    for env_key in ["NATIVE_MODEL_PATH", "RERANKER_MODEL_PATH"].iter() {
        if let Ok(val) = std::env::var(env_key) {
            if val.is_empty() {
                continue;
            }
            let p = PathBuf::from(&val);
            if p.is_file() && is_weight_file(&p) {
                let parent = p.parent().unwrap_or(home);
                out.push(gguf_record_from_path(parent, &p, "hanzo"));
            }
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Shared helper for plain weight-file sources (LM Studio / Hanzo)
// ---------------------------------------------------------------------------

/// Build a record for a standalone weight file. The model id is
/// `<publisher>/<repo>` when at least two directory levels sit under `root`,
/// otherwise the file stem.
fn gguf_record_from_path(root: &Path, file: &Path, source: &str) -> LocalModelRecord {
    let stem = file_stem_str(file);
    let model_id = derive_publisher_repo_id(root, file).unwrap_or_else(|| stem.clone());

    let size = fs::metadata(file).map(|m| m.len()).unwrap_or(0);
    let format = match ext_lower(file).as_deref() {
        Some("gguf") => "gguf",
        Some("safetensors") => "safetensors",
        _ => "",
    }
    .to_string();

    let details = ModelDetails {
        format,
        parameter_size: parse_param_size(&stem),
        quantization_level: parse_quant(&stem),
        ..Default::default()
    };

    LocalModelRecord {
        model: model_id.clone(),
        name: model_id,
        digest: String::new(),
        modified_at: mtime_rfc3339(file),
        size,
        port_used: String::new(),
        details,
        source: source.to_string(),
        path: file.to_string_lossy().to_string(),
    }
}

/// Given a file like `<root>/<publisher>/<repo>/model.gguf`, return
/// `<publisher>/<repo>`. Returns None if fewer than two dir levels separate the
/// file from `root`.
fn derive_publisher_repo_id(root: &Path, file: &Path) -> Option<String> {
    let rel = file.strip_prefix(root).ok()?;
    let dirs: Vec<String> = rel
        .parent()?
        .components()
        .filter_map(|c| c.as_os_str().to_str().map(|s| s.to_string()))
        .filter(|s| !s.is_empty())
        .collect();
    match dirs.len() {
        0 => None,
        1 => Some(dirs[0].clone()),
        _ => {
            // Use the two deepest directory levels as publisher/repo.
            let n = dirs.len();
            Some(format!("{}/{}", dirs[n - 2], dirs[n - 1]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_param_size() {
        assert_eq!(parse_param_size("zen-nano-0.6b"), "0.6B");
        assert_eq!(parse_param_size("Qwen3-Embedding-0.6B"), "0.6B");
        assert_eq!(parse_param_size("llama-3-70B-instruct"), "70B");
        assert_eq!(parse_param_size("model-494m"), "494M");
        assert_eq!(parse_param_size("base-model"), "");
        assert_eq!(parse_param_size("no-size-here"), "");
    }

    #[test]
    fn test_parse_quant() {
        assert_eq!(parse_quant("model-Q4_K_M.gguf"), "Q4_K_M");
        assert_eq!(parse_quant("model.Q8_0"), "Q8_0");
        assert_eq!(parse_quant("model-IQ3_XS-here"), "IQ3_XS");
        assert_eq!(parse_quant("model-f16"), "F16");
        assert_eq!(parse_quant("repo-FP8"), "FP8");
        assert_eq!(parse_quant("plain-model"), "");
    }

    #[test]
    fn test_strip_sha_prefix() {
        assert_eq!(strip_sha_prefix("sha256:abc123"), "abc123");
        assert_eq!(strip_sha_prefix("sha256-abc123"), "abc123");
        assert_eq!(strip_sha_prefix("abc123"), "abc123");
    }

    #[test]
    fn test_sha_ref_to_blob_path() {
        let blobs = Path::new("/home/u/.ollama/models/blobs");
        let p = sha_ref_to_blob_path(blobs, "sha256:deadbeef");
        assert_eq!(p, Path::new("/home/u/.ollama/models/blobs/sha256-deadbeef"));
    }

    #[test]
    fn test_derive_publisher_repo_id() {
        let root = Path::new("/m/models");
        assert_eq!(
            derive_publisher_repo_id(root, Path::new("/m/models/pub/repo/x.gguf")),
            Some("pub/repo".to_string())
        );
        assert_eq!(
            derive_publisher_repo_id(root, Path::new("/m/models/repo/x.gguf")),
            Some("repo".to_string())
        );
        assert_eq!(
            derive_publisher_repo_id(root, Path::new("/m/models/x.gguf")),
            None
        );
    }

    #[test]
    fn test_scan_never_panics_on_missing_home_dirs() {
        // Point HOME at an empty temp dir; every source dir is absent.
        let tmp = std::env::temp_dir().join(format!("hanzo_scan_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&tmp);
        let recs = scan_ollama(&tmp);
        assert!(recs.is_empty());
        let recs = scan_huggingface(&tmp);
        assert!(recs.is_empty());
        let recs = scan_lmstudio(&tmp);
        assert!(recs.is_empty());
        let recs = scan_hanzo(&tmp);
        assert!(recs.is_empty());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_record_serializes_to_frontend_shape() {
        let rec = LocalModelRecord {
            model: "qwen2.5:0.5b".to_string(),
            name: "qwen2.5:0.5b".to_string(),
            digest: "abc".to_string(),
            modified_at: "2025-01-01T00:00:00+00:00".to_string(),
            size: 123,
            port_used: String::new(),
            details: ModelDetails::default(),
            source: "ollama".to_string(),
            path: "/x".to_string(),
        };
        let v = serde_json::to_value(&rec).unwrap();
        // Required frontend keys present.
        for key in [
            "model",
            "name",
            "digest",
            "modified_at",
            "size",
            "port_used",
            "details",
        ] {
            assert!(v.get(key).is_some(), "missing key {}", key);
        }
        let details = v.get("details").unwrap();
        for key in [
            "format",
            "family",
            "families",
            "parameter_size",
            "quantization_level",
            "parent_model",
        ] {
            assert!(details.get(key).is_some(), "missing details key {}", key);
        }
        // Extra keys for richer UI.
        assert_eq!(v.get("source").unwrap(), "ollama");
        assert_eq!(v.get("path").unwrap(), "/x");
    }
}
