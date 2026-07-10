//! Gateway reconciler — KrakenD-based API gateway.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{ConfigMap, Service as CoreService};
use k8s_openapi::api::networking::v1::Ingress;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, OwnerReference};
use kube::api::Api;
use kube::runtime::controller::{Action, Controller};
use kube::runtime::watcher::Config;
use kube::{Client, ResourceExt};
use std::collections::BTreeMap;
use tracing::{debug, error, info, warn};

use crate::apply;
use crate::core::{OperatorError, Result};
use crate::crd::{Gateway as GatewayCR, GatewaySpec, ImageSpec, ServicePort as CrServicePort};
use crate::manifests;

use super::owner_ref_for;

#[derive(Clone)]
pub struct Ctx {
    pub client: Client,
    pub api_group: String,
}

const DEFAULT_GATEWAY_IMAGE: &str = "ghcr.io/hanzoai/gateway";
const DEFAULT_GATEWAY_TAG: &str = "v1.0.0";

pub async fn reconcile(cr: Arc<GatewayCR>, ctx: Arc<Ctx>) -> Result<Action> {
    let name = cr.name_any();
    let namespace = cr
        .namespace()
        .ok_or_else(|| OperatorError::Config("Gateway has no namespace".into()))?;
    let api_version = format!("{}/v1", ctx.api_group);
    let owner = owner_ref_for(cr.as_ref(), &api_version, "Gateway");
    reconcile_inner(&ctx.client, &name, &namespace, &cr.spec, owner).await?;
    Ok(Action::requeue(Duration::from_secs(60)))
}

async fn reconcile_inner(
    client: &Client,
    name: &str,
    namespace: &str,
    spec: &GatewaySpec,
    owner: OwnerReference,
) -> Result<()> {
    let image = spec.image.clone().unwrap_or(ImageSpec {
        repository: DEFAULT_GATEWAY_IMAGE.to_string(),
        tag: DEFAULT_GATEWAY_TAG.to_string(),
        pull_policy: "IfNotPresent".to_string(),
    });
    let labels = manifests::standard_labels(name, "gateway", "", &image.tag);
    let sel = manifests::selector_labels(name);

    // Render routes as a krakend.json ConfigMap.
    let config_json = serde_json::json!({
        "version": 3,
        "name": name,
        "port": 8080,
        "endpoints": spec.routes.iter().map(|r| {
            serde_json::json!({
                "endpoint": r.prefix,
                "method": if r.methods.is_empty() { vec!["GET".to_string()] } else { r.methods.clone() },
                "backend": [{ "url_pattern": "/", "host": [format!("http://{}", r.backend)] }],
            })
        }).collect::<Vec<_>>(),
    });
    let mut cm_data = BTreeMap::new();
    cm_data.insert("krakend.json".to_string(), config_json.to_string());
    let cm = ConfigMap {
        metadata: ObjectMeta {
            name: Some(format!("{}-config", name)),
            namespace: Some(namespace.to_string()),
            labels: Some(labels.clone()),
            owner_references: Some(vec![owner.clone()]),
            ..Default::default()
        },
        data: Some(cm_data),
        ..Default::default()
    };
    let cms: Api<ConfigMap> = Api::namespaced(client.clone(), namespace);
    apply::apply_configmap(&cms, &cm).await?;

    // Deployment.
    let ports = vec![CrServicePort {
        name: "http".to_string(),
        container_port: 8080,
        service_port: Some(80),
        protocol: "TCP".to_string(),
    }];
    let main = manifests::build_container(
        name,
        &manifests::image_ref(&image.repository, &image.tag),
        &image.pull_policy,
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
    let mut deploy = manifests::build_deployment(
        name,
        namespace,
        labels.clone(),
        sel.clone(),
        spec.replicas.or(Some(2)),
        vec![main],
        vec![],
        "RollingUpdate",
        vec![],
        "",
    );
    deploy.metadata.owner_references = Some(vec![owner.clone()]);
    let deps: Api<Deployment> = Api::namespaced(client.clone(), namespace);
    apply::apply(&deps, &deploy).await?;

    let svc_ports = manifests::service_ports(&ports);
    let mut svc = manifests::build_service(name, namespace, labels.clone(), svc_ports, sel.clone());
    svc.metadata.owner_references = Some(vec![owner.clone()]);
    let svcs: Api<CoreService> = Api::namespaced(client.clone(), namespace);
    apply::apply(&svcs, &svc).await?;

    if let Some(ing_spec) = &spec.ingress {
        if ing_spec.enabled {
            let mut ing = manifests::build_ingress(name, namespace, ing_spec, name, 80, labels);
            ing.metadata.owner_references = Some(vec![owner.clone()]);
            let ings: Api<Ingress> = Api::namespaced(client.clone(), namespace);
            apply::apply(&ings, &ing).await?;
        }
    }

    debug!(name, namespace, "Gateway reconciled");
    Ok(())
}

pub fn on_error(_obj: Arc<GatewayCR>, err: &OperatorError, _ctx: Arc<Ctx>) -> Action {
    error!(error = %err, "Gateway reconcile failed");
    Action::requeue(Duration::from_secs(30))
}

pub async fn run_gateway_controller(client: Client, namespace: String, api_group: String) {
    let api: Api<GatewayCR> = if namespace.is_empty() {
        Api::all(client.clone())
    } else {
        Api::namespaced(client.clone(), &namespace)
    };
    info!(group = %api_group, "Starting Gateway controller");
    let ctx = Arc::new(Ctx { client, api_group });
    Controller::new(api, Config::default())
        .run(reconcile, on_error, ctx)
        .for_each(|res| async move {
            if let Err(e) = res {
                warn!(error = %e, "Gateway reconcile error");
            }
        })
        .await;
}
