// Copyright 2026 Hanzo AI Inc. All Rights Reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! The controller: watch `apps.hanzo.ai`, reconcile each to its owned objects.
//! Thin *effect* layer over [`crate::manifests::plan`] (the decision) and
//! [`crate::coordinator::Coordinator`] (the leaderless gate). Idempotent SSA
//! under field manager `hanzod`.
//!
//! Safety behaviors:
//! - **Fail-closed on unmodeled fields** (HIGH-1): a CR carrying a spec field
//!   hanzod does not model is `status.phase=Rejected` and NOT reconciled — never
//!   a silent no-op that would drop e.g. `persistence`.
//! - **Prune on disable** (MED-3): a disabled Ingress/HPA is DELETED, not
//!   orphaned. A data PVC is never deleted.
//! - **Backoff + quarantine** (MED-8): repeated failures back off exponentially,
//!   then `status.phase=Invalid` and stop (no 30s hot-loop).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::autoscaling::v2 as hpav2;
use k8s_openapi::api::core::v1 as corev1;
use k8s_openapi::api::networking::v1 as netv1;
use k8s_openapi::api::policy::v1 as policyv1;
use kube::api::{Api, DeleteParams, ListParams, Patch, PatchParams};
use kube::runtime::controller::{Action, Controller};
use kube::runtime::watcher;
use kube::{Client, Resource, ResourceExt};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};

use crate::coordinator::{self, Coordinator};
use crate::crd::App;
use crate::manifests;

pub const FIELD_MANAGER: &str = "hanzod";

const REQUEUE_OK: Duration = Duration::from_secs(300);
const BACKOFF_BASE: Duration = Duration::from_secs(5);
const BACKOFF_CAP: Duration = Duration::from_secs(600);
const REQUEUE_STANDBY: Duration = Duration::from_secs(120);
/// After this many consecutive failures a CR is quarantined (phase=Invalid).
const MAX_FAILURES: u32 = 6;

pub struct Context {
    pub client: Client,
    pub coordinator: Arc<dyn Coordinator>,
    /// Per-object consecutive failure count, for backoff + quarantine.
    failures: Mutex<HashMap<String, u32>>,
}

impl Context {
    pub fn new(client: Client, coordinator: Arc<dyn Coordinator>) -> Self {
        Self { client, coordinator, failures: Mutex::new(HashMap::new()) }
    }
    fn reset(&self, key: &str) {
        self.failures.lock().unwrap().remove(key);
    }
    fn record_failure(&self, key: &str) -> u32 {
        let mut f = self.failures.lock().unwrap();
        let c = f.entry(key.to_string()).or_insert(0);
        *c += 1;
        *c
    }
    fn failure_count(&self, key: &str) -> u32 {
        self.failures.lock().unwrap().get(key).copied().unwrap_or(0)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("kube api: {0}")]
    Kube(#[from] kube::Error),
    #[error("serialize: {0}")]
    Json(#[from] serde_json::Error),
    #[error("plan: {0}")]
    Plan(#[from] anyhow::Error),
    #[error("missing namespace on {0}")]
    NoNamespace(String),
}

/// Wire the controller and run it. Verifies the CRD is reachable and enforces
/// the MED-5 single-replica guard before starting.
pub async fn run(client: Client, coordinator: Arc<dyn Coordinator>) -> anyhow::Result<()> {
    let apps: Api<App> = Api::all(client.clone());
    apps.list(&ListParams::default().limit(1))
        .await
        .map_err(|e| anyhow::anyhow!("apps.hanzo.ai not reachable — is the CRD installed? ({e})"))?;

    // MED-3/MED-5: fail CLOSED. In-cluster, a single-replica coordinator MUST
    // verify the operator is at replicas:1; if it cannot read its own count it
    // refuses to start rather than risk a silent split-brain.
    let (in_cluster, replicas) = own_deployment_replicas(&client).await;
    coordinator::singleton_boot_decision(coordinator.requires_single_replica(), in_cluster, replicas)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if coordinator.requires_single_replica() {
        tracing::info!(in_cluster, ?replicas, "single-replica boot guard passed");
    }

    let ctx = Arc::new(Context::new(client.clone(), coordinator));

    tracing::info!(field_manager = FIELD_MANAGER, "hanzod operator: watching apps.hanzo.ai");
    Controller::new(apps, watcher::Config::default())
        .owns(Api::<Deployment>::all(client.clone()), watcher::Config::default())
        .owns(Api::<corev1::Service>::all(client.clone()), watcher::Config::default())
        .owns(Api::<netv1::Ingress>::all(client.clone()), watcher::Config::default())
        .owns(Api::<hpav2::HorizontalPodAutoscaler>::all(client.clone()), watcher::Config::default())
        .shutdown_on_signal()
        .run(reconcile, error_policy, ctx)
        .for_each(|res| async move {
            match res {
                Ok((obj, _)) => tracing::debug!(app = %obj.name, "reconciled"),
                Err(e) => tracing::warn!(error = %e, "controller error"),
            }
        })
        .await;
    Ok(())
}

async fn reconcile(app: Arc<App>, ctx: Arc<Context>) -> Result<Action, Error> {
    let ns = app.namespace().ok_or_else(|| Error::NoNamespace(app.name_any()))?;
    let name = app.name_any();
    let key = format!("{ns}/{name}");

    // Leaderless gate: only the owning hanzod acts. Fail-closed.
    match ctx.coordinator.should_reconcile(&key).await {
        Ok(true) => {}
        Ok(false) => {
            tracing::debug!(%key, "not owner; standing aside");
            return Ok(Action::requeue(REQUEUE_STANDBY));
        }
        Err(e) => {
            tracing::warn!(%key, error = %e, "coordinator unavailable; standing aside (fail-closed)");
            return Ok(Action::requeue(REQUEUE_STANDBY));
        }
    }

    // LOW-5: a CR already terminal (Rejected/Invalid) for its CURRENT generation
    // is not re-processed (e.g. after an operator restart) — don't re-spend the
    // retry budget or re-log. A spec change bumps generation and re-opens it.
    if is_terminal_for_current_gen(&app) {
        tracing::debug!(%key, "already terminal for this generation; skipping");
        return Ok(Action::await_change());
    }

    // HIGH-1 + MED-4: fail-closed on unmodeled fields (top-level AND nested) and
    // on missing required persistence fields — never a silent default/no-op.
    let reasons = rejection_reasons(&app);
    if !reasons.is_empty() {
        ctx.reset(&key);
        let msg = reasons.join("; ");
        tracing::warn!(%key, reasons = %msg, "rejected");
        write_status(&ctx.client, &app, terminal_status(&app, "Rejected", &msg)).await?;
        return Ok(Action::await_change()); // terminal until the CR changes
    }

    match apply_plan(&ctx, &ns, &name, &app).await {
        Ok(()) => {
            ctx.reset(&key);
            let (ready, available) = deployment_replicas(&ctx.client, &ns, &name).await?;
            write_status(&ctx.client, &app, running_status(&app, ready, available)).await?;
            Ok(Action::requeue(REQUEUE_OK))
        }
        Err(e) => {
            let n = ctx.record_failure(&key);
            if n >= MAX_FAILURES {
                tracing::error!(%key, error = %e, failures = n, "quarantining CR (phase=Invalid)");
                let msg = format!("reconcile failed {n}x; last error: {e}");
                let _ = write_status(&ctx.client, &app, terminal_status(&app, "Invalid", &msg)).await;
                Ok(Action::await_change()) // stop the hot-loop
            } else {
                Err(e) // error_policy backs off
            }
        }
    }
}

/// Apply all owned objects; prune the ones a disabled feature no longer wants.
async fn apply_plan(ctx: &Context, ns: &str, name: &str, app: &App) -> Result<(), Error> {
    let plan = manifests::plan(app)?;

    // ConfigMap + PVC before Deployment so the config file + volume exist when
    // the pod schedules. The PVC is never pruned — deleting it is data loss.
    if let Some(cm) = &plan.configmap {
        apply(&ctx.client, ns, cm).await?;
    }
    // Bucket-init Job before the Deployment: a greenfield app's first snapshot
    // 404s (NoSuchBucket) without it. Idempotent (`mb --ignore-existing`); never
    // pruned (TTL cleans it up).
    if let Some(job) = &plan.bucket_job {
        apply(&ctx.client, ns, job).await?;
    }
    if let Some(pvc) = &plan.pvc {
        apply(&ctx.client, ns, pvc).await?;
    }
    apply(&ctx.client, ns, &plan.deployment).await?;
    apply(&ctx.client, ns, &plan.service).await?;

    match &plan.ingress {
        Some(ing) => apply(&ctx.client, ns, ing).await?,
        None => prune::<netv1::Ingress>(&ctx.client, ns, name).await?, // MED-3
    }
    match &plan.hpa {
        Some(hpa) => apply(&ctx.client, ns, hpa).await?,
        None => prune::<hpav2::HorizontalPodAutoscaler>(&ctx.client, ns, name).await?,
    }
    match &plan.pdb {
        Some(pdb) => apply(&ctx.client, ns, pdb).await?,
        None => prune::<policyv1::PodDisruptionBudget>(&ctx.client, ns, name).await?,
    }
    Ok(())
}

/// Exponential backoff keyed on the object's consecutive failure count.
fn error_policy(app: Arc<App>, err: &Error, ctx: Arc<Context>) -> Action {
    let key = format!("{}/{}", app.namespace().unwrap_or_default(), app.name_any());
    let n = ctx.failure_count(&key);
    let backoff = backoff_duration(n);
    tracing::warn!(%key, error = %err, failures = n, backoff_s = backoff.as_secs(), "reconcile failed; backing off");
    Action::requeue(backoff)
}

/// Exponential backoff: `BACKOFF_BASE * 2^n`, capped at `BACKOFF_CAP`. Pure so
/// MED-8 is unit-tested without a controller.
fn backoff_duration(n: u32) -> Duration {
    BACKOFF_BASE
        .saturating_mul(2u32.saturating_pow(n.min(7)))
        .min(BACKOFF_CAP)
}

/// Unmodeled keys anywhere hanzod carries an `extra` catch-all, as sorted dotted
/// paths (`ingress.zeroTrustPolicy`, `persistence.dataDi`, …). MED-4: nested, not
/// just top-level. NOTE the authoritative structural CRD prunes truly-unknown
/// keys at admission, so this is defense-in-depth; the data-critical typos are
/// caught by `persistence_reasons` below (a pruned required field ⇒ missing ⇒
/// reject) rather than by observing the typo.
fn unknown_fields(app: &App) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |prefix: &str, extra: &std::collections::BTreeMap<String, serde_json::Value>| {
        for k in extra.keys() {
            out.push(if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") });
        }
    };
    push("", &app.spec.extra);
    if let Some(i) = &app.spec.ingress {
        push("ingress", &i.extra);
    }
    if let Some(p) = &app.spec.persistence {
        push("persistence", &p.extra);
        if let Some(st) = &p.storage {
            push("persistence.storage", &st.extra);
        }
    }
    if let Some(a) = &app.spec.autoscaling {
        push("autoscaling", &a.extra);
    }
    if let Some(pdb) = &app.spec.pdb {
        push("pdb", &pdb.extra);
    }
    if let Some(pr) = &app.spec.liveness_probe {
        push("livenessProbe", &pr.extra);
    }
    if let Some(pr) = &app.spec.readiness_probe {
        push("readinessProbe", &pr.extra);
    }
    out.sort();
    out
}

/// Required-field checks for enabled persistence — catches a pruned/typo'd
/// critical field (dataDir/dbPath/bucket/storage) that would otherwise silently
/// default and mount the DB at the wrong (ephemeral) path = data loss.
fn persistence_reasons(app: &App) -> Vec<String> {
    let mut r = Vec::new();
    if let Some(p) = app.spec.persistence.as_ref().filter(|p| p.enabled) {
        if p.dir_mode {
            r.push("persistence.dirMode: multi-DB directory mode is not supported by hanzod yet".into());
        }
        if p.bucket.as_deref().unwrap_or("").is_empty() {
            r.push("persistence.bucket: required when persistence is enabled".into());
        }
        if p.data_dir.as_deref().unwrap_or("").is_empty() {
            r.push("persistence.dataDir: required when persistence is enabled".into());
        }
        if !p.dir_mode && p.db_path.as_deref().unwrap_or("").is_empty() {
            r.push("persistence.dbPath: required unless dirMode".into());
        }
        match p.storage.as_ref() {
            None => r.push("persistence.storage: required (retained PVC size)".into()),
            Some(st) if st.size.trim().is_empty() => r.push("persistence.storage.size: required".into()),
            _ => {}
        }
    }
    r
}

/// All reasons to reject a CR (fail-closed). Empty ⇒ reconcilable.
fn rejection_reasons(app: &App) -> Vec<String> {
    let mut r = unknown_fields(app)
        .into_iter()
        .map(|f| format!("unsupported field: {f}"))
        .collect::<Vec<_>>();
    r.extend(persistence_reasons(app));
    r
}

/// Whether the CR is already in a terminal phase (Rejected/Invalid) for its
/// CURRENT generation — the LOW-5 short-circuit signal.
fn is_terminal_for_current_gen(app: &App) -> bool {
    let gen = app.meta().generation.unwrap_or(0);
    app.status.as_ref().is_some_and(|st| {
        matches!(st.phase.as_deref(), Some("Rejected") | Some("Invalid")) && st.observed_generation == gen
    })
}

/// Server-side apply one owned object (GVK injected — SSA requires it).
async fn apply<K>(client: &Client, ns: &str, obj: &K) -> Result<(), Error>
where
    K: Resource<DynamicType = (), Scope = k8s_openapi::NamespaceResourceScope>
        + Serialize
        + DeserializeOwned
        + Clone
        + std::fmt::Debug,
{
    let api: Api<K> = Api::namespaced(client.clone(), ns);
    let name = obj.name_any();
    let val = manifests::ssa_body(obj)?;
    api.patch(&name, &PatchParams::apply(FIELD_MANAGER).force(), &Patch::Apply(&val))
        .await?;
    Ok(())
}

/// Delete a HANZOD-OWNED object by name (MED-2). Only deletes when the object
/// exists AND carries `app.kubernetes.io/managed-by=hanzod` — never a
/// hand-created same-named Ingress/HPA, and never a spurious DELETE when the
/// object was already absent.
async fn prune<K>(client: &Client, ns: &str, name: &str) -> Result<(), Error>
where
    K: Resource<DynamicType = (), Scope = k8s_openapi::NamespaceResourceScope>
        + Clone
        + DeserializeOwned
        + std::fmt::Debug,
{
    let api: Api<K> = Api::namespaced(client.clone(), ns);
    let Some(obj) = api.get_opt(name).await? else {
        return Ok(()); // absent — nothing to prune, no DELETE call
    };
    if !owned_by_hanzod(&obj) {
        tracing::debug!(kind = K::kind(&()).as_ref(), name, "not hanzod-owned; leaving in place");
        return Ok(());
    }
    match api.delete(name, &DeleteParams::default()).await {
        Ok(_) => {
            tracing::debug!(kind = K::kind(&()).as_ref(), name, "pruned disabled owned object");
            Ok(())
        }
        Err(kube::Error::Api(ae)) if ae.code == 404 => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// True iff the object carries hanzod's manager label — the marker every owned
/// object gets via `manifests::base_labels`.
fn owned_by_hanzod<K: ResourceExt>(obj: &K) -> bool {
    obj.labels().get("app.kubernetes.io/managed-by").map(String::as_str) == Some("hanzod")
}

async fn deployment_replicas(client: &Client, ns: &str, name: &str) -> Result<(i32, i32), Error> {
    let deps: Api<Deployment> = Api::namespaced(client.clone(), ns);
    Ok(deps
        .get_opt(name)
        .await?
        .and_then(|d| d.status)
        .map(|s| (s.ready_replicas.unwrap_or(0), s.available_replicas.unwrap_or(0)))
        .unwrap_or((0, 0)))
}

fn running_status(app: &App, ready: i32, available: i32) -> Value {
    let desired = app.spec.replicas.unwrap_or(1);
    let phase = if desired > 0 && ready >= desired {
        "Running"
    } else if ready > 0 {
        "Degraded"
    } else {
        "Creating"
    };
    json!({ "status": {
        "observedGeneration": app.meta().generation.unwrap_or(0),
        "readyReplicas": ready,
        "availableReplicas": available,
        "phase": phase,
        "message": Value::Null, // clear any prior Rejected/Invalid message
    }})
}

fn terminal_status(app: &App, phase: &str, message: &str) -> Value {
    json!({ "status": {
        "observedGeneration": app.meta().generation.unwrap_or(0),
        "phase": phase,
        "message": message,
    }})
}

async fn write_status(client: &Client, app: &App, status: Value) -> Result<(), Error> {
    let ns = app.namespace().ok_or_else(|| Error::NoNamespace(app.name_any()))?;
    let api: Api<App> = Api::namespaced(client.clone(), &ns);
    api.patch_status(&app.name_any(), &PatchParams::default(), &Patch::Merge(&status))
        .await?;
    Ok(())
}

/// `(in_cluster, replicas)` for the fail-closed boot guard. `in_cluster` is
/// whether the operator runs under a k8s ServiceAccount (SA namespace file or
/// `POD_NAMESPACE`). `replicas` is `Some` only when the operator's OWN
/// Deployment was actually read. `HANZOD_DEPLOYMENT_NAME` MUST name that
/// Deployment in-cluster — there is NO default (the shipped name differs per
/// install, e.g. `operator-controller-manager`); when unset/mismatched,
/// `replicas` is `None` and the boot guard refuses to start.
async fn own_deployment_replicas(client: &Client) -> (bool, Option<i32>) {
    let ns = std::fs::read_to_string("/var/run/secrets/kubernetes.io/serviceaccount/namespace")
        .ok()
        .map(|s| s.trim().to_string())
        .or_else(|| std::env::var("POD_NAMESPACE").ok());
    let in_cluster = ns.is_some();
    let (Some(ns), Ok(name)) = (ns, std::env::var("HANZOD_DEPLOYMENT_NAME")) else {
        return (in_cluster, None);
    };
    let deps: Api<Deployment> = Api::namespaced(client.clone(), &ns);
    let replicas = deps
        .get(&name)
        .await
        .ok()
        .map(|d| d.spec.and_then(|s| s.replicas).unwrap_or(1));
    (in_cluster, replicas)
}


#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn app_with(spec: serde_json::Value) -> App {
        serde_json::from_value(json!({
            "apiVersion": "hanzo.ai/v1",
            "kind": "App",
            "metadata": {"name": "x", "namespace": "hanzo"},
            "spec": spec,
        }))
        .unwrap()
    }

    #[test]
    fn unknown_fields_flags_unmodeled_spec_keys() {
        // networkPolicy is still unmodeled -> flagged for reject (pdb is modeled now).
        let app = app_with(json!({"image": {"repository": "r"}, "networkPolicy": {"enabled": true}}));
        assert_eq!(unknown_fields(&app), vec!["networkPolicy".to_string()]);
    }

    #[test]
    fn unknown_fields_empty_for_fully_modeled_cr() {
        let app = app_with(json!({
            "image": {"repository": "r"},
            "replicas": 2,
            "persistence": {"enabled": true, "bucket": "b"},
            "autoscaling": {"enabled": true}
        }));
        assert!(unknown_fields(&app).is_empty());
    }

    #[test]
    fn backoff_is_exponential_and_capped() {
        assert_eq!(backoff_duration(0), BACKOFF_BASE);
        assert_eq!(backoff_duration(1), BACKOFF_BASE * 2);
        assert_eq!(backoff_duration(2), BACKOFF_BASE * 4);
        // monotonic non-decreasing, never above the cap
        let mut prev = Duration::ZERO;
        for n in 0..20 {
            let b = backoff_duration(n);
            assert!(b >= prev);
            assert!(b <= BACKOFF_CAP);
            prev = b;
        }
        assert_eq!(backoff_duration(100), BACKOFF_CAP);
    }

    #[test]
    fn nested_unknown_field_flagged_with_dotted_path() {
        // MED-4: an unmodeled key under ingress is caught with a dotted path.
        let app = app_with(json!({
            "image": {"repository": "r"},
            "ingress": {"enabled": true, "zeroTrustPolicy": {"enabled": true}}
        }));
        assert_eq!(unknown_fields(&app), vec!["ingress.zeroTrustPolicy".to_string()]);
    }

    #[test]
    fn persistence_missing_required_fields_is_rejected() {
        // enabled persistence with only a bucket -> dataDir/dbPath/storage missing.
        let app = app_with(json!({
            "image": {"repository": "r"},
            "persistence": {"enabled": true, "bucket": "b"}
        }));
        let reasons = rejection_reasons(&app);
        assert!(reasons.iter().any(|r| r.contains("persistence.dataDir")));
        assert!(reasons.iter().any(|r| r.contains("persistence.dbPath")));
        assert!(reasons.iter().any(|r| r.contains("persistence.storage")));
    }

    #[test]
    fn persistence_dir_mode_is_rejected() {
        let app = app_with(json!({
            "image": {"repository": "r"},
            "persistence": {"enabled": true, "dirMode": true, "bucket": "b",
                            "dataDir": "/d", "storage": {"size": "1Gi"}}
        }));
        assert!(rejection_reasons(&app).iter().any(|r| r.contains("dirMode")));
    }

    #[test]
    fn cloud_like_cr_with_pdb_and_surge_is_not_rejected() {
        // cloud.yaml carries pdb + surgeColocation, which USED to be rejected.
        let app = app_with(json!({
            "image": {"repository": "ghcr.io/hanzoai/cloud", "tag": "1"},
            "replicas": 1,
            "strategy": "Recreate",
            "surgeColocation": false,
            "pdb": {"enabled": true, "maxUnavailable": 1},
            "partOf": "cloud", "component": "api"
        }));
        assert!(rejection_reasons(&app).is_empty(), "cloud CR must not be rejected: {:?}", rejection_reasons(&app));
    }

    #[test]
    fn complete_persistence_cr_is_accepted() {
        let app = app_with(json!({
            "image": {"repository": "r"},
            "persistence": {"enabled": true, "bucket": "b", "dataDir": "/d",
                            "dbPath": "a.db", "storage": {"size": "1Gi"}}
        }));
        assert!(rejection_reasons(&app).is_empty());
    }

    #[test]
    fn prune_owner_check_only_matches_hanzod_managed() {
        use k8s_openapi::api::networking::v1::Ingress;
        use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
        let mut managed = Ingress::default();
        managed.metadata = ObjectMeta {
            labels: Some(std::collections::BTreeMap::from([(
                "app.kubernetes.io/managed-by".to_string(),
                "hanzod".to_string(),
            )])),
            ..Default::default()
        };
        assert!(owned_by_hanzod(&managed));
        // a hand-created object without the label must NOT be pruned
        assert!(!owned_by_hanzod(&Ingress::default()));
    }

    #[test]
    fn terminal_short_circuit_only_for_current_generation() {
        let mut app = app_with(json!({"image": {"repository": "r"}}));
        app.metadata.generation = Some(3);
        app.status = Some(crate::crd::AppStatus {
            phase: Some("Rejected".into()),
            observed_generation: 3,
            ..Default::default()
        });
        assert!(is_terminal_for_current_gen(&app)); // same gen -> skip
        // a spec change bumps generation -> re-open for reconcile
        app.metadata.generation = Some(4);
        assert!(!is_terminal_for_current_gen(&app));
    }
}
