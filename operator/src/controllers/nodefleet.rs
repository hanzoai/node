//! NodeFleet reconciler — "1 archive serves N state-sync replicas" topology.
//!
//! Renders, via the shared `manifests` + `apply` helpers:
//!   1. Archive headless + ClusterIP Services (the replicas' state-sync target).
//!   2. Archive StatefulSet (1 replica, holds full history).
//!   3. State-sync headless Service.
//!   4. State-sync StatefulSet (N pruned replicas).
//!
//! Roles are distinguished by a name suffix (`-archive` / `-state-sync`) and a
//! `fleet-role` label so Services/NetworkPolicies can target either set.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use k8s_openapi::api::apps::v1::StatefulSet;
use k8s_openapi::api::core::v1::Service as CoreService;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use kube::api::{Api, Patch, PatchParams};
use kube::runtime::controller::{Action, Controller};
use kube::runtime::watcher::Config;
use kube::{Client, Resource, ResourceExt};
use tracing::{error, info, warn};

use crate::apply;
use crate::core::{OperatorError, Result};
use crate::crd::{NodeFleet, NodeFleetSpec, NodeFleetStatus, Phase, ServicePort as CrServicePort};
use crate::crd_types::{build_condition, LocalObjectReference};
use crate::manifests;

use super::owner_ref_for;

#[derive(Clone)]
pub struct Ctx {
    pub client: Client,
    pub api_group: String,
}

pub async fn reconcile(cr: Arc<NodeFleet>, ctx: Arc<Ctx>) -> Result<Action> {
    let name = cr.name_any();
    let namespace = cr
        .namespace()
        .ok_or_else(|| OperatorError::Config("NodeFleet has no namespace".into()))?;
    let api_version = format!("{}/v1", ctx.api_group);
    let owner = owner_ref_for(cr.as_ref(), &api_version, "NodeFleet");
    reconcile_inner(&ctx.client, &name, &namespace, &cr.spec, owner).await?;
    write_status(&ctx.client, &name, &namespace, &cr).await;
    Ok(Action::requeue(Duration::from_secs(60)))
}

async fn reconcile_inner(
    client: &Client,
    name: &str,
    namespace: &str,
    spec: &NodeFleetSpec,
    owner: OwnerReference,
) -> Result<()> {
    let staking_port = if spec.staking_port > 0 {
        spec.staking_port
    } else {
        9631
    };
    let http_port = if spec.http_port > 0 {
        spec.http_port
    } else {
        9630
    };
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
    let svc_ports = manifests::service_ports(&ports);
    let ips: Vec<_> = spec
        .image_pull_secrets
        .iter()
        .map(|n| LocalObjectReference { name: n.clone() }.to_k8s())
        .collect();
    let svcs: Api<CoreService> = Api::namespaced(client.clone(), namespace);
    let stss: Api<StatefulSet> = Api::namespaced(client.clone(), namespace);

    // ---- Archive role (singleton, full history) ----
    let archive_name = format!("{}-archive", name);
    let archive_labels =
        manifests::standard_labels(&archive_name, "archive", name, &spec.image.tag);
    let archive_sel = manifests::selector_labels(&archive_name);

    let archive_main = manifests::build_container(
        &archive_name,
        &manifests::image_ref(&spec.image.repository, &spec.image.tag),
        &spec.image.pull_policy,
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        manifests::container_ports(&ports),
        spec.archive
            .resources
            .as_ref()
            .map(manifests::to_k8s_resources),
        None,
        None,
    );
    let archive_pvcs = if let Some(storage) = &spec.archive.storage {
        vec![manifests::build_pvc_template(
            "data",
            &storage.storage_class_name,
            storage.size.as_str(),
        )]
    } else {
        vec![]
    };
    let mut archive_sts = manifests::build_statefulset(
        &archive_name,
        namespace,
        archive_labels.clone(),
        archive_sel.clone(),
        Some(1),
        vec![archive_main],
        vec![],
        archive_pvcs,
        ips.clone(),
        &format!("{}-headless", archive_name),
    );
    archive_sts.metadata.owner_references = Some(vec![owner.clone()]);
    apply::apply(&stss, &archive_sts).await?;

    let mut archive_hs = manifests::build_headless_service(
        &format!("{}-headless", archive_name),
        namespace,
        archive_labels.clone(),
        svc_ports.clone(),
        archive_sel.clone(),
    );
    archive_hs.metadata.owner_references = Some(vec![owner.clone()]);
    apply::apply(&svcs, &archive_hs).await?;

    let mut archive_clip = manifests::build_service(
        &archive_name,
        namespace,
        archive_labels,
        svc_ports.clone(),
        archive_sel,
    );
    archive_clip.metadata.owner_references = Some(vec![owner.clone()]);
    apply::apply(&svcs, &archive_clip).await?;

    // ---- State-sync role (N pruned replicas) ----
    let sync_name = format!("{}-state-sync", name);
    let sync_labels = manifests::standard_labels(&sync_name, "state-sync", name, &spec.image.tag);
    let sync_sel = manifests::selector_labels(&sync_name);

    let sync_main = manifests::build_container(
        &sync_name,
        &manifests::image_ref(&spec.image.repository, &spec.image.tag),
        &spec.image.pull_policy,
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        manifests::container_ports(&ports),
        spec.state_sync
            .resources
            .as_ref()
            .map(manifests::to_k8s_resources),
        None,
        None,
    );
    let sync_pvcs = if let Some(storage) = &spec.state_sync.storage {
        vec![manifests::build_pvc_template(
            "data",
            &storage.storage_class_name,
            storage.size.as_str(),
        )]
    } else {
        vec![]
    };
    let mut sync_sts = manifests::build_statefulset(
        &sync_name,
        namespace,
        sync_labels.clone(),
        sync_sel.clone(),
        Some(spec.state_sync.replicas),
        vec![sync_main],
        vec![],
        sync_pvcs,
        ips,
        &format!("{}-headless", sync_name),
    );
    sync_sts.metadata.owner_references = Some(vec![owner.clone()]);
    apply::apply(&stss, &sync_sts).await?;

    let mut sync_hs = manifests::build_headless_service(
        &format!("{}-headless", sync_name),
        namespace,
        sync_labels,
        svc_ports,
        sync_sel,
    );
    sync_hs.metadata.owner_references = Some(vec![owner.clone()]);
    apply::apply(&svcs, &sync_hs).await?;

    info!(
        name,
        namespace,
        network_id = spec.network_id,
        replicas = spec.state_sync.replicas,
        chains = spec.chains.len(),
        "NodeFleet reconciled"
    );
    Ok(())
}

async fn write_status(client: &Client, name: &str, namespace: &str, cr: &NodeFleet) {
    let stss: Api<StatefulSet> = Api::namespaced(client.clone(), namespace);
    let archive = stss
        .get_opt(&format!("{}-archive", name))
        .await
        .ok()
        .flatten();
    let sync = stss
        .get_opt(&format!("{}-state-sync", name))
        .await
        .ok()
        .flatten();

    let mut status = NodeFleetStatus {
        observed_generation: cr.meta().generation.unwrap_or(0),
        ..Default::default()
    };
    status.archive_ready = archive
        .and_then(|x| x.status)
        .map(|s| s.ready_replicas.unwrap_or(0) >= 1)
        .unwrap_or(false);
    if let Some(s) = sync.and_then(|x| x.status) {
        status.ready_replicas = s.ready_replicas.unwrap_or(0);
    }
    let desired = cr.spec.state_sync.replicas;
    status.phase = Some(
        if status.archive_ready && status.ready_replicas >= desired {
            Phase::Running
        } else if status.archive_ready || status.ready_replicas > 0 {
            Phase::Degraded
        } else {
            Phase::Creating
        },
    );
    let ready = matches!(status.phase, Some(Phase::Running));
    status.conditions.push(build_condition(
        "Ready",
        ready,
        if ready { "Available" } else { "NotReady" },
        &format!(
            "archive_ready={} replicas {}/{}",
            status.archive_ready, status.ready_replicas, desired
        ),
        status.observed_generation,
    ));
    let api: Api<NodeFleet> = Api::namespaced(client.clone(), namespace);
    let patch = serde_json::json!({ "status": status });
    let pp = PatchParams::apply(apply::FIELD_MANAGER);
    if let Err(e) = api.patch_status(name, &pp, &Patch::Merge(&patch)).await {
        warn!(error = %e, "failed to update NodeFleet status");
    }
}

pub fn on_error(_obj: Arc<NodeFleet>, err: &OperatorError, _ctx: Arc<Ctx>) -> Action {
    error!(error = %err, "NodeFleet reconcile failed");
    Action::requeue(Duration::from_secs(30))
}

pub async fn run_nodefleet_controller(client: Client, namespace: String, api_group: String) {
    let api: Api<NodeFleet> = if namespace.is_empty() {
        Api::all(client.clone())
    } else {
        Api::namespaced(client.clone(), &namespace)
    };
    info!(group = %api_group, "Starting NodeFleet controller");
    let ctx = Arc::new(Ctx { client, api_group });
    Controller::new(api, Config::default())
        .run(reconcile, on_error, ctx)
        .for_each(|res| async move {
            if let Err(e) = res {
                warn!(error = %e, "NodeFleet reconcile error");
            }
        })
        .await;
}

#[cfg(test)]
mod tests {
    use crate::crd::{
        ArchiveSpec, FleetChainSpec, ImageSpec, NodeFleetSpec, StateSyncSpec, StorageSpec,
    };

    fn spec() -> NodeFleetSpec {
        NodeFleetSpec {
            network_id: 1,
            image: ImageSpec {
                repository: "ghcr.io/luxfi/node".to_string(),
                tag: "v1.13.0".to_string(),
                pull_policy: "IfNotPresent".to_string(),
            },
            chains: vec![FleetChainSpec {
                alias: "C".to_string(),
                vm_id: "evm".to_string(),
            }],
            archive: ArchiveSpec {
                storage: Some(StorageSpec {
                    size: "2Ti".to_string(),
                    ..Default::default()
                }),
                snapshot_cache_mb: 512,
                ..Default::default()
            },
            state_sync: StateSyncSpec {
                replicas: 3,
                storage: Some(StorageSpec {
                    size: "100Gi".to_string(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn nodefleet_spec_round_trips_through_json() {
        let s = spec();
        let json = serde_json::to_value(&s).expect("serialize");
        assert_eq!(json["networkID"], 1);
        assert_eq!(json["stateSync"]["replicas"], 3);
        assert_eq!(json["archive"]["snapshotCacheMB"], 512);
        assert_eq!(json["chains"][0]["alias"], "C");
        let back: NodeFleetSpec = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back.network_id, 1);
        assert_eq!(back.state_sync.replicas, 3);
        assert_eq!(back.archive.snapshot_cache_mb, 512);
    }
}
