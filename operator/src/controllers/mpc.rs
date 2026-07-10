//! MPC reconciler — multi-party computation threshold signing cluster.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use k8s_openapi::api::apps::v1::StatefulSet;
use k8s_openapi::api::core::v1::Service as CoreService;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use kube::api::Api;
use kube::runtime::controller::{Action, Controller};
use kube::runtime::watcher::Config;
use kube::{Client, ResourceExt};
use tracing::{debug, error, info, warn};

use crate::apply;
use crate::core::{OperatorError, Result};
use crate::crd::{MPCSpec, ServicePort as CrServicePort, MPC};
use crate::manifests;

use super::owner_ref_for;

#[derive(Clone)]
pub struct Ctx {
    pub client: Client,
    pub api_group: String,
}

pub async fn reconcile(cr: Arc<MPC>, ctx: Arc<Ctx>) -> Result<Action> {
    let name = cr.name_any();
    let namespace = cr
        .namespace()
        .ok_or_else(|| OperatorError::Config("MPC has no namespace".into()))?;
    let api_version = format!("{}/v1", ctx.api_group);
    let owner = owner_ref_for(cr.as_ref(), &api_version, "MPC");
    reconcile_inner(&ctx.client, &name, &namespace, &cr.spec, owner).await?;
    Ok(Action::requeue(Duration::from_secs(60)))
}

async fn reconcile_inner(
    client: &Client,
    name: &str,
    namespace: &str,
    spec: &MPCSpec,
    owner: OwnerReference,
) -> Result<()> {
    if spec.replicas < spec.threshold + 1 {
        return Err(OperatorError::Config(format!(
            "MPC replicas ({}) must be >= threshold ({}) + 1",
            spec.replicas, spec.threshold
        )));
    }

    let labels = manifests::standard_labels(name, "mpc", "", &spec.image.tag);
    let sel = manifests::selector_labels(name);

    let p2p = if spec.p2p_port > 0 {
        spec.p2p_port
    } else {
        4000
    };
    let api_port = if spec.api_port > 0 {
        spec.api_port
    } else {
        8080
    };
    let ports = vec![
        CrServicePort {
            name: "p2p".to_string(),
            container_port: p2p,
            service_port: None,
            protocol: "TCP".to_string(),
        },
        CrServicePort {
            name: "api".to_string(),
            container_port: api_port,
            service_port: None,
            protocol: "TCP".to_string(),
        },
    ];

    let main = manifests::build_container(
        name,
        &manifests::image_ref(&spec.image.repository, &spec.image.tag),
        &spec.image.pull_policy,
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        manifests::container_ports(&ports),
        spec.resources.as_ref().map(manifests::to_k8s_resources),
        None,
        None,
    );

    let mut sts = manifests::build_statefulset(
        name,
        namespace,
        labels.clone(),
        sel.clone(),
        Some(spec.replicas),
        vec![main],
        vec![],
        vec![],
        vec![],
        &format!("{}-hs", name),
    );
    sts.metadata.owner_references = Some(vec![owner.clone()]);
    let stss: Api<StatefulSet> = Api::namespaced(client.clone(), namespace);
    apply::apply(&stss, &sts).await?;

    let svc_ports = manifests::service_ports(&ports);
    let mut hs = manifests::build_headless_service(
        &format!("{}-hs", name),
        namespace,
        labels.clone(),
        svc_ports.clone(),
        sel.clone(),
    );
    hs.metadata.owner_references = Some(vec![owner.clone()]);
    let svcs: Api<CoreService> = Api::namespaced(client.clone(), namespace);
    apply::apply(&svcs, &hs).await?;

    let mut clip = manifests::build_service(name, namespace, labels, svc_ports, sel);
    clip.metadata.owner_references = Some(vec![owner.clone()]);
    apply::apply(&svcs, &clip).await?;

    debug!(name, namespace, "MPC reconciled");
    Ok(())
}

pub fn on_error(_obj: Arc<MPC>, err: &OperatorError, _ctx: Arc<Ctx>) -> Action {
    error!(error = %err, "MPC reconcile failed");
    Action::requeue(Duration::from_secs(30))
}

pub async fn run_mpc_controller(client: Client, namespace: String, api_group: String) {
    let api: Api<MPC> = if namespace.is_empty() {
        Api::all(client.clone())
    } else {
        Api::namespaced(client.clone(), &namespace)
    };
    info!(group = %api_group, "Starting MPC controller");
    let ctx = Arc::new(Ctx { client, api_group });
    Controller::new(api, Config::default())
        .run(reconcile, on_error, ctx)
        .for_each(|res| async move {
            if let Err(e) = res {
                warn!(error = %e, "MPC reconcile error");
            }
        })
        .await;
}
