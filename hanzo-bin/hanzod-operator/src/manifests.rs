// Copyright 2026 Hanzo AI Inc. All Rights Reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Pure mapping: a `services.hanzo.ai` Service CR → the owned objects that run
//! it (Deployment + Service + optional Ingress). No I/O, no client — the whole
//! reconcile *decision* lives here as deterministic functions, so it is unit
//! tested directly (`Service CR → Deployment spec`) without a cluster. The
//! reconcile loop is then a thin *effect*: gate on the coordinator, [`plan`],
//! server-side-apply. Decision and effect stay unbraided (Hickey: values, not
//! places).

use std::collections::BTreeMap;

use k8s_openapi::api::apps::v1::{Deployment, DeploymentSpec, DeploymentStrategy};
use k8s_openapi::api::core::v1 as corev1;
use k8s_openapi::api::networking::v1 as netv1;
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta, OwnerReference};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::{Resource, ResourceExt};

use crate::crd::{self, Service};

/// The full set of owned objects a Service CR reconciles to. `ingress` is
/// present only when `spec.ingress.enabled`.
#[derive(Debug)]
pub struct Plan {
    pub deployment: Deployment,
    pub service: corev1::Service,
    pub ingress: Option<netv1::Ingress>,
}

/// Map a Service CR to its owned objects. Deterministic and pure — the same CR
/// always yields the same Plan.
pub fn plan(svc: &Service) -> anyhow::Result<Plan> {
    Ok(Plan {
        deployment: build_deployment(svc)?,
        service: build_service(svc),
        ingress: build_ingress(svc),
    })
}

/// The workload Deployment: one container from `spec.image`, plus init/sidecar
/// containers, env, ports, probes, resources, replicas, volumes.
pub fn build_deployment(svc: &Service) -> anyhow::Result<Deployment> {
    let name = svc.name_any();
    let spec = &svc.spec;

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
    // sidecars run alongside the main container in the same pod.
    let mut containers = vec![main];
    for s in &spec.sidecars {
        containers.push(extra_container(s));
    }

    let volumes = spec
        .volumes
        .iter()
        .map(volume)
        .collect::<anyhow::Result<Vec<_>>>()?;

    let pod_spec = corev1::PodSpec {
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

    let mut template_labels = base_labels(svc, &name);
    template_labels.extend(selector_labels(&name));

    Ok(Deployment {
        metadata: object_meta(svc, &name),
        spec: Some(DeploymentSpec {
            replicas: Some(spec.replicas.unwrap_or(1)),
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

/// The ClusterIP Service fronting the pods. Each `spec.ports` entry becomes a
/// ServicePort (`port` = servicePort||containerPort, `targetPort` = containerPort).
pub fn build_service(svc: &Service) -> corev1::Service {
    let name = svc.name_any();
    let ports = svc
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
        metadata: object_meta(svc, &name),
        spec: Some(corev1::ServiceSpec {
            selector: Some(selector_labels(&name)),
            ports: opt_vec(&ports),
            type_: Some("ClusterIP".to_string()),
            ..Default::default()
        }),
        status: None,
    }
}

/// The Ingress, or `None` when `spec.ingress` is absent/disabled. One `/` Prefix
/// rule per host to the Service's first port unless explicit `pathRules` are set.
/// Emits `kubernetes.io/ingress.class` (hanzoai/ingress matches on it) as well as
/// `spec.ingressClassName`, and a cert-manager cluster-issuer annotation for TLS.
pub fn build_ingress(svc: &Service) -> Option<netv1::Ingress> {
    let ing = svc.spec.ingress.as_ref().filter(|i| i.enabled)?;
    let name = svc.name_any();

    let default_port = svc
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
            .map(|r| {
                http_path(
                    &r.path,
                    &r.path_type,
                    r.service_name.as_deref().unwrap_or(&name),
                    r.port,
                )
            })
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

    let mut meta = object_meta(svc, &name);
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

// ---- shared helpers (one mapping for each concern) -------------------------

/// Immutable, minimal selector — the ONE label that binds Deployment↔pods↔Service.
/// Kept to a single stable key so the Deployment selector never needs mutation.
fn selector_labels(name: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("app.kubernetes.io/name".to_string(), name.to_string())])
}

/// The recommended labels on every owned object: name, manager, and the CR's
/// `partOf`/`component` plus any `spec.labels`.
fn base_labels(svc: &Service, name: &str) -> BTreeMap<String, String> {
    let mut l = selector_labels(name);
    l.insert("app.kubernetes.io/managed-by".into(), "hanzod".into());
    if let Some(p) = &svc.spec.part_of {
        l.insert("app.kubernetes.io/part-of".into(), p.clone());
    }
    if let Some(c) = &svc.spec.component {
        l.insert("app.kubernetes.io/component".into(), c.clone());
    }
    if let Some(extra) = &svc.spec.labels {
        l.extend(extra.clone());
    }
    l
}

/// Metadata for an owned object: name in the CR's namespace, base labels, and an
/// ownerReference back to the CR (set the controller = true so `kubectl` GC and
/// the kube runtime's `owns()` watch adopt it).
fn object_meta(svc: &Service, name: &str) -> ObjectMeta {
    ObjectMeta {
        name: Some(name.to_string()),
        namespace: svc.namespace(),
        labels: Some(base_labels(svc, name)),
        owner_references: owner_reference(svc).map(|o| vec![o]),
        ..Default::default()
    }
}

fn owner_reference(svc: &Service) -> Option<OwnerReference> {
    let uid = svc.uid()?;
    Some(OwnerReference {
        api_version: Service::api_version(&()).into_owned(),
        kind: Service::kind(&()).into_owned(),
        name: svc.name_any(),
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
        _ => None, // default RollingUpdate
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

/// Handler precedence exec > tcpSocket > httpGet (matches the CRD's rule so a
/// non-HTTP store's probe is never mangled into an invalid `httpGet{port:0}`).
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

/// Opaque volume source passthrough: the CR carries `{name, <source>}` and we
/// deserialize it straight into a typed k8s Volume (hanzod does not interpret
/// which source it is — it only wires `name` to the mount).
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

/// `None` for an empty collection so a serialized object omits the field
/// (idempotent apply: an absent field never fights the API server's default).
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
        EnvVar, EnvVarSource, ExecAction, Image, Ingress, KeySelector, Port, Probe,
        ResourceRequirements, ServiceSpec,
    };

    /// A representative CR mirroring the shipped billing/cdn Service CRs.
    fn billing() -> Service {
        let spec = ServiceSpec {
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
        let mut svc = Service::new("billing", spec);
        svc.metadata.namespace = Some("hanzo".into());
        svc.metadata.uid = Some("uid-billing-123".into());
        svc.metadata.generation = Some(2);
        svc
    }

    #[test]
    fn deployment_maps_core_fields() {
        let d = build_deployment(&billing()).unwrap();
        assert_eq!(d.metadata.name.as_deref(), Some("billing"));
        assert_eq!(d.metadata.namespace.as_deref(), Some("hanzo"));

        let owner = &d.metadata.owner_references.as_ref().unwrap()[0];
        assert_eq!(owner.kind, "Service");
        assert_eq!(owner.api_version, "hanzo.ai/v1");
        assert_eq!(owner.uid, "uid-billing-123");
        assert_eq!(owner.controller, Some(true));

        let spec = d.spec.unwrap();
        assert_eq!(spec.replicas, Some(1));
        assert_eq!(
            spec.selector.match_labels.as_ref().unwrap().get("app.kubernetes.io/name").unwrap(),
            "billing"
        );

        let pod = spec.template.spec.as_ref().unwrap();
        let c = &pod.containers[0];
        assert_eq!(c.name, "billing");
        assert_eq!(c.image.as_deref(), Some("ghcr.io/hanzoai/billing:1.0.2"));
        assert_eq!(c.image_pull_policy.as_deref(), Some("Always"));
        assert_eq!(c.ports.as_ref().unwrap()[0].container_port, 3000);

        let req = c.resources.as_ref().unwrap().requests.as_ref().unwrap();
        assert_eq!(req.get("cpu").unwrap().0, "50m");
        assert_eq!(req.get("memory").unwrap().0, "32Mi");

        let http = c.readiness_probe.as_ref().unwrap().http_get.as_ref().unwrap();
        assert_eq!(http.path.as_deref(), Some("/health"));
        assert!(matches!(http.port, IntOrString::Int(3000)));

        let e = &c.env.as_ref().unwrap()[0];
        assert_eq!(e.name, "AWS_SECRET_ACCESS_KEY");
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
        assert_eq!(
            spec.selector.as_ref().unwrap().get("app.kubernetes.io/name").unwrap(),
            "billing"
        );
    }

    #[test]
    fn ingress_emitted_when_enabled() {
        let ing = build_ingress(&billing()).unwrap();
        let anns = ing.metadata.annotations.as_ref().unwrap();
        assert_eq!(anns.get("kubernetes.io/ingress.class").unwrap(), "ingress");

        let spec = ing.spec.unwrap();
        assert_eq!(spec.ingress_class_name.as_deref(), Some("ingress"));
        let tls = spec.tls.unwrap();
        assert_eq!(tls[0].secret_name.as_deref(), Some("billing-tls"));

        let rule = &spec.rules.as_ref().unwrap()[0];
        assert_eq!(rule.host.as_deref(), Some("billing.hanzo.ai"));
        let path = &rule.http.as_ref().unwrap().paths[0];
        assert_eq!(path.path.as_deref(), Some("/"));
        assert_eq!(path.path_type, "Prefix");
        let backend = path.backend.service.as_ref().unwrap();
        assert_eq!(backend.name, "billing");
        assert_eq!(backend.port.as_ref().unwrap().number, Some(80));
    }

    #[test]
    fn ingress_absent_when_disabled() {
        let mut svc = billing();
        svc.spec.ingress = None;
        assert!(build_ingress(&svc).is_none());
    }

    #[test]
    fn image_defaults_tag_to_latest() {
        let mut svc = billing();
        svc.spec.image.tag = None;
        let d = build_deployment(&svc).unwrap();
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
    fn plan_composes_all_owned_objects() {
        let p = plan(&billing()).unwrap();
        assert_eq!(p.deployment.metadata.name.as_deref(), Some("billing"));
        assert_eq!(p.service.metadata.name.as_deref(), Some("billing"));
        assert!(p.ingress.is_some());
    }
}
