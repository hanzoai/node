//! Observability reconciler — newtype facade over Service. Grafana / OTEL Collector / VictoriaMetrics
//! components run as ordinary Service-shaped workloads; the Observability CRD is the
//! semantic marker for "this is an observability component", letting platform-side
//! tooling key off the Kind without inspecting the inner spec.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use kube::api::Api;
use kube::runtime::controller::{Action, Controller};
use kube::runtime::watcher::Config;
use kube::{Client, ResourceExt};
use tracing::{error, info};

use crate::core::{OperatorError, Result};
use crate::crd::Observability;

use super::{owner_ref_for, service};

#[derive(Clone)]
pub struct Ctx {
    pub client: Client,
    pub api_group: String,
}

pub async fn reconcile(cr: Arc<Observability>, ctx: Arc<Ctx>) -> Result<Action> {
    let name = cr.name_any();
    let namespace = cr
        .namespace()
        .ok_or_else(|| OperatorError::Config("Observability has no namespace".into()))?;
    let api_version = format!("{}/v1", ctx.api_group);
    let owner = owner_ref_for(cr.as_ref(), &api_version, "Observability");
    service::reconcile_service_inner_pub(&ctx.client, &name, &namespace, &cr.spec.0, owner).await?;
    Ok(Action::requeue(Duration::from_secs(60)))
}

pub fn on_error(_obj: Arc<Observability>, err: &OperatorError, _ctx: Arc<Ctx>) -> Action {
    error!(error = %err, "Observability reconcile failed");
    Action::requeue(Duration::from_secs(30))
}

pub async fn run_observability_controller(client: Client, namespace: String, api_group: String) {
    let api: Api<Observability> = if namespace.is_empty() {
        Api::all(client.clone())
    } else {
        Api::namespaced(client.clone(), &namespace)
    };
    info!(group = %api_group, "Starting Observability controller");
    let ctx = Arc::new(Ctx { client, api_group });
    Controller::new(api, Config::default())
        .run(reconcile, on_error, ctx)
        .for_each(|_| async {})
        .await;
}
