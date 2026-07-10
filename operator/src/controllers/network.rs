//! Network reconciler — manages a blockchain network with validators,
//! chains, indexer, explorer, bridge, and bootnode.

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
use tracing::{error, info, warn};

use crate::apply;
use crate::core::{OperatorError, Result};
use crate::crd::{Network, NetworkSpec, ServicePort as CrServicePort};
use crate::manifests;

use super::owner_ref_for;

#[derive(Clone)]
pub struct Ctx {
    pub client: Client,
    pub api_group: String,
}

pub async fn reconcile(cr: Arc<Network>, ctx: Arc<Ctx>) -> Result<Action> {
    let name = cr.name_any();
    let namespace = cr
        .namespace()
        .ok_or_else(|| OperatorError::Config("Network has no namespace".into()))?;
    let api_version = format!("{}/v1", ctx.api_group);
    let owner = owner_ref_for(cr.as_ref(), &api_version, "Network");
    reconcile_inner(&ctx.client, &name, &namespace, &cr.spec, owner).await?;
    Ok(Action::requeue(Duration::from_secs(60)))
}

async fn reconcile_inner(
    client: &Client,
    name: &str,
    namespace: &str,
    spec: &NetworkSpec,
    owner: OwnerReference,
) -> Result<()> {
    // Networks declaring validators == 0 emit no validator workloads —
    // the chains listed in this CR are served by the existing validator
    // set on the network identified by spec.network_id.
    if !spec.has_own_validator_set() {
        info!(
            name,
            namespace,
            mode = ?spec.network_mode(),
            network_id = spec.network_id,
            "Network borrows existing validator set; no workloads emitted"
        );
        return Ok(());
    }
    // The per-validator pod-spec template is required when the network
    // runs its own consensus.
    let v = spec.validator_template.as_ref().ok_or_else(|| {
        OperatorError::Config(
            "Network with validators > 0 must declare spec.validatorTemplate".into(),
        )
    })?;
    let labels = manifests::standard_labels(name, "validator", "", &v.image.tag);
    let sel = manifests::selector_labels(name);

    let staking_port = if v.staking_port > 0 {
        v.staking_port
    } else {
        9631
    };
    let http_port = if v.http_port > 0 { v.http_port } else { 9630 };
    let ports = vec![
        CrServicePort {
            name: "staking".to_string(),
            container_port: staking_port,
            service_port: None,
            protocol: "TCP".to_string(),
        },
        CrServicePort {
            name: "http".to_string(),
            container_port: http_port,
            service_port: None,
            protocol: "TCP".to_string(),
        },
    ];

    let main = manifests::build_container(
        name,
        &manifests::image_ref(&v.image.repository, &v.image.tag),
        &v.image.pull_policy,
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        manifests::container_ports(&ports),
        v.resources.as_ref().map(manifests::to_k8s_resources),
        None,
        None,
    );

    let pvc_templates = if let Some(storage) = &v.storage {
        vec![manifests::build_pvc_template(
            "data",
            &storage.storage_class_name,
            storage.size.as_str(),
        )]
    } else {
        vec![]
    };

    // Replica count: explicit pod-template override wins; otherwise honour
    // the spec-declared validator count; default to 3 when neither is set.
    let replicas = v
        .replicas
        .or(if spec.validators > 0 {
            Some(spec.validators)
        } else {
            None
        })
        .or(Some(3));
    let mut sts = manifests::build_statefulset(
        name,
        namespace,
        labels.clone(),
        sel.clone(),
        replicas,
        vec![main],
        vec![],
        pvc_templates,
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

    info!(
        name,
        namespace,
        network_id = spec.network_id,
        mode = ?spec.network_mode(),
        "Network reconciled"
    );
    Ok(())
}

pub fn on_error(_obj: Arc<Network>, err: &OperatorError, _ctx: Arc<Ctx>) -> Action {
    error!(error = %err, "Network reconcile failed");
    Action::requeue(Duration::from_secs(30))
}

pub async fn run_network_controller(client: Client, namespace: String, api_group: String) {
    let api: Api<Network> = if namespace.is_empty() {
        Api::all(client.clone())
    } else {
        Api::namespaced(client.clone(), &namespace)
    };
    info!(group = %api_group, "Starting Network controller");
    let ctx = Arc::new(Ctx { client, api_group });
    Controller::new(api, Config::default())
        .run(reconcile, on_error, ctx)
        .for_each(|res| async move {
            if let Err(e) = res {
                warn!(error = %e, "Network reconcile error");
            }
        })
        .await;
}
