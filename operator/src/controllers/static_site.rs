//! Static reconciler — hanzoai/static via Hanzo Ingress middleware plugin.
//!
//! No separate pod. The ingress controller's static middleware serves files
//! from a ConfigMap mount or an OCI image (loaded by an init job that pushes
//! content to a shared volume). Materializes an Ingress with the
//! `hanzoai/static` middleware annotation pointing at the content source.
//!
//! For v0.3.2 the controller only validates the spec and requeues. Full
//! Ingress materialization with the static middleware annotation lands in
//! v0.3.3 once the hanzoai/ingress plugin spec stabilizes.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use kube::api::Api;
use kube::runtime::controller::{Action, Controller};
use kube::runtime::watcher::Config;
use kube::{Client, ResourceExt};
use tracing::{error, info, warn};

use crate::core::{OperatorError, Result};
use crate::crd::Static;

#[derive(Clone)]
pub struct Ctx {
    pub client: Client,
    pub api_group: String,
}

pub async fn reconcile(cr: Arc<Static>, _ctx: Arc<Ctx>) -> Result<Action> {
    let name = cr.name_any();
    let namespace = cr
        .namespace()
        .ok_or_else(|| OperatorError::Config("Static has no namespace".into()))?;
    let inner = &cr.spec.0;

    if inner.config_map.is_empty() && inner.image.is_none() {
        warn!(
            name = %name,
            namespace = %namespace,
            "Static CR has neither config_map nor image — nothing to serve"
        );
    } else {
        info!(
            name = %name,
            namespace = %namespace,
            "Static CR observed (full Ingress middleware materialization pending v0.3.3)"
        );
    }

    Ok(Action::requeue(Duration::from_secs(300)))
}

pub fn on_error(_obj: Arc<Static>, err: &OperatorError, _ctx: Arc<Ctx>) -> Action {
    error!(error = %err, "Static reconcile failed");
    Action::requeue(Duration::from_secs(60))
}

pub async fn run_static_controller(client: Client, namespace: String, api_group: String) {
    let api: Api<Static> = if namespace.is_empty() {
        Api::all(client.clone())
    } else {
        Api::namespaced(client.clone(), &namespace)
    };
    info!(group = %api_group, "Starting Static controller");
    let ctx = Arc::new(Ctx { client, api_group });
    Controller::new(api, Config::default())
        .run(reconcile, on_error, ctx)
        .for_each(|_| async {})
        .await;
}
