//! Base reconciler — hanzoai/base-ha cluster with deterministic writer
//! election (Quasar) and gateway integration.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use k8s_openapi::api::apps::v1::StatefulSet;
use k8s_openapi::api::core::v1::{EnvVar as K8sEnvVar, Service as CoreService, ServicePort};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::api::Api;
use kube::runtime::controller::{Action, Controller};
use kube::runtime::watcher::Config;
use kube::{Client, ResourceExt};
use tracing::{debug, error, info, warn};

use crate::apply;
use crate::core::{OperatorError, Result};
use crate::crd::{Base, BaseSpec, ServicePort as CrServicePort};
use crate::crd_types;
use crate::manifests;

use super::owner_ref_for;

#[derive(Clone)]
pub struct Ctx {
    pub client: Client,
    pub api_group: String,
}

pub async fn reconcile(cr: Arc<Base>, ctx: Arc<Ctx>) -> Result<Action> {
    let name = cr.name_any();
    let namespace = cr
        .namespace()
        .ok_or_else(|| OperatorError::Config("Base has no namespace".into()))?;
    let api_version = format!("{}/v1", ctx.api_group);
    let owner = owner_ref_for(cr.as_ref(), &api_version, "Base");
    reconcile_inner(&ctx.client, &name, &namespace, &cr.spec, owner).await?;
    Ok(Action::requeue(Duration::from_secs(60)))
}

async fn reconcile_inner(
    client: &Client,
    name: &str,
    namespace: &str,
    spec: &BaseSpec,
    owner: OwnerReference,
) -> Result<()> {
    let port = if spec.port > 0 { spec.port } else { 8090 };
    let replicas = spec.replicas.unwrap_or(3);
    let labels = manifests::standard_labels(name, "base-ha", &spec.part_of, &spec.image.tag);
    let sel = manifests::selector_labels(name);

    let ports = vec![CrServicePort {
        name: "http".to_string(),
        container_port: port,
        service_port: None,
        protocol: "TCP".to_string(),
    }];

    // BASE_* env wired from ordinals — base-ha reads its pod index for
    // deterministic writer election.
    let mut env: Vec<K8sEnvVar> = spec.env.iter().map(crd_types::EnvVar::to_k8s).collect();
    env.push(K8sEnvVar {
        name: "BASE_PORT".to_string(),
        value: Some(port.to_string()),
        ..Default::default()
    });
    env.push(K8sEnvVar {
        name: "BASE_REPLICAS".to_string(),
        value: Some(replicas.to_string()),
        ..Default::default()
    });
    env.push(K8sEnvVar {
        name: "BASE_CONSENSUS".to_string(),
        value: Some(if spec.consensus.is_empty() {
            "quasar".to_string()
        } else {
            spec.consensus.clone()
        }),
        ..Default::default()
    });
    env.push(K8sEnvVar {
        name: "BASE_HEADLESS_SERVICE".to_string(),
        value: Some(format!("{}-hs.{}.svc.cluster.local", name, namespace)),
        ..Default::default()
    });
    if !spec.iam_app.is_empty() {
        env.push(K8sEnvVar {
            name: "IAM_APP".to_string(),
            value: Some(spec.iam_app.clone()),
            ..Default::default()
        });
    }
    let env_from_k8s: Vec<_> = spec
        .env_from
        .iter()
        .map(crd_types::EnvFromSource::to_k8s)
        .collect();

    let liveness = manifests::build_http_probe(&crate::crd::ProbeSpec {
        path: "/v1/health".to_string(),
        port,
        initial_delay_seconds: 5,
        period_seconds: 10,
        ..Default::default()
    });
    let readiness = manifests::build_http_probe(&crate::crd::ProbeSpec {
        path: "/v1/health".to_string(),
        port,
        initial_delay_seconds: 5,
        period_seconds: 5,
        ..Default::default()
    });

    let main = manifests::build_container(
        name,
        &manifests::image_ref(&spec.image.repository, &spec.image.tag),
        &spec.image.pull_policy,
        vec![],
        vec![],
        env,
        env_from_k8s,
        vec![],
        manifests::container_ports(&ports),
        spec.resources.as_ref().map(manifests::to_k8s_resources),
        Some(liveness),
        Some(readiness),
    );

    let pvc = manifests::build_pvc_template(
        "data",
        &spec.storage.storage_class_name,
        spec.storage.size.as_str(),
    );

    let ips_k8s: Vec<_> = spec
        .image_pull_secrets
        .iter()
        .map(crd_types::LocalObjectReference::to_k8s)
        .collect();
    let mut sts = manifests::build_statefulset(
        name,
        namespace,
        labels.clone(),
        sel.clone(),
        Some(replicas),
        vec![main],
        vec![],
        vec![pvc],
        ips_k8s,
        &format!("{}-hs", name),
    );
    sts.metadata.owner_references = Some(vec![owner.clone()]);
    let stss: Api<StatefulSet> = Api::namespaced(client.clone(), namespace);
    apply::apply(&stss, &sts).await?;

    // Headless Service for pod DNS.
    let svc_ports = vec![ServicePort {
        name: Some("http".to_string()),
        port,
        target_port: Some(IntOrString::Int(port)),
        ..Default::default()
    }];
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

    // ClusterIP Service for gateway round-robin reads.
    let mut clip = manifests::build_service(name, namespace, labels, svc_ports, sel);
    clip.metadata.owner_references = Some(vec![owner.clone()]);
    apply::apply(&svcs, &clip).await?;

    debug!(name, namespace, replicas, "Base reconciled");
    Ok(())
}

pub fn on_error(_obj: Arc<Base>, err: &OperatorError, _ctx: Arc<Ctx>) -> Action {
    error!(error = %err, "Base reconcile failed");
    Action::requeue(Duration::from_secs(30))
}

pub async fn run_base_controller(client: Client, namespace: String, api_group: String) {
    let api: Api<Base> = if namespace.is_empty() {
        Api::all(client.clone())
    } else {
        Api::namespaced(client.clone(), &namespace)
    };
    info!(group = %api_group, "Starting Base controller");
    let ctx = Arc::new(Ctx { client, api_group });
    Controller::new(api, Config::default())
        .run(reconcile, on_error, ctx)
        .for_each(|res| async move {
            if let Err(e) = res {
                warn!(error = %e, "Base reconcile error");
            }
        })
        .await;
}
