//! LuxRuntime reconciler — luxd validator-set deployment. Renders a luxd
//! StatefulSet plus a headless Service (pod DNS) and a ClusterIP Service
//! (JSON-RPC / staking) via the shared `manifests` + `apply` helpers — the
//! same render path the canonical `Network` controller uses. The rich
//! seed-restore / plugin-fetch / export-CronJob machinery in the Go impl is
//! reconcile-internal; the canonical workload a LuxRuntime CR produces is the
//! validator StatefulSet + its Services.

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
use crate::crd::{
    LuxRuntime, LuxRuntimeSpec, LuxRuntimeStatus, Phase, ServicePort as CrServicePort,
};
use crate::crd_types::build_condition;
use crate::manifests;

use super::owner_ref_for;

#[derive(Clone)]
pub struct Ctx {
    pub client: Client,
    pub api_group: String,
}

pub async fn reconcile(cr: Arc<LuxRuntime>, ctx: Arc<Ctx>) -> Result<Action> {
    let name = cr.name_any();
    let namespace = cr
        .namespace()
        .ok_or_else(|| OperatorError::Config("LuxRuntime has no namespace".into()))?;
    let api_version = format!("{}/v1", ctx.api_group);
    let owner = owner_ref_for(cr.as_ref(), &api_version, "LuxRuntime");
    reconcile_inner(&ctx.client, &name, &namespace, &cr.spec, owner).await?;
    write_status(&ctx.client, &name, &namespace, &cr).await;
    Ok(Action::requeue(Duration::from_secs(60)))
}

async fn reconcile_inner(
    client: &Client,
    name: &str,
    namespace: &str,
    spec: &LuxRuntimeSpec,
    owner: OwnerReference,
) -> Result<()> {
    let labels = manifests::standard_labels(name, "validator", "", &spec.image.tag);
    let sel = manifests::selector_labels(name);

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

    let pvc_templates = if let Some(storage) = &spec.storage {
        vec![manifests::build_pvc_template(
            "data",
            &storage.storage_class_name,
            storage.size.as_str(),
        )]
    } else {
        vec![]
    };

    let ips: Vec<_> = spec
        .image_pull_secrets
        .iter()
        .map(|n| crate::crd_types::LocalObjectReference { name: n.clone() }.to_k8s())
        .collect();

    let replicas = spec.validators.or(Some(1));
    let mut sts = manifests::build_statefulset(
        name,
        namespace,
        labels.clone(),
        sel.clone(),
        replicas,
        vec![main],
        vec![],
        pvc_templates,
        ips,
        &format!("{}-headless", name),
    );
    sts.metadata.owner_references = Some(vec![owner.clone()]);
    let stss: Api<StatefulSet> = Api::namespaced(client.clone(), namespace);
    apply::apply(&stss, &sts).await?;

    let svc_ports = manifests::service_ports(&ports);
    let mut hs = manifests::build_headless_service(
        &format!("{}-headless", name),
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
        chains = spec.chains.len(),
        "LuxRuntime reconciled"
    );
    Ok(())
}

async fn write_status(client: &Client, name: &str, namespace: &str, cr: &LuxRuntime) {
    let stss: Api<StatefulSet> = Api::namespaced(client.clone(), namespace);
    let sts = match stss.get_opt(name).await {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "failed to fetch StatefulSet for LuxRuntime status");
            return;
        }
    };
    let mut status = LuxRuntimeStatus {
        observed_generation: cr.meta().generation.unwrap_or(0),
        ..Default::default()
    };
    if let Some(s) = sts.and_then(|x| x.status) {
        status.active_validators = s.ready_replicas.unwrap_or(0);
    }
    let desired = cr.spec.validators.unwrap_or(1);
    status.phase = Some(if status.active_validators >= desired && desired > 0 {
        Phase::Running
    } else if status.active_validators > 0 {
        Phase::Degraded
    } else {
        Phase::Creating
    });
    let ready = matches!(status.phase, Some(Phase::Running));
    status.conditions.push(build_condition(
        "Ready",
        ready,
        if ready { "Available" } else { "NotReady" },
        &format!("{}/{} validators ready", status.active_validators, desired),
        status.observed_generation,
    ));
    let api: Api<LuxRuntime> = Api::namespaced(client.clone(), namespace);
    let patch = serde_json::json!({ "status": status });
    let pp = PatchParams::apply(apply::FIELD_MANAGER);
    if let Err(e) = api.patch_status(name, &pp, &Patch::Merge(&patch)).await {
        warn!(error = %e, "failed to update LuxRuntime status");
    }
}

pub fn on_error(_obj: Arc<LuxRuntime>, err: &OperatorError, _ctx: Arc<Ctx>) -> Action {
    error!(error = %err, "LuxRuntime reconcile failed");
    Action::requeue(Duration::from_secs(30))
}

pub async fn run_luxruntime_controller(client: Client, namespace: String, api_group: String) {
    let api: Api<LuxRuntime> = if namespace.is_empty() {
        Api::all(client.clone())
    } else {
        Api::namespaced(client.clone(), &namespace)
    };
    info!(group = %api_group, "Starting LuxRuntime controller");
    let ctx = Arc::new(Ctx { client, api_group });
    Controller::new(api, Config::default())
        .run(reconcile, on_error, ctx)
        .for_each(|res| async move {
            if let Err(e) = res {
                warn!(error = %e, "LuxRuntime reconcile error");
            }
        })
        .await;
}

#[cfg(test)]
mod tests {
    use crate::crd::{ImageSpec, LuxChainSpec, LuxRuntimeSpec, ReplicationSpec, StorageSpec};

    fn spec() -> LuxRuntimeSpec {
        LuxRuntimeSpec {
            network_id: 1,
            validators: Some(5),
            image: ImageSpec {
                repository: "ghcr.io/luxfi/node".to_string(),
                tag: "v1.13.0".to_string(),
                pull_policy: "IfNotPresent".to_string(),
            },
            storage: Some(StorageSpec {
                size: "200Gi".to_string(),
                ..Default::default()
            }),
            chains: vec![LuxChainSpec {
                chain_id: "C".to_string(),
                vm_id: "evm".to_string(),
                genesis_config_map: String::new(),
                bootstrap_blocking: Some(true),
                component: String::new(),
            }],
            replication: Some(ReplicationSpec {
                enabled: true,
                s3_endpoint: "https://s3.lux.network".to_string(),
                s3_bucket: "replicate".to_string(),
                s3_use_ssl: true,
                source_node_index: Some(0),
                snapshot_interval_seconds: 3600,
                incremental_interval_seconds: 5,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn luxruntime_spec_round_trips_through_json() {
        let s = spec();
        let json = serde_json::to_value(&s).expect("serialize");
        // camelCase + rename overrides land on the wire as the Go CRD expects.
        assert_eq!(json["networkID"], 1);
        assert_eq!(json["validators"], 5);
        assert_eq!(json["chains"][0]["chainID"], "C");
        assert_eq!(json["chains"][0]["bootstrapBlocking"], true);
        let back: LuxRuntimeSpec = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back.network_id, 1);
        assert_eq!(back.validators, Some(5));
        assert_eq!(back.chains.len(), 1);
        assert_eq!(back.chains[0].chain_id, "C");
    }

    #[test]
    fn replication_field_matches_go_wire_keys() {
        let s = spec();
        let json = serde_json::to_value(&s).expect("serialize");
        let rep = &json["replication"];
        // Field keys must be byte-identical to Go `ReplicationSpec` json tags.
        assert_eq!(rep["enabled"], true);
        assert_eq!(rep["s3Endpoint"], "https://s3.lux.network");
        assert_eq!(rep["s3Bucket"], "replicate");
        // Go tag is `s3UseSsl` (lowercase `ssl`), NOT the camelCase default
        // `s3UseSSL` — the explicit serde rename must preserve it.
        assert_eq!(rep["s3UseSsl"], true);
        assert_eq!(rep["sourceNodeIndex"], 0);
        assert_eq!(rep["snapshotIntervalSeconds"], 3600);
        assert_eq!(rep["incrementalIntervalSeconds"], 5);
        let back: LuxRuntimeSpec = serde_json::from_value(json).expect("deserialize");
        let br = back.replication.expect("replication present");
        assert!(br.enabled);
        assert!(br.s3_use_ssl);
        assert_eq!(br.source_node_index, Some(0));
        assert_eq!(br.snapshot_interval_seconds, 3600);
    }
}
