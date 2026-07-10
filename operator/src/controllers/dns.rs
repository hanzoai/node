//! DNS reconciler — multi-tenant DNS zones with CoreDNS and optional
//! Cloudflare edge sync.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::Service as CoreService;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use kube::api::Api;
use kube::runtime::controller::{Action, Controller};
use kube::runtime::watcher::Config;
use kube::{Client, ResourceExt};
use tracing::{debug, error, info, warn};

use crate::apply;
use crate::core::{OperatorError, Result};
use crate::crd::{DNSSpec, ServicePort as CrServicePort, DNS as DNSCR};
use crate::manifests;

use super::owner_ref_for;

#[derive(Clone)]
pub struct Ctx {
    pub client: Client,
    pub api_group: String,
}

pub async fn reconcile(cr: Arc<DNSCR>, ctx: Arc<Ctx>) -> Result<Action> {
    let name = cr.name_any();
    let namespace = cr
        .namespace()
        .ok_or_else(|| OperatorError::Config("DNS has no namespace".into()))?;
    let api_version = format!("{}/v1", ctx.api_group);
    let owner = owner_ref_for(cr.as_ref(), &api_version, "DNS");
    reconcile_inner(&ctx.client, &name, &namespace, &cr.spec, owner).await?;
    Ok(Action::requeue(Duration::from_secs(60)))
}

/// Public inner handler — shared entrypoint for the DNS reconcile.
pub async fn reconcile_dns_inner_pub(
    client: &Client,
    name: &str,
    namespace: &str,
    spec: &DNSSpec,
    owner: OwnerReference,
) -> Result<()> {
    reconcile_inner(client, name, namespace, spec, owner).await
}

async fn reconcile_inner(
    client: &Client,
    name: &str,
    namespace: &str,
    spec: &DNSSpec,
    owner: OwnerReference,
) -> Result<()> {
    let coredns = spec.coredns.clone().unwrap_or_default();
    let image = if coredns.image.is_empty() {
        "ghcr.io/hanzoai/dns:latest"
    } else {
        &coredns.image
    };
    let api_port = if coredns.api_port > 0 {
        coredns.api_port
    } else {
        8443
    };
    let dns_port = if coredns.dns_port > 0 {
        coredns.dns_port
    } else {
        53
    };
    let replicas = coredns.replicas.unwrap_or(2);

    let labels = manifests::standard_labels(name, "dns", "", "");
    let sel = manifests::selector_labels(name);

    let ports = vec![
        CrServicePort {
            name: "api".to_string(),
            container_port: api_port,
            service_port: None,
            protocol: "TCP".to_string(),
        },
        CrServicePort {
            name: "dns-tcp".to_string(),
            container_port: dns_port,
            service_port: None,
            protocol: "TCP".to_string(),
        },
        CrServicePort {
            name: "dns-udp".to_string(),
            container_port: dns_port,
            service_port: None,
            protocol: "UDP".to_string(),
        },
    ];

    let main = manifests::build_container(
        name,
        image,
        "IfNotPresent",
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        manifests::container_ports(&ports),
        coredns.resources.as_ref().map(manifests::to_k8s_resources),
        None,
        None,
    );

    let mut dep = manifests::build_deployment(
        name,
        namespace,
        labels.clone(),
        sel.clone(),
        Some(replicas),
        vec![main],
        vec![],
        "RollingUpdate",
        vec![],
        "",
    );
    dep.metadata.owner_references = Some(vec![owner.clone()]);
    let deps: Api<Deployment> = Api::namespaced(client.clone(), namespace);
    apply::apply(&deps, &dep).await?;

    let svc_ports = manifests::service_ports(&ports);
    let mut svc = manifests::build_service(name, namespace, labels, svc_ports, sel);
    svc.metadata.owner_references = Some(vec![owner.clone()]);
    let svcs: Api<CoreService> = Api::namespaced(client.clone(), namespace);
    apply::apply(&svcs, &svc).await?;

    debug!(name, namespace, zones = spec.zones.len(), "DNS reconciled");
    Ok(())
}

pub fn on_error(_obj: Arc<DNSCR>, err: &OperatorError, _ctx: Arc<Ctx>) -> Action {
    error!(error = %err, "DNS reconcile failed");
    Action::requeue(Duration::from_secs(30))
}

pub async fn run_dns_controller(client: Client, namespace: String, api_group: String) {
    let api: Api<DNSCR> = if namespace.is_empty() {
        Api::all(client.clone())
    } else {
        Api::namespaced(client.clone(), &namespace)
    };
    info!(group = %api_group, "Starting DNS controller");
    let ctx = Arc::new(Ctx { client, api_group });
    Controller::new(api, Config::default())
        .run(reconcile, on_error, ctx)
        .for_each(|res| async move {
            if let Err(e) = res {
                warn!(error = %e, "DNS reconcile error");
            }
        })
        .await;
}
