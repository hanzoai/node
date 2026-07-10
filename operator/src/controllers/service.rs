//! Service reconciler — the load-bearing controller.
//!
//! Watches `Service`. For
//! each CR, materializes: Deployment, Service, Ingress (when enabled), HPA,
//! PDB, NetworkPolicy, and KMSSecret children.
//!
//! ## Critical invariant
//!
//! `spec.env`, `spec.volumes`, and `spec.volumeMounts` MUST be honored
//! on the generated Deployment. The gateway 503 root cause (May 2026) was
//! the legacy Go operator silently dropping these. The Rust port carries
//! tests asserting the round-trip.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::autoscaling::v2::{CrossVersionObjectReference, HorizontalPodAutoscaler};
use k8s_openapi::api::core::v1::{ConfigMap, Service as CoreService};
use k8s_openapi::api::networking::v1::{Ingress, NetworkPolicy};
use k8s_openapi::api::policy::v1::PodDisruptionBudget;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use kube::api::{Api, Patch, PatchParams};
use kube::runtime::controller::{Action, Controller};
use kube::runtime::watcher::Config;
use kube::{Client, Resource, ResourceExt};
use tracing::{debug, error, info, warn};

use crate::apply;
use crate::core::{OperatorError, Result};
use crate::crd::{
    KMSSecretRef, PersistenceSpec, Phase, Service as ServiceCR, ServiceSpec, ServiceStatus,
};
use crate::crd_types;
use crate::manifests;

use super::owner_ref_for;
use crate::crd_types::{build_condition, carry_transition_time, status_changed, Condition};

/// Upsert a condition in-place by `type_`.
fn upsert_condition(conditions: &mut Vec<Condition>, new_cond: Condition) {
    if let Some(slot) = conditions.iter_mut().find(|c| c.type_ == new_cond.type_) {
        *slot = new_cond;
    } else {
        conditions.push(new_cond);
    }
}

// ============================================================================
// persistence — durable SeaweedFS-backed SQLite via hanzoai/replicate.
//
// Generalizes the proven console-sqlite wiring (restore initContainer +
// replicate sidecar + replicate-config ConfigMap + app-db PVC) into ONE
// `spec.persistence` field that any Service Kind can set. All builders here
// are pure functions of a resolved `PersistenceSpec` so they are unit-tested
// without a cluster.
// ============================================================================

/// Default `hanzoai/replicate` image. Pinned to the repo's `VERSION` (0.5.13;
/// the `v0.5.13` tag is published to GHCR). Semver-only per house rule —
/// never `:latest`.
const REPLICATE_IMAGE: &str = "ghcr.io/hanzoai/replicate:v0.5.13";
const REPLICATE_CMD: &str = "/usr/local/bin/replicate";
/// Shared volume name for the live DB file (mounted on main + init + sidecar).
const APP_DB_VOLUME: &str = "app-db";
/// Volume name for the mounted `replicate.yml` ConfigMap.
const REPLICATE_CONFIG_VOLUME: &str = "replicate-config";
const REPLICATE_CONFIG_MOUNT: &str = "/etc/replicate";

/// Apply sane defaults to a user-supplied `PersistenceSpec`. The user only
/// has to set `enabled` + `data_dir` (+ `db_path` or `dir_mode`); everything
/// else (endpoint, region, secrets, image) defaults to the in-cluster
/// SeaweedFS convention. `<service-name>` substitutions are resolved here.
fn resolved_persistence(name: &str, p: &PersistenceSpec) -> PersistenceSpec {
    let mut r = p.clone();
    if r.pattern.is_empty() {
        r.pattern = "**/*.db".to_string();
    }
    if r.s3_endpoint.is_empty() {
        r.s3_endpoint = "http://s3.hanzo.svc:9000".to_string();
    }
    if r.s3_region.is_empty() {
        r.s3_region = "us-east-1".to_string();
    }
    if r.credentials_secret.is_empty() {
        r.credentials_secret = "s3-credentials".to_string();
    }
    if r.age_secret.is_empty() {
        r.age_secret = format!("{}-replicate-age", name);
    }
    if r.image.is_empty() {
        r.image = REPLICATE_IMAGE.to_string();
    }
    r
}

/// The ConfigMap name holding `replicate.yml` for this service.
fn replicate_config_name(name: &str) -> String {
    format!("{}-replicate-config", name)
}

/// The PVC name for the shared `app-db` working volume.
fn app_db_pvc_name(name: &str) -> String {
    format!("{}-app-db", name)
}

/// Render `replicate.yml`. Single-DB mode emits a `path:`; `dir_mode` emits
/// `dir:` + `pattern:` + `watch: true` (replicate appends each DB's relative
/// path to the S3 `path` prefix automatically). Creds + age material are
/// injected as `${...}` env so the ConfigMap stays secret-free.
fn render_replicate_yml(p: &PersistenceSpec) -> String {
    let target = if p.dir_mode {
        // The glob MUST be quoted — a bare YAML scalar starting with `*`
        // (e.g. `**/*.db`) is parsed as an alias reference and is invalid.
        format!(
            "    dir: {}\n    pattern: \"{}\"\n    watch: true\n",
            p.data_dir, p.pattern
        )
    } else {
        format!("    path: {}/{}\n", p.data_dir, p.db_path)
    };
    format!(
        "# hanzoai/replicate -- SQLite WAL -> S3 (SeaweedFS), age-encrypted.\n\
         dbs:\n\
         \x20 - \n\
{target}\
         \x20   replicas:\n\
         \x20     - type: s3\n\
         \x20       bucket: {bucket}\n\
         \x20       path: {s3_path}\n\
         \x20       endpoint: {endpoint}\n\
         \x20       region: {region}\n\
         \x20       force-path-style: {fps}\n\
         \x20       access-key-id: ${{S3_ACCESS_KEY_ID}}\n\
         \x20       secret-access-key: ${{S3_SECRET_ACCESS_KEY}}\n\
         \x20       age:\n\
         \x20         identities:\n\
         \x20           - ${{AGE_IDENTITY}}\n\
         \x20         recipients:\n\
         \x20           - ${{AGE_RECIPIENT}}\n",
        target = target,
        bucket = p.bucket,
        s3_path = p.s3_path,
        endpoint = p.s3_endpoint,
        region = p.s3_region,
        fps = p.force_path_style,
    )
}

/// The pod volume for the live DB file: PVC if `storage` is set, else
/// emptyDir. Shared by main + init + sidecar.
fn app_db_volume(name: &str, p: &PersistenceSpec) -> crd_types::Volume {
    let source = if p.storage.is_some() {
        serde_json::json!({
            "persistentVolumeClaim": { "claimName": app_db_pvc_name(name) }
        })
    } else {
        serde_json::json!({ "emptyDir": {} })
    };
    crd_types::Volume {
        name: APP_DB_VOLUME.to_string(),
        source,
    }
}

/// The pod volume that mounts the `replicate.yml` ConfigMap.
fn replicate_config_volume(name: &str) -> crd_types::Volume {
    crd_types::Volume {
        name: REPLICATE_CONFIG_VOLUME.to_string(),
        source: serde_json::json!({ "configMap": { "name": replicate_config_name(name) } }),
    }
}

/// The two volumeMounts every replicate container shares: the live DB dir and
/// the read-only config.
fn replicate_volume_mounts(p: &PersistenceSpec) -> Vec<crd_types::VolumeMount> {
    vec![
        crd_types::VolumeMount {
            name: APP_DB_VOLUME.to_string(),
            mount_path: p.data_dir.clone(),
            sub_path: String::new(),
            read_only: None,
        },
        crd_types::VolumeMount {
            name: REPLICATE_CONFIG_VOLUME.to_string(),
            mount_path: REPLICATE_CONFIG_MOUNT.to_string(),
            sub_path: String::new(),
            read_only: Some(true),
        },
    ]
}

/// S3 creds + age keypair as container env, sourced from the configured
/// Secrets. Shared by the restore init and the replication sidecar.
fn replicate_env(p: &PersistenceSpec) -> Vec<crd_types::EnvVar> {
    let secret_ref = |secret: &str, key: &str| crd_types::EnvVarSource {
        secret_key_ref: Some(crd_types::SecretKeySelector {
            name: secret.to_string(),
            key: key.to_string(),
            optional: None,
        }),
        ..Default::default()
    };
    vec![
        crd_types::EnvVar {
            name: "S3_ACCESS_KEY_ID".to_string(),
            value: None,
            value_from: Some(secret_ref(&p.credentials_secret, "access-key")),
        },
        crd_types::EnvVar {
            name: "S3_SECRET_ACCESS_KEY".to_string(),
            value: None,
            value_from: Some(secret_ref(&p.credentials_secret, "secret-key")),
        },
        crd_types::EnvVar {
            name: "AGE_IDENTITY".to_string(),
            value: None,
            value_from: Some(secret_ref(&p.age_secret, "identity")),
        },
        crd_types::EnvVar {
            name: "AGE_RECIPIENT".to_string(),
            value: None,
            value_from: Some(secret_ref(&p.age_secret, "recipients")),
        },
    ]
}

/// The `replicate-restore` initContainer (single-DB mode only — see
/// `dir_mode` handling at the callsite). No-op (exit 0) when `app.db` already
/// exists on the volume or no snapshot exists yet in the bucket.
fn replicate_restore_init(p: &PersistenceSpec) -> crd_types::Container {
    crd_types::Container {
        name: "replicate-restore".to_string(),
        image: p.image.clone(),
        command: vec![REPLICATE_CMD.to_string()],
        args: vec![
            "restore".to_string(),
            "-config".to_string(),
            format!("{}/replicate.yml", REPLICATE_CONFIG_MOUNT),
            "-if-db-not-exists".to_string(),
            "-if-replica-exists".to_string(),
            format!("{}/{}", p.data_dir, p.db_path),
        ],
        env: replicate_env(p),
        env_from: vec![],
        volume_mounts: replicate_volume_mounts(p),
        image_pull_policy: "IfNotPresent".to_string(),
    }
}

/// The `replicate` sidecar: continuously stream the SQLite WAL to S3,
/// age-encrypting client-side. Shares `app-db` with the main container.
fn replicate_sidecar(p: &PersistenceSpec) -> crd_types::Container {
    crd_types::Container {
        name: "replicate".to_string(),
        image: p.image.clone(),
        command: vec![REPLICATE_CMD.to_string()],
        args: vec![
            "replicate".to_string(),
            "-config".to_string(),
            format!("{}/replicate.yml", REPLICATE_CONFIG_MOUNT),
        ],
        env: replicate_env(p),
        env_from: vec![],
        volume_mounts: replicate_volume_mounts(p),
        image_pull_policy: "IfNotPresent".to_string(),
    }
}

/// The main container's mount of the shared `app-db` volume, so the app
/// reads/writes the same DB file the sidecar streams.
fn main_app_db_mount(p: &PersistenceSpec) -> crd_types::VolumeMount {
    crd_types::VolumeMount {
        name: APP_DB_VOLUME.to_string(),
        mount_path: p.data_dir.clone(),
        sub_path: String::new(),
        read_only: None,
    }
}

#[derive(Clone)]
pub struct Ctx {
    pub client: Client,
    pub api_group: String,
}

/// Reconcile a canonical `Service` CR.
pub async fn reconcile_service(cr: Arc<ServiceCR>, ctx: Arc<Ctx>) -> Result<Action> {
    let name = cr.name_any();
    let namespace = cr
        .namespace()
        .ok_or_else(|| OperatorError::Config("Service has no namespace".into()))?;
    let api_version = format!("{}/v1", ctx.api_group);
    let owner = owner_ref_for(cr.as_ref(), &api_version, "Service");

    reconcile_service_inner(&ctx.client, &name, &namespace, &cr.spec, owner).await?;

    // Prior status — used to keep condition timestamps stable and to skip
    // no-op status writes (see the patch guard below).
    let prior_status = cr.status.clone().unwrap_or_default();

    // Status writeback: poll the Deployment for ready replica count.
    let dep_api: Api<Deployment> = Api::namespaced(ctx.client.clone(), &namespace);
    let dep = dep_api.get_opt(&name).await?;
    let mut status = ServiceStatus {
        observed_generation: cr.meta().generation.unwrap_or(0),
        ..Default::default()
    };
    if let Some(d) = dep {
        if let Some(s) = d.status {
            status.ready_replicas = s.ready_replicas.unwrap_or(0);
            status.available_replicas = s.available_replicas.unwrap_or(0);
        }
    }
    let desired_replicas = cr.spec.replicas.unwrap_or(1);

    let phase = if status.ready_replicas >= desired_replicas && desired_replicas > 0 {
        Phase::Running
    } else if status.ready_replicas > 0 {
        Phase::Degraded
    } else {
        Phase::Creating
    };
    status.phase = Some(phase.clone());

    let ready = matches!(phase, Phase::Running);
    let mut cond = build_condition(
        "Ready",
        ready,
        if ready { "Available" } else { "NotReady" },
        &format!(
            "{}/{} replicas ready",
            status.ready_replicas, desired_replicas
        ),
        status.observed_generation,
    );
    // Only advance lastTransitionTime on a real Ready flip — otherwise the
    // fresh now() timestamp would make every reconcile mutate the CR.
    carry_transition_time(&prior_status.conditions, &mut cond);
    upsert_condition(&mut status.conditions, cond);

    // Compute endpoint URLs from ingress hosts.
    if let Some(ing) = &cr.spec.ingress {
        if ing.enabled {
            let scheme = if ing.tls { "https" } else { "http" };
            status.endpoints = ing
                .hosts
                .iter()
                .map(|h| format!("{}://{}", scheme, h))
                .collect();
        }
    }

    // Skip the write when nothing changed. An unconditional status merge bumps
    // resourceVersion on every reconcile, which the watch re-delivers as an
    // `object updated` event → a self-triggered reconcile storm (~2.75/s/CR
    // across the fleet). Writing only on real change breaks the loop.
    if status_changed(&status, &prior_status) {
        let api: Api<ServiceCR> = Api::namespaced(ctx.client.clone(), &namespace);
        let patch = serde_json::json!({"status": status});
        let pp = PatchParams::apply(apply::FIELD_MANAGER);
        if let Err(e) = api.patch_status(&name, &pp, &Patch::Merge(&patch)).await {
            warn!(error = %e, "failed to update Service status (CRD may not be installed)");
        } else {
            debug!(name, namespace, ?phase, "Service status updated");
        }
    }

    Ok(Action::requeue(Duration::from_secs(60)))
}

/// Inject surge co-location affinity iff: the CR opted in (`surgeColocation`),
/// the strategy is a RollingUpdate (anything but `Recreate`; empty defaults to
/// RollingUpdate), AND the pod mounts a real PVC. Pure so the fleet-safety gate
/// is unit-tested without a cluster.
fn should_colocate(surge_colocation: bool, strategy: &str, mounts_pvc: bool) -> bool {
    surge_colocation && strategy != "Recreate" && mounts_pvc
}

/// Public alias for use by compat facades.
pub async fn reconcile_service_inner_pub(
    client: &Client,
    name: &str,
    namespace: &str,
    spec: &ServiceSpec,
    owner: OwnerReference,
) -> Result<()> {
    reconcile_service_inner(client, name, namespace, spec, owner).await
}

/// Shared implementation. Materializes Deployment + Service + Ingress +
/// HPA + PDB + NetworkPolicy + KMSSecret children.
async fn reconcile_service_inner(
    client: &Client,
    name: &str,
    namespace: &str,
    spec: &ServiceSpec,
    owner: OwnerReference,
) -> Result<()> {
    let std_labels =
        manifests::standard_labels(name, &spec.component, &spec.part_of, &spec.image.tag);
    let sel_labels = manifests::selector_labels(name);
    let extra_labels = spec.labels.clone().unwrap_or_default();
    let all_labels = manifests::merge_labels(&[&std_labels, &extra_labels]);

    // Resolve persistence once (defaults filled in) when enabled. Drives the
    // auto-injected app-db mount, ConfigMap, restore init, and sidecar below.
    let persistence = spec
        .persistence
        .as_ref()
        .filter(|p| p.enabled)
        .map(|p| resolved_persistence(name, p));

    // 1. Build the main container honoring spec.env/volumes/volumeMounts.
    let env_k8s: Vec<_> = spec.env.iter().map(crd_types::EnvVar::to_k8s).collect();
    let env_from_k8s: Vec<_> = spec
        .env_from
        .iter()
        .map(crd_types::EnvFromSource::to_k8s)
        .collect();
    // Honor spec.volume_mounts, then auto-inject the shared app-db mount on
    // the MAIN container so the app reads/writes the DB the sidecar streams.
    let mut main_vms: Vec<crd_types::VolumeMount> = spec.volume_mounts.clone();
    if let Some(p) = &persistence {
        main_vms.push(main_app_db_mount(p));
    }
    let vm_k8s: Vec<_> = main_vms
        .iter()
        .map(crd_types::VolumeMount::to_k8s)
        .collect();
    let main = manifests::build_container(
        name,
        &manifests::image_ref(&spec.image.repository, &spec.image.tag),
        &spec.image.pull_policy,
        spec.command.clone(),
        spec.args.clone(),
        env_k8s,
        env_from_k8s,
        vm_k8s,
        manifests::container_ports(&spec.ports),
        spec.resources.as_ref().map(manifests::to_k8s_resources),
        spec.liveness_probe
            .as_ref()
            .and_then(manifests::build_probe),
        spec.readiness_probe
            .as_ref()
            .and_then(manifests::build_probe),
    );
    let mut containers = vec![main];
    containers.extend(spec.sidecars.iter().map(crd_types::Container::to_k8s));
    // Auto-inject the replicate sidecar (streams the WAL to SeaweedFS).
    if let Some(p) = &persistence {
        containers.push(replicate_sidecar(p).to_k8s());
    }

    // 2. Build and apply Deployment.
    //
    // When HPA is enabled, the operator MUST NOT own `spec.replicas` — server-
    // side apply would otherwise fight the HPA on every reconcile cycle.
    // Passing `None` here removes the field from the desired state, so the
    // HPA becomes the sole field manager for replicas. The initial scale is
    // then determined by `spec.autoscaling.minReplicas` (the HPA's floor).
    let mut all_volumes: Vec<crd_types::Volume> = spec.volumes.clone();
    if let Some(p) = &persistence {
        // Shared live-DB volume (PVC or emptyDir) + the replicate.yml mount.
        all_volumes.push(app_db_volume(name, p));
        all_volumes.push(replicate_config_volume(name));
    }
    let volumes_k8s: Vec<_> = all_volumes.iter().map(crd_types::Volume::to_k8s).collect();
    // Does the pod mount a real PVC? Computed before volumes_k8s is moved into
    // build_deployment — the precondition for surge co-location.
    let mounts_pvc = volumes_k8s
        .iter()
        .any(|v| v.persistent_volume_claim.is_some());
    let ips_k8s: Vec<_> = spec
        .image_pull_secrets
        .iter()
        .map(crd_types::LocalObjectReference::to_k8s)
        .collect();
    let replicas_for_deployment = if spec.autoscaling.as_ref().is_some_and(|a| a.enabled) {
        None
    } else {
        Some(spec.replicas.unwrap_or(1))
    };
    let mut deploy = manifests::build_deployment(
        name,
        namespace,
        all_labels.clone(),
        sel_labels.clone(),
        replicas_for_deployment,
        containers,
        volumes_k8s,
        &spec.strategy,
        ips_k8s,
        &spec.service_account_name,
    );
    if let Some(d_spec) = deploy.spec.as_mut() {
        if let Some(annotations) = &spec.annotations {
            if let Some(meta) = d_spec.template.metadata.as_mut() {
                meta.annotations = Some(annotations.clone());
            }
        }
        // Zero-downtime surge co-location — OPT-IN (spec.surgeColocation). Only a
        // RollingUpdate service whose data is a single RWO PVC needs it, and only
        // if the store is safe under a brief same-host two-pod overlap (SQLite
        // WAL + busy_timeout). Pin the surge pod to the volume's node so it
        // bind-mounts the already-attached volume instead of dead-locking on
        // Multi-Attach. Exclusive-lock engines opt OUT (they use strategy
        // Recreate), so the affinity is never injected implicitly.
        if should_colocate(spec.surge_colocation, &spec.strategy, mounts_pvc) {
            if let Some(pod) = d_spec.template.spec.as_mut() {
                pod.affinity = Some(manifests::colocation_affinity(&sel_labels));
            }
        }
        // Spec init containers, plus the auto-injected replicate-restore init.
        // dir_mode omits the restore init — directory restore is best-effort
        // via the sidecar's restore-on-boot (a single file path can't address
        // a fan-out of per-org/user DBs).
        let mut inits: Vec<_> = spec
            .init_containers
            .iter()
            .map(crd_types::Container::to_k8s)
            .collect();
        if let Some(p) = &persistence {
            if !p.dir_mode {
                inits.push(replicate_restore_init(p).to_k8s());
            }
        }
        if !inits.is_empty() {
            if let Some(pod) = d_spec.template.spec.as_mut() {
                pod.init_containers = Some(inits);
            }
        }
        // Pod securityContext.fsGroup — opt-in (spec.fsGroup). Lets a non-root
        // image write a persistence PVC (the kubelet chowns the volume to this
        // GID + adds it to every container's supplementary groups).
        if let Some(fsg) = spec.fs_group {
            if let Some(pod) = d_spec.template.spec.as_mut() {
                pod.security_context = Some(k8s_openapi::api::core::v1::PodSecurityContext {
                    fs_group: Some(fsg),
                    ..Default::default()
                });
            }
        }
    }
    set_owner(&mut deploy.metadata.owner_references, &owner);
    let deps: Api<Deployment> = Api::namespaced(client.clone(), namespace);
    // Deployments may carry a stale server-defaulted volume source (an
    // `emptyDir` left from a past source-less apply) or a duplicate probe
    // handler that SSA-merge cannot clear; `apply_or_recreate` deterministically
    // recreates from the desired (single-source) spec in that case. Standalone
    // PVCs re-attach; healthy Deployments apply cleanly and never recreate.
    apply::apply_or_recreate(&deps, &deploy).await?;

    // 2b. Persistence ConfigMap (`replicate.yml`). Owned by the Service so it
    // is GC'd with the CR.
    if let Some(p) = &persistence {
        let mut cm_data = std::collections::BTreeMap::new();
        cm_data.insert("replicate.yml".to_string(), render_replicate_yml(p));
        let mut cm = manifests::build_configmap(
            &replicate_config_name(name),
            namespace,
            all_labels.clone(),
            cm_data,
        );
        set_owner(&mut cm.metadata.owner_references, &owner);
        let cms: Api<ConfigMap> = Api::namespaced(client.clone(), namespace);
        apply::apply_configmap(&cms, &cm).await?;
    }

    // 3. Service (only if ports are defined).
    if !spec.ports.is_empty() {
        let svc_ports = manifests::service_ports(&spec.ports);
        let mut svc = manifests::build_service(
            name,
            namespace,
            all_labels.clone(),
            svc_ports,
            sel_labels.clone(),
        );
        set_owner(&mut svc.metadata.owner_references, &owner);
        let svcs: Api<CoreService> = Api::namespaced(client.clone(), namespace);
        apply::apply(&svcs, &svc).await?;
    }

    // 4. Ingress.
    if let Some(ing_spec) = &spec.ingress {
        if ing_spec.enabled && !spec.ports.is_empty() {
            let port = manifests::primary_port(&spec.ports);
            let mut ing =
                manifests::build_ingress(name, namespace, ing_spec, name, port, all_labels.clone());
            set_owner(&mut ing.metadata.owner_references, &owner);
            let ings: Api<Ingress> = Api::namespaced(client.clone(), namespace);
            apply::apply(&ings, &ing).await?;
        }
    }

    // 5. HPA.
    if let Some(as_spec) = &spec.autoscaling {
        if as_spec.enabled {
            let target = CrossVersionObjectReference {
                api_version: Some("apps/v1".to_string()),
                kind: "Deployment".to_string(),
                name: name.to_string(),
            };
            let mut hpa =
                manifests::build_hpa(name, namespace, target, as_spec, all_labels.clone());
            set_owner(&mut hpa.metadata.owner_references, &owner);
            let hpas: Api<HorizontalPodAutoscaler> = Api::namespaced(client.clone(), namespace);
            apply::apply(&hpas, &hpa).await?;
        }
    }

    // 6. PDB.
    if let Some(pdb_spec) = &spec.pdb {
        if pdb_spec.enabled {
            let mut pdb = manifests::build_pdb(
                name,
                namespace,
                pdb_spec,
                sel_labels.clone(),
                all_labels.clone(),
            );
            set_owner(&mut pdb.metadata.owner_references, &owner);
            let pdbs: Api<PodDisruptionBudget> = Api::namespaced(client.clone(), namespace);
            apply::apply(&pdbs, &pdb).await?;
        }
    }

    // 7. NetworkPolicy.
    if let Some(np_spec) = &spec.network_policy {
        if np_spec.enabled.unwrap_or(true) {
            let mut np = manifests::build_network_policy(
                name,
                namespace,
                np_spec,
                sel_labels.clone(),
                all_labels.clone(),
            );
            set_owner(&mut np.metadata.owner_references, &owner);
            let nps: Api<NetworkPolicy> = Api::namespaced(client.clone(), namespace);
            apply::apply(&nps, &np).await?;
        }
    }

    // 8. KMSSecret children (dynamic — written via DynamicObject so we
    // don't depend on the KMS CRD types being known to this binary).
    for ref_spec in &spec.kms_secrets {
        if let Err(e) = reconcile_kms_secret(client, namespace, ref_spec, &owner, &all_labels).await
        {
            warn!(name = %ref_spec.managed_secret_name, error = %e, "KMSSecret reconcile failed (CRD may not be installed)");
        }
    }

    debug!(name, namespace, "Service reconciled");
    Ok(())
}

/// Write a KMSSecret CR as a DynamicObject. The KMS CRD lives in
/// `kms.hanzo.ai` and is reconciled by the KMS operator — this controller
/// only declares the desired state.
async fn reconcile_kms_secret(
    client: &Client,
    namespace: &str,
    ref_spec: &KMSSecretRef,
    owner: &OwnerReference,
    labels: &std::collections::BTreeMap<String, String>,
) -> Result<()> {
    use kube::core::{ApiResource, DynamicObject, GroupVersionKind};

    let gvk = GroupVersionKind::gvk("kms.hanzo.ai", "v1alpha1", "KMSSecret");
    let ar = ApiResource::from_gvk(&gvk);
    let kms_api: Api<DynamicObject> = Api::namespaced_with(client.clone(), namespace, &ar);

    let mut obj = DynamicObject::new(&ref_spec.managed_secret_name, &ar);
    obj.metadata.namespace = Some(namespace.to_string());
    obj.metadata.labels = Some(labels.clone());
    obj.metadata.owner_references = Some(vec![owner.clone()]);

    let spec_json = serde_json::json!({
        "hostAPI": ref_spec.host_api,
        "projectSlug": ref_spec.project_slug,
        "envSlug": ref_spec.env_slug,
        "secretsPath": ref_spec.secrets_path,
        "credentialsRef": {
            "name": ref_spec.credentials_ref.name,
            "namespace": ref_spec.credentials_ref.namespace,
        },
        "resyncInterval": ref_spec.resync_interval,
        "managedSecretName": ref_spec.managed_secret_name,
    });
    obj.data = serde_json::json!({ "spec": spec_json });
    apply::apply_dynamic(&kms_api, &obj).await?;
    Ok(())
}

fn set_owner(refs: &mut Option<Vec<OwnerReference>>, owner: &OwnerReference) {
    let v = refs.get_or_insert_with(Vec::new);
    v.retain(|r| r.uid != owner.uid);
    v.push(owner.clone());
}

pub fn on_error_service(_obj: Arc<ServiceCR>, err: &OperatorError, _ctx: Arc<Ctx>) -> Action {
    error!(error = %err, "Service reconcile failed");
    Action::requeue(Duration::from_secs(30))
}

/// Run the canonical Service controller.
pub async fn run_service_controller(client: Client, namespace: String, api_group: String) {
    let api: Api<ServiceCR> = if namespace.is_empty() {
        Api::all(client.clone())
    } else {
        Api::namespaced(client.clone(), &namespace)
    };
    info!(group = %api_group, "Starting Service controller");
    let ctx = Arc::new(Ctx { client, api_group });
    Controller::new(api, Config::default())
        .run(reconcile_service, on_error_service, ctx)
        .for_each(|res| async move {
            if let Err(e) = res {
                warn!(error = %e, "Service reconcile error");
            }
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crd::{AutoscalingSpec, ImageSpec, ServicePort as CrServicePort};

    fn base_spec() -> ServiceSpec {
        ServiceSpec {
            image: ImageSpec {
                repository: "ghcr.io/hanzoai/test".to_string(),
                tag: "v1.0.0".to_string(),
                pull_policy: "IfNotPresent".to_string(),
            },
            replicas: Some(2),
            ports: vec![CrServicePort {
                name: "http".to_string(),
                container_port: 8080,
                service_port: None,
                protocol: "TCP".to_string(),
            }],
            env: vec![crd_types::EnvVar {
                name: "FOO".to_string(),
                value: Some("bar".to_string()),
                value_from: None,
            }],
            volumes: vec![crd_types::Volume {
                name: "data".to_string(),
                source: serde_json::json!({}),
            }],
            volume_mounts: vec![crd_types::VolumeMount {
                name: "data".to_string(),
                mount_path: "/data".to_string(),
                sub_path: String::new(),
                read_only: None,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn env_is_carried_to_main_container() {
        // CRITICAL: spec.env MUST appear on the generated Deployment's main
        // container. Gateway 503 root cause was this not happening.
        let spec = base_spec();
        let env_k8s: Vec<_> = spec.env.iter().map(crd_types::EnvVar::to_k8s).collect();
        let vm_k8s: Vec<_> = spec
            .volume_mounts
            .iter()
            .map(crd_types::VolumeMount::to_k8s)
            .collect();
        let main = manifests::build_container(
            "test",
            &manifests::image_ref(&spec.image.repository, &spec.image.tag),
            &spec.image.pull_policy,
            spec.command.clone(),
            spec.args.clone(),
            env_k8s,
            vec![],
            vm_k8s,
            manifests::container_ports(&spec.ports),
            None,
            None,
            None,
        );
        let env = main.env.expect("env must be set");
        assert_eq!(env.len(), 1);
        assert_eq!(env[0].name, "FOO");
        assert_eq!(env[0].value.as_deref(), Some("bar"));
    }

    #[test]
    fn volume_mounts_are_carried_to_main_container() {
        let spec = base_spec();
        let vm_k8s: Vec<_> = spec
            .volume_mounts
            .iter()
            .map(crd_types::VolumeMount::to_k8s)
            .collect();
        let main = manifests::build_container(
            "test",
            &manifests::image_ref(&spec.image.repository, &spec.image.tag),
            "",
            vec![],
            vec![],
            vec![],
            vec![],
            vm_k8s,
            vec![],
            None,
            None,
            None,
        );
        let vms = main.volume_mounts.expect("volume_mounts must be set");
        assert_eq!(vms.len(), 1);
        assert_eq!(vms[0].mount_path, "/data");
    }

    #[test]
    fn deployment_carries_volumes() {
        let spec = base_spec();
        let labels = manifests::standard_labels("test", "", "", "v1.0.0");
        let sel = manifests::selector_labels("test");
        let vols_k8s: Vec<_> = spec.volumes.iter().map(crd_types::Volume::to_k8s).collect();
        let dep = manifests::build_deployment(
            "test",
            "default",
            labels,
            sel,
            Some(2),
            vec![],
            vols_k8s,
            "",
            vec![],
            "",
        );
        let pod_spec = dep.spec.unwrap().template.spec.unwrap();
        let vols = pod_spec.volumes.expect("volumes must be on pod spec");
        assert_eq!(vols.len(), 1);
        assert_eq!(vols[0].name, "data");
    }

    // ---- Replicas / HPA interaction ----

    /// Helper that mirrors the runtime logic in `reconcile_service_inner` for
    /// deciding what to pass to `build_deployment` as `replicas`. Keep this
    /// function in lockstep with the controller body.
    fn replicas_for_deployment(spec: &ServiceSpec) -> Option<i32> {
        if spec.autoscaling.as_ref().is_some_and(|a| a.enabled) {
            None
        } else {
            Some(spec.replicas.unwrap_or(1))
        }
    }

    #[test]
    fn deployment_omits_replicas_when_autoscaling_enabled() {
        // When HPA is enabled the operator must NOT own spec.replicas.
        // Server-side apply would otherwise fight the HPA every reconcile.
        let mut spec = base_spec();
        spec.replicas = Some(2);
        spec.autoscaling = Some(AutoscalingSpec {
            enabled: true,
            min_replicas: Some(2),
            max_replicas: Some(20),
            target_cpu_utilization: Some(70),
            target_memory_utilization: None,
        });
        assert_eq!(
            replicas_for_deployment(&spec),
            None,
            "with HPA enabled, deployment.replicas must be None so HPA owns the field"
        );
    }

    #[test]
    fn deployment_keeps_replicas_when_autoscaling_disabled() {
        let mut spec = base_spec();
        spec.replicas = Some(3);
        spec.autoscaling = Some(AutoscalingSpec {
            enabled: false,
            min_replicas: None,
            max_replicas: None,
            target_cpu_utilization: None,
            target_memory_utilization: None,
        });
        assert_eq!(replicas_for_deployment(&spec), Some(3));
    }

    #[test]
    fn deployment_keeps_replicas_when_autoscaling_unset() {
        let mut spec = base_spec();
        spec.replicas = Some(4);
        spec.autoscaling = None;
        assert_eq!(replicas_for_deployment(&spec), Some(4));
    }

    #[test]
    fn deployment_defaults_to_one_replica_when_unset_and_no_hpa() {
        let mut spec = base_spec();
        spec.replicas = None;
        spec.autoscaling = None;
        assert_eq!(replicas_for_deployment(&spec), Some(1));
    }

    // ---- persistence (SeaweedFS-backed SQLite via hanzoai/replicate) ----

    use crate::crd::{PersistenceSpec, StorageSpec};

    /// Single-DB persistence spec (the console-sqlite shape): only the fields
    /// a user would set — defaults fill in endpoint/region/secrets/image.
    fn persistence_spec() -> PersistenceSpec {
        PersistenceSpec {
            enabled: true,
            data_dir: "/var/lib/hanzo/console".to_string(),
            db_path: "app.db".to_string(),
            bucket: "console-db".to_string(),
            s3_path: "console/app".to_string(),
            // serde default for the field is `true` (see `default = "default_true"`);
            // set it here so this hand-built spec matches what a CR deserializes to.
            force_path_style: true,
            storage: Some(StorageSpec {
                size: "10Gi".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// Assemble the Deployment exactly as `reconcile_service_inner` does for a
    /// Service with persistence enabled — the same resolution + helper calls,
    /// fed into the same `build_deployment` (mirrors `deployment_carries_volumes`).
    fn build_persisted_deployment(
        name: &str,
        spec: &ServiceSpec,
    ) -> k8s_openapi::api::apps::v1::Deployment {
        let p = spec
            .persistence
            .as_ref()
            .filter(|p| p.enabled)
            .map(|p| resolved_persistence(name, p));

        let mut main_vms: Vec<crd_types::VolumeMount> = spec.volume_mounts.clone();
        if let Some(p) = &p {
            main_vms.push(main_app_db_mount(p));
        }
        let vm_k8s: Vec<_> = main_vms
            .iter()
            .map(crd_types::VolumeMount::to_k8s)
            .collect();
        let main = manifests::build_container(
            name,
            &manifests::image_ref(&spec.image.repository, &spec.image.tag),
            &spec.image.pull_policy,
            spec.command.clone(),
            spec.args.clone(),
            spec.env.iter().map(crd_types::EnvVar::to_k8s).collect(),
            vec![],
            vm_k8s,
            manifests::container_ports(&spec.ports),
            None,
            None,
            None,
        );
        let mut containers = vec![main];
        containers.extend(spec.sidecars.iter().map(crd_types::Container::to_k8s));
        if let Some(p) = &p {
            containers.push(replicate_sidecar(p).to_k8s());
        }

        let mut all_volumes: Vec<crd_types::Volume> = spec.volumes.clone();
        if let Some(p) = &p {
            all_volumes.push(app_db_volume(name, p));
            all_volumes.push(replicate_config_volume(name));
        }
        let volumes_k8s: Vec<_> = all_volumes.iter().map(crd_types::Volume::to_k8s).collect();

        let mut deploy = manifests::build_deployment(
            name,
            "hanzo",
            manifests::standard_labels(name, "", "", &spec.image.tag),
            manifests::selector_labels(name),
            Some(1),
            containers,
            volumes_k8s,
            "Recreate",
            vec![],
            "",
        );
        if let Some(d_spec) = deploy.spec.as_mut() {
            let mut inits: Vec<_> = spec
                .init_containers
                .iter()
                .map(crd_types::Container::to_k8s)
                .collect();
            if let Some(p) = &p {
                if !p.dir_mode {
                    inits.push(replicate_restore_init(p).to_k8s());
                }
            }
            if !inits.is_empty() {
                if let Some(pod) = d_spec.template.spec.as_mut() {
                    pod.init_containers = Some(inits);
                }
            }
        }
        deploy
    }

    #[test]
    fn persistence_emits_replicate_restore_init() {
        let mut spec = base_spec();
        spec.persistence = Some(persistence_spec());
        let dep = build_persisted_deployment("console", &spec);
        let inits = dep
            .spec
            .unwrap()
            .template
            .spec
            .unwrap()
            .init_containers
            .expect("init containers must be present");
        assert!(
            inits.iter().any(|c| c.name == "replicate-restore"),
            "single-DB persistence must inject a replicate-restore initContainer"
        );
    }

    #[test]
    fn persistence_emits_replicate_sidecar() {
        let mut spec = base_spec();
        spec.persistence = Some(persistence_spec());
        let dep = build_persisted_deployment("console", &spec);
        let containers = dep.spec.unwrap().template.spec.unwrap().containers;
        assert!(
            containers.iter().any(|c| c.name == "replicate"),
            "persistence must inject a `replicate` sidecar container"
        );
    }

    #[test]
    fn persistence_mounts_app_db_on_main_and_sidecar() {
        let mut spec = base_spec();
        spec.persistence = Some(persistence_spec());
        let dep = build_persisted_deployment("console", &spec);
        let pod = dep.spec.unwrap().template.spec.unwrap();

        // app-db volume exists on the pod.
        let vols = pod.volumes.expect("volumes must be on pod spec");
        assert!(
            vols.iter().any(|v| v.name == "app-db"),
            "app-db volume must be on the pod"
        );

        let data_dir = "/var/lib/hanzo/console";
        // Mounted at data_dir on the MAIN container (containers[0]).
        let main = &pod.containers[0];
        let main_vms = main.volume_mounts.as_ref().expect("main mounts");
        assert!(
            main_vms
                .iter()
                .any(|m| m.name == "app-db" && m.mount_path == data_dir),
            "main container must mount app-db at the data_dir"
        );
        // Mounted at data_dir on the sidecar.
        let sidecar = pod
            .containers
            .iter()
            .find(|c| c.name == "replicate")
            .expect("replicate sidecar");
        let side_vms = sidecar.volume_mounts.as_ref().expect("sidecar mounts");
        assert!(
            side_vms
                .iter()
                .any(|m| m.name == "app-db" && m.mount_path == data_dir),
            "replicate sidecar must mount app-db at the data_dir"
        );
    }

    #[test]
    fn persistence_configmap_has_bucket_and_endpoint() {
        let p = resolved_persistence("console", &persistence_spec());
        let yml = render_replicate_yml(&p);
        assert!(yml.contains("bucket: console-db"), "must carry the bucket");
        assert!(
            yml.contains("endpoint: http://s3.hanzo.svc:9000"),
            "must carry the http:// endpoint (scheme is load-bearing)"
        );
        assert!(
            yml.contains("force-path-style: true"),
            "must carry force-path-style for SeaweedFS"
        );
        assert!(
            yml.contains("path: /var/lib/hanzo/console/app.db"),
            "single-DB mode must point at the data_dir/db_path file"
        );
    }

    #[test]
    fn persistence_dir_mode_watches_and_omits_restore_init() {
        let mut pspec = persistence_spec();
        pspec.dir_mode = true;
        pspec.db_path = String::new();
        let p = resolved_persistence("console", &pspec);

        // ConfigMap uses dir: + watch: true, NOT a single path:.
        let yml = render_replicate_yml(&p);
        assert!(
            yml.contains("dir: /var/lib/hanzo/console"),
            "dir_mode emits dir:"
        );
        assert!(yml.contains("watch: true"), "dir_mode emits watch: true");
        assert!(
            yml.contains("pattern: \"**/*.db\""),
            "dir_mode emits the glob, quoted (a bare `*` scalar is invalid YAML)"
        );
        assert!(
            !yml.contains("\n    path:"),
            "dir_mode must NOT emit a single path:"
        );

        // No restore init in dir_mode.
        let mut spec = base_spec();
        spec.persistence = Some(pspec);
        let dep = build_persisted_deployment("console", &spec);
        let inits = dep.spec.unwrap().template.spec.unwrap().init_containers;
        let has_restore = inits
            .map(|v| v.iter().any(|c| c.name == "replicate-restore"))
            .unwrap_or(false);
        assert!(
            !has_restore,
            "dir_mode must NOT inject a restore initContainer"
        );
    }

    /// Surge co-location gate — the fleet-safety property. OPT-IN + RollingUpdate
    /// + a mounted PVC are ALL required; anything else must NOT get the affinity
    /// (an exclusive-lock engine on Recreate, a non-opted service, or a
    /// volume-less service would only stall or crashloop under it).
    #[test]
    fn colocate_only_when_opted_in_rolling_and_pvc() {
        assert!(should_colocate(true, "RollingUpdate", true));
        assert!(should_colocate(true, "", true)); // empty strategy ⇒ RollingUpdate
        assert!(!should_colocate(false, "RollingUpdate", true)); // not opted in
        assert!(!should_colocate(true, "Recreate", true)); // exclusive-lock default
        assert!(!should_colocate(true, "RollingUpdate", false)); // no PVC to anchor
    }

    /// The co-location affinity shape: SOFT (preferred, never required — a failed
    /// co-location degrades to a fail-safe stalled roll, not an outage), weight
    /// 100, hostname topology, self-selector (matches the app's own pods).
    #[test]
    fn colocation_affinity_is_soft_self_hostname() {
        let mut sel = std::collections::BTreeMap::new();
        sel.insert("app.kubernetes.io/name".to_string(), "iam".to_string());
        let aff = manifests::colocation_affinity(&sel);
        let pa = aff.pod_affinity.unwrap();
        assert!(
            pa.required_during_scheduling_ignored_during_execution
                .is_none(),
            "must be SOFT — never a required (hard) constraint"
        );
        let terms = pa
            .preferred_during_scheduling_ignored_during_execution
            .unwrap();
        assert_eq!(terms.len(), 1);
        assert_eq!(terms[0].weight, 100);
        assert_eq!(
            terms[0].pod_affinity_term.topology_key,
            "kubernetes.io/hostname"
        );
        assert_eq!(
            terms[0]
                .pod_affinity_term
                .label_selector
                .as_ref()
                .unwrap()
                .match_labels
                .as_ref()
                .unwrap()
                .get("app.kubernetes.io/name"),
            Some(&"iam".to_string())
        );
    }
}
