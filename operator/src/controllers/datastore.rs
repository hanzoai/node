//! Datastore reconciler — dispatches by `spec.type` to PostgreSQL, Valkey,
//! DocDB (FerretDB), MinIO, NATS, or generic datastore engines.
//!
//! Each type runs as a StatefulSet with a headless Service for pod DNS plus
//! a ClusterIP Service for client connections.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use k8s_openapi::api::apps::v1::StatefulSet;
use k8s_openapi::api::core::v1::Service as CoreService;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use kube::api::Api;
use kube::runtime::controller::{Action, Controller};
use kube::runtime::watcher::Config;
use kube::{Client, Resource, ResourceExt};
use tracing::{debug, error, info, warn};

use crate::apply;
use crate::core::{OperatorError, Result};
use crate::crd::{Datastore as DatastoreCR, DatastoreSpec, DatastoreStatus, ImageSpec, Phase};
use crate::crd_types;
use crate::manifests;

use super::owner_ref_for;
use crate::crd_types::{build_condition, Condition};

fn upsert_condition(conditions: &mut Vec<Condition>, new_cond: Condition) {
    if let Some(slot) = conditions.iter_mut().find(|c| c.type_ == new_cond.type_) {
        *slot = new_cond;
    } else {
        conditions.push(new_cond);
    }
}

#[derive(Clone)]
pub struct Ctx {
    pub client: Client,
    pub api_group: String,
}

/// Resolve the default image for a given datastore type.
fn default_image_for(type_: &str) -> ImageSpec {
    match type_ {
        "postgresql" => ImageSpec {
            repository: "ghcr.io/hanzoai/sql".to_string(),
            tag: "16".to_string(),
            pull_policy: "IfNotPresent".to_string(),
        },
        "valkey" => ImageSpec {
            repository: "ghcr.io/hanzoai/kv".to_string(),
            tag: "8".to_string(),
            pull_policy: "IfNotPresent".to_string(),
        },
        "docdb" => ImageSpec {
            repository: "ghcr.io/hanzoai/docdb".to_string(),
            tag: "latest".to_string(),
            pull_policy: "IfNotPresent".to_string(),
        },
        "minio" => ImageSpec {
            repository: "ghcr.io/hanzoai/s3".to_string(),
            tag: "latest".to_string(),
            pull_policy: "IfNotPresent".to_string(),
        },
        "nats" => ImageSpec {
            repository: "nats".to_string(),
            tag: "2.10".to_string(),
            pull_policy: "IfNotPresent".to_string(),
        },
        _ => ImageSpec {
            repository: "ghcr.io/hanzoai/datastore".to_string(),
            tag: "latest".to_string(),
            pull_policy: "IfNotPresent".to_string(),
        },
    }
}

/// Reconcile a canonical `Datastore` CR.
pub async fn reconcile_datastore(cr: Arc<DatastoreCR>, ctx: Arc<Ctx>) -> Result<Action> {
    let name = cr.name_any();
    let namespace = cr
        .namespace()
        .ok_or_else(|| OperatorError::Config("Datastore has no namespace".into()))?;
    let api_version = format!("{}/v1", ctx.api_group);
    let owner = owner_ref_for(cr.as_ref(), &api_version, "Datastore");
    // Stamp the ownership / cost-attribution labels the control plane put on the
    // CR onto the materialized workload + pods, so a per-org cost dashboard can
    // attribute a dedicated tenant instance's footprint by label (the namespace
    // is the isolation boundary; these are the attribution key). Never the
    // immutable selector.
    reconcile_datastore_inner(
        &ctx.client,
        &name,
        &namespace,
        &cr.spec,
        owner,
        &attribution_labels(&cr),
    )
    .await?;
    write_datastore_status(&ctx.client, &name, &namespace, &cr).await;
    Ok(Action::requeue(Duration::from_secs(60)))
}

/// Copy the well-known `hanzo.ai/*` ownership labels (org, resource id,
/// managed-by) from a `Datastore` CR onto its workload. Only these fixed keys
/// are propagated — arbitrary CR labels are not, and the selector is untouched.
fn attribution_labels(cr: &DatastoreCR) -> BTreeMap<String, String> {
    let src = cr.labels();
    let mut out = BTreeMap::new();
    for k in ["hanzo.ai/org", "hanzo.ai/resource", "hanzo.ai/managed-by"] {
        if let Some(v) = src.get(k) {
            out.insert(k.to_string(), v.clone());
        }
    }
    out
}

/// Public alias for use by compat facades.
pub async fn reconcile_datastore_inner_pub(
    client: &Client,
    name: &str,
    namespace: &str,
    spec: &DatastoreSpec,
    owner: OwnerReference,
) -> Result<()> {
    reconcile_datastore_inner(client, name, namespace, spec, owner, &BTreeMap::new()).await
}

/// Like [`reconcile_datastore_inner_pub`] but stamps `extra_labels` onto the
/// workload metadata + pod template (never the immutable selector). Used by
/// the `ManagedDatabase` facade to tag per-tenant workloads so the control
/// plane can scope discovery to one tenant.
pub async fn reconcile_datastore_inner_labeled(
    client: &Client,
    name: &str,
    namespace: &str,
    spec: &DatastoreSpec,
    owner: OwnerReference,
    extra_labels: &BTreeMap<String, String>,
) -> Result<()> {
    reconcile_datastore_inner(client, name, namespace, spec, owner, extra_labels).await
}

async fn reconcile_datastore_inner(
    client: &Client,
    name: &str,
    namespace: &str,
    spec: &DatastoreSpec,
    owner: OwnerReference,
    extra_labels: &BTreeMap<String, String>,
) -> Result<()> {
    let image = spec
        .image
        .clone()
        .unwrap_or_else(|| default_image_for(&spec.type_));

    let base_labels = manifests::standard_labels(name, &spec.type_, &spec.part_of, &image.tag);
    // Merge tenant/extra labels into workload + pod-template metadata only;
    // `selector_labels` stays minimal and immutable.
    let std_labels = if extra_labels.is_empty() {
        base_labels
    } else {
        manifests::merge_labels(&[&base_labels, extra_labels])
    };
    let sel_labels = manifests::selector_labels(name);

    let ports = if spec.ports.is_empty() {
        vec![crate::crd::ServicePort {
            name: spec.type_.clone(),
            container_port: default_port_for(&spec.type_),
            service_port: None,
            protocol: "TCP".to_string(),
        }]
    } else {
        spec.ports.clone()
    };

    let env_k8s: Vec<_> = spec.env.iter().map(crd_types::EnvVar::to_k8s).collect();
    let env_from_k8s: Vec<_> = spec
        .env_from
        .iter()
        .map(crd_types::EnvFromSource::to_k8s)
        .collect();
    // The `volumeClaimTemplate` name (immutable on the StatefulSet) + the
    // container's data mounts. See `vct_name` / `resolve_volume_mounts`.
    let vct = vct_name(spec);
    let mounts = resolve_volume_mounts(spec, &vct);
    let vm_k8s: Vec<_> = mounts.iter().map(crd_types::VolumeMount::to_k8s).collect();
    let main = manifests::build_container(
        name,
        &manifests::image_ref(&image.repository, &image.tag),
        &image.pull_policy,
        spec.command.clone(),
        spec.args.clone(),
        env_k8s,
        env_from_k8s,
        vm_k8s,
        manifests::container_ports(&ports),
        spec.resources.as_ref().map(manifests::to_k8s_resources),
        None,
        None,
    );
    let mut containers = vec![main];
    containers.extend(spec.sidecars.iter().map(crd_types::Container::to_k8s));

    let pvc_template = manifests::build_pvc_template(
        &vct,
        &spec.storage.storage_class_name,
        spec.storage.size.as_str(),
    );

    let volumes_k8s: Vec<_> = spec.volumes.iter().map(crd_types::Volume::to_k8s).collect();
    let ips_k8s: Vec<_> = spec
        .image_pull_secrets
        .iter()
        .map(crd_types::LocalObjectReference::to_k8s)
        .collect();

    let mut sts = manifests::build_statefulset(
        name,
        namespace,
        std_labels.clone(),
        sel_labels.clone(),
        Some(spec.replicas.unwrap_or(1)),
        containers,
        volumes_k8s,
        vec![pvc_template],
        ips_k8s,
        &format!("{}-hs", name),
    );
    apply_fs_group(&mut sts, spec.fs_group);
    set_owner(&mut sts.metadata.owner_references, &owner);
    let stss: Api<StatefulSet> = Api::namespaced(client.clone(), namespace);
    apply::apply(&stss, &sts).await?;

    // ClusterIP Service for clients.
    let svc_ports = manifests::service_ports(&ports);
    let mut svc = manifests::build_service(
        name,
        namespace,
        std_labels.clone(),
        svc_ports.clone(),
        sel_labels.clone(),
    );
    set_owner(&mut svc.metadata.owner_references, &owner);
    let svcs: Api<CoreService> = Api::namespaced(client.clone(), namespace);
    apply::apply(&svcs, &svc).await?;

    // Headless Service for pod DNS.
    let mut hs = manifests::build_headless_service(
        &format!("{}-hs", name),
        namespace,
        std_labels.clone(),
        svc_ports.clone(),
        sel_labels.clone(),
    );
    set_owner(&mut hs.metadata.owner_references, &owner);
    apply::apply(&svcs, &hs).await?;

    // Service aliases (backward-compatible DNS names).
    for alias in &spec.service_aliases {
        let mut a = manifests::build_service(
            alias,
            namespace,
            std_labels.clone(),
            svc_ports.clone(),
            sel_labels.clone(),
        );
        set_owner(&mut a.metadata.owner_references, &owner);
        apply::apply(&svcs, &a).await?;
    }

    debug!(name, namespace, type_ = %spec.type_, "Datastore reconciled");
    Ok(())
}

/// Pod securityContext.fsGroup — opt-in (spec.fsGroup). Lets a non-root engine
/// image (FerretDB docdb runs as uid:gid 1000, distroless — no entrypoint can
/// chown) write its data PVC: the kubelet chowns the mounted volume to this GID
/// + adds it to every container's supplementary groups. `None` → the pod is
/// left untouched → byte-identical StatefulSet for root/self-chowning engines
/// (ClickHouse datastore).
fn apply_fs_group(sts: &mut StatefulSet, fs_group: Option<i64>) {
    let Some(fsg) = fs_group else { return };
    if let Some(pod) = sts.spec.as_mut().and_then(|s| s.template.spec.as_mut()) {
        pod.security_context = Some(k8s_openapi::api::core::v1::PodSecurityContext {
            fs_group: Some(fsg),
            ..Default::default()
        });
    }
}

fn default_port_for(type_: &str) -> i32 {
    match type_ {
        "postgresql" => 5432,
        "valkey" => 6379,
        "docdb" => 27017,
        "minio" => 9000,
        "nats" => 4222,
        _ => 8080,
    }
}

/// Default container mount path for a datastore engine's data volume. Matches
/// the paths the live fleet already uses so an adopted StatefulSet's pod spec
/// is byte-identical (no needless rollout): Postgres writes under
/// `/var/lib/postgresql/data`, Valkey/MinIO/NATS under `/data`, FerretDB
/// (docdb) under `/state`.
fn default_data_path_for(type_: &str) -> String {
    match type_ {
        "postgresql" => "/var/lib/postgresql/data",
        "docdb" => "/state",
        _ => "/data",
    }
    .to_string()
}

/// Name of the `volumeClaimTemplate` and the auto-injected data mount. Defaults
/// to `data`. IMMUTABLE on the StatefulSet — a datastore adopting a
/// pre-existing StatefulSet MUST set `storage.volumeName` to the live
/// template's name (e.g. `sql-data`), else the apply is rejected as an
/// immutable-field update and the workload stops reconciling.
fn vct_name(spec: &DatastoreSpec) -> String {
    spec.storage
        .volume_name
        .clone()
        .unwrap_or_else(|| "data".to_string())
}

/// The container's data mounts: the CR's explicit `volumeMounts` when set,
/// otherwise a single auto-injected mount of the VCT at the engine's default
/// data path. Auto-injection fixes the prior bug where the template was created
/// but never mounted (a fresh datastore wrote to the container's ephemeral fs
/// and lost its data on restart). CRs that mount the volume themselves
/// (docdb tenants → `/state`) are left exactly as declared.
fn resolve_volume_mounts(spec: &DatastoreSpec, vct: &str) -> Vec<crd_types::VolumeMount> {
    if !spec.volume_mounts.is_empty() {
        return spec.volume_mounts.clone();
    }
    vec![crd_types::VolumeMount {
        name: vct.to_string(),
        mount_path: default_data_path_for(&spec.type_),
        sub_path: String::new(),
        read_only: None,
    }]
}

/// Best-effort, password-free in-cluster DSN for a datastore workload. Carries
/// the scheme, ClusterIP service DNS, and client port only. The control plane
/// reads this off `DatastoreStatus.connection_string` to discover tenant
/// databases.
pub fn connection_string_for(spec: &DatastoreSpec, name: &str, namespace: &str) -> String {
    let port = spec
        .ports
        .first()
        .map(|p| p.service_port.unwrap_or(p.container_port))
        .unwrap_or_else(|| default_port_for(&spec.type_));
    let host = format!("{name}.{namespace}.svc");
    match spec.type_.as_str() {
        "postgresql" => format!("postgresql://{host}:{port}"),
        "valkey" => format!("redis://{host}:{port}"),
        "docdb" => format!("mongodb://{host}:{port}"),
        "minio" => format!("http://{host}:{port}"),
        "nats" => format!("nats://{host}:{port}"),
        _ => format!("tcp://{host}:{port}"),
    }
}

/// Compute + patch `DatastoreStatus` for any Datastore-family CR `K`. Reads the
/// backing StatefulSet `name` for readiness and derives the DSN from `spec`.
/// Best-effort: logs and returns on any API error.
pub async fn write_status<K>(
    client: &Client,
    name: &str,
    namespace: &str,
    spec: &DatastoreSpec,
    generation: i64,
) where
    K: Resource<DynamicType = (), Scope = k8s_openapi::NamespaceResourceScope>
        + Clone
        + serde::de::DeserializeOwned
        + std::fmt::Debug,
{
    use kube::api::{Patch, PatchParams};
    let stss: Api<StatefulSet> = Api::namespaced(client.clone(), namespace);
    let sts = match stss.get_opt(name).await {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "failed to fetch StatefulSet for status");
            return;
        }
    };
    let mut status = DatastoreStatus {
        observed_generation: generation,
        connection_string: connection_string_for(spec, name, namespace),
        ..Default::default()
    };
    if let Some(s) = sts.and_then(|x| x.status) {
        status.ready_replicas = s.ready_replicas.unwrap_or(0);
    }
    let desired = spec.replicas.unwrap_or(1);
    status.phase = Some(if status.ready_replicas >= desired && desired > 0 {
        Phase::Running
    } else if status.ready_replicas > 0 {
        Phase::Degraded
    } else {
        Phase::Creating
    });
    let ready = matches!(status.phase, Some(Phase::Running));
    let cond = build_condition(
        "Ready",
        ready,
        if ready { "Available" } else { "NotReady" },
        &format!("{}/{} replicas ready", status.ready_replicas, desired),
        status.observed_generation,
    );
    upsert_condition(&mut status.conditions, cond);
    let api: Api<K> = Api::namespaced(client.clone(), namespace);
    let patch = serde_json::json!({"status": status});
    let pp = PatchParams::apply(apply::FIELD_MANAGER);
    if let Err(e) = api.patch_status(name, &pp, &Patch::Merge(&patch)).await {
        warn!(error = %e, kind = %K::kind(&()), "failed to update status");
    }
}

/// Write status for the canonical `Datastore` CR.
async fn write_datastore_status(client: &Client, name: &str, namespace: &str, cr: &DatastoreCR) {
    write_status::<DatastoreCR>(
        client,
        name,
        namespace,
        &cr.spec,
        cr.meta().generation.unwrap_or(0),
    )
    .await;
}

fn set_owner(refs: &mut Option<Vec<OwnerReference>>, owner: &OwnerReference) {
    let v = refs.get_or_insert_with(Vec::new);
    v.retain(|r| r.uid != owner.uid);
    v.push(owner.clone());
}

pub fn on_error(_obj: Arc<DatastoreCR>, err: &OperatorError, _ctx: Arc<Ctx>) -> Action {
    error!(error = %err, "Datastore reconcile failed");
    Action::requeue(Duration::from_secs(30))
}

/// Run the canonical Datastore controller.
pub async fn run_datastore_controller(client: Client, namespace: String, api_group: String) {
    let api: Api<DatastoreCR> = if namespace.is_empty() {
        Api::all(client.clone())
    } else {
        Api::namespaced(client.clone(), &namespace)
    };
    info!(group = %api_group, "Starting Datastore controller");
    let ctx = Arc::new(Ctx { client, api_group });
    Controller::new(api, Config::default())
        .run(reconcile_datastore, on_error, ctx)
        .for_each(|res| async move {
            if let Err(e) = res {
                warn!(error = %e, "Datastore reconcile error");
            }
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crd::StorageSpec;

    fn ds(labels: &[(&str, &str)]) -> DatastoreCR {
        let mut cr = DatastoreCR::new(
            "ds-x",
            DatastoreSpec {
                type_: "datastore".to_string(),
                storage: StorageSpec {
                    size: "10Gi".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        cr.metadata.labels = Some(
            labels
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        );
        cr
    }

    // The org/resource ownership labels the control plane stamps on a dedicated
    // Datastore CR propagate to the workload for per-org cost attribution; an
    // unrelated label does not.
    #[test]
    fn attribution_labels_propagates_only_well_known_keys() {
        let cr = ds(&[
            ("hanzo.ai/org", "acme"),
            ("hanzo.ai/resource", "rs_123"),
            ("hanzo.ai/managed-by", "provisioning"),
            ("unrelated", "x"),
        ]);
        let got = attribution_labels(&cr);
        assert_eq!(got.get("hanzo.ai/org"), Some(&"acme".to_string()));
        assert_eq!(got.get("hanzo.ai/resource"), Some(&"rs_123".to_string()));
        assert_eq!(
            got.get("hanzo.ai/managed-by"),
            Some(&"provisioning".to_string())
        );
        assert!(!got.contains_key("unrelated"));
    }

    #[test]
    fn attribution_labels_empty_when_unlabeled() {
        assert!(attribution_labels(&ds(&[])).is_empty());
    }

    fn empty_sts() -> StatefulSet {
        use k8s_openapi::api::apps::v1::StatefulSetSpec;
        use k8s_openapi::api::core::v1::{PodSpec, PodTemplateSpec};
        StatefulSet {
            spec: Some(StatefulSetSpec {
                template: PodTemplateSpec {
                    spec: Some(PodSpec::default()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    // A non-root engine (docdb=FerretDB, uid 1000) sets spec.fsGroup so the
    // kubelet group-owns its data PVC — else it CrashLoops writing /state.
    #[test]
    fn fs_group_set_stamps_pod_security_context() {
        let mut sts = empty_sts();
        apply_fs_group(&mut sts, Some(1000));
        let sc = sts
            .spec
            .unwrap()
            .template
            .spec
            .unwrap()
            .security_context
            .expect("fsGroup engine must stamp a pod securityContext");
        assert_eq!(sc.fs_group, Some(1000));
    }

    // A root/self-chowning engine (ClickHouse datastore) leaves fsGroup None →
    // no securityContext → byte-identical StatefulSet (no needless restart).
    #[test]
    fn fs_group_none_leaves_pod_untouched() {
        let mut sts = empty_sts();
        apply_fs_group(&mut sts, None);
        assert!(sts
            .spec
            .unwrap()
            .template
            .spec
            .unwrap()
            .security_context
            .is_none());
    }

    fn spec_with_storage(type_: &str, volume_name: Option<&str>) -> DatastoreSpec {
        DatastoreSpec {
            type_: type_.to_string(),
            storage: StorageSpec {
                size: "20Gi".to_string(),
                volume_name: volume_name.map(|s| s.to_string()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    // A fresh datastore's volumeClaimTemplate defaults to `data`.
    #[test]
    fn vct_name_defaults_to_data() {
        assert_eq!(vct_name(&spec_with_storage("postgresql", None)), "data");
    }

    // Adopting a pre-existing StatefulSet: `storage.volumeName` names the VCT so
    // the operator's template matches the live (immutable) `sql-data` template
    // instead of forcing a forbidden rename. This is the sql/kv adoption fix.
    #[test]
    fn vct_name_honors_volume_name_for_adoption() {
        assert_eq!(
            vct_name(&spec_with_storage("postgresql", Some("sql-data"))),
            "sql-data"
        );
    }

    // With no explicit mounts, the datastore auto-mounts its VCT at the engine's
    // real data path — so sql's rendered pod matches the live pod
    // (`sql-data` → `/var/lib/postgresql/data`) and adoption is a clean no-op.
    #[test]
    fn resolve_mounts_auto_injects_vct_at_engine_path() {
        let spec = spec_with_storage("postgresql", Some("sql-data"));
        let vct = vct_name(&spec);
        let mounts = resolve_volume_mounts(&spec, &vct);
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].name, "sql-data");
        assert_eq!(mounts[0].mount_path, "/var/lib/postgresql/data");

        // Valkey adopts `kv-data` → `/data`.
        let kv = spec_with_storage("valkey", Some("kv-data"));
        let kvm = resolve_volume_mounts(&kv, &vct_name(&kv));
        assert_eq!(kvm[0].mount_path, "/data");
    }

    // A CR that declares its own mounts (docdb tenants → `/state`, volume
    // `data`) is left EXACTLY as-is — the operator must not double-mount or
    // move a working tenant datastore.
    #[test]
    fn resolve_mounts_honors_explicit_cr_mounts() {
        let mut spec = spec_with_storage("docdb", None);
        spec.volume_mounts = vec![crd_types::VolumeMount {
            name: "data".to_string(),
            mount_path: "/state".to_string(),
            sub_path: String::new(),
            read_only: None,
        }];
        let mounts = resolve_volume_mounts(&spec, &vct_name(&spec));
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].name, "data");
        assert_eq!(mounts[0].mount_path, "/state");
    }
}
