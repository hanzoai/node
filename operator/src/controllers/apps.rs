//! Apps reconciler — the DRIVE path of the apps lifecycle.
//!
//! Implements PR 5 of `hanzoai/platform/docs/APPS_LIFECYCLE.md` §4 "Operator
//! reconciler": watch the platform `apps` table for `declared_tag !=
//! running_tag`, patch the Deployment to `declared_tag`, wait for rollout, and
//! report the result back to the apps view. It is the inverse of the
//! running-tag reader (PR 2): the reader observes the cluster into the table;
//! this controller drives the table back onto the cluster.
//!
//! ## Why this is not a `kube::Controller`
//!
//! Every other controller in this binary reconciles a CRD Kind — its reconcile
//! source is a watch on objects in *this* cluster. The Apps controller's source
//! is the platform `apps` table, which lives in a *different* cluster's database
//! and is read over HTTP (`GET /v1/apps`, the one read authority — see
//! `core::apps_client`). So this is a periodic poll loop, not a watch: each tick
//! pulls the rows for this operator's cluster and converges the ones that drift.
//!
//! ## Safety — the load-bearing part of this PR
//!
//! This controller can `kubectl set image` across EVERY production service. A
//! bug, a stale row, or a fat-fingered `declared_tag` would roll the fleet. So
//! it is **fail-safe by construction**, with three independent gates that must
//! ALL open before a single Deployment is patched:
//!
//! 1. **Master enable** — `APPS_CONTROLLER=true`. Default off. When off the
//!    loop never starts (mirrors the `kms_zap` opt-in). First deploy of a binary
//!    carrying this code is inert.
//! 2. **Drive mode** — `APPS_DRIVE_MODE` ∈ {`off` (default), `dry-run`, `on`}.
//!    `off`/`dry-run` NEVER patch — they observe drift and report the action
//!    they *would* take (log + k8s Event). Only `on` can patch.
//! 3. **Per-app allow-list** — `APPS_DRIVE_ALLOW` (comma-separated
//!    `<org>/<app>/<env>` | `<org>/<app>` | `<org>/*` | `*`). Even in `on` mode,
//!    an app NOT matched here is treated as dry-run. So enabling drive globally
//!    does not reconcile-and-patch everything — you opt each app in explicitly.
//!
//! And one gate at the reconcile boundary that NO config can override:
//!
//! 4. **Semver-only** — a `declared_tag` that is not `^v\d+\.\d+\.\d+$` is
//!    refused (`core::apps_client::is_semver`). A floating `declared_tag` (the
//!    kms `multi-issuer` seed, a stray `:main`) can never be applied. This is
//!    the contract's keystone: "Semver-only at the reconcile boundary."
//!
//! ## What it patches
//!
//! It matches the Deployment in the row's namespace whose container image
//! repository equals `apps.registry` — the SAME join key the running-tag reader
//! uses (NOT the Deployment name, which the operator derives from a CR and can
//! differ, e.g. `cloud` → `cloud-api`). It then patches only that container's
//! `image` to `<registry>:<declared_tag>`. The Deployment's own CR/owner keeps
//! managing everything else; this is a targeted image bump, the declarative
//! equivalent of the emergency `kubectl set image` the lifecycle replaces.

use std::time::Duration;

use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{Event, EventSource, ObjectReference};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{MicroTime, ObjectMeta, Time};
use kube::api::{Api, Patch, PatchParams, PostParams};
use kube::Client;
use tracing::{error, info, warn};

use crate::core::apps_client::{
    is_semver, list_apps_for_cluster, parse_image_ref, AppView, AppsClientConfig,
};

/// Field manager for the image patch — distinct from the CR controllers'
/// `hanzo-operator` SSA manager so an image bump driven here is attributable and
/// never silently fights a Service/Datastore reconcile over the same field.
const FIELD_MANAGER: &str = "hanzo-operator-apps";

/// Reporter string stamped on emitted k8s Events (`source.component`).
const EVENT_REPORTER: &str = "hanzo-operator-apps";

/// Default poll cadence for the table read. Drift is loud but not urgent — a
/// minute between sweeps keeps platform load trivial while still converging
/// promptly after a declared-tag bump.
const DEFAULT_POLL_SECS: u64 = 60;

/// How long to wait for a patched Deployment's rollout to complete before
/// giving up *for this tick* (the next sweep re-checks). Not a hard failure —
/// a slow rollout is reported, not error-looped.
const ROLLOUT_TIMEOUT_SECS: u64 = 180;

/// Poll interval between rollout-status checks while waiting.
const ROLLOUT_POLL_SECS: u64 = 5;

/// Drive mode — the second safety gate. Parsed from `APPS_DRIVE_MODE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveMode {
    /// Never patch. Observe drift only (no Event spam, just logs).
    Off,
    /// Never patch, but emit a k8s Event + log describing the patch that WOULD
    /// be applied. The safe way to watch the controller "think" in prod.
    DryRun,
    /// Patch — subject to the per-app allow-list and the semver gate.
    On,
}

impl DriveMode {
    /// Parse `APPS_DRIVE_MODE`. Anything unrecognized (or unset) is `Off` —
    /// the safe default. `dry-run` and `dryrun` both map to `DryRun`.
    pub fn from_env_str(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "on" | "true" | "drive" => DriveMode::On,
            "dry-run" | "dryrun" | "dry" => DriveMode::DryRun,
            _ => DriveMode::Off,
        }
    }
}

/// Per-app allow-list — the third safety gate. A set of patterns, each one of:
/// `*` (everything), `<org>/*`, `<org>/<app>`, or `<org>/<app>/<env>`.
#[derive(Debug, Clone, Default)]
pub struct DriveAllowList {
    patterns: Vec<String>,
}

impl DriveAllowList {
    /// Parse `APPS_DRIVE_ALLOW` — comma/whitespace-separated patterns. Empty →
    /// an empty list that matches nothing (so `on` without an allow-list still
    /// patches nothing).
    pub fn from_env_str(raw: &str) -> Self {
        let patterns = raw
            .split([',', ' ', '\t', '\n'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        Self { patterns }
    }

    /// True iff `app` is permitted to be patched. Matches against `<org>/<app>/
    /// <env>`, `<org>/<app>`, `<org>/*`, and the bare `*` wildcard.
    pub fn allows(&self, app: &AppView) -> bool {
        let full = format!("{}/{}/{}", app.org, app.app, app.env);
        let org_app = format!("{}/{}", app.org, app.app);
        let org_glob = format!("{}/*", app.org);
        self.patterns
            .iter()
            .any(|p| p == "*" || p == &full || p == &org_app || p == &org_glob)
    }

    fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }
}

/// What the policy says to do with one drifting app — pure, no I/O. Computed by
/// [`decide`] so the gate logic is unit-testable without a cluster or platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Do nothing and say why (e.g. floating declared tag refused).
    Skip { reason: String },
    /// Report (log + Event) the intended patch but DO NOT apply it. Carries the
    /// target image so the report is concrete.
    Report {
        target_image: String,
        reason: String,
    },
    /// Apply the patch to `target_image`.
    Patch { target_image: String },
}

/// The single decision function — the brain of the safety gate. Pure over
/// `(mode, allow, app)`. Order of checks matters and is itself a safety
/// property:
///
/// 1. If the app does not drift → it never reaches here (caller filters), but we
///    still guard `needs_drive` for total correctness.
/// 2. **Semver gate first, unconditionally.** A floating declared tag is
///    `Skip`ped regardless of mode — we never even *report* a patch we would
///    refuse, so dry-run output never lies about what `on` would do.
/// 3. Mode `Off` → `Skip` (silent-ish; caller logs at debug).
/// 4. Mode `DryRun`, OR (`On` AND not allow-listed) → `Report`. This is the
///    "drive opt-in per app" rule: global `on` still only *reports* for apps you
///    have not explicitly allowed.
/// 5. Mode `On` AND allow-listed → `Patch`.
pub fn decide(mode: DriveMode, allow: &DriveAllowList, app: &AppView) -> Decision {
    if !app.needs_drive() {
        return Decision::Skip {
            reason: "in sync".into(),
        };
    }
    let declared = match &app.declared_tag {
        Some(d) => d.clone(),
        None => {
            return Decision::Skip {
                reason: "no declared tag".into(),
            }
        }
    };

    // Gate 4 — semver-only at the reconcile boundary. No mode overrides this.
    if !is_semver(&declared) {
        return Decision::Skip {
            reason: format!("declared tag '{declared}' is not semver (vX.Y.Z) — refusing to apply floating reference"),
        };
    }

    let target_image = format!("{}:{}", app.registry, declared);

    match mode {
        DriveMode::Off => Decision::Skip {
            reason: "drive mode off".into(),
        },
        DriveMode::DryRun => Decision::Report {
            target_image,
            reason: "dry-run mode".into(),
        },
        DriveMode::On => {
            if allow.allows(app) {
                Decision::Patch { target_image }
            } else {
                Decision::Report {
                    target_image,
                    reason: "app not in APPS_DRIVE_ALLOW allow-list".into(),
                }
            }
        }
    }
}

/// Runtime config for the controller, resolved once from the environment.
#[derive(Clone, Debug)]
pub struct AppsControllerConfig {
    /// Read-side client config (`GET /v1/apps`).
    pub client: AppsClientConfig,
    /// The cluster this operator runs in — only rows with this `cluster` are
    /// driven. Defaults to `hanzo-k8s` (the platform's own cluster), matching
    /// the running-tag reader's `APPS_LOCAL_CLUSTER` default.
    pub cluster: String,
    /// Second gate.
    pub mode: DriveMode,
    /// Third gate.
    pub allow: DriveAllowList,
    /// Poll cadence.
    pub poll: Duration,
}

impl AppsControllerConfig {
    /// Build from the environment. Returns `None` when the platform URL or
    /// service token is missing — without a read source the controller cannot
    /// run, so the caller skips it (it is opt-in anyway).
    pub fn from_env() -> Option<Self> {
        let base_url = std::env::var("APPS_PLATFORM_URL")
            .ok()
            .filter(|s| !s.is_empty())?;
        let token = first_env(&[
            "APPS_SERVICE_TOKEN",
            "PLATFORM_SERVICE_TOKEN",
            "HANZO_API_KEY",
        ])
        .filter(|s| !s.is_empty())?;
        let org_id = std::env::var("APPS_ORG_ID").ok().filter(|s| !s.is_empty());
        let cluster = std::env::var("APPS_CLUSTER")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                std::env::var("APPS_LOCAL_CLUSTER")
                    .ok()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "hanzo-k8s".to_string())
            });
        let mode = DriveMode::from_env_str(&std::env::var("APPS_DRIVE_MODE").unwrap_or_default());
        let allow =
            DriveAllowList::from_env_str(&std::env::var("APPS_DRIVE_ALLOW").unwrap_or_default());
        let poll = Duration::from_secs(
            std::env::var("APPS_POLL_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .filter(|&n| n >= 5)
                .unwrap_or(DEFAULT_POLL_SECS),
        );

        Some(Self {
            client: AppsClientConfig {
                base_url,
                token,
                org_id,
            },
            cluster,
            mode,
            allow,
            poll,
        })
    }
}

/// First non-empty value among `keys`, in order.
fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| std::env::var(k).ok().filter(|s| !s.is_empty()))
}

/// Opt-in entrypoint — slots into `run_all_controllers`' `tokio::join!` with the
/// same `(client, namespace, enabled)` shape as the other opt-in controller
/// (`kms_zap`). When `enabled` is false (the default — `APPS_CONTROLLER` unset)
/// this returns immediately and the controller never reads or patches anything.
///
/// `namespace` is accepted for call-shape symmetry; the Apps controller is
/// inherently cross-namespace (each app row carries its own namespace), so the
/// argument is informational only.
pub async fn run_apps_controller(client: Client, _namespace: String, enabled: bool) {
    if !enabled {
        info!("Apps controller disabled (set APPS_CONTROLLER=true to enable)");
        return;
    }
    let Some(config) = AppsControllerConfig::from_env() else {
        warn!(
            "Apps controller enabled but APPS_PLATFORM_URL / service token unset — \
             nothing to read, staying idle. Set APPS_PLATFORM_URL + APPS_SERVICE_TOKEN."
        );
        return;
    };

    info!(
        cluster = %config.cluster,
        mode = ?config.mode,
        allow_patterns = config.allow.patterns.len(),
        poll_secs = config.poll.as_secs(),
        "Starting Apps controller (DRIVE path)"
    );
    if config.mode == DriveMode::On && config.allow.is_empty() {
        warn!(
            "APPS_DRIVE_MODE=on but APPS_DRIVE_ALLOW is empty — every app will be \
             dry-run-reported, nothing patched. Add apps to the allow-list to drive them."
        );
    }

    let mut tick = tokio::time::interval(config.poll);
    loop {
        tick.tick().await;
        if let Err(e) = sweep(&client, &config).await {
            // A sweep failure (platform unreachable, auth) is transient — log and
            // wait for the next tick. Never crash the operator over it.
            warn!(error = %e, "Apps controller sweep failed; will retry next tick");
        }
    }
}

/// One sweep: read the rows for this cluster, decide + act on each drifting one.
async fn sweep(client: &Client, config: &AppsControllerConfig) -> crate::core::Result<()> {
    let apps = list_apps_for_cluster(&config.client, &config.cluster).await?;
    let drifting: Vec<&AppView> = apps.iter().filter(|a| a.needs_drive()).collect();
    info!(
        cluster = %config.cluster,
        total = apps.len(),
        drifting = drifting.len(),
        "Apps sweep"
    );

    for app in drifting {
        match decide(config.mode, &config.allow, app) {
            Decision::Skip { reason } => {
                info!(app = %app.id, %reason, "skip");
            }
            Decision::Report {
                target_image,
                reason,
            } => {
                info!(
                    app = %app.id,
                    intended_image = %target_image,
                    %reason,
                    "WOULD patch (not applied)"
                );
                emit_event(
                    client,
                    app,
                    "Normal",
                    "DriveIntended",
                    &format!("would set image to {target_image} ({reason}); no change applied"),
                )
                .await;
            }
            Decision::Patch { target_image } => {
                if let Err(e) = drive_one(client, app, &target_image).await {
                    error!(app = %app.id, target = %target_image, error = %e, "drive failed");
                    emit_event(
                        client,
                        app,
                        "Warning",
                        "DriveFailed",
                        &format!("failed to set image to {target_image}: {e}"),
                    )
                    .await;
                }
            }
        }
    }
    Ok(())
}

/// Drive one app: find its Deployment by registry, patch the matching
/// container's image, wait for rollout, emit an Event.
async fn drive_one(client: &Client, app: &AppView, target_image: &str) -> crate::core::Result<()> {
    let namespace = app.namespace.as_deref().unwrap_or("default");
    let deps: Api<Deployment> = Api::namespaced(client.clone(), namespace);

    // Find the Deployment + container index whose image repo == registry.
    let (dep_name, container_name) = match find_target(&deps, &app.registry).await? {
        Some(hit) => hit,
        None => {
            return Err(crate::core::OperatorError::NotFound(format!(
                "no Deployment in {namespace} runs image repo {} (app {})",
                app.registry, app.id
            )));
        }
    };

    info!(
        app = %app.id,
        deployment = %dep_name,
        container = %container_name,
        target = %target_image,
        "patching image"
    );

    // Strategic-merge patch of ONLY the matching container's image — minimal,
    // idempotent, and it does not disturb any other field the CR owns.
    let patch = serde_json::json!({
        "spec": { "template": { "spec": { "containers": [
            { "name": container_name, "image": target_image }
        ]}}}
    });
    let pp = PatchParams::apply(FIELD_MANAGER).force();
    deps.patch(&dep_name, &pp, &Patch::Strategic(&patch))
        .await
        .map_err(crate::core::OperatorError::KubeApi)?;

    // Wait for rollout — best-effort within the timeout. A slow rollout is
    // reported, not error-looped (the next sweep observes the final state).
    let rolled = wait_for_rollout(&deps, &dep_name).await;
    if rolled {
        info!(app = %app.id, deployment = %dep_name, target = %target_image, "rollout complete");
        emit_event(
            client,
            app,
            "Normal",
            "Driven",
            &format!("set image to {target_image} and rollout completed"),
        )
        .await;
    } else {
        warn!(
            app = %app.id,
            deployment = %dep_name,
            target = %target_image,
            "patched but rollout not complete within timeout; will re-check next sweep"
        );
        emit_event(
            client,
            app,
            "Warning",
            "DriveRolloutPending",
            &format!(
                "set image to {target_image}; rollout not complete within {ROLLOUT_TIMEOUT_SECS}s"
            ),
        )
        .await;
    }
    Ok(())
}

/// Locate the `(deployment_name, container_name)` in `deps` whose container
/// image repository equals `registry`. First match wins (a repo is normally
/// owned by exactly one Deployment in a namespace — same assumption the
/// running-tag reader makes). Init containers are not considered.
async fn find_target(
    deps: &Api<Deployment>,
    registry: &str,
) -> crate::core::Result<Option<(String, String)>> {
    use kube::api::ListParams;
    let list = deps
        .list(&ListParams::default())
        .await
        .map_err(crate::core::OperatorError::KubeApi)?;
    for dep in list {
        let dep_name = match &dep.metadata.name {
            Some(n) => n.clone(),
            None => continue,
        };
        let containers = dep
            .spec
            .as_ref()
            .and_then(|s| s.template.spec.as_ref())
            .map(|ps| ps.containers.clone())
            .unwrap_or_default();
        for c in containers {
            if let Some(image) = &c.image {
                let (repo, _tag) = parse_image_ref(image);
                if repo == registry {
                    return Ok(Some((dep_name, c.name)));
                }
            }
        }
    }
    Ok(None)
}

/// True when the named Deployment reports a complete rollout (observed
/// generation caught up to metadata.generation, and updated == desired ==
/// ready, with no stale replicas). Polls until complete or the timeout.
async fn wait_for_rollout(deps: &Api<Deployment>, name: &str) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(ROLLOUT_TIMEOUT_SECS);
    loop {
        if let Ok(dep) = deps.get(name).await {
            if rollout_complete(&dep) {
                return true;
            }
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_secs(ROLLOUT_POLL_SECS)).await;
    }
}

/// Pure rollout-completeness check over a `Deployment` — the same condition
/// `kubectl rollout status` waits on: the controller has observed the latest
/// spec generation, every desired replica is updated and available, and no
/// old replicas linger.
fn rollout_complete(dep: &Deployment) -> bool {
    let generation = dep.metadata.generation;
    let spec_replicas = dep.spec.as_ref().and_then(|s| s.replicas).unwrap_or(1);
    let status = match &dep.status {
        Some(s) => s,
        None => return false,
    };
    // The controller must have acted on the current spec.
    if let (Some(gen), Some(observed)) = (generation, status.observed_generation) {
        if observed < gen {
            return false;
        }
    }
    let updated = status.updated_replicas.unwrap_or(0);
    let available = status.available_replicas.unwrap_or(0);
    let total = status.replicas.unwrap_or(0);
    // Every desired replica updated to the new template…
    updated >= spec_replicas
        // …no old replicas still around (total not exceeding desired)…
        && total <= updated
        // …and all desired replicas available.
        && available >= spec_replicas
}

/// Emit a Kubernetes Event against the app's namespace, referencing the app by
/// its lifecycle id. Best-effort: an Event write failure is logged and
/// swallowed — it must never block or fail a reconcile. These events surface on
/// the apps view at `platform.hanzo.ai/apps` and via `kubectl get events`.
async fn emit_event(client: &Client, app: &AppView, kind: &str, reason: &str, message: &str) {
    let namespace = app.namespace.as_deref().unwrap_or("default");
    // k8s-openapi 0.28 backs meta/v1 Time + MicroTime with jiff::Timestamp.
    let now = jiff::Timestamp::now();
    // A unique-ish name; the operator is the single writer so a timestamp suffix
    // is enough to avoid 409s within a namespace.
    let ev_name = format!("apps-{}-{}", app.app, now.as_nanosecond());
    let event = Event {
        metadata: ObjectMeta {
            name: Some(ev_name),
            namespace: Some(namespace.to_string()),
            ..Default::default()
        },
        // involvedObject points at the app's lifecycle id, namespaced; we use a
        // synthetic kind ("App") so the event is attributable to the lifecycle
        // row even though there is no in-cluster App object.
        involved_object: ObjectReference {
            kind: Some("App".to_string()),
            namespace: Some(namespace.to_string()),
            name: Some(app.id.clone()),
            ..Default::default()
        },
        reason: Some(reason.to_string()),
        message: Some(message.to_string()),
        type_: Some(kind.to_string()),
        reporting_component: Some(EVENT_REPORTER.to_string()),
        reporting_instance: Some(EVENT_REPORTER.to_string()),
        action: Some("Drive".to_string()),
        source: Some(EventSource {
            component: Some(EVENT_REPORTER.to_string()),
            ..Default::default()
        }),
        first_timestamp: Some(Time(now)),
        last_timestamp: Some(Time(now)),
        event_time: Some(MicroTime(now)),
        count: Some(1),
        ..Default::default()
    };
    let api: Api<Event> = Api::namespaced(client.clone(), namespace);
    if let Err(e) = api.create(&PostParams::default(), &event).await {
        warn!(app = %app.id, error = %e, "failed to emit Event (non-fatal)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(org: &str, name: &str, env: &str, declared: &str, running: Option<&str>) -> AppView {
        AppView {
            id: format!("{org}/{name}/{env}"),
            org: org.into(),
            app: name.into(),
            env: env.into(),
            registry: format!("ghcr.io/{org}/{name}"),
            declared_tag: Some(declared.into()),
            running_tag: running.map(String::from),
            cluster: Some("hanzo-k8s".into()),
            namespace: Some(format!("{org}-system")),
        }
    }

    // ---- DriveMode parsing ----

    #[test]
    fn drive_mode_defaults_off() {
        assert_eq!(DriveMode::from_env_str(""), DriveMode::Off);
        assert_eq!(DriveMode::from_env_str("garbage"), DriveMode::Off);
        assert_eq!(DriveMode::from_env_str("off"), DriveMode::Off);
    }

    #[test]
    fn drive_mode_parses_variants() {
        assert_eq!(DriveMode::from_env_str("on"), DriveMode::On);
        assert_eq!(DriveMode::from_env_str("ON"), DriveMode::On);
        assert_eq!(DriveMode::from_env_str("drive"), DriveMode::On);
        assert_eq!(DriveMode::from_env_str("dry-run"), DriveMode::DryRun);
        assert_eq!(DriveMode::from_env_str("dryrun"), DriveMode::DryRun);
        assert_eq!(DriveMode::from_env_str(" Dry "), DriveMode::DryRun);
    }

    // ---- allow-list matching ----

    #[test]
    fn allow_list_wildcard() {
        let a = DriveAllowList::from_env_str("*");
        assert!(a.allows(&app("hanzoai", "iam", "main", "v1.0.0", None)));
    }

    #[test]
    fn allow_list_org_glob() {
        let a = DriveAllowList::from_env_str("hanzoai/*");
        assert!(a.allows(&app("hanzoai", "iam", "main", "v1.0.0", None)));
        assert!(!a.allows(&app("luxfi", "node", "main", "v1.0.0", None)));
    }

    #[test]
    fn allow_list_org_app_and_full() {
        let a = DriveAllowList::from_env_str("hanzoai/iam, hanzoai/kms/main");
        // org/app matches every env of iam
        assert!(a.allows(&app("hanzoai", "iam", "main", "v1.0.0", None)));
        assert!(a.allows(&app("hanzoai", "iam", "test", "v1.0.0", None)));
        // org/app/env matches only that env of kms
        assert!(a.allows(&app("hanzoai", "kms", "main", "v1.0.0", None)));
        assert!(!a.allows(&app("hanzoai", "kms", "test", "v1.0.0", None)));
    }

    #[test]
    fn allow_list_empty_matches_nothing() {
        let a = DriveAllowList::from_env_str("");
        assert!(a.is_empty());
        assert!(!a.allows(&app("hanzoai", "iam", "main", "v1.0.0", None)));
    }

    // ---- decide(): the safety gate, exhaustively ----

    #[test]
    fn decide_in_sync_is_skip() {
        let d = decide(
            DriveMode::On,
            &DriveAllowList::from_env_str("*"),
            &app("hanzoai", "iam", "main", "v1.2.3", Some("v1.2.3")),
        );
        assert!(matches!(d, Decision::Skip { .. }));
    }

    #[test]
    fn decide_floating_declared_is_always_skipped_even_when_on_and_allowed() {
        // The kms `multi-issuer` seed: floating, allow-listed, mode on. Must
        // STILL be refused — semver gate overrides everything.
        for mode in [DriveMode::Off, DriveMode::DryRun, DriveMode::On] {
            let d = decide(
                mode,
                &DriveAllowList::from_env_str("*"),
                &app("hanzoai", "kms", "main", "multi-issuer", Some("v1.0.0")),
            );
            match d {
                Decision::Skip { reason } => assert!(
                    reason.contains("not semver"),
                    "expected semver refusal, got: {reason}"
                ),
                other => panic!("floating tag must be Skip, got {other:?}"),
            }
        }
    }

    #[test]
    fn decide_off_never_patches() {
        let d = decide(
            DriveMode::Off,
            &DriveAllowList::from_env_str("*"),
            &app("hanzoai", "iam", "main", "v1.2.3", Some("v1.2.2")),
        );
        assert!(matches!(d, Decision::Skip { .. }));
    }

    #[test]
    fn decide_dry_run_reports_never_patches() {
        let d = decide(
            DriveMode::DryRun,
            &DriveAllowList::from_env_str("*"),
            &app("hanzoai", "iam", "main", "v1.2.3", Some("v1.2.2")),
        );
        match d {
            Decision::Report { target_image, .. } => {
                assert_eq!(target_image, "ghcr.io/hanzoai/iam:v1.2.3");
            }
            other => panic!("dry-run must Report, got {other:?}"),
        }
    }

    #[test]
    fn decide_on_but_not_allowlisted_only_reports() {
        // The critical "don't reconcile everything on first enable" property:
        // mode on, but this app is not in the allow-list → Report, not Patch.
        let d = decide(
            DriveMode::On,
            &DriveAllowList::from_env_str("hanzoai/kms"),
            &app("hanzoai", "iam", "main", "v1.2.3", Some("v1.2.2")),
        );
        match d {
            Decision::Report { reason, .. } => assert!(reason.contains("allow-list")),
            other => panic!("on+not-allowed must Report, got {other:?}"),
        }
    }

    #[test]
    fn decide_on_and_allowlisted_patches() {
        let d = decide(
            DriveMode::On,
            &DriveAllowList::from_env_str("hanzoai/iam"),
            &app("hanzoai", "iam", "main", "v1.2.3", Some("v1.2.2")),
        );
        match d {
            Decision::Patch { target_image } => {
                assert_eq!(target_image, "ghcr.io/hanzoai/iam:v1.2.3");
            }
            other => panic!("on+allowed must Patch, got {other:?}"),
        }
    }

    #[test]
    fn decide_drives_when_nothing_running() {
        // declared set, running None → drift, and (on+allowed) → patch.
        let d = decide(
            DriveMode::On,
            &DriveAllowList::from_env_str("*"),
            &app("hanzoai", "iam", "main", "v1.2.3", None),
        );
        assert!(matches!(d, Decision::Patch { .. }));
    }

    // ---- rollout_complete ----

    #[test]
    fn rollout_complete_true_when_converged() {
        use k8s_openapi::api::apps::v1::{DeploymentSpec, DeploymentStatus};
        let dep = Deployment {
            metadata: ObjectMeta {
                generation: Some(3),
                ..Default::default()
            },
            spec: Some(DeploymentSpec {
                replicas: Some(2),
                ..Default::default()
            }),
            status: Some(DeploymentStatus {
                observed_generation: Some(3),
                updated_replicas: Some(2),
                available_replicas: Some(2),
                replicas: Some(2),
                ..Default::default()
            }),
        };
        assert!(rollout_complete(&dep));
    }

    #[test]
    fn rollout_incomplete_when_observed_generation_behind() {
        use k8s_openapi::api::apps::v1::{DeploymentSpec, DeploymentStatus};
        let dep = Deployment {
            metadata: ObjectMeta {
                generation: Some(4),
                ..Default::default()
            },
            spec: Some(DeploymentSpec {
                replicas: Some(2),
                ..Default::default()
            }),
            status: Some(DeploymentStatus {
                observed_generation: Some(3), // controller hasn't seen new spec
                updated_replicas: Some(2),
                available_replicas: Some(2),
                replicas: Some(2),
                ..Default::default()
            }),
        };
        assert!(!rollout_complete(&dep));
    }

    #[test]
    fn rollout_incomplete_when_old_replicas_linger() {
        use k8s_openapi::api::apps::v1::{DeploymentSpec, DeploymentStatus};
        let dep = Deployment {
            metadata: ObjectMeta {
                generation: Some(2),
                ..Default::default()
            },
            spec: Some(DeploymentSpec {
                replicas: Some(2),
                ..Default::default()
            }),
            status: Some(DeploymentStatus {
                observed_generation: Some(2),
                updated_replicas: Some(2),
                available_replicas: Some(2),
                replicas: Some(4), // 2 new + 2 old mid-rollout
                ..Default::default()
            }),
        };
        assert!(!rollout_complete(&dep));
    }

    #[test]
    fn rollout_incomplete_when_no_status() {
        use k8s_openapi::api::apps::v1::DeploymentSpec;
        let dep = Deployment {
            spec: Some(DeploymentSpec {
                replicas: Some(1),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(!rollout_complete(&dep));
    }
}
