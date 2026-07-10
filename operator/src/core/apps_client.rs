//! Read-side client for the platform `apps`-lifecycle table.
//!
//! The Apps reconciler (PR 5 of `hanzoai/platform/docs/APPS_LIFECYCLE.md`) is
//! the DRIVE half of the apps lifecycle: it makes the cluster's *running* state
//! converge on the *declared* state recorded in the platform `apps` table. The
//! table itself lives in `platform.hanzo.ai`'s database; the four cron readers
//! (PR 2) write its observed columns and `GET /v1/apps` (PR 3) serves them.
//!
//! This module is the operator's narrow window onto that table: one HTTP GET,
//! one DTO, and the two pure predicates the reconcile boundary needs
//! (`is_semver` for the floating-tag refusal, `parse_image_ref` for matching a
//! Deployment's container to an app's registry). It deliberately holds no
//! reconcile policy — the controller owns that.
//!
//! ## Why HTTP, not direct DB
//!
//! The contract keeps exactly one read authority: `/v1/apps`, which projects
//! each row through the single `computeDrift` module so the page, external
//! callers, and this operator can never disagree about drift. The operator runs
//! in a different cluster from the platform DB; reaching for the database
//! directly would fork the read-side and re-introduce the drift-of-drift bug
//! the lifecycle exists to kill. So we read the same endpoint the UI reads.
//!
//! ## Trust model
//!
//! Mirrors `core::iam_admin`: a single service token, sourced from the
//! environment, presented as `Authorization: Bearer <token>`. The token never
//! leaves the cluster. Org scoping uses the platform's `X-Org-Id` convention.

use super::error::{OperatorError, Result};
use serde::Deserialize;
use std::time::Duration;

/// HTTP timeout for the `/v1/apps` read. Short — the reconcile loop polls on a
/// cadence and a slow platform must not wedge the operator.
const HTTP_TIMEOUT_SECS: u64 = 15;

/// One app row as served by `GET /v1/apps` — the `AppView` wire shape from
/// `app/platform/server/apps/apps-api.ts`. Only the fields the reconciler acts
/// on are decoded; unknown fields (drift verdict, release metadata, timestamps)
/// are ignored so the operator never couples to the page's projection.
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
pub struct AppView {
    /// `<org>/<app>/<env>`, the table's primary key.
    pub id: String,
    pub org: String,
    pub app: String,
    pub env: String,
    /// `ghcr.io/<org>/<app>` — the join key against a Deployment's container
    /// image repository (NOT the Deployment name, which the operator derives
    /// from its CR and can differ, e.g. `cloud` → `cloud-api`).
    pub registry: String,
    /// Declared semver tag from the universe operator CR. The drive target.
    #[serde(rename = "declaredTag")]
    pub declared_tag: Option<String>,
    /// Running tag observed on the cluster. Drive fires when this ≠ declared.
    #[serde(rename = "runningTag")]
    pub running_tag: Option<String>,
    /// Cluster the row belongs to (`hanzo-k8s`, `lux-k8s`, …). The reconciler
    /// only drives rows for the cluster it itself runs in.
    pub cluster: Option<String>,
    /// Deployment namespace. `None` → the reconciler treats it as `default`,
    /// matching the reader.
    pub namespace: Option<String>,
}

impl AppView {
    /// True when the cluster's running tag has not yet caught up to the declared
    /// tag. The drive trigger. A `None` declared tag is never actionable (there
    /// is nothing to converge on); a `None` running tag with a declared tag IS
    /// actionable (the workload exists in the CR but not — or not observably —
    /// on the cluster).
    pub fn needs_drive(&self) -> bool {
        match &self.declared_tag {
            None => false,
            Some(declared) => self.running_tag.as_deref() != Some(declared.as_str()),
        }
    }
}

/// The `GET /v1/apps` envelope: ordered rows + a drift summary. Only `apps` is
/// decoded — the summary is the page's concern.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AppsListResponse {
    #[serde(default)]
    pub apps: Vec<AppView>,
}

/// Configuration for the apps read-side. Wired from the environment by the
/// controller; kept as an explicit struct so it is unit-testable and so the
/// per-deploy platform URL / token / org are not baked into call sites.
#[derive(Clone, Debug)]
pub struct AppsClientConfig {
    /// Platform base URL, e.g. `https://platform.hanzo.ai` or the in-cluster
    /// `http://platform.hanzo-system.svc.cluster.local:3000`.
    pub base_url: String,
    /// Service token for `Authorization: Bearer`. Empty is rejected before any
    /// request — the read is authenticated, never anonymous.
    pub token: String,
    /// Optional org scope (the `X-Org-Id` convention). `None` → no header, the
    /// platform returns every org the token may see.
    pub org_id: Option<String>,
}

/// Fetch the apps rows for a single cluster from `GET /v1/apps`.
///
/// The cluster filter is applied client-side: the endpoint's documented filters
/// are org/env/health/drift (not cluster), so we ask for all rows the token can
/// see and keep the ones whose `cluster` matches. This is intentionally robust
/// to the platform adding a server-side `cluster` filter later — narrowing
/// client-side is always correct.
pub async fn list_apps_for_cluster(
    config: &AppsClientConfig,
    cluster: &str,
) -> Result<Vec<AppView>> {
    let all = list_apps(config).await?;
    Ok(all
        .into_iter()
        .filter(|a| a.cluster.as_deref() == Some(cluster))
        .collect())
}

/// Fetch every apps row the token is scoped to. Used by `list_apps_for_cluster`
/// and exposed for callers that filter differently.
pub async fn list_apps(config: &AppsClientConfig) -> Result<Vec<AppView>> {
    if config.token.is_empty() {
        return Err(OperatorError::Config(
            "apps read called with empty service token".into(),
        ));
    }
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()
        .map_err(|e| OperatorError::Other(format!("build apps http client: {e}")))?;

    let url = format!("{}/v1/apps", config.base_url.trim_end_matches('/'));
    let mut req = http.get(&url).bearer_auth(&config.token);
    if let Some(org) = &config.org_id {
        req = req.header("X-Org-Id", org);
    }

    let resp = req.send().await.map_err(OperatorError::Http)?;

    let status = resp.status();
    let body = resp.text().await.map_err(OperatorError::Http)?;

    if !status.is_success() {
        return Err(OperatorError::Other(format!(
            "GET {url} failed (status={status}): {}",
            &body[..body.len().min(400)]
        )));
    }

    let parsed: AppsListResponse = serde_json::from_str(&body).map_err(|e| {
        OperatorError::Other(format!(
            "apps response parse: {e}; body={}",
            &body[..body.len().min(400)]
        ))
    })?;
    Ok(parsed.apps)
}

/// Semver predicate for the reconcile-boundary refusal: `^v\d+\.\d+\.\d+$`.
///
/// The contract's keystone safety rule (§4): "The same controller refuses to
/// apply a patch where `declared_tag` is a floating reference (anything not
/// matching `^v\d+\.\d+\.\d+$`). Semver-only at the reconcile boundary." We
/// hand-roll the match rather than pull a regex crate for one pattern — the
/// grammar is tiny and fixed.
///
/// Accepts: `v1.2.3`, `v0.0.0`, `v10.20.30`.
/// Rejects: `1.2.3` (no `v`), `v1.2` (no patch), `v1.2.3-rc1` (pre-release),
/// `latest`, `main`, `dev`, `sha-abc1234`, `multi-issuer`, `` (empty).
pub fn is_semver(tag: &str) -> bool {
    let Some(rest) = tag.strip_prefix('v') else {
        return false;
    };
    let mut parts = rest.split('.');
    let (Some(major), Some(minor), Some(patch), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    [major, minor, patch]
        .iter()
        .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Split a docker image reference into `(repository, tag)` — the Rust mirror of
/// the platform's single canonical `parseImageRef` (`pkg/platform/src/services/
/// ci/image-ref.ts`). The reconciler matches a Deployment container to an app
/// by `repository == apps.registry`, the exact same join key the running-tag
/// reader uses, so the two stay symmetric.
///
/// - `postgres:16-alpine`        → (`postgres`, `16-alpine`)
/// - `postgres`                  → (`postgres`, ``)
/// - `ghcr.io/hanzoai/iam:v1.2.3`→ (`ghcr.io/hanzoai/iam`, `v1.2.3`)
/// - `ghcr.io:5000/x/y`          → (`ghcr.io:5000/x/y`, ``) (registry port)
pub fn parse_image_ref(image: &str) -> (String, String) {
    match image.rfind(':') {
        // `idx == 0` is a leading colon — degenerate, treat as no tag.
        Some(idx) if idx > 0 => {
            let after = &image[idx + 1..];
            // A `/` after the colon means it was a `host:port/` prefix, not a tag.
            if after.contains('/') {
                (image.to_string(), String::new())
            } else {
                (image[..idx].to_string(), after.to_string())
            }
        }
        _ => (image.to_string(), String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_accepts_canonical() {
        for ok in ["v1.2.3", "v0.0.0", "v10.20.30", "v123.456.789"] {
            assert!(is_semver(ok), "{ok} should be semver");
        }
    }

    #[test]
    fn semver_rejects_floating_and_malformed() {
        for bad in [
            "1.2.3",      // no leading v
            "v1.2",       // missing patch
            "v1.2.3.4",   // extra segment
            "v1.2.3-rc1", // pre-release suffix
            "v1.2.x",     // non-numeric
            "vx.2.3",     // non-numeric major
            "v1..3",      // empty minor
            "latest",
            "main",
            "dev",
            "multi-issuer", // the live kms floating tag the seed kept verbatim
            "sha-abc1234",  // immutable build artifact, but NOT a release tag
            "",
            "v",
        ] {
            assert!(!is_semver(bad), "{bad} must be refused at the boundary");
        }
    }

    #[test]
    fn image_ref_splits_tag() {
        assert_eq!(
            parse_image_ref("ghcr.io/hanzoai/iam:v1.2.3"),
            ("ghcr.io/hanzoai/iam".into(), "v1.2.3".into())
        );
        assert_eq!(
            parse_image_ref("postgres:16-alpine"),
            ("postgres".into(), "16-alpine".into())
        );
    }

    #[test]
    fn image_ref_no_tag() {
        assert_eq!(
            parse_image_ref("ghcr.io/hanzoai/iam"),
            ("ghcr.io/hanzoai/iam".into(), String::new())
        );
    }

    #[test]
    fn image_ref_registry_port_is_not_a_tag() {
        assert_eq!(
            parse_image_ref("ghcr.io:5000/x/y"),
            ("ghcr.io:5000/x/y".into(), String::new())
        );
    }

    #[test]
    fn needs_drive_logic() {
        let mk = |declared: Option<&str>, running: Option<&str>| AppView {
            declared_tag: declared.map(String::from),
            running_tag: running.map(String::from),
            ..Default::default()
        };
        // declared == running → in sync, no drive.
        assert!(!mk(Some("v1.2.3"), Some("v1.2.3")).needs_drive());
        // declared != running → drive.
        assert!(mk(Some("v1.2.3"), Some("v1.2.2")).needs_drive());
        // declared set, nothing running → drive (CR has it, cluster doesn't).
        assert!(mk(Some("v1.2.3"), None).needs_drive());
        // no declared tag → never actionable.
        assert!(!mk(None, Some("v1.2.3")).needs_drive());
        assert!(!mk(None, None).needs_drive());
    }

    #[test]
    fn appview_decodes_v1_apps_wire_shape() {
        // A row as `GET /v1/apps` serves it (camelCase keys, extra fields the
        // operator ignores). Locks the field-rename contract.
        let json = r#"{
            "id": "hanzoai/iam/main",
            "org": "hanzoai",
            "app": "iam",
            "env": "main",
            "repo": "hanzoai/iam",
            "registry": "ghcr.io/hanzoai/iam",
            "declaredTag": "v1.15.0",
            "runningTag": "v1.14.29",
            "latestTag": "v1.15.0",
            "releaseUrl": "https://github.com/hanzoai/iam/releases/tag/v1.15.0",
            "releaseAssets": 4,
            "health": "green",
            "cluster": "hanzo-k8s",
            "namespace": "hanzo-system",
            "lastObserved": "2026-06-20T00:00:00.000Z",
            "updatedAt": "2026-06-20T00:00:00.000Z",
            "drift": { "severity": "yellow" }
        }"#;
        let v: AppView = serde_json::from_str(json).expect("decode AppView");
        assert_eq!(v.id, "hanzoai/iam/main");
        assert_eq!(v.registry, "ghcr.io/hanzoai/iam");
        assert_eq!(v.declared_tag.as_deref(), Some("v1.15.0"));
        assert_eq!(v.running_tag.as_deref(), Some("v1.14.29"));
        assert_eq!(v.cluster.as_deref(), Some("hanzo-k8s"));
        assert_eq!(v.namespace.as_deref(), Some("hanzo-system"));
        assert!(v.needs_drive());
    }

    #[test]
    fn envelope_decodes_and_is_lenient() {
        let json = r#"{"apps":[],"summary":{"total":0,"byDrift":{"ok":0}}}"#;
        let r: AppsListResponse = serde_json::from_str(json).expect("decode envelope");
        assert!(r.apps.is_empty());
        // Missing `apps` key degrades to empty, never an error.
        let empty: AppsListResponse = serde_json::from_str("{}").expect("decode empty");
        assert!(empty.apps.is_empty());
    }
}
