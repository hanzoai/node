// Copyright 2026 Hanzo AI Inc. All Rights Reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! The controller: watch `services.hanzo.ai`, reconcile each to its owned
//! objects. This is the thin *effect* layer — the decision (what objects) is
//! [`crate::manifests::plan`], the leaderless gate (whether THIS hanzod acts) is
//! [`crate::coordinator::Coordinator`]. Reconcile = gate, plan, server-side
//! apply, write status. Idempotent: every apply is an SSA with field manager
//! `hanzod`, so repeated reconciles converge without churn.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1 as corev1;
use k8s_openapi::api::networking::v1 as netv1;
use kube::api::{Api, ListParams, Patch, PatchParams};
use kube::runtime::controller::{Action, Controller};
use kube::runtime::watcher;
use kube::{Client, Resource, ResourceExt};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::json;

use crate::coordinator::Coordinator;
use crate::crd::Service;
use crate::manifests;

/// SSA field manager. All owned objects are owned by this manager so hanzod's
/// writes are attributable and never fight another manager's fields.
pub const FIELD_MANAGER: &str = "hanzod";

const REQUEUE_OK: Duration = Duration::from_secs(300);
const REQUEUE_ERR: Duration = Duration::from_secs(30);
const REQUEUE_STANDBY: Duration = Duration::from_secs(120);

/// Reconcile-loop dependencies. `coordinator` is the leaderless seam — swap
/// `StaticCoordinator` for an HRW/ZAP-BFT one with no change to this module.
pub struct Context {
    pub client: Client,
    pub coordinator: Arc<dyn Coordinator>,
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

/// Wire the controller and run it until shutdown. Verifies the CRD is reachable
/// first so a missing CRD fails fast with a clear message instead of a silent
/// empty watch.
pub async fn run(client: Client, coordinator: Arc<dyn Coordinator>) -> anyhow::Result<()> {
    let services: Api<Service> = Api::all(client.clone());
    services
        .list(&ListParams::default().limit(1))
        .await
        .map_err(|e| anyhow::anyhow!("services.hanzo.ai not reachable — is the CRD installed? ({e})"))?;

    let ctx = Arc::new(Context { client: client.clone(), coordinator });

    tracing::info!(field_manager = FIELD_MANAGER, "hanzod operator: watching services.hanzo.ai");
    Controller::new(services, watcher::Config::default())
        .owns(Api::<Deployment>::all(client.clone()), watcher::Config::default())
        .owns(Api::<corev1::Service>::all(client.clone()), watcher::Config::default())
        .owns(Api::<netv1::Ingress>::all(client.clone()), watcher::Config::default())
        .shutdown_on_signal()
        .run(reconcile, error_policy, ctx)
        .for_each(|res| async move {
            match res {
                Ok((obj, _)) => tracing::debug!(service = %obj.name, "reconciled"),
                Err(e) => tracing::warn!(error = %e, "controller error"),
            }
        })
        .await;
    Ok(())
}

async fn reconcile(svc: Arc<Service>, ctx: Arc<Context>) -> Result<Action, Error> {
    let ns = svc.namespace().ok_or_else(|| Error::NoNamespace(svc.name_any()))?;
    let name = svc.name_any();
    let key = format!("{ns}/{name}");

    // Leaderless gate: only the owning hanzod reconciles this object. Fail
    // closed — a coordinator error means stand aside, never risk two writers.
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

    let plan = manifests::plan(&svc)?;
    apply(&ctx.client, &ns, &plan.deployment).await?;
    apply(&ctx.client, &ns, &plan.service).await?;
    if let Some(ing) = &plan.ingress {
        apply(&ctx.client, &ns, ing).await?;
    }

    let deps: Api<Deployment> = Api::namespaced(ctx.client.clone(), &ns);
    let (ready, available) = deps
        .get_opt(&name)
        .await?
        .and_then(|d| d.status)
        .map(|s| (s.ready_replicas.unwrap_or(0), s.available_replicas.unwrap_or(0)))
        .unwrap_or((0, 0));

    write_status(&ctx.client, &svc, ready, available).await?;
    Ok(Action::requeue(REQUEUE_OK))
}

fn error_policy(svc: Arc<Service>, err: &Error, _ctx: Arc<Context>) -> Action {
    tracing::warn!(service = %svc.name_any(), error = %err, "reconcile failed; requeueing");
    Action::requeue(REQUEUE_ERR)
}

/// Server-side apply one owned object. k8s-openapi types serialize without
/// apiVersion/kind, which SSA requires, so inject the GVK from the `Resource`
/// impl before patching. `force` claims fields hanzod owns.
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

/// Write the status subresource: observed generation, replica counts, and a
/// derived phase (`kubectl get hsvc` reads phase + readyReplicas).
async fn write_status(client: &Client, svc: &Service, ready: i32, available: i32) -> Result<(), Error> {
    let ns = svc.namespace().ok_or_else(|| Error::NoNamespace(svc.name_any()))?;
    let api: Api<Service> = Api::namespaced(client.clone(), &ns);
    let desired = svc.spec.replicas.unwrap_or(1);
    let phase = if desired > 0 && ready >= desired {
        "Running"
    } else if ready > 0 {
        "Degraded"
    } else {
        "Creating"
    };
    let patch = json!({
        "status": {
            "observedGeneration": svc.meta().generation.unwrap_or(0),
            "readyReplicas": ready,
            "availableReplicas": available,
            "phase": phase,
        }
    });
    api.patch_status(&svc.name_any(), &PatchParams::default(), &Patch::Merge(&patch))
        .await?;
    Ok(())
}
