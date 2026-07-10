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

    // MED-5: a coordination-free coordinator is only safe at one replica.
    if coordinator.requires_single_replica() {
        if let Some(replicas) = own_deployment_replicas(&client).await {
            coordinator::check_singleton(true, replicas).map_err(|e| anyhow::anyhow!("{e}"))?;
            tracing::info!(replicas, "single-replica guard passed");
        } else {
            tracing::warn!("could not read own Deployment replicas; single-replica guard is best-effort");
        }
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

    // HIGH-1: fail-closed on any spec field hanzod does not model.
    let unknown = unknown_fields(&app);
    if !unknown.is_empty() {
        ctx.reset(&key);
        let msg = format!("unsupported spec field(s): {} — the authoritative CRD (crds.yaml) is a superset; hanzod does not yet reconcile these", unknown.join(", "));
        tracing::warn!(%key, fields = %unknown.join(","), "rejected: unmodeled fields");
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

    // PVC before Deployment so the volume exists when the pod schedules. Never
    // pruned — deleting a data PVC is data loss.
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

/// Spec keys hanzod does not model (captured by `AppSpec::extra`), sorted.
fn unknown_fields(app: &App) -> Vec<String> {
    let mut keys: Vec<String> = app.spec.extra.keys().cloned().collect();
    keys.sort();
    keys
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
    let mut val = serde_json::to_value(obj)?;
    val["apiVersion"] = json!(K::api_version(&()));
    val["kind"] = json!(K::kind(&()));
    api.patch(&name, &PatchParams::apply(FIELD_MANAGER).force(), &Patch::Apply(&val))
        .await?;
    Ok(())
}

/// Delete an owned object by name, ignoring NotFound (idempotent prune).
async fn prune<K>(client: &Client, ns: &str, name: &str) -> Result<(), Error>
where
    K: Resource<DynamicType = (), Scope = k8s_openapi::NamespaceResourceScope>
        + Clone
        + DeserializeOwned
        + std::fmt::Debug,
{
    let api: Api<K> = Api::namespaced(client.clone(), ns);
    match api.delete(name, &DeleteParams::default()).await {
        Ok(_) => {
            tracing::debug!(kind = K::kind(&()).as_ref(), name, "pruned disabled owned object");
            Ok(())
        }
        Err(kube::Error::Api(ae)) if ae.code == 404 => Ok(()),
        Err(e) => Err(e.into()),
    }
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

/// Best-effort read of the operator's OWN Deployment replica count for the
/// MED-5 guard. Namespace from the in-cluster SA file (or `POD_NAMESPACE`);
/// Deployment name from `HANZOD_DEPLOYMENT_NAME` (default `hanzod-operator`).
async fn own_deployment_replicas(client: &Client) -> Option<i32> {
    let ns = std::fs::read_to_string("/var/run/secrets/kubernetes.io/serviceaccount/namespace")
        .ok()
        .map(|s| s.trim().to_string())
        .or_else(|| std::env::var("POD_NAMESPACE").ok())?;
    let name = std::env::var("HANZOD_DEPLOYMENT_NAME").unwrap_or_else(|_| "hanzod-operator".to_string());
    let deps: Api<Deployment> = Api::namespaced(client.clone(), &ns);
    let dep = deps.get_opt(&name).await.ok()??;
    Some(dep.spec.and_then(|s| s.replicas).unwrap_or(1))
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
        // A CR using pdb (which hanzod does not model yet) is flagged for reject.
        let app = app_with(json!({"image": {"repository": "r"}, "pdb": {"enabled": true}}));
        assert_eq!(unknown_fields(&app), vec!["pdb".to_string()]);
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
}
