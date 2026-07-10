// Copyright 2026 Hanzo AI Inc. All Rights Reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Pure mapping: an `apps.hanzo.ai` App CR → the owned objects that run it
//! (Deployment + Service + optional Ingress + optional PVC + optional HPA). No
//! I/O — the whole reconcile *decision* is deterministic functions, unit-tested
//! with no cluster. The controller is then a thin *effect*: gate → [`plan`] →
//! apply/prune.

use std::collections::BTreeMap;

use k8s_openapi::api::apps::v1::{Deployment, DeploymentSpec, DeploymentStrategy};
use k8s_openapi::api::autoscaling::v2 as hpav2;
use k8s_openapi::api::core::v1 as corev1;
use k8s_openapi::api::networking::v1 as netv1;
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta, OwnerReference};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::{Resource, ResourceExt};

use crate::crd::{self, App, Persistence};

const REPLICATE_IMAGE: &str = "ghcr.io/hanzoai/replicate:0.8.0-amd64";
const DEFAULT_S3_ENDPOINT: &str = "http://s3.hanzo.svc:9000";
const DEFAULT_S3_REGION: &str = "us-east-1";
const DEFAULT_CREDS_SECRET: &str = "s3-credentials";
const DEFAULT_VOLUME: &str = "data";
const DEFAULT_DB: &str = "app.db";

/// The full set of owned objects an App CR reconciles to. `ingress`/`hpa`
/// `None` mean "ensure absent" (prune); `pvc None` means "no managed volume"
/// (an existing data PVC is never deleted by hanzod).
#[derive(Debug)]
pub struct Plan {
    pub deployment: Deployment,
    pub service: corev1::Service,
    pub ingress: Option<netv1::Ingress>,
    pub pvc: Option<corev1::PersistentVolumeClaim>,
    pub hpa: Option<hpav2::HorizontalPodAutoscaler>,
}

pub fn plan(app: &App) -> anyhow::Result<Plan> {
    Ok(Plan {
        deployment: build_deployment(app)?,
        service: build_service(app),
        ingress: build_ingress(app),
        pvc: build_pvc(app),
        hpa: build_hpa(app),
    })
}

/// Whether autoscaling is on — when true, Deployment.replicas is left unset so
/// hanzod does not fight the HPA (MED-6).
fn autoscaling_enabled(app: &App) -> bool {
    app.spec.autoscaling.as_ref().is_some_and(|a| a.enabled)
}

pub fn build_deployment(app: &App) -> anyhow::Result<Deployment> {
    let name = app.name_any();
    let spec = &app.spec;

    let main = corev1::Container {
        name: name.clone(),
        image: Some(spec.image.reference()),
        image_pull_policy: spec.image.pull_policy.clone(),
        command: opt_vec(&spec.command),
        args: opt_vec(&spec.args),
        env: opt_vec(&spec.env.iter().map(env_var).collect::<Vec<_>>()),
        env_from: opt_vec(&spec.env_from.iter().map(env_from).collect::<Vec<_>>()),
        ports: opt_vec(&spec.ports.iter().map(container_port).collect::<Vec<_>>()),
        resources: spec.resources.as_ref().map(resources),
        liveness_probe: spec.liveness_probe.as_ref().map(probe),
        readiness_probe: spec.readiness_probe.as_ref().map(probe),
        volume_mounts: opt_vec(&spec.volume_mounts.iter().map(volume_mount).collect::<Vec<_>>()),
        ..Default::default()
    };

    let mut containers = vec![main];
    for s in &spec.sidecars {
        containers.push(extra_container(s));
    }

    let volumes = spec
        .volumes
        .iter()
        .map(volume)
        .collect::<anyhow::Result<Vec<_>>>()?;

    let mut pod_spec = corev1::PodSpec {
        service_account_name: spec.service_account_name.clone(),
        image_pull_secrets: opt_vec(
            &spec
                .image_pull_secrets
                .iter()
                .map(|r| corev1::LocalObjectReference { name: r.name.clone() })
                .collect::<Vec<_>>(),
        ),
        security_context: spec.fs_group.map(|g| corev1::PodSecurityContext {
            fs_group: Some(g),
            ..Default::default()
        }),
        init_containers: opt_vec(&spec.init_containers.iter().map(extra_container).collect::<Vec<_>>()),
        containers,
        volumes: opt_vec(&volumes),
        ..Default::default()
    };

    // Durable-SQLite wiring: volume + restore init (after user inits) + WAL sidecar.
    if let Some(p) = spec.persistence.as_ref().filter(|p| p.enabled) {
        wire_persistence(&mut pod_spec, &name, p);
    }

    let mut template_labels = base_labels(app, &name);
    template_labels.extend(selector_labels(&name));

    let replicas = if autoscaling_enabled(app) {
        None // HPA owns replica count
    } else {
        Some(spec.replicas.unwrap_or(1))
    };

    Ok(Deployment {
        metadata: object_meta(app, &name),
        spec: Some(DeploymentSpec {
            replicas,
            selector: LabelSelector {
                match_labels: Some(selector_labels(&name)),
                match_expressions: None,
            },
            strategy: strategy(spec.strategy.as_deref()),
            template: corev1::PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(template_labels),
                    annotations: spec.annotations.clone(),
                    ..Default::default()
                }),
                spec: Some(pod_spec),
            },
            ..Default::default()
        }),
        status: None,
    })
}

pub fn build_service(app: &App) -> corev1::Service {
    let name = app.name_any();
    let ports = app
        .spec
        .ports
        .iter()
        .map(|p| corev1::ServicePort {
            name: Some(p.name.clone()),
            port: p.effective_service_port(),
            target_port: Some(IntOrString::Int(p.container_port)),
            protocol: p.protocol.clone(),
            ..Default::default()
        })
        .collect::<Vec<_>>();

    corev1::Service {
        metadata: object_meta(app, &name),
        spec: Some(corev1::ServiceSpec {
            selector: Some(selector_labels(&name)),
            ports: opt_vec(&ports),
            type_: Some("ClusterIP".to_string()),
            ..Default::default()
        }),
        status: None,
    }
}

pub fn build_ingress(app: &App) -> Option<netv1::Ingress> {
    let ing = app.spec.ingress.as_ref().filter(|i| i.enabled)?;
    let name = app.name_any();

    let default_port = app
        .spec
        .ports
        .first()
        .map(crd::Port::effective_service_port)
        .unwrap_or(80);

    let paths: Vec<netv1::HTTPIngressPath> = if ing.path_rules.is_empty() {
        vec![http_path("/", "Prefix", &name, default_port)]
    } else {
        ing.path_rules
            .iter()
            .map(|r| http_path(&r.path, &r.path_type, r.service_name.as_deref().unwrap_or(&name), r.port))
            .collect()
    };

    let rules = ing
        .hosts
        .iter()
        .map(|h| netv1::IngressRule {
            host: Some(h.clone()),
            http: Some(netv1::HTTPIngressRuleValue { paths: paths.clone() }),
        })
        .collect::<Vec<_>>();

    let tls = (ing.tls && !ing.hosts.is_empty()).then(|| {
        vec![netv1::IngressTLS {
            hosts: Some(ing.hosts.clone()),
            secret_name: Some(format!("{name}-tls")),
        }]
    });

    let mut annotations = ing.annotations.clone().unwrap_or_default();
    if let Some(class) = &ing.ingress_class_name {
        annotations.insert("kubernetes.io/ingress.class".into(), class.clone());
    }
    if ing.tls {
        if let Some(issuer) = &ing.cluster_issuer {
            annotations.insert("cert-manager.io/cluster-issuer".into(), issuer.clone());
        }
    }

    let mut meta = object_meta(app, &name);
    if !annotations.is_empty() {
        meta.annotations = Some(annotations);
    }

    Some(netv1::Ingress {
        metadata: meta,
        spec: Some(netv1::IngressSpec {
            ingress_class_name: ing.ingress_class_name.clone(),
            tls,
            rules: opt_vec(&rules),
            ..Default::default()
        }),
        status: None,
    })
}

/// The retained data volume for persistence. Only emitted when a `storage.size`
/// is set (else the pod uses an emptyDir, no PVC). Named `<app>-<volume>`.
pub fn build_pvc(app: &App) -> Option<corev1::PersistentVolumeClaim> {
    let p = app.spec.persistence.as_ref().filter(|p| p.enabled)?;
    let storage = p.storage.as_ref()?;
    let name = pvc_name(&app.name_any(), p);
    Some(corev1::PersistentVolumeClaim {
        metadata: object_meta(app, &name),
        spec: Some(corev1::PersistentVolumeClaimSpec {
            access_modes: Some(vec!["ReadWriteOnce".to_string()]),
            resources: Some(corev1::VolumeResourceRequirements {
                requests: Some(BTreeMap::from([("storage".to_string(), Quantity(storage.size.clone()))])),
                limits: None,
            }),
            storage_class_name: storage.storage_class_name.clone(),
            ..Default::default()
        }),
        status: None,
    })
}

/// The HPA, when autoscaling is enabled. Scales the owned Deployment on CPU
/// (default) and/or memory utilization.
pub fn build_hpa(app: &App) -> Option<hpav2::HorizontalPodAutoscaler> {
    let a = app.spec.autoscaling.as_ref().filter(|a| a.enabled)?;
    let name = app.name_any();

    let mut metrics = Vec::new();
    if let Some(pct) = a.target_cpu_utilization {
        metrics.push(resource_metric("cpu", pct));
    }
    if let Some(pct) = a.target_memory_utilization {
        metrics.push(resource_metric("memory", pct));
    }
    if metrics.is_empty() {
        metrics.push(resource_metric("cpu", 80)); // sensible default
    }

    Some(hpav2::HorizontalPodAutoscaler {
        metadata: object_meta(app, &name),
        spec: hpav2::HorizontalPodAutoscalerSpec {
            scale_target_ref: hpav2::CrossVersionObjectReference {
                api_version: Some("apps/v1".to_string()),
                kind: "Deployment".to_string(),
                name: name.clone(),
            },
            min_replicas: a.min_replicas,
            max_replicas: a.max_replicas.unwrap_or(10),
            metrics: Some(metrics),
            behavior: None,
        },
        status: None,
    })
}

// ---- persistence wiring ----------------------------------------------------

fn pvc_name(app_name: &str, p: &Persistence) -> String {
    let vol = p
        .storage
        .as_ref()
        .and_then(|s| s.volume_name.clone())
        .unwrap_or_else(|| DEFAULT_VOLUME.to_string());
    format!("{app_name}-{vol}")
}

/// Add the data volume, mount it into the main container, append the
/// `restore -if-db-not-exists` init container (AFTER user init containers) and
/// the continuous `replicate` WAL sidecar. Mirrors the Go operator; config via
/// `REPLICATE_*` env.
fn wire_persistence(pod: &mut corev1::PodSpec, app_name: &str, p: &Persistence) {
    let vol = p
        .storage
        .as_ref()
        .and_then(|s| s.volume_name.clone())
        .unwrap_or_else(|| DEFAULT_VOLUME.to_string());
    let data_dir = p.data_dir.clone().unwrap_or_else(|| "/data".to_string());

    // The volume: a retained PVC when sized, else an emptyDir.
    let source = if p.storage.is_some() {
        corev1::Volume {
            name: vol.clone(),
            persistent_volume_claim: Some(corev1::PersistentVolumeClaimVolumeSource {
                claim_name: pvc_name(app_name, p),
                read_only: None,
            }),
            ..Default::default()
        }
    } else {
        corev1::Volume { name: vol.clone(), empty_dir: Some(Default::default()), ..Default::default() }
    };
    pod.volumes.get_or_insert_with(Vec::new).push(source);

    let mount = corev1::VolumeMount {
        name: vol.clone(),
        mount_path: data_dir.clone(),
        ..Default::default()
    };
    if let Some(main) = pod.containers.first_mut() {
        main.volume_mounts.get_or_insert_with(Vec::new).push(mount.clone());
    }

    let db_target = if p.dir_mode {
        data_dir.clone()
    } else {
        let db = p.db_path.clone().unwrap_or_else(|| DEFAULT_DB.to_string());
        format!("{data_dir}/{db}")
    };
    let image = p.image.clone().unwrap_or_else(|| REPLICATE_IMAGE.to_string());

    // Restore init — appended AFTER user init containers (paas.yaml rationale:
    // a migrate-first init would create an empty DB and skip the snapshot).
    let restore = corev1::Container {
        name: "replicate-restore".to_string(),
        image: Some(image.clone()),
        command: Some(vec![
            "replicate".to_string(),
            "restore".to_string(),
            "-if-db-not-exists".to_string(),
            db_target.clone(),
        ]),
        env: Some(replicate_env(app_name, p, true)),
        volume_mounts: Some(vec![mount.clone()]),
        ..Default::default()
    };
    pod.init_containers.get_or_insert_with(Vec::new).push(restore);

    // Continuous WAL replication sidecar.
    let sidecar = corev1::Container {
        name: "replicate".to_string(),
        image: Some(image),
        command: Some(vec!["replicate".to_string(), "replicate".to_string(), db_target]),
        env: Some(replicate_env(app_name, p, false)),
        volume_mounts: Some(vec![mount]),
        ..Default::default()
    };
    pod.containers.push(sidecar);
}

/// `REPLICATE_*` env for the replicate init/sidecar. `restore` selects the age
/// identity (decrypt, restore) vs recipient (encrypt, replicate).
fn replicate_env(app_name: &str, p: &Persistence, restore: bool) -> Vec<corev1::EnvVar> {
    let bucket = p.bucket.clone().unwrap_or_default();
    let s3_path = p.s3_path.clone().unwrap_or_default();
    let creds = p.credentials_secret.clone().unwrap_or_else(|| DEFAULT_CREDS_SECRET.to_string());
    let age = p.age_secret.clone().unwrap_or_else(|| format!("{app_name}-replicate-age"));

    let mut env = vec![
        plain_env("REPLICATE_BUCKET", &bucket),
        plain_env("REPLICATE_PATH", &s3_path),
        plain_env("REPLICATE_REPLICA_URL", &format!("s3://{bucket}/{s3_path}")),
        plain_env("REPLICATE_ENDPOINT", p.s3_endpoint.as_deref().unwrap_or(DEFAULT_S3_ENDPOINT)),
        plain_env("REPLICATE_REGION", p.s3_region.as_deref().unwrap_or(DEFAULT_S3_REGION)),
        plain_env("REPLICATE_FORCE_PATH_STYLE", if p.force_path_style { "true" } else { "false" }),
        plain_env("REPLICATE_ALLOW_PLAINTEXT", "1"),
        secret_env("REPLICATE_ACCESS_KEY_ID", &creds, "access-key"),
        secret_env("REPLICATE_SECRET_ACCESS_KEY", &creds, "secret-key"),
    ];
    if restore {
        env.push(secret_env("REPLICATE_AGE_IDENTITY", &age, "identity"));
    } else {
        env.push(secret_env("REPLICATE_AGE_RECIPIENT", &age, "recipients"));
    }
    env
}

fn plain_env(name: &str, value: &str) -> corev1::EnvVar {
    corev1::EnvVar { name: name.to_string(), value: Some(value.to_string()), value_from: None }
}

fn secret_env(name: &str, secret: &str, key: &str) -> corev1::EnvVar {
    corev1::EnvVar {
        name: name.to_string(),
        value: None,
        value_from: Some(corev1::EnvVarSource {
            secret_key_ref: Some(corev1::SecretKeySelector {
                name: secret.to_string(),
                key: key.to_string(),
                optional: None,
            }),
            ..Default::default()
        }),
    }
}

fn resource_metric(name: &str, pct: i32) -> hpav2::MetricSpec {
    hpav2::MetricSpec {
        type_: "Resource".to_string(),
        resource: Some(hpav2::ResourceMetricSource {
            name: name.to_string(),
            target: hpav2::MetricTarget {
                type_: "Utilization".to_string(),
                average_utilization: Some(pct),
                ..Default::default()
            },
        }),
        ..Default::default()
    }
}

// ---- shared helpers --------------------------------------------------------

fn selector_labels(name: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("app.kubernetes.io/name".to_string(), name.to_string())])
}

fn base_labels(app: &App, name: &str) -> BTreeMap<String, String> {
    let mut l = selector_labels(name);
    l.insert("app.kubernetes.io/managed-by".into(), "hanzod".into());
    if let Some(p) = &app.spec.part_of {
        l.insert("app.kubernetes.io/part-of".into(), p.clone());
    }
    if let Some(c) = &app.spec.component {
        l.insert("app.kubernetes.io/component".into(), c.clone());
    }
    if let Some(extra) = &app.spec.labels {
        l.extend(extra.clone());
    }
    l
}

fn object_meta(app: &App, name: &str) -> ObjectMeta {
    ObjectMeta {
        name: Some(name.to_string()),
        namespace: app.namespace(),
        labels: Some(base_labels(app, name)),
        owner_references: owner_reference(app).map(|o| vec![o]),
        ..Default::default()
    }
}

fn owner_reference(app: &App) -> Option<OwnerReference> {
    let uid = app.uid()?;
    Some(OwnerReference {
        api_version: App::api_version(&()).into_owned(),
        kind: App::kind(&()).into_owned(),
        name: app.name_any(),
        uid,
        controller: Some(true),
        block_owner_deletion: Some(true),
    })
}

fn strategy(kind: Option<&str>) -> Option<DeploymentStrategy> {
    match kind {
        Some("Recreate") => Some(DeploymentStrategy {
            type_: Some("Recreate".to_string()),
            rolling_update: None,
        }),
        _ => None,
    }
}

fn container_port(p: &crd::Port) -> corev1::ContainerPort {
    corev1::ContainerPort {
        name: Some(p.name.clone()),
        container_port: p.container_port,
        protocol: p.protocol.clone(),
        ..Default::default()
    }
}

fn env_var(e: &crd::EnvVar) -> corev1::EnvVar {
    corev1::EnvVar {
        name: e.name.clone(),
        value: e.value.clone(),
        value_from: e.value_from.as_ref().map(|vf| corev1::EnvVarSource {
            config_map_key_ref: vf.config_map_key_ref.as_ref().map(|k| corev1::ConfigMapKeySelector {
                key: k.key.clone(),
                name: k.name.clone(),
                optional: k.optional,
            }),
            secret_key_ref: vf.secret_key_ref.as_ref().map(|k| corev1::SecretKeySelector {
                key: k.key.clone(),
                name: k.name.clone(),
                optional: k.optional,
            }),
            field_ref: vf.field_ref.as_ref().map(|f| corev1::ObjectFieldSelector {
                api_version: f.api_version.clone(),
                field_path: f.field_path.clone(),
            }),
            file_key_ref: None,
            resource_field_ref: None,
        }),
    }
}

fn env_from(e: &crd::EnvFromSource) -> corev1::EnvFromSource {
    corev1::EnvFromSource {
        prefix: e.prefix.clone(),
        config_map_ref: e.config_map_ref.as_ref().map(|r| corev1::ConfigMapEnvSource {
            name: r.name.clone(),
            optional: r.optional,
        }),
        secret_ref: e.secret_ref.as_ref().map(|r| corev1::SecretEnvSource {
            name: r.name.clone(),
            optional: r.optional,
        }),
    }
}

fn resources(r: &crd::ResourceRequirements) -> corev1::ResourceRequirements {
    corev1::ResourceRequirements {
        requests: r.requests.as_ref().map(quantities),
        limits: r.limits.as_ref().map(quantities),
        claims: None,
    }
}

fn quantities(m: &BTreeMap<String, String>) -> BTreeMap<String, Quantity> {
    m.iter().map(|(k, v)| (k.clone(), Quantity(v.clone()))).collect()
}

fn probe(p: &crd::Probe) -> corev1::Probe {
    let mut out = corev1::Probe {
        initial_delay_seconds: p.initial_delay_seconds,
        period_seconds: p.period_seconds,
        ..Default::default()
    };
    if let Some(e) = &p.exec {
        out.exec = Some(corev1::ExecAction { command: opt_vec(&e.command) });
    } else if let Some(t) = &p.tcp_socket {
        out.tcp_socket = Some(corev1::TCPSocketAction { port: IntOrString::Int(t.port), host: None });
    } else if let Some(port) = p.port.filter(|&x| x > 0) {
        out.http_get = Some(corev1::HTTPGetAction {
            path: Some(p.path.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| "/".into())),
            port: IntOrString::Int(port),
            ..Default::default()
        });
    }
    out
}

fn volume_mount(v: &crd::VolumeMount) -> corev1::VolumeMount {
    corev1::VolumeMount {
        name: v.name.clone(),
        mount_path: v.mount_path.clone(),
        read_only: v.read_only,
        sub_path: v.sub_path.clone(),
        ..Default::default()
    }
}

fn volume(v: &crd::Volume) -> anyhow::Result<corev1::Volume> {
    let mut obj = serde_json::Map::new();
    obj.insert("name".into(), serde_json::Value::String(v.name.clone()));
    for (k, val) in &v.source {
        obj.insert(k.clone(), val.clone());
    }
    Ok(serde_json::from_value(serde_json::Value::Object(obj))?)
}

fn extra_container(c: &crd::ExtraContainer) -> corev1::Container {
    corev1::Container {
        name: c.name.clone(),
        image: Some(c.image.clone()),
        image_pull_policy: c.image_pull_policy.clone(),
        command: opt_vec(&c.command),
        args: opt_vec(&c.args),
        env: opt_vec(&c.env.iter().map(env_var).collect::<Vec<_>>()),
        env_from: opt_vec(&c.env_from.iter().map(env_from).collect::<Vec<_>>()),
        volume_mounts: opt_vec(&c.volume_mounts.iter().map(volume_mount).collect::<Vec<_>>()),
        ..Default::default()
    }
}

fn http_path(path: &str, path_type: &str, service: &str, port: i32) -> netv1::HTTPIngressPath {
    netv1::HTTPIngressPath {
        path: Some(path.to_string()),
        path_type: path_type.to_string(),
        backend: netv1::IngressBackend {
            service: Some(netv1::IngressServiceBackend {
                name: service.to_string(),
                port: Some(netv1::ServiceBackendPort { number: Some(port), name: None }),
            }),
            resource: None,
        },
    }
}

fn opt_vec<T: Clone>(v: &[T]) -> Option<Vec<T>> {
    if v.is_empty() {
        None
    } else {
        Some(v.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crd::{
        Autoscaling, EnvVar, EnvVarSource, ExecAction, Image, Ingress, KeySelector, Persistence,
        Port, Probe, ResourceRequirements, Storage,
    };

    fn billing() -> App {
        let spec = crate::crd::AppSpec {
            image: Image {
                repository: "ghcr.io/hanzoai/billing".into(),
                tag: Some("1.0.2".into()),
                pull_policy: Some("Always".into()),
            },
            replicas: Some(1),
            ports: vec![Port {
                name: "http".into(),
                container_port: 3000,
                service_port: Some(80),
                protocol: None,
            }],
            resources: Some(ResourceRequirements {
                requests: Some(BTreeMap::from([
                    ("cpu".into(), "50m".into()),
                    ("memory".into(), "32Mi".into()),
                ])),
                limits: None,
            }),
            readiness_probe: Some(Probe {
                path: Some("/health".into()),
                port: Some(3000),
                initial_delay_seconds: Some(3),
                period_seconds: Some(10),
                ..Default::default()
            }),
            env: vec![EnvVar {
                name: "AWS_SECRET_ACCESS_KEY".into(),
                value: None,
                value_from: Some(EnvVarSource {
                    secret_key_ref: Some(KeySelector {
                        key: "secret-key".into(),
                        name: "s3-credentials".into(),
                        optional: None,
                    }),
                    ..Default::default()
                }),
            }],
            ingress: Some(Ingress {
                enabled: true,
                hosts: vec!["billing.hanzo.ai".into()],
                tls: true,
                ingress_class_name: Some("ingress".into()),
                ..Default::default()
            }),
            part_of: Some("platform".into()),
            component: Some("billing".into()),
            ..Default::default()
        };
        let mut app = App::new("billing", spec);
        app.metadata.namespace = Some("hanzo".into());
        app.metadata.uid = Some("uid-billing-123".into());
        app.metadata.generation = Some(2);
        app
    }

    fn chat() -> App {
        let mut app = App::new(
            "chat",
            crate::crd::AppSpec {
                image: Image { repository: "ghcr.io/hanzoai/chat".into(), tag: Some("1".into()), ..Default::default() },
                persistence: Some(Persistence {
                    enabled: true,
                    bucket: Some("chat-db".into()),
                    data_dir: Some("/var/lib/hanzo/chat".into()),
                    db_path: Some("chat.db".into()),
                    dir_mode: false,
                    force_path_style: true,
                    age_secret: Some("chat-replicate-age".into()),
                    image: Some("ghcr.io/hanzoai/replicate:0.8.0-amd64".into()),
                    s3_path: Some("chat/app".into()),
                    storage: Some(Storage {
                        size: "10Gi".into(),
                        storage_class_name: Some("do-block-storage".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        app.metadata.namespace = Some("hanzo".into());
        app.metadata.uid = Some("uid-chat".into());
        app
    }

    #[test]
    fn deployment_maps_core_fields() {
        let d = build_deployment(&billing()).unwrap();
        assert_eq!(d.metadata.name.as_deref(), Some("billing"));
        assert_eq!(d.metadata.namespace.as_deref(), Some("hanzo"));

        let owner = &d.metadata.owner_references.as_ref().unwrap()[0];
        assert_eq!(owner.kind, "App");
        assert_eq!(owner.api_version, "hanzo.ai/v1");
        assert_eq!(owner.uid, "uid-billing-123");
        assert_eq!(owner.controller, Some(true));

        let spec = d.spec.unwrap();
        assert_eq!(spec.replicas, Some(1));
        assert_eq!(
            spec.selector.match_labels.as_ref().unwrap().get("app.kubernetes.io/name").unwrap(),
            "billing"
        );

        let c = &spec.template.spec.as_ref().unwrap().containers[0];
        assert_eq!(c.name, "billing");
        assert_eq!(c.image.as_deref(), Some("ghcr.io/hanzoai/billing:1.0.2"));
        assert_eq!(c.image_pull_policy.as_deref(), Some("Always"));
        assert_eq!(c.ports.as_ref().unwrap()[0].container_port, 3000);

        let req = c.resources.as_ref().unwrap().requests.as_ref().unwrap();
        assert_eq!(req.get("cpu").unwrap().0, "50m");

        let http = c.readiness_probe.as_ref().unwrap().http_get.as_ref().unwrap();
        assert_eq!(http.path.as_deref(), Some("/health"));
        assert!(matches!(http.port, IntOrString::Int(3000)));

        let e = &c.env.as_ref().unwrap()[0];
        let skr = e.value_from.as_ref().unwrap().secret_key_ref.as_ref().unwrap();
        assert_eq!(skr.name, "s3-credentials");
        assert_eq!(skr.key, "secret-key");
    }

    #[test]
    fn deployment_labels_carry_part_of_and_component() {
        let d = build_deployment(&billing()).unwrap();
        let l = d.metadata.labels.as_ref().unwrap();
        assert_eq!(l.get("app.kubernetes.io/managed-by").unwrap(), "hanzod");
        assert_eq!(l.get("app.kubernetes.io/part-of").unwrap(), "platform");
        assert_eq!(l.get("app.kubernetes.io/component").unwrap(), "billing");
    }

    #[test]
    fn service_maps_ports() {
        let s = build_service(&billing());
        let spec = s.spec.unwrap();
        let sp = &spec.ports.as_ref().unwrap()[0];
        assert_eq!(sp.port, 80);
        assert!(matches!(sp.target_port.as_ref().unwrap(), IntOrString::Int(3000)));
        assert_eq!(spec.type_.as_deref(), Some("ClusterIP"));
    }

    #[test]
    fn ingress_emitted_when_enabled() {
        let ing = build_ingress(&billing()).unwrap();
        let anns = ing.metadata.annotations.as_ref().unwrap();
        assert_eq!(anns.get("kubernetes.io/ingress.class").unwrap(), "ingress");
        let spec = ing.spec.unwrap();
        let tls = spec.tls.unwrap();
        assert_eq!(tls[0].secret_name.as_deref(), Some("billing-tls"));
        let path = &spec.rules.as_ref().unwrap()[0].http.as_ref().unwrap().paths[0];
        assert_eq!(path.path.as_deref(), Some("/"));
        assert_eq!(path.backend.service.as_ref().unwrap().port.as_ref().unwrap().number, Some(80));
    }

    #[test]
    fn ingress_pruned_when_disabled() {
        // plan.ingress == None is the signal the reconcile loop DELETES the owned
        // Ingress (MED-3), not leaves it orphaned.
        let mut app = billing();
        app.spec.ingress = None;
        assert!(build_ingress(&app).is_none());
        assert!(plan(&app).unwrap().ingress.is_none());
    }

    #[test]
    fn image_defaults_tag_to_latest() {
        let mut app = billing();
        app.spec.image.tag = None;
        let d = build_deployment(&app).unwrap();
        let c = &d.spec.unwrap().template.spec.unwrap().containers[0];
        assert_eq!(c.image.as_deref(), Some("ghcr.io/hanzoai/billing:latest"));
    }

    #[test]
    fn probe_precedence_exec_over_http() {
        let p = Probe {
            path: Some("/h".into()),
            port: Some(8080),
            exec: Some(ExecAction { command: vec!["sh".into(), "-c".into(), "true".into()] }),
            ..Default::default()
        };
        let out = probe(&p);
        assert!(out.exec.is_some());
        assert!(out.http_get.is_none());
    }

    #[test]
    fn persistence_wires_pvc_volume_restore_init_and_sidecar() {
        let d = build_deployment(&chat()).unwrap();
        let pod = d.spec.unwrap().template.spec.unwrap();

        // data volume, PVC-backed with claim name <app>-data
        let vol = pod.volumes.as_ref().unwrap().iter().find(|v| v.name == "data").unwrap();
        assert_eq!(vol.persistent_volume_claim.as_ref().unwrap().claim_name, "chat-data");

        // main container mounts it at dataDir
        let main = &pod.containers[0];
        let m = main.volume_mounts.as_ref().unwrap().iter().find(|m| m.name == "data").unwrap();
        assert_eq!(m.mount_path, "/var/lib/hanzo/chat");

        // restore init container, appended, restore-if-db-not-exists <dataDir/db>
        let restore = pod.init_containers.as_ref().unwrap().iter().find(|c| c.name == "replicate-restore").unwrap();
        assert_eq!(
            restore.command.as_ref().unwrap(),
            &vec!["replicate".to_string(), "restore".into(), "-if-db-not-exists".into(), "/var/lib/hanzo/chat/chat.db".into()]
        );
        let ident = restore.env.as_ref().unwrap().iter().find(|e| e.name == "REPLICATE_AGE_IDENTITY").unwrap();
        assert_eq!(ident.value_from.as_ref().unwrap().secret_key_ref.as_ref().unwrap().name, "chat-replicate-age");

        // replication sidecar
        let sidecar = pod.containers.iter().find(|c| c.name == "replicate").unwrap();
        assert_eq!(sidecar.command.as_ref().unwrap()[1], "replicate");
        assert!(sidecar.env.as_ref().unwrap().iter().any(|e| e.name == "REPLICATE_AGE_RECIPIENT"));
    }

    #[test]
    fn persistence_builds_retained_pvc() {
        let pvc = build_pvc(&chat()).unwrap();
        assert_eq!(pvc.metadata.name.as_deref(), Some("chat-data"));
        let spec = pvc.spec.unwrap();
        assert_eq!(spec.access_modes.as_ref().unwrap()[0], "ReadWriteOnce");
        assert_eq!(spec.resources.as_ref().unwrap().requests.as_ref().unwrap().get("storage").unwrap().0, "10Gi");
        assert_eq!(spec.storage_class_name.as_deref(), Some("do-block-storage"));
    }

    #[test]
    fn autoscaling_omits_replicas_and_builds_hpa() {
        let mut app = billing();
        app.spec.autoscaling = Some(Autoscaling {
            enabled: true,
            min_replicas: Some(2),
            max_replicas: Some(8),
            target_cpu_utilization: Some(70),
            target_memory_utilization: None,
        });
        // MED-6: Deployment must NOT force replicas under an HPA.
        let d = build_deployment(&app).unwrap();
        assert!(d.spec.unwrap().replicas.is_none());

        let hpa = build_hpa(&app).unwrap();
        let hspec = hpa.spec;
        assert_eq!(hspec.max_replicas, 8);
        assert_eq!(hspec.min_replicas, Some(2));
        assert_eq!(hspec.scale_target_ref.name, "billing");
        assert_eq!(hspec.scale_target_ref.kind, "Deployment");
        let m = &hspec.metrics.as_ref().unwrap()[0];
        assert_eq!(m.resource.as_ref().unwrap().name, "cpu");
        assert_eq!(m.resource.as_ref().unwrap().target.average_utilization, Some(70));
    }

    #[test]
    fn plan_composes_owned_objects() {
        // billing: deployment+service+ingress, no pvc/hpa
        let p = plan(&billing()).unwrap();
        assert!(p.ingress.is_some());
        assert!(p.pvc.is_none());
        assert!(p.hpa.is_none());
        // chat: adds a pvc
        assert!(plan(&chat()).unwrap().pvc.is_some());
    }
}
