//! SPA reconciler — hanzoai/spa standalone Go runtime, per-site pod.
//!
//! Materializes:
//! - Deployment with the runtime image
//! - initContainer COPYing from spec.content image into shared /public emptyDir
//! - SPA_* env vars from spec.config map (runtime templates /public/config.json
//!   from these at startup)
//! - Service exposing :3000
//! - Ingress (if spec.ingress.enabled)
//! - PDB (if spec.pdb)

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use kube::api::Api;
use kube::runtime::controller::{Action, Controller};
use kube::runtime::watcher::Config;
use kube::{Client, ResourceExt};
use tracing::{error, info};

use crate::core::{OperatorError, Result};
use crate::crd::SPA;

use super::{owner_ref_for, service};
use crate::crd::{ServicePort, ServiceSpec};

#[derive(Clone)]
pub struct Ctx {
    pub client: Client,
    pub api_group: String,
}

pub async fn reconcile(cr: Arc<SPA>, ctx: Arc<Ctx>) -> Result<Action> {
    let name = cr.name_any();
    let namespace = cr
        .namespace()
        .ok_or_else(|| OperatorError::Config("SPA has no namespace".into()))?;
    let api_version = format!("{}/v1", ctx.api_group);
    let owner = owner_ref_for(cr.as_ref(), &api_version, "SPA");

    // Synthesize a ServiceSpec from the SPA's content+runtime+config:
    //   - main container = runtime image, port 3000
    //   - SPA_* env vars from spec.config
    //   - one emptyDir volume /public shared with initContainer
    //   - initContainer pulls spec.content and COPYs to /public
    // For v0.3.2 we use the Service reconciler with runtime image directly and
    // env-vars carrying the SPA_* config. initContainer fan-out lands in v0.3.3.
    let inner = &cr.spec.0;
    let svc_spec = ServiceSpec {
        image: inner.runtime.clone(),
        replicas: Some(inner.replicas),
        ports: vec![ServicePort {
            container_port: 3000,
            name: "http".into(),
            service_port: None,
            protocol: String::new(),
        }],
        env: inner
            .config
            .iter()
            .map(|(k, v)| crate::crd_types::EnvVar {
                name: k.clone(),
                value: Some(v.clone()),
                value_from: None,
            })
            .collect(),
        ingress: inner.ingress.clone(),
        pdb: inner.pdb.clone(),
        resources: inner.resources.clone(),
        ..Default::default()
    };

    service::reconcile_service_inner_pub(&ctx.client, &name, &namespace, &svc_spec, owner).await?;
    Ok(Action::requeue(Duration::from_secs(60)))
}

pub fn on_error(_obj: Arc<SPA>, err: &OperatorError, _ctx: Arc<Ctx>) -> Action {
    error!(error = %err, "SPA reconcile failed");
    Action::requeue(Duration::from_secs(30))
}

pub async fn run_spa_controller(client: Client, namespace: String, api_group: String) {
    let api: Api<SPA> = if namespace.is_empty() {
        Api::all(client.clone())
    } else {
        Api::namespaced(client.clone(), &namespace)
    };
    info!(group = %api_group, "Starting SPA controller");
    let ctx = Arc::new(Ctx { client, api_group });
    Controller::new(api, Config::default())
        .run(reconcile, on_error, ctx)
        .for_each(|_| async {})
        .await;
}
