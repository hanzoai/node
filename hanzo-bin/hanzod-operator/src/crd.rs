// Copyright 2026 Hanzo AI Inc. All Rights Reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! `services.hanzo.ai/v1` — the CRD hanzod reconciles.
//!
//! These types are the Rust source of truth for the `kind: Service` schema
//! (`kubectl get hsvc`). The published CRD in
//! `universe/infra/k8s/operator/crds.yaml` carries the `#[kube(...)]` derive's
//! `Auto-generated derived type for ServiceSpec` marker — it was generated FROM
//! a struct like this one. `hanzod-operator crd` re-emits it from here.
//!
//! Scope note: this foundation models the fields that map to the core workload
//! (Deployment + Service + Ingress). serde ignores unknown fields on watch, so a
//! CR carrying advanced fields not yet reconciled here (autoscaling, pdb,
//! networkPolicy, serviceMonitor, kmsSecrets, persistence) deserializes cleanly;
//! those become their own reconcilers next (`crds.yaml` stays authoritative for
//! the full schema until the Rust types cover it end-to-end).

use std::collections::BTreeMap;

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `spec` of a `services.hanzo.ai/v1` Service. The `#[kube]` derive generates
/// the `Service` root object (`apiVersion: hanzo.ai/v1`, `kind: Service`) plus
/// its `ServiceStatus` subresource.
#[derive(CustomResource, Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[kube(
    group = "hanzo.ai",
    version = "v1",
    kind = "Service",
    plural = "services",
    singular = "service",
    shortname = "hsvc",
    namespaced,
    status = "ServiceStatus",
    derive = "Default",
    printcolumn = r#"{"name":"Phase","jsonPath":".status.phase","type":"string"}"#,
    printcolumn = r#"{"name":"Ready","jsonPath":".status.readyReplicas","type":"integer"}"#,
    printcolumn = r#"{"name":"Image","jsonPath":".spec.image.repository","type":"string"}"#,
    printcolumn = r#"{"name":"Age","jsonPath":".metadata.creationTimestamp","type":"date"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct ServiceSpec {
    /// Container image. `repository` is required; `tag` defaults to `latest`.
    pub image: Image,

    /// Desired replica count. `None` → 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replicas: Option<i32>,

    /// Deployment strategy: `RollingUpdate` (default) or `Recreate`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy: Option<String>,

    /// Entrypoint override (`command`) / arguments (`args`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,

    /// Environment: literal, from a ref, or bulk from a ConfigMap/Secret.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<EnvVar>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env_from: Vec<EnvFromSource>,

    /// Exposed ports. Each becomes a container port and a Service port.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<Port>,

    /// Compute requests/limits (quantity strings, e.g. `cpu: "200m"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourceRequirements>,

    /// Health checks. `exec` > `tcpSocket` > `httpGet` precedence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub liveness_probe: Option<Probe>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readiness_probe: Option<Probe>,

    /// Extra labels / annotations propagated to the pod template.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<BTreeMap<String, String>>,

    /// Pod service account and image-pull secrets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_account_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub image_pull_secrets: Vec<LocalObjectReference>,

    /// Pod-level `securityContext.fsGroup` (non-root images writing a PVC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fs_group: Option<i64>,

    /// init / sidecar containers (share the env + volumeMount mapping).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub init_containers: Vec<ExtraContainer>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sidecars: Vec<ExtraContainer>,

    /// Main-container volume mounts + the pod volumes they bind.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volume_mounts: Vec<VolumeMount>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<Volume>,

    /// Optional Ingress. Emitted only when `enabled`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ingress: Option<Ingress>,

    /// `app.kubernetes.io/part-of` / `component` labels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part_of: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Image {
    pub repository: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_policy: Option<String>,
}

impl Image {
    /// `repository:tag`, defaulting the tag to `latest`.
    pub fn reference(&self) -> String {
        match &self.tag {
            Some(t) if !t.is_empty() => format!("{}:{}", self.repository, t),
            _ => format!("{}:latest", self.repository),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Port {
    pub name: String,
    pub container_port: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_port: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
}

impl Port {
    /// The Service port: `servicePort` if set, else the container port.
    pub fn effective_service_port(&self) -> i32 {
        self.service_port.unwrap_or(self.container_port)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnvVar {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_from: Option<EnvVarSource>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnvVarSource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_map_key_ref: Option<KeySelector>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_key_ref: Option<KeySelector>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field_ref: Option<FieldSelector>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeySelector {
    pub key: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optional: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FieldSelector {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_version: Option<String>,
    pub field_path: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnvFromSource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_map_ref: Option<LocalObjectReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<LocalObjectReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LocalObjectReference {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optional: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResourceRequirements {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requests: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<BTreeMap<String, String>>,
}

/// A health probe. Handler precedence on materialization is `exec` >
/// `tcpSocket` > `httpGet` (`path`/`port`) — matching the CRD's documented rule
/// so non-HTTP stores (pg_isready, redis ping) are not mangled into httpGet.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec: Option<ExecAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcp_socket: Option<TcpSocketAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_delay_seconds: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period_seconds: Option<i32>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExecAction {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TcpSocketAction {
    pub port: i32,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VolumeMount {
    pub name: String,
    pub mount_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub_path: Option<String>,
}

/// A pod volume. The volume source (emptyDir/configMap/secret/…) is passed
/// through verbatim (`x-kubernetes-preserve-unknown-fields`) — hanzod does not
/// interpret it, it only wires the `name` to the mount.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    pub name: String,
    #[serde(flatten)]
    #[schemars(with = "std::collections::BTreeMap<String, serde_json::Value>")]
    pub source: BTreeMap<String, serde_json::Value>,
}

/// An init or sidecar container. Reuses the same env / mount value types as the
/// main container (DRY), so the two container kinds map identically.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExtraContainer {
    pub name: String,
    pub image: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_pull_policy: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<EnvVar>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env_from: Vec<EnvFromSource>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volume_mounts: Vec<VolumeMount>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Ingress {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
    /// Terminate TLS (cert-manager). Defaults true on the CRD.
    #[serde(default = "default_true")]
    pub tls: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ingress_class_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_issuer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<BTreeMap<String, String>>,
    /// Explicit path routing. Empty → a single `/` Prefix rule per host to the
    /// first Service port.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path_rules: Vec<PathRule>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PathRule {
    pub path: String,
    pub path_type: String,
    pub port: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_name: Option<String>,
}

/// Status subresource. hanzod writes `observedGeneration` + replica counts +
/// `phase` after each reconcile; the phase drives `kubectl get hsvc`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServiceStatus {
    #[serde(default)]
    pub observed_generation: i64,
    #[serde(default)]
    pub ready_replicas: i32,
    #[serde(default)]
    pub available_replicas: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub endpoints: Vec<String>,
}

fn default_true() -> bool {
    true
}
