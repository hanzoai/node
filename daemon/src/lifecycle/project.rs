// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! Static project discovery & multi-project isolation (`hanzo-app-<slug>`).
//!
//! Inspects project manifests purely statically (never executing untrusted code)
//! to distinguish `hanzoai/cloud` from user applications and derive canonical
//! slugs, display names, Kubernetes namespaces, and conflict-free endpoints.

use super::state::AppState;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetectedRepo {
    CloudPlatform { project_dir: PathBuf },
    UserApp(DiscoveredApp),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredApp {
    pub slug: String,
    pub name: String,
    pub project_dir: PathBuf,
    pub namespace: String,
    pub manifest_kind: String,
    pub preferred_port: Option<u16>,
}

impl DiscoveredApp {
    pub fn assign_endpoints(&self, occupied_ports: &BTreeSet<u16>) -> (u16, Vec<String>) {
        let port = choose_port(&self.slug, self.preferred_port, occupied_ports);
        let endpoints = vec![
            format!("http://{}.hanzo.localhost", self.slug),
            format!("http://127.0.0.1:{port}"),
            format!(
                "http://{}.{}.svc.cluster.local:{port}",
                self.slug, self.namespace
            ),
        ];
        (port, endpoints)
    }
}

pub fn canonical_dir(dir: &Path) -> PathBuf {
    std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf())
}

pub fn is_cloud_platform_repo(dir: &Path) -> bool {
    if dir.join("cmd").join("cloud-host").is_dir()
        || dir.join("cmd").join("cloud-host").join("main.go").is_file()
    {
        return true;
    }
    if let Ok(gomod) = std::fs::read_to_string(dir.join("go.mod")) {
        for line in gomod.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("module ") {
                let m = rest.trim();
                if m == "github.com/hanzoai/cloud"
                    || m.starts_with("github.com/hanzoai/cloud/")
                    || m == "hanzo.ai/cloud"
                {
                    return true;
                }
            }
        }
    }
    if let Ok(git_cfg) = std::fs::read_to_string(dir.join(".git").join("config")) {
        let lower = git_cfg.to_ascii_lowercase();
        if lower.contains("hanzoai/cloud.git")
            || lower.contains("hanzoai/cloud\n")
            || lower.contains("hanzoai/cloud\r")
        {
            return true;
        }
    }
    if let Ok(raw) = std::fs::read_to_string(dir.join("hanzo.toml")) {
        if let Ok(tbl) = raw.parse::<toml::Table>() {
            let kind = tbl
                .get("kind")
                .or_else(|| tbl.get("project").and_then(|p| p.get("kind")))
                .and_then(|v| v.as_str());
            let repo = tbl
                .get("repo")
                .or_else(|| tbl.get("project").and_then(|p| p.get("repo")))
                .and_then(|v| v.as_str());
            if kind == Some("cloud-platform") || repo == Some("hanzoai/cloud") {
                return true;
            }
        }
    }
    false
}

pub fn discover_project(dir: &Path) -> Option<DetectedRepo> {
    if !dir.is_dir() {
        return None;
    }
    let canon = canonical_dir(dir);
    if is_cloud_platform_repo(&canon) {
        return Some(DetectedRepo::CloudPlatform { project_dir: canon });
    }

    let fallback_raw = canon
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("app");

    let (manifest_kind, raw_slug, raw_display, preferred_port) =
        if canon.join("hanzo.toml").is_file() {
            let (s, n, p) = inspect_hanzo_toml(&canon.join("hanzo.toml"));
            ("hanzo.toml", s, n, p)
        } else if canon.join("hanzo.yaml").is_file() {
            let (s, n, p) = inspect_yaml_like(&canon.join("hanzo.yaml"));
            ("hanzo.yaml", s, n, p)
        } else if canon.join("hanzo.yml").is_file() {
            let (s, n, p) = inspect_yaml_like(&canon.join("hanzo.yml"));
            ("hanzo.yaml", s, n, p)
        } else if let Some(cfg_path) = find_first_file(
            &canon,
            &[
                "hanzo.config.json",
                "hanzo.config.ts",
                "hanzo.config.js",
                "hanzo.config.mjs",
            ],
        ) {
            let (s, n, p) = inspect_hanzo_config(&cfg_path);
            ("hanzo.config", s, n, p)
        } else if let Some(comp_path) = find_first_file(
            &canon,
            &[
                "compose.yaml",
                "compose.yml",
                "docker-compose.yml",
                "docker-compose.yaml",
            ],
        ) {
            let (s, n, p) = inspect_compose(&comp_path);
            ("compose", s, n, p)
        } else if let Some(df_path) = find_first_file(&canon, &["Dockerfile", "Containerfile"]) {
            let p = inspect_dockerfile_port(&df_path);
            ("dockerfile", None, None, p)
        } else if canon.join("package.json").is_file() {
            let (s, n, p) = inspect_package_json(&canon.join("package.json"));
            ("node", s, n, p)
        } else if canon.join("Cargo.toml").is_file() {
            let (s, p) = inspect_cargo_toml(&canon.join("Cargo.toml"));
            ("rust", s, None, p)
        } else if canon.join("go.mod").is_file() {
            let s = inspect_go_mod(&canon.join("go.mod"));
            ("go", s, None, None)
        } else {
            return None;
        };

    let base_for_slug = raw_slug
        .as_deref()
        .or(raw_display.as_deref())
        .unwrap_or(fallback_raw);
    let slug = slugify(base_for_slug);
    if slug.is_empty() {
        return None;
    }
    let name = match raw_display {
        Some(n) if has_explicit_display_casing(&n) => n.trim().to_string(),
        _ => humanize_slug(&slug),
    };
    let namespace = format!("hanzo-app-{slug}");

    Some(DetectedRepo::UserApp(DiscoveredApp {
        slug,
        name,
        project_dir: canon,
        namespace,
        manifest_kind: manifest_kind.to_string(),
        preferred_port,
    }))
}

fn find_first_file(dir: &Path, names: &[&str]) -> Option<PathBuf> {
    for name in names {
        let p = dir.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn has_explicit_display_casing(s: &str) -> bool {
    let t = s.trim();
    !t.is_empty() && (t.contains(' ') || t.chars().any(|c| c.is_ascii_uppercase()))
}

pub fn slugify(raw: &str) -> String {
    let stripped = raw.trim().trim_start_matches('@');
    let tail = stripped.rsplit('/').next().unwrap_or(stripped);
    let mut out = String::with_capacity(tail.len());
    let mut prev_dash = false;
    for ch in tail.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
        if out.len() >= 48 {
            break;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "app".to_string()
    } else {
        out
    }
}

pub fn humanize_slug(slug: &str) -> String {
    const ACRONYMS: &[&str] = &[
        "ai", "api", "cli", "crm", "cms", "db", "erp", "gpu", "iam", "id", "io", "kms", "llm",
        "mcp", "ml", "oss", "rpc", "sdk", "sql", "sso", "tls", "ui", "ux", "vfs", "vm",
    ];
    let parts: Vec<String> = slug
        .split('-')
        .filter(|s| !s.is_empty())
        .map(|part| {
            let lower = part.to_ascii_lowercase();
            if ACRONYMS.contains(&lower.as_str()) {
                lower.to_ascii_uppercase()
            } else {
                let mut chars = lower.chars();
                match chars.next() {
                    Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                    None => String::new(),
                }
            }
        })
        .collect();
    if parts.is_empty() {
        "App".to_string()
    } else {
        parts.join(" ")
    }
}

pub fn deterministic_base_port(slug: &str) -> u16 {
    let mut h = Sha256::new();
    h.update(slug.as_bytes());
    let digest = h.finalize();
    let raw = u16::from_be_bytes([digest[0], digest[1]]);
    14000 + (raw % 10000)
}

pub fn choose_port(
    slug: &str,
    preferred: Option<u16>,
    occupied_ports: &BTreeSet<u16>,
) -> u16 {
    if let Some(p) = preferred {
        if p >= 1024 && p != 3690 && p != 6443 && !occupied_ports.contains(&p) {
            return p;
        }
    }
    let start = deterministic_base_port(slug);
    for offset in 0..10000u16 {
        let cand = 14000 + ((start - 14000 + offset) % 10000);
        if !occupied_ports.contains(&cand) {
            return cand;
        }
    }
    start
}

pub fn resolve_app_slug(
    apps: &BTreeMap<String, AppState>,
    query: &str,
) -> Result<String, String> {
    let q = query.trim();
    if q.is_empty() {
        return Err("application name or slug is required".to_string());
    }
    let q_slug = slugify(q);
    if apps.contains_key(q) {
        return Ok(q.to_string());
    }
    if apps.contains_key(&q_slug) {
        return Ok(q_slug);
    }
    let q_lower = q.to_ascii_lowercase();
    for (slug, app) in apps {
        if app.name.to_ascii_lowercase() == q_lower {
            return Ok(slug.clone());
        }
    }
    let mut prefix_hits: Vec<String> = apps
        .keys()
        .filter(|slug| {
            slug.starts_with(&q_slug)
                || slug.starts_with(&q_lower)
                || apps[*slug].name.to_ascii_lowercase().starts_with(&q_lower)
        })
        .cloned()
        .collect();
    prefix_hits.sort();
    prefix_hits.dedup();
    match prefix_hits.len() {
        1 => Ok(prefix_hits.remove(0)),
        0 => Err(format!("application '{q}' is not registered")),
        _ => Err(format!(
            "application '{q}' is ambiguous (matches: {})",
            prefix_hits.join(", ")
        )),
    }
}

pub fn find_app_by_dir(apps: &BTreeMap<String, AppState>, dir: &Path) -> Option<String> {
    let canon = canonical_dir(dir);
    let canon_str = canon.display().to_string();
    for (slug, app) in apps {
        let app_canon = canonical_dir(Path::new(&app.project_dir));
        if app.project_dir == canon_str || canon == app_canon || canon.starts_with(&app_canon) {
            return Some(slug.clone());
        }
    }
    None
}

fn inspect_hanzo_toml(path: &Path) -> (Option<String>, Option<String>, Option<u16>) {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return (None, None, None);
    };
    let Ok(tbl) = raw.parse::<toml::Table>() else {
        return (None, None, None);
    };
    let section = tbl.get("project").or_else(|| tbl.get("app"));
    let slug = section
        .and_then(|s| s.get("slug"))
        .or_else(|| tbl.get("slug"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let name = section
        .and_then(|s| s.get("name"))
        .or_else(|| tbl.get("name"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let port = section
        .and_then(|s| s.get("port"))
        .or_else(|| tbl.get("port"))
        .or_else(|| tbl.get("server").and_then(|s| s.get("port")))
        .and_then(|v| v.as_integer())
        .and_then(|n| u16::try_from(n).ok());
    (slug.or_else(|| name.clone()), name, port)
}

fn inspect_yaml_like(path: &Path) -> (Option<String>, Option<String>, Option<u16>) {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return (None, None, None);
    };
    let mut slug = None;
    let mut name = None;
    let mut port = None;
    for line in raw.lines() {
        let trimmed = line.split('#').next().unwrap_or("").trim();
        if trimmed.is_empty() {
            continue;
        }
        let indent = line.chars().take_while(|c| *c == ' ').count();
        if indent <= 2 {
            if let Some(v) = trimmed.strip_prefix("slug:") {
                slug = clean_scalar(v);
            } else if let Some(v) = trimmed.strip_prefix("name:") {
                if name.is_none() {
                    name = clean_scalar(v);
                }
            } else if let Some(v) = trimmed.strip_prefix("port:") {
                port = clean_scalar(v).and_then(|s| s.parse::<u16>().ok());
            }
        }
    }
    (slug.or_else(|| name.clone()), name, port)
}

fn inspect_hanzo_config(path: &Path) -> (Option<String>, Option<String>, Option<u16>) {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return (None, None, None);
    };
    if path.extension().and_then(|e| e.to_str()) == Some("json") {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
            let slug = v.get("slug").and_then(|x| x.as_str()).map(str::to_string);
            let name = v.get("name").and_then(|x| x.as_str()).map(str::to_string);
            let port = v
                .get("port")
                .and_then(|x| x.as_u64())
                .and_then(|n| u16::try_from(n).ok());
            return (slug.or_else(|| name.clone()), name, port);
        }
    }
    // Static literal scan for .ts/.js/.mjs without executing code.
    let slug = extract_js_string_prop(&raw, "slug");
    let name = extract_js_string_prop(&raw, "name");
    let port = extract_js_u16_prop(&raw, "port");
    (slug.or_else(|| name.clone()), name, port)
}

fn inspect_compose(path: &Path) -> (Option<String>, Option<String>, Option<u16>) {
    let (slug, name, mut port) = inspect_yaml_like(path);
    if port.is_none() {
        if let Ok(raw) = std::fs::read_to_string(path) {
            for line in raw.lines() {
                let t = line.trim().trim_start_matches('-').trim();
                let unquoted = t.trim_matches('"').trim_matches('\'');
                if let Some((host_part, _container_part)) = unquoted.split_once(':') {
                    let hp = host_part.rsplit(':').next().unwrap_or(host_part);
                    if let Ok(p) = hp.parse::<u16>() {
                        port = Some(p);
                        break;
                    }
                }
            }
        }
    }
    (slug, name, port)
}

fn inspect_dockerfile_port(path: &Path) -> Option<u16> {
    let raw = std::fs::read_to_string(path).ok()?;
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.len() > 7 && trimmed[..7].eq_ignore_ascii_case("EXPOSE ") {
            let tok = trimmed[7..].split_whitespace().next()?;
            let num = tok.split('/').next().unwrap_or(tok);
            if let Ok(p) = num.parse::<u16>() {
                return Some(p);
            }
        }
    }
    None
}

fn inspect_package_json(path: &Path) -> (Option<String>, Option<String>, Option<u16>) {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return (None, None, None);
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return (None, None, None);
    };
    let pkg_name = v.get("name").and_then(|x| x.as_str()).map(str::to_string);
    let display = v
        .get("displayName")
        .or_else(|| v.get("productName"))
        .and_then(|x| x.as_str())
        .map(str::to_string);
    let port = v
        .get("config")
        .and_then(|c| c.get("port"))
        .and_then(|x| x.as_u64())
        .and_then(|n| u16::try_from(n).ok());
    (pkg_name, display, port)
}

fn inspect_cargo_toml(path: &Path) -> (Option<String>, Option<u16>) {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return (None, None);
    };
    let Ok(tbl) = raw.parse::<toml::Table>() else {
        return (None, None);
    };
    let name = tbl
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    (name, None)
}

fn inspect_go_mod(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    for line in raw.lines() {
        if let Some(rest) = line.trim().strip_prefix("module ") {
            let m = rest.trim();
            let last = m.rsplit('/').next().unwrap_or(m);
            if !last.is_empty() {
                return Some(last.to_string());
            }
        }
    }
    None
}

fn clean_scalar(raw: &str) -> Option<String> {
    let t = raw.trim().trim_matches('"').trim_matches('\'').trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

fn extract_js_string_prop(src: &str, key: &str) -> Option<String> {
    for line in src.lines() {
        let t = line.trim();
        for pat in [format!("{key}:"), format!("\"{key}\":"), format!("'{key}':")] {
            if let Some(rest) = t.strip_prefix(&pat) {
                let val = rest.trim().trim_end_matches(',');
                if (val.starts_with('"') && val.ends_with('"') && val.len() >= 2)
                    || (val.starts_with('\'') && val.ends_with('\'') && val.len() >= 2)
                {
                    return Some(val[1..val.len() - 1].to_string());
                }
            }
        }
    }
    None
}

fn extract_js_u16_prop(src: &str, key: &str) -> Option<u16> {
    for line in src.lines() {
        let t = line.trim();
        for pat in [format!("{key}:"), format!("\"{key}\":"), format!("'{key}':")] {
            if let Some(rest) = t.strip_prefix(&pat) {
                let val = rest.trim().trim_end_matches(',').trim();
                if let Ok(p) = val.parse::<u16>() {
                    return Some(p);
                }
            }
        }
    }
    None
}
