//! IAM reconciler — newtype facade over Service. The Hanzo IAM identity
//! provider (Casdoor) runs as an ordinary Service-shaped workload; the IAM
//! CRD is the semantic marker for "this is the identity provider", letting
//! platform-side tooling key off the Kind without inspecting the inner spec.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use kube::api::Api;
use kube::runtime::controller::{Action, Controller};
use kube::runtime::watcher::Config;
use kube::{Client, ResourceExt};
use tracing::{error, info};

use crate::core::{OperatorError, Result};
use crate::crd::IAM;

use super::{owner_ref_for, service};

#[derive(Clone)]
pub struct Ctx {
    pub client: Client,
    pub api_group: String,
}

pub async fn reconcile(cr: Arc<IAM>, ctx: Arc<Ctx>) -> Result<Action> {
    let name = cr.name_any();
    let namespace = cr
        .namespace()
        .ok_or_else(|| OperatorError::Config("IAM has no namespace".into()))?;
    let api_version = format!("{}/v1", ctx.api_group);
    let owner = owner_ref_for(cr.as_ref(), &api_version, "IAM");
    service::reconcile_service_inner_pub(&ctx.client, &name, &namespace, &cr.spec.0, owner).await?;
    Ok(Action::requeue(Duration::from_secs(60)))
}

pub fn on_error(_obj: Arc<IAM>, err: &OperatorError, _ctx: Arc<Ctx>) -> Action {
    error!(error = %err, "IAM reconcile failed");
    Action::requeue(Duration::from_secs(30))
}

pub async fn run_iam_controller(client: Client, namespace: String, api_group: String) {
    let api: Api<IAM> = if namespace.is_empty() {
        Api::all(client.clone())
    } else {
        Api::namespaced(client.clone(), &namespace)
    };
    info!(group = %api_group, "Starting IAM controller");
    let ctx = Arc::new(Ctx { client, api_group });
    Controller::new(api, Config::default())
        .run(reconcile, on_error, ctx)
        .for_each(|_| async {})
        .await;
}

#[cfg(test)]
mod tests {
    use crate::crd::{IAMSpec, ImageSpec, ServiceSpec};

    #[test]
    fn iam_spec_unwraps_to_service_spec() {
        let inner = ServiceSpec {
            image: ImageSpec {
                repository: "ghcr.io/hanzoai/iam".to_string(),
                tag: "v1.2.3".to_string(),
                pull_policy: "IfNotPresent".to_string(),
            },
            ..Default::default()
        };
        let facade = IAMSpec(inner.clone());
        assert_eq!(facade.0.image.repository, inner.image.repository);
        assert_eq!(facade.0.image.tag, "v1.2.3");
    }
}
