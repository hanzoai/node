//! K8s object builders.
//!
//! Parallel to the Go `internal/manifests/` package. Pure functions that
//! return canonical `k8s_openapi` types — no I/O, no controller dispatch.

use std::collections::BTreeMap;

use k8s_openapi::api::apps::v1::{
    Deployment, DeploymentSpec, DeploymentStrategy, RollingUpdateDeployment, StatefulSet,
    StatefulSetSpec, StatefulSetUpdateStrategy,
};
use k8s_openapi::api::autoscaling::v2::{
    CrossVersionObjectReference, HorizontalPodAutoscaler, HorizontalPodAutoscalerSpec, MetricSpec,
    MetricTarget, ResourceMetricSource,
};
use k8s_openapi::api::core::v1::{
    Affinity, ConfigMap, Container, ContainerPort, EnvFromSource, EnvVar, ExecAction,
    HTTPGetAction, Lifecycle, LifecycleHandler, LocalObjectReference, PersistentVolumeClaim,
    PodAffinity, PodAffinityTerm, PodSpec, PodTemplateSpec, Probe,
    ResourceRequirements as K8sResourceRequirements, Service as CoreService, ServicePort,
    ServiceSpec as CoreServiceSpec, TCPSocketAction, Volume, VolumeMount, WeightedPodAffinityTerm,
};
use k8s_openapi::api::networking::v1::{
    HTTPIngressPath, HTTPIngressRuleValue, Ingress, IngressBackend, IngressRule,
    IngressServiceBackend, IngressSpec as K8sIngressSpec, IngressTLS, NetworkPolicy,
    NetworkPolicyIngressRule, NetworkPolicyPeer as K8sNetworkPolicyPeer,
    NetworkPolicySpec as K8sNetworkPolicySpec, ServiceBackendPort,
};
use k8s_openapi::api::policy::v1::{PodDisruptionBudget, PodDisruptionBudgetSpec as K8sPDBSpec};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;

use crate::crd::{
    AutoscalingSpec, IngressSpec, NetworkPolicySpec, PodDisruptionBudgetSpec, ProbeSpec,
    ResourceRequirements, ServicePort as CrServicePort,
};

pub const LABEL_NAME: &str = "app.kubernetes.io/name";
pub const LABEL_INSTANCE: &str = "app.kubernetes.io/instance";
pub const LABEL_COMPONENT: &str = "app.kubernetes.io/component";
pub const LABEL_PART_OF: &str = "app.kubernetes.io/part-of";
pub const LABEL_VERSION: &str = "app.kubernetes.io/version";
pub const LABEL_MANAGED_BY: &str = "app.kubernetes.io/managed-by";
pub const MANAGED_BY_VALUE: &str = "hanzo-operator";

/// Coerce an image tag / ref into a valid Kubernetes label value for
/// `app.kubernetes.io/version`. A label value must be ≤63 chars, contain only
/// `[A-Za-z0-9._-]`, and start + end alphanumeric.
///
/// Digest-pinned refs are now the canonical deploy pattern (universe#445), so
/// `spec.image.tag` can carry `v8.4.118@sha256:9820e153…`. That value blows
/// BOTH the 63-char limit and the charset (`@`, `:` are illegal), so inserting
/// it verbatim made the API server reject the whole Deployment
/// (`metadata.labels: Invalid value`) — the `console` reconcile storm.
///
/// Rule: keep the human tag before any `@` digest, replace remaining illegal
/// chars with `-`, cap at 63, and trim back to an alphanumeric boundary. A
/// bare-digest ref (nothing before `@`) folds to a valid `sha256-<hex…>`.
/// Deterministic: same ref → same value (no rollout churn).
pub fn sanitize_label_value(v: &str) -> String {
    // Prefer the human tag before a digest; fall back to the whole ref.
    let base = match v.split_once('@') {
        Some((tag, _digest)) if !tag.is_empty() => tag,
        _ => v,
    };
    // Replace any char outside the label alphabet. ASCII-only after this, so a
    // subsequent byte-truncate can't split a char.
    let mapped: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let capped = &mapped[..mapped.len().min(63)];
    // Must start AND end with an alphanumeric.
    capped
        .trim_matches(|c: char| !c.is_ascii_alphanumeric())
        .to_string()
}

/// Build the standard `app.kubernetes.io/*` label set. Empty values omitted;
/// the `version` label is sanitized via [`sanitize_label_value`].
pub fn standard_labels(
    name: &str,
    component: &str,
    part_of: &str,
    version: &str,
) -> BTreeMap<String, String> {
    let mut labels = BTreeMap::new();
    labels.insert(LABEL_NAME.to_string(), name.to_string());
    labels.insert(LABEL_INSTANCE.to_string(), name.to_string());
    labels.insert(LABEL_MANAGED_BY.to_string(), MANAGED_BY_VALUE.to_string());
    if !component.is_empty() {
        labels.insert(LABEL_COMPONENT.to_string(), component.to_string());
    }
    if !part_of.is_empty() {
        labels.insert(LABEL_PART_OF.to_string(), part_of.to_string());
    }
    // Sanitize: a digest-pinned `version` (repo tag `vX.Y.Z@sha256:…`) is not a
    // valid label value and would fail the Deployment apply.
    let version = sanitize_label_value(version);
    if !version.is_empty() {
        labels.insert(LABEL_VERSION.to_string(), version);
    }
    labels
}

/// Minimal label set for pod selectors. Must be immutable after creation.
pub fn selector_labels(name: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    out.insert(LABEL_NAME.to_string(), name.to_string());
    out.insert(LABEL_INSTANCE.to_string(), name.to_string());
    out
}

/// Merge label maps in order; later entries override earlier on key collision.
pub fn merge_labels(maps: &[&BTreeMap<String, String>]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for m in maps {
        for (k, v) in m.iter() {
            out.insert(k.clone(), v.clone());
        }
    }
    out
}

/// Inject a preStop sleep on every container that lacks one. Gives pods 5
/// seconds to drain before SIGTERM.
fn inject_pre_stop(containers: Vec<Container>) -> Vec<Container> {
    containers
        .into_iter()
        .map(|mut c| {
            let lifecycle = c.lifecycle.get_or_insert_with(Lifecycle::default);
            if lifecycle.pre_stop.is_none() {
                lifecycle.pre_stop = Some(LifecycleHandler {
                    exec: Some(ExecAction {
                        command: Some(vec![
                            "/bin/sh".to_string(),
                            "-c".to_string(),
                            "sleep 5".to_string(),
                        ]),
                    }),
                    ..Default::default()
                });
            }
            c
        })
        .collect()
}

/// Convert operator ResourceRequirements to k8s ResourceRequirements.
/// Maps `String` quantities → k8s `Quantity` (String is wire-compatible).
pub fn to_k8s_resources(spec: &ResourceRequirements) -> K8sResourceRequirements {
    use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
    let to_q = |m: &BTreeMap<String, String>| {
        m.iter()
            .map(|(k, v)| (k.clone(), Quantity(v.clone())))
            .collect::<BTreeMap<_, _>>()
    };
    K8sResourceRequirements {
        requests: spec.requests.as_ref().map(to_q),
        limits: spec.limits.as_ref().map(to_q),
        ..Default::default()
    }
}

/// Build an HTTP GET probe.
pub fn build_http_probe(spec: &ProbeSpec) -> Probe {
    Probe {
        http_get: Some(HTTPGetAction {
            path: if spec.path.is_empty() {
                Some("/health".to_string())
            } else {
                Some(spec.path.clone())
            },
            port: IntOrString::Int(spec.port),
            ..Default::default()
        }),
        initial_delay_seconds: if spec.initial_delay_seconds > 0 {
            Some(spec.initial_delay_seconds)
        } else {
            Some(5)
        },
        period_seconds: if spec.period_seconds > 0 {
            Some(spec.period_seconds)
        } else {
            Some(10)
        },
        ..Default::default()
    }
}

/// Build a k8s `Probe` from a CR `ProbeSpec`, dispatching to the declared
/// handler. Returns `None` when the spec declares no usable handler, so the
/// caller emits NO probe rather than an invalid one.
///
/// Handler precedence: `exec` → `tcpSocket` → `httpGet` (`port > 0`).
///
/// This is the fix for the reconcile storm where non-HTTP datastores
/// (`insights-sql` `pg_isready`, `insights-kv` `redis-cli ping`,
/// `insights-kafka` TCP `9092`) declared `exec`/`tcpSocket` probes that the
/// old HTTP-only `ProbeSpec` dropped — leaving `port: 0` and emitting an
/// `httpGet` the API server rejected (`port: Invalid value: 0: must be between
/// 1 and 65535`). We now honor the real handler and NEVER emit a port-0
/// `httpGet`.
pub fn build_probe(spec: &ProbeSpec) -> Option<Probe> {
    let timing = |mut p: Probe| -> Probe {
        p.initial_delay_seconds = Some(if spec.initial_delay_seconds > 0 {
            spec.initial_delay_seconds
        } else {
            5
        });
        p.period_seconds = Some(if spec.period_seconds > 0 {
            spec.period_seconds
        } else {
            10
        });
        p
    };
    if let Some(e) = &spec.exec {
        if !e.command.is_empty() {
            return Some(timing(Probe {
                exec: Some(ExecAction {
                    command: Some(e.command.clone()),
                }),
                ..Default::default()
            }));
        }
    }
    if let Some(t) = &spec.tcp_socket {
        if t.port > 0 {
            return Some(timing(Probe {
                tcp_socket: Some(TCPSocketAction {
                    port: IntOrString::Int(t.port),
                    ..Default::default()
                }),
                ..Default::default()
            }));
        }
    }
    if spec.port > 0 {
        // Reuse the HTTP builder (already applies path/timing defaults).
        return Some(build_http_probe(spec));
    }
    None
}

/// Convert CR ServicePorts to k8s ContainerPorts.
pub fn container_ports(ports: &[CrServicePort]) -> Vec<ContainerPort> {
    ports
        .iter()
        .map(|p| ContainerPort {
            name: Some(p.name.clone()),
            container_port: p.container_port,
            protocol: if p.protocol.is_empty() {
                None
            } else {
                Some(p.protocol.clone())
            },
            ..Default::default()
        })
        .collect()
}

/// Convert CR ServicePorts to k8s Service ports.
pub fn service_ports(ports: &[CrServicePort]) -> Vec<ServicePort> {
    ports
        .iter()
        .map(|p| ServicePort {
            name: Some(p.name.clone()),
            port: p.service_port.unwrap_or(p.container_port),
            target_port: Some(IntOrString::Int(p.container_port)),
            protocol: if p.protocol.is_empty() {
                None
            } else {
                Some(p.protocol.clone())
            },
            ..Default::default()
        })
        .collect()
}

/// Build a Deployment with standard rolling-update settings.
#[allow(clippy::too_many_arguments)]
pub fn build_deployment(
    name: &str,
    namespace: &str,
    labels: BTreeMap<String, String>,
    selector_labels_map: BTreeMap<String, String>,
    replicas: Option<i32>,
    containers: Vec<Container>,
    volumes: Vec<Volume>,
    strategy: &str,
    image_pull_secrets: Vec<LocalObjectReference>,
    service_account_name: &str,
) -> Deployment {
    let s = if strategy == "Recreate" {
        DeploymentStrategy {
            type_: Some("Recreate".to_string()),
            ..Default::default()
        }
    } else {
        DeploymentStrategy {
            type_: Some("RollingUpdate".to_string()),
            rolling_update: Some(RollingUpdateDeployment {
                max_surge: Some(IntOrString::Int(1)),
                max_unavailable: Some(IntOrString::Int(0)),
            }),
        }
    };

    let containers = inject_pre_stop(containers);

    Deployment {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(namespace.to_string()),
            labels: Some(labels.clone()),
            ..Default::default()
        },
        spec: Some(DeploymentSpec {
            replicas,
            min_ready_seconds: Some(10),
            selector: LabelSelector {
                match_labels: Some(selector_labels_map),
                ..Default::default()
            },
            strategy: Some(s),
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(labels),
                    ..Default::default()
                }),
                spec: Some(PodSpec {
                    containers,
                    volumes: if volumes.is_empty() {
                        None
                    } else {
                        Some(volumes)
                    },
                    image_pull_secrets: if image_pull_secrets.is_empty() {
                        None
                    } else {
                        Some(image_pull_secrets)
                    },
                    service_account_name: if service_account_name.is_empty() {
                        None
                    } else {
                        Some(service_account_name.to_string())
                    },
                    termination_grace_period_seconds: Some(30),
                    ..Default::default()
                }),
            },
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Soft self-podAffinity that co-locates a rolling surge pod on the SAME node
/// as the app's already-running pods (topologyKey hostname, matching the app's
/// own selector). For a service whose data lives on a single ReadWriteOnce PVC,
/// this lets the surge pod bind-mount the already-attached volume — DO block
/// storage is single-attach, so RWO permits multiple pods per NODE but not a
/// second node — instead of dead-locking on a "Multi-Attach" error. That turns
/// a RollingUpdate over the volume into a zero-downtime, same-host handoff.
///
/// PREFERRED, never required: with no anchor pod (cold start / node loss) the
/// surge still schedules anywhere and recovers; a rare failure to co-locate is a
/// fail-SAFE stalled roll (old pod keeps serving under maxUnavailable=0), never
/// an outage or a cross-node split-brain writer.
///
/// OPT-IN by the caller (`ServiceSpec.surgeColocation`): the brief same-node
/// two-pod overlap is only safe for stores that tolerate concurrent same-host
/// opens (SQLite in WAL mode + `busy_timeout`). An exclusive-lock single-open
/// engine (Badger, LMDB, Qdrant, …) must stay on strategy `Recreate` instead,
/// so this is never applied automatically.
pub fn colocation_affinity(selector_labels_map: &BTreeMap<String, String>) -> Affinity {
    Affinity {
        pod_affinity: Some(PodAffinity {
            preferred_during_scheduling_ignored_during_execution: Some(vec![
                WeightedPodAffinityTerm {
                    weight: 100,
                    pod_affinity_term: PodAffinityTerm {
                        label_selector: Some(LabelSelector {
                            match_labels: Some(selector_labels_map.clone()),
                            ..Default::default()
                        }),
                        topology_key: "kubernetes.io/hostname".to_string(),
                        ..Default::default()
                    },
                },
            ]),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Build a StatefulSet.
#[allow(clippy::too_many_arguments)]
pub fn build_statefulset(
    name: &str,
    namespace: &str,
    labels: BTreeMap<String, String>,
    selector_labels_map: BTreeMap<String, String>,
    replicas: Option<i32>,
    containers: Vec<Container>,
    volumes: Vec<Volume>,
    pvc_templates: Vec<PersistentVolumeClaim>,
    image_pull_secrets: Vec<LocalObjectReference>,
    service_name: &str,
) -> StatefulSet {
    let containers = inject_pre_stop(containers);

    StatefulSet {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(namespace.to_string()),
            labels: Some(labels.clone()),
            ..Default::default()
        },
        spec: Some(StatefulSetSpec {
            replicas,
            min_ready_seconds: Some(10),
            // k8s 1.33: StatefulSetSpec.service_name is now Option<String>.
            service_name: Some(service_name.to_string()),
            update_strategy: Some(StatefulSetUpdateStrategy {
                type_: Some("RollingUpdate".to_string()),
                ..Default::default()
            }),
            selector: LabelSelector {
                match_labels: Some(selector_labels_map),
                ..Default::default()
            },
            volume_claim_templates: if pvc_templates.is_empty() {
                None
            } else {
                Some(pvc_templates)
            },
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(labels),
                    ..Default::default()
                }),
                spec: Some(PodSpec {
                    containers,
                    volumes: if volumes.is_empty() {
                        None
                    } else {
                        Some(volumes)
                    },
                    image_pull_secrets: if image_pull_secrets.is_empty() {
                        None
                    } else {
                        Some(image_pull_secrets)
                    },
                    termination_grace_period_seconds: Some(30),
                    ..Default::default()
                }),
            },
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Build a ClusterIP Service.
pub fn build_service(
    name: &str,
    namespace: &str,
    labels: BTreeMap<String, String>,
    ports: Vec<ServicePort>,
    selector_labels_map: BTreeMap<String, String>,
) -> CoreService {
    CoreService {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(namespace.to_string()),
            labels: Some(labels),
            ..Default::default()
        },
        spec: Some(CoreServiceSpec {
            type_: Some("ClusterIP".to_string()),
            selector: Some(selector_labels_map),
            ports: Some(ports),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Build a headless Service (`ClusterIP: None`) for StatefulSet pod DNS.
pub fn build_headless_service(
    name: &str,
    namespace: &str,
    labels: BTreeMap<String, String>,
    ports: Vec<ServicePort>,
    selector_labels_map: BTreeMap<String, String>,
) -> CoreService {
    let mut svc = build_service(name, namespace, labels, ports, selector_labels_map);
    if let Some(s) = svc.spec.as_mut() {
        s.cluster_ip = Some("None".to_string());
    }
    svc
}

/// Build an Ingress with cert-manager annotations.
pub fn build_ingress(
    name: &str,
    namespace: &str,
    spec: &IngressSpec,
    service_name: &str,
    service_port: i32,
    labels: BTreeMap<String, String>,
) -> Ingress {
    let mut annotations: BTreeMap<String, String> = BTreeMap::new();
    if spec.tls {
        let issuer = if spec.cluster_issuer.is_empty() {
            "letsencrypt-prod"
        } else {
            &spec.cluster_issuer
        };
        annotations.insert(
            "cert-manager.io/cluster-issuer".to_string(),
            issuer.to_string(),
        );
    }
    if let Some(ann) = &spec.annotations {
        for (k, v) in ann {
            annotations.insert(k.clone(), v.clone());
        }
    }
    // hanzoai/ingress (Traefik fork) silently drops spec.tls when the caller
    // sets spec.ingressClassName instead of the annotation. Emit the
    // annotation form so TLS stays hooked up.
    if !spec.ingress_class_name.is_empty() {
        annotations.insert(
            "kubernetes.io/ingress.class".to_string(),
            spec.ingress_class_name.clone(),
        );
    }

    let path_type = "Prefix".to_string();
    let mut rules = Vec::new();
    for host in &spec.hosts {
        let mut paths = vec![HTTPIngressPath {
            path: Some("/".to_string()),
            path_type: path_type.clone(),
            backend: IngressBackend {
                service: Some(IngressServiceBackend {
                    name: service_name.to_string(),
                    port: Some(ServiceBackendPort {
                        number: Some(service_port),
                        ..Default::default()
                    }),
                }),
                ..Default::default()
            },
        }];

        for pr in &spec.path_rules {
            let pt = match pr.path_type.as_str() {
                "Exact" => "Exact".to_string(),
                "ImplementationSpecific" => "ImplementationSpecific".to_string(),
                _ => "Prefix".to_string(),
            };
            let backend_name = if pr.service_name.is_empty() {
                service_name
            } else {
                &pr.service_name
            };
            paths.push(HTTPIngressPath {
                path: Some(pr.path.clone()),
                path_type: pt,
                backend: IngressBackend {
                    service: Some(IngressServiceBackend {
                        name: backend_name.to_string(),
                        port: Some(ServiceBackendPort {
                            number: Some(pr.port),
                            ..Default::default()
                        }),
                    }),
                    ..Default::default()
                },
            });
        }

        rules.push(IngressRule {
            host: Some(host.clone()),
            http: Some(HTTPIngressRuleValue { paths }),
        });
    }

    let tls = if spec.tls && !spec.hosts.is_empty() {
        Some(vec![IngressTLS {
            hosts: Some(spec.hosts.clone()),
            secret_name: Some(format!("{}-tls", name)),
        }])
    } else {
        None
    };

    Ingress {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(namespace.to_string()),
            labels: Some(labels),
            annotations: Some(annotations),
            ..Default::default()
        },
        spec: Some(K8sIngressSpec {
            rules: Some(rules),
            tls,
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Build an HPA targeting a Deployment.
pub fn build_hpa(
    name: &str,
    namespace: &str,
    target_ref: CrossVersionObjectReference,
    spec: &AutoscalingSpec,
    labels: BTreeMap<String, String>,
) -> HorizontalPodAutoscaler {
    let mut metrics = Vec::new();

    if let Some(cpu) = spec.target_cpu_utilization {
        metrics.push(MetricSpec {
            type_: "Resource".to_string(),
            resource: Some(ResourceMetricSource {
                name: "cpu".to_string(),
                target: MetricTarget {
                    type_: "Utilization".to_string(),
                    average_utilization: Some(cpu),
                    ..Default::default()
                },
            }),
            ..Default::default()
        });
    }
    if let Some(mem) = spec.target_memory_utilization {
        metrics.push(MetricSpec {
            type_: "Resource".to_string(),
            resource: Some(ResourceMetricSource {
                name: "memory".to_string(),
                target: MetricTarget {
                    type_: "Utilization".to_string(),
                    average_utilization: Some(mem),
                    ..Default::default()
                },
            }),
            ..Default::default()
        });
    }

    HorizontalPodAutoscaler {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(namespace.to_string()),
            labels: Some(labels),
            ..Default::default()
        },
        spec: Some(HorizontalPodAutoscalerSpec {
            scale_target_ref: target_ref,
            min_replicas: spec.min_replicas,
            max_replicas: spec.max_replicas.unwrap_or(10),
            metrics: Some(metrics),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Build a PodDisruptionBudget.
pub fn build_pdb(
    name: &str,
    namespace: &str,
    spec: &PodDisruptionBudgetSpec,
    selector_labels_map: BTreeMap<String, String>,
    labels: BTreeMap<String, String>,
) -> PodDisruptionBudget {
    let mut pdb_spec = K8sPDBSpec {
        selector: Some(LabelSelector {
            match_labels: Some(selector_labels_map),
            ..Default::default()
        }),
        ..Default::default()
    };
    if let Some(min) = spec.min_available {
        pdb_spec.min_available = Some(IntOrString::Int(min));
    } else if let Some(max) = spec.max_unavailable {
        pdb_spec.max_unavailable = Some(IntOrString::Int(max));
    } else {
        pdb_spec.min_available = Some(IntOrString::Int(1));
    }
    PodDisruptionBudget {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(namespace.to_string()),
            labels: Some(labels),
            ..Default::default()
        },
        spec: Some(pdb_spec),
        ..Default::default()
    }
}

/// Build a NetworkPolicy.
pub fn build_network_policy(
    name: &str,
    namespace: &str,
    spec: &NetworkPolicySpec,
    selector_labels_map: BTreeMap<String, String>,
    labels: BTreeMap<String, String>,
) -> NetworkPolicy {
    let mut from: Vec<K8sNetworkPolicyPeer> = Vec::new();

    let allow_intra = spec.allow_intra_namespace.unwrap_or(true);
    if allow_intra {
        from.push(K8sNetworkPolicyPeer {
            pod_selector: Some(LabelSelector::default()),
            ..Default::default()
        });
    }
    for peer in &spec.allow_from {
        from.push(K8sNetworkPolicyPeer {
            pod_selector: peer.pod_selector.as_ref().map(|s| LabelSelector {
                match_labels: s.match_labels.clone(),
                ..Default::default()
            }),
            namespace_selector: peer.namespace_selector.as_ref().map(|s| LabelSelector {
                match_labels: s.match_labels.clone(),
                ..Default::default()
            }),
            ..Default::default()
        });
    }

    let ingress = if spec.allow_ingress {
        Some(vec![NetworkPolicyIngressRule::default()])
    } else if !from.is_empty() {
        Some(vec![NetworkPolicyIngressRule {
            from: Some(from),
            ..Default::default()
        }])
    } else {
        None
    };

    NetworkPolicy {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(namespace.to_string()),
            labels: Some(labels),
            ..Default::default()
        },
        spec: Some(K8sNetworkPolicySpec {
            // k8s 1.33: NetworkPolicySpec.pod_selector is now Option<LabelSelector>.
            pod_selector: Some(LabelSelector {
                match_labels: Some(selector_labels_map),
                ..Default::default()
            }),
            policy_types: Some(vec!["Ingress".to_string()]),
            ingress,
            ..Default::default()
        }),
    }
}

/// Build a single container with image+ports+env+volumes+probes wired.
#[allow(clippy::too_many_arguments)]
pub fn build_container(
    name: &str,
    image: &str,
    image_pull_policy: &str,
    command: Vec<String>,
    args: Vec<String>,
    env: Vec<EnvVar>,
    env_from: Vec<EnvFromSource>,
    volume_mounts: Vec<VolumeMount>,
    ports: Vec<ContainerPort>,
    resources: Option<K8sResourceRequirements>,
    liveness_probe: Option<Probe>,
    readiness_probe: Option<Probe>,
) -> Container {
    Container {
        name: name.to_string(),
        image: Some(image.to_string()),
        image_pull_policy: if image_pull_policy.is_empty() {
            None
        } else {
            Some(image_pull_policy.to_string())
        },
        command: if command.is_empty() {
            None
        } else {
            Some(command)
        },
        args: if args.is_empty() { None } else { Some(args) },
        env: if env.is_empty() { None } else { Some(env) },
        env_from: if env_from.is_empty() {
            None
        } else {
            Some(env_from)
        },
        volume_mounts: if volume_mounts.is_empty() {
            None
        } else {
            Some(volume_mounts)
        },
        ports: if ports.is_empty() { None } else { Some(ports) },
        resources,
        liveness_probe,
        readiness_probe,
        ..Default::default()
    }
}

/// Build a ConfigMap from a `key -> contents` map.
pub fn build_configmap(
    name: &str,
    namespace: &str,
    labels: BTreeMap<String, String>,
    data: BTreeMap<String, String>,
) -> ConfigMap {
    ConfigMap {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(namespace.to_string()),
            labels: Some(labels),
            ..Default::default()
        },
        data: Some(data),
        ..Default::default()
    }
}

/// True when a ConfigMap carries no config — both `data` and `binaryData`
/// are absent or empty. Such a ConfigMap must NEVER be force-applied: SSA
/// would strip every key the operator's field manager owns, blanking a
/// mounted config file and crashlooping the workload. `apply::apply_configmap`
/// enforces this gate (root cause of the hanzo.id auth outage: `iam-conf`
/// regenerated empty → `panic: unable to open database file`).
pub fn configmap_is_empty(cm: &ConfigMap) -> bool {
    let data_empty = cm.data.as_ref().map_or(true, |d| d.is_empty());
    let binary_empty = cm.binary_data.as_ref().map_or(true, |d| d.is_empty());
    data_empty && binary_empty
}

/// Resolve image repository + tag into a single image reference.
pub fn image_ref(repository: &str, tag: &str) -> String {
    if tag.is_empty() {
        repository.to_string()
    } else {
        format!("{}:{}", repository, tag)
    }
}

/// Compute the primary service port (first defined) — used for default
/// Ingress backend.
pub fn primary_port(ports: &[CrServicePort]) -> i32 {
    if let Some(p) = ports.first() {
        p.service_port.unwrap_or(p.container_port)
    } else {
        80
    }
}

/// Build a PersistentVolumeClaim template for a StatefulSet.
pub fn build_pvc_template(name: &str, storage_class: &str, size: &str) -> PersistentVolumeClaim {
    // k8s 1.33: PersistentVolumeClaimSpec.resources is now the dedicated
    // VolumeResourceRequirements type (requests/limits only, no `claims`).
    use k8s_openapi::api::core::v1::{PersistentVolumeClaimSpec, VolumeResourceRequirements};
    use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
    PersistentVolumeClaim {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            ..Default::default()
        },
        spec: Some(PersistentVolumeClaimSpec {
            access_modes: Some(vec!["ReadWriteOnce".to_string()]),
            storage_class_name: if storage_class.is_empty() {
                Some("do-block-storage".to_string())
            } else {
                Some(storage_class.to_string())
            },
            resources: Some(VolumeResourceRequirements {
                requests: Some({
                    let mut m = BTreeMap::new();
                    m.insert("storage".to_string(), Quantity(size.to_string()));
                    m
                }),
                limits: None,
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crd::{ExecAction as CrExec, TcpSocketAction as CrTcp};

    fn probe(port: i32) -> ProbeSpec {
        ProbeSpec {
            path: String::new(),
            port,
            exec: None,
            tcp_socket: None,
            initial_delay_seconds: 0,
            period_seconds: 0,
        }
    }

    // An `exec` probe (Postgres `pg_isready`, Valkey `redis-cli ping`) renders as
    // an exec handler — NOT a mangled `httpGet{port:0}` — and carries no other
    // handler.
    #[test]
    fn build_probe_renders_exec_handler() {
        let mut p = probe(0);
        p.exec = Some(CrExec {
            command: vec!["pg_isready".into(), "-U".into(), "hanzo".into()],
        });
        let out = build_probe(&p).expect("exec probe must render");
        assert_eq!(
            out.exec.unwrap().command.unwrap(),
            vec!["pg_isready", "-U", "hanzo"]
        );
        assert!(out.http_get.is_none(), "exec probe must not emit httpGet");
        assert!(out.tcp_socket.is_none());
    }

    // A `tcpSocket` probe (Kafka TCP :9092) renders as a tcpSocket handler with
    // the right port and no httpGet.
    #[test]
    fn build_probe_renders_tcp_socket_handler() {
        let mut p = probe(0);
        p.tcp_socket = Some(CrTcp { port: 9092 });
        let out = build_probe(&p).expect("tcp probe must render");
        assert_eq!(out.tcp_socket.unwrap().port, IntOrString::Int(9092));
        assert!(out.http_get.is_none(), "tcp probe must not emit httpGet");
    }

    // A plain HTTP probe (port > 0) still renders as httpGet.
    #[test]
    fn build_probe_renders_http_handler() {
        let out = build_probe(&probe(7700)).expect("http probe must render");
        let hg = out.http_get.expect("http probe must emit httpGet");
        assert_eq!(hg.port, IntOrString::Int(7700));
        assert!(out.exec.is_none() && out.tcp_socket.is_none());
    }

    // The regression guard: a probe with NO usable handler (port 0, no
    // exec/tcpSocket) renders NOTHING rather than an invalid `httpGet{port:0}`
    // the API server rejects — the root of the 33 err/min reconcile storm.
    #[test]
    fn build_probe_never_emits_port_zero_http() {
        assert!(
            build_probe(&probe(0)).is_none(),
            "an empty probe must yield None, never httpGet{{port:0}}"
        );
    }

    // Handler precedence is exec > tcpSocket > httpGet: a CR that (wrongly)
    // sets several picks exactly one, so the object is never rejected for
    // specifying more than one handler type.
    #[test]
    fn build_probe_handler_precedence_is_exec_then_tcp_then_http() {
        let mut p = probe(8080);
        p.tcp_socket = Some(CrTcp { port: 9092 });
        p.exec = Some(CrExec {
            command: vec!["true".into()],
        });
        let out = build_probe(&p).unwrap();
        assert!(out.exec.is_some());
        assert!(out.tcp_socket.is_none() && out.http_get.is_none());

        let mut p2 = probe(8080);
        p2.tcp_socket = Some(CrTcp { port: 9092 });
        let out2 = build_probe(&p2).unwrap();
        assert!(out2.tcp_socket.is_some() && out2.http_get.is_none());
    }

    // The console regression: a digest-pinned image tag (now canonical per
    // universe#445) must NOT land verbatim in a label — it exceeds 63 chars and
    // contains illegal `@`/`:`, which rejected the whole Deployment apply.
    #[test]
    fn sanitize_label_value_strips_digest_from_pinned_tag() {
        let v = "v8.4.118@sha256:9820e1539f1a51c36179a595fda500c9470461e9b2ea0e42c7166decbc70b77a";
        assert_eq!(sanitize_label_value(v), "v8.4.118");
        assert!(is_valid_label_value(&sanitize_label_value(v)));
    }

    // A plain semver tag is a valid label value and passes through unchanged.
    #[test]
    fn sanitize_label_value_passes_plain_tags_through() {
        for t in ["18", "0.1.1", "v2.7.1", "latest"] {
            assert_eq!(sanitize_label_value(t), t, "plain tag must be unchanged");
        }
    }

    // A bare-digest ref (no human tag before `@`) folds to a valid `sha256-…`.
    #[test]
    fn sanitize_label_value_folds_bare_digest() {
        let out = sanitize_label_value(
            "@sha256:9820e1539f1a51c36179a595fda500c9470461e9b2ea0e42c7166decbc70b77a",
        );
        assert!(out.starts_with("sha256-"));
        assert!(
            is_valid_label_value(&out),
            "bare digest must sanitize valid: {out}"
        );
    }

    // Any long/illegal value is capped at 63 chars and trimmed to an
    // alphanumeric boundary — the two hard label constraints.
    #[test]
    fn sanitize_label_value_caps_length_and_boundaries() {
        let out = sanitize_label_value(&format!("v1.2.3@{}", "a".repeat(200)));
        assert_eq!(out, "v1.2.3");
        // A value that is illegal chars + long still ends valid.
        let messy = sanitize_label_value(&"_-.".repeat(30));
        assert!(is_valid_label_value(&messy) || messy.is_empty());
    }

    // The end-to-end guard: standard_labels emits a VALID version label for a
    // digest-pinned image (previously the FieldValueInvalid on console).
    #[test]
    fn standard_labels_version_is_a_valid_label_for_pinned_image() {
        let l = standard_labels(
            "console",
            "app",
            "cloud",
            "v8.4.118@sha256:9820e1539f1a51c36179a595fda500c9470461e9b2ea0e42c7166decbc70b77a",
        );
        let ver = l.get(LABEL_VERSION).expect("version label present");
        assert_eq!(ver, "v8.4.118");
        assert!(is_valid_label_value(ver));
    }

    /// Mirror of the k8s label-value validation (RFC 1123-ish): ≤63 chars,
    /// `[A-Za-z0-9._-]`, start + end alphanumeric.
    fn is_valid_label_value(v: &str) -> bool {
        !v.is_empty()
            && v.len() <= 63
            && v.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            && v.chars().next().unwrap().is_ascii_alphanumeric()
            && v.chars().last().unwrap().is_ascii_alphanumeric()
    }

    /// The outage guard: a ConfigMap built from an empty CR config source is
    /// flagged empty so `apply::apply_configmap` skips it and leaves any
    /// existing populated ConfigMap untouched.
    #[test]
    fn empty_config_source_yields_empty_configmap_flag() {
        let cm = build_configmap("iam-conf", "hanzo", BTreeMap::new(), BTreeMap::new());
        assert!(
            configmap_is_empty(&cm),
            "empty-data ConfigMap must be flagged empty so the apply guard skips it"
        );
    }

    #[test]
    fn populated_config_source_not_empty() {
        let mut data = BTreeMap::new();
        data.insert("app.conf".to_string(), "listen = :8000\n".to_string());
        let cm = build_configmap("iam-conf", "hanzo", BTreeMap::new(), data);
        assert!(
            !configmap_is_empty(&cm),
            "populated ConfigMap must apply normally"
        );
    }

    #[test]
    fn binary_only_configmap_not_empty() {
        use k8s_openapi::ByteString;
        let mut cm = build_configmap("bin-conf", "hanzo", BTreeMap::new(), BTreeMap::new());
        let mut bd = BTreeMap::new();
        bd.insert("blob".to_string(), ByteString(vec![1, 2, 3]));
        cm.binary_data = Some(bd);
        assert!(
            !configmap_is_empty(&cm),
            "binary-only ConfigMap carries config and must apply"
        );
    }
}
