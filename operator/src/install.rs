//! `operator install` — apply the operator's own CRDs + Deployment/RBAC so a
//! single `hanzod install` brings the control plane up in-cluster.
//!
//! This is the "hanzod IS the operator" install path: it renders (and, unless
//! `--dry-run`, server-side-applies) the exact objects the operator needs to
//! run as the in-cluster reconciler:
//!
//! - the 28 managed CRDs (from the shared [`crate::crd_bundle`], group-rewritten),
//! - a Namespace, ServiceAccount, ClusterRole + ClusterRoleBinding, and
//! - a Deployment whose container command is the supervised entrypoint
//!   (default `hanzod operator`).
//!
//! The object builders are pure and unit-tested; the apply step reuses the
//! proven [`crate::apply::apply`] SSA round (field manager `hanzo-operator`),
//! so install and reconcile share one apply path — no second way to write.

use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{Namespace, ServiceAccount};
use k8s_openapi::api::rbac::v1::{ClusterRole, ClusterRoleBinding};
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::Api;
use serde_json::{json, Value};

use crate::api_group::ApiGroup;
use crate::apply::apply;
use crate::controllers::tenant_rbac::TENANT_CLUSTER_ROLE;
use crate::crd_bundle::bundle;

/// The install field manager + canonical names. Kept in one place so the
/// Deployment, ServiceAccount, and RBAC all agree.
const OPERATOR_NAME: &str = "hanzo-operator";

/// Configuration for an install. Built from the CLI (`operator install …` /
/// `hanzod install …`) or directly by an embedder.
#[derive(Clone, Debug)]
pub struct InstallConfig {
    /// Resolved API group (e.g. `hanzo.ai`, `lux.cloud`). Drives both the CRD
    /// group and the `OPERATOR_API_GROUP` env on the Deployment.
    pub api_group: String,
    /// Image the operator Deployment runs. Must contain the operator binary
    /// (or `hanzod`, which supervises it). No default — install refuses to
    /// render a `:latest`/placeholder image (semver-pin policy).
    pub image: String,
    /// Namespace the operator runs in (Lease + Deployment + SA live here).
    pub operator_namespace: String,
    /// Namespace the operator watches. Empty = all namespaces.
    pub watch_namespace: String,
    /// Container command for the Deployment. Default `["hanzod","operator"]` —
    /// the supervised entrypoint of the merged node image.
    pub command: Vec<String>,
    /// Apply the CRD bundle too (false → only Deployment/RBAC).
    pub apply_crds: bool,
}

impl InstallConfig {
    /// The KMS CRD group the operator also needs RBAC over (`kms.hanzo.ai` for
    /// the default group, `kms.<group>` otherwise) — mirrors `ApiGroup::kms_group`.
    fn kms_group(&self) -> String {
        ApiGroup::new(self.api_group.clone()).kms_group()
    }
}

/// The managed CRDs for this install's group.
pub fn crds(cfg: &InstallConfig) -> Vec<CustomResourceDefinition> {
    bundle(&cfg.api_group)
}

/// The operator Namespace.
pub fn namespace(cfg: &InstallConfig) -> Namespace {
    from_static(json!({
        "apiVersion": "v1",
        "kind": "Namespace",
        "metadata": { "name": cfg.operator_namespace, "labels": labels() },
    }))
}

/// The operator ServiceAccount.
pub fn service_account(cfg: &InstallConfig) -> ServiceAccount {
    from_static(json!({
        "apiVersion": "v1",
        "kind": "ServiceAccount",
        "metadata": {
            "name": OPERATOR_NAME,
            "namespace": cfg.operator_namespace,
            "labels": labels(),
        },
    }))
}

/// The operator ClusterRole — the union of every resource the controllers
/// materialize (workloads, networking, autoscaling, PDB, batch, leases, events)
/// plus full authority over its own CRD groups (`<group>` + `kms.<group>`).
pub fn cluster_role(cfg: &InstallConfig) -> ClusterRole {
    from_static(json!({
        "apiVersion": "rbac.authorization.k8s.io/v1",
        "kind": "ClusterRole",
        "metadata": { "name": OPERATOR_NAME, "labels": labels() },
        "rules": [
            {
                "apiGroups": [""],
                "resources": [
                    "services", "configmaps", "secrets", "persistentvolumeclaims",
                    "serviceaccounts", "pods", "events"
                ],
                "verbs": ["get", "list", "watch", "create", "update", "patch", "delete"]
            },
            {
                // Tenant-RBAC controller: watch platform-managed `tenant-<org>`
                // namespaces to know where to project cloud-api's RoleBinding.
                "apiGroups": [""],
                "resources": ["namespaces"],
                "verbs": ["get", "list", "watch"]
            },
            {
                // Tenant-RBAC controller: create the namespaced cloud-api-platform
                // RoleBinding in each tenant namespace.
                "apiGroups": ["rbac.authorization.k8s.io"],
                "resources": ["rolebindings"],
                "verbs": ["get", "list", "watch", "create", "update", "patch", "delete"]
            },
            {
                // RBAC escalation guard: writing a RoleBinding that references a
                // ClusterRole requires `bind` on it. Granted narrowly — ONLY the
                // tenant ClusterRole — so the operator can never bind a broader one.
                "apiGroups": ["rbac.authorization.k8s.io"],
                "resources": ["clusterroles"],
                "resourceNames": [TENANT_CLUSTER_ROLE],
                "verbs": ["bind"]
            },
            {
                "apiGroups": ["apps"],
                "resources": ["deployments", "statefulsets", "replicasets"],
                "verbs": ["get", "list", "watch", "create", "update", "patch", "delete"]
            },
            {
                "apiGroups": ["networking.k8s.io"],
                "resources": ["ingresses", "networkpolicies"],
                "verbs": ["get", "list", "watch", "create", "update", "patch", "delete"]
            },
            {
                "apiGroups": ["autoscaling"],
                "resources": ["horizontalpodautoscalers"],
                "verbs": ["get", "list", "watch", "create", "update", "patch", "delete"]
            },
            {
                "apiGroups": ["policy"],
                "resources": ["poddisruptionbudgets"],
                "verbs": ["get", "list", "watch", "create", "update", "patch", "delete"]
            },
            {
                "apiGroups": ["batch"],
                "resources": ["jobs", "cronjobs"],
                "verbs": ["get", "list", "watch", "create", "update", "patch", "delete"]
            },
            {
                "apiGroups": ["coordination.k8s.io"],
                "resources": ["leases"],
                "verbs": ["get", "list", "watch", "create", "update", "patch", "delete"]
            },
            {
                "apiGroups": ["apiextensions.k8s.io"],
                "resources": ["customresourcedefinitions"],
                "verbs": ["get", "list", "watch", "create", "update", "patch"]
            },
            {
                // The operator's own CRD groups — full authority incl. /status.
                "apiGroups": [cfg.api_group, cfg.kms_group()],
                "resources": ["*"],
                "verbs": ["get", "list", "watch", "create", "update", "patch", "delete"]
            }
        ],
    }))
}

/// The scoped `/v1/platform` grant cloud-api is bound to per tenant
/// (`hanzo-cloud-platform-tenant`). The tenant-RBAC controller creates a
/// NAMESPACED RoleBinding to this ClusterRole in each tenant namespace — never a
/// ClusterRoleBinding: `services.<group>` CRs live in every tenant namespace, so
/// a cluster-wide bind would be a cross-tenant deploy hole. Mirrors the canonical
/// universe `operator/rbac/cloud-platform-rbac.yaml`; the operator's own Kinds
/// follow `cfg.api_group`, while KMSSecret stays on the kms-operator's own group.
pub fn tenant_cluster_role(cfg: &InstallConfig) -> ClusterRole {
    from_static(json!({
        "apiVersion": "rbac.authorization.k8s.io/v1",
        "kind": "ClusterRole",
        "metadata": { "name": TENANT_CLUSTER_ROLE, "labels": labels() },
        "rules": [
            // The application Service CR cloud-api deploys per app.
            { "apiGroups": [cfg.api_group], "resources": ["services"],
              "verbs": ["get", "create", "patch", "delete"] },
            // Per-tenant bounds cloud-api sets — waitForTenantRBAC polls
            // `get resourcequotas` as its readiness probe.
            { "apiGroups": [""], "resources": ["resourcequotas", "limitranges"],
              "verbs": ["get", "create", "patch"] },
            // Dedicated per-tenant database instances (clients/provisioning/dedicated.go).
            { "apiGroups": [cfg.api_group], "resources": ["datastores", "docdbs", "manageddatabases"],
              "verbs": ["get", "list", "create", "patch", "delete"] },
            // The per-instance DB admin Secret cloud-api generates + reaps on drop.
            { "apiGroups": [""], "resources": ["secrets"], "verbs": ["get", "create", "delete"] },
            // The dedicated instance's retained data volume, reaped on drop.
            { "apiGroups": [""], "resources": ["persistentvolumeclaims"], "verbs": ["get", "delete"] },
            // KMS-sealed app secret env — the kms-operator's KMSSecret CRD group.
            { "apiGroups": ["secrets.lux.network"], "resources": ["kmssecrets"],
              "verbs": ["get", "create", "patch", "delete"] }
        ],
    }))
}

/// Bind the ClusterRole to the operator ServiceAccount.
pub fn cluster_role_binding(cfg: &InstallConfig) -> ClusterRoleBinding {
    from_static(json!({
        "apiVersion": "rbac.authorization.k8s.io/v1",
        "kind": "ClusterRoleBinding",
        "metadata": { "name": OPERATOR_NAME, "labels": labels() },
        "roleRef": {
            "apiGroup": "rbac.authorization.k8s.io",
            "kind": "ClusterRole",
            "name": OPERATOR_NAME,
        },
        "subjects": [{
            "kind": "ServiceAccount",
            "name": OPERATOR_NAME,
            "namespace": cfg.operator_namespace,
        }],
    }))
}

/// The operator Deployment — one replica (lease-elected), non-root, health
/// probes on 8081, config driven by env, command = the supervised entrypoint.
pub fn deployment(cfg: &InstallConfig) -> Deployment {
    from_static(json!({
        "apiVersion": "apps/v1",
        "kind": "Deployment",
        "metadata": {
            "name": OPERATOR_NAME,
            "namespace": cfg.operator_namespace,
            "labels": labels(),
        },
        "spec": {
            "replicas": 1,
            "selector": { "matchLabels": { "app": OPERATOR_NAME } },
            "template": {
                "metadata": { "labels": labels() },
                "spec": {
                    "serviceAccountName": OPERATOR_NAME,
                    "securityContext": { "runAsNonRoot": true },
                    "containers": [{
                        "name": "operator",
                        "image": cfg.image,
                        "command": cfg.command,
                        "env": [
                            { "name": "OPERATOR_API_GROUP", "value": cfg.api_group },
                            { "name": "WATCH_NAMESPACE", "value": cfg.watch_namespace },
                            { "name": "OPERATOR_NAMESPACE", "value": cfg.operator_namespace },
                            { "name": "LEADER_ELECT", "value": "true" }
                        ],
                        "ports": [{ "name": "health", "containerPort": 8081 }],
                        "livenessProbe": {
                            "httpGet": { "path": "/healthz", "port": 8081 },
                            "initialDelaySeconds": 10, "periodSeconds": 20
                        },
                        "readinessProbe": {
                            "httpGet": { "path": "/readyz", "port": 8081 },
                            "initialDelaySeconds": 5, "periodSeconds": 10
                        },
                        "securityContext": {
                            "allowPrivilegeEscalation": false,
                            "readOnlyRootFilesystem": true,
                            "runAsNonRoot": true,
                            "runAsUser": 65532,
                            "capabilities": { "drop": ["ALL"] }
                        }
                    }]
                }
            }
        }
    }))
}

/// Render every install object as a multi-document YAML stream — the
/// `--dry-run` output, and pipeable to `kubectl apply -f -`.
pub fn render(cfg: &InstallConfig) -> anyhow::Result<String> {
    let mut out = String::new();
    if cfg.apply_crds {
        for crd in crds(cfg) {
            out.push_str("---\n");
            out.push_str(&serde_yaml::to_string(&crd)?);
        }
    }
    out.push_str("---\n");
    out.push_str(&serde_yaml::to_string(&namespace(cfg))?);
    out.push_str("---\n");
    out.push_str(&serde_yaml::to_string(&service_account(cfg))?);
    out.push_str("---\n");
    out.push_str(&serde_yaml::to_string(&cluster_role(cfg))?);
    out.push_str("---\n");
    out.push_str(&serde_yaml::to_string(&cluster_role_binding(cfg))?);
    out.push_str("---\n");
    out.push_str(&serde_yaml::to_string(&tenant_cluster_role(cfg))?);
    out.push_str("---\n");
    out.push_str(&serde_yaml::to_string(&deployment(cfg))?);
    Ok(out)
}

/// Apply the install objects into the connected cluster via server-side apply.
/// CRDs + ClusterRole/Binding + Namespace are cluster-scoped; SA + Deployment
/// are namespaced. Reuses [`crate::apply::apply`] so install and reconcile
/// share one SSA path.
pub async fn install(cfg: InstallConfig) -> anyhow::Result<()> {
    let client = kube::Client::try_default().await?;

    if cfg.apply_crds {
        let crd_api: Api<CustomResourceDefinition> = Api::all(client.clone());
        for crd in crds(&cfg) {
            let name = crd.metadata.name.clone().unwrap_or_default();
            apply(&crd_api, &crd)
                .await
                .map_err(|e| anyhow::anyhow!("apply CRD {name}: {e}"))?;
        }
    }

    apply(&Api::<Namespace>::all(client.clone()), &namespace(&cfg))
        .await
        .map_err(|e| anyhow::anyhow!("apply Namespace: {e}"))?;
    apply(
        &Api::<ServiceAccount>::namespaced(client.clone(), &cfg.operator_namespace),
        &service_account(&cfg),
    )
    .await
    .map_err(|e| anyhow::anyhow!("apply ServiceAccount: {e}"))?;
    apply(&Api::<ClusterRole>::all(client.clone()), &cluster_role(&cfg))
        .await
        .map_err(|e| anyhow::anyhow!("apply ClusterRole: {e}"))?;
    apply(
        &Api::<ClusterRole>::all(client.clone()),
        &tenant_cluster_role(&cfg),
    )
    .await
    .map_err(|e| anyhow::anyhow!("apply tenant ClusterRole: {e}"))?;
    apply(
        &Api::<ClusterRoleBinding>::all(client.clone()),
        &cluster_role_binding(&cfg),
    )
    .await
    .map_err(|e| anyhow::anyhow!("apply ClusterRoleBinding: {e}"))?;
    apply(
        &Api::<Deployment>::namespaced(client.clone(), &cfg.operator_namespace),
        &deployment(&cfg),
    )
    .await
    .map_err(|e| anyhow::anyhow!("apply Deployment: {e}"))?;

    Ok(())
}

/// Standard labels stamped on every install object.
fn labels() -> Value {
    json!({
        "app.kubernetes.io/name": OPERATOR_NAME,
        "app.kubernetes.io/managed-by": OPERATOR_NAME,
    })
}

/// Deserialize a static, hand-written manifest into its typed object. The input
/// is a literal in this file, so a failure is a programming error a unit test
/// catches (`deployment`/`cluster_role` tests deserialize every object) — never
/// runtime input. `expect` here is a compile-of-thought assertion, not I/O.
fn from_static<T: serde::de::DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).expect("static install manifest must be a valid typed object")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> InstallConfig {
        InstallConfig {
            api_group: "hanzo.ai".to_string(),
            image: "ghcr.io/hanzoai/node:v1.2.3".to_string(),
            operator_namespace: "hanzo-operator-system".to_string(),
            watch_namespace: String::new(),
            command: vec!["hanzod".to_string(), "operator".to_string()],
            apply_crds: true,
        }
    }

    #[test]
    fn all_objects_deserialize_into_their_typed_form() {
        // `from_static` .expect() would panic on any malformed literal; calling
        // every builder proves the manifests are valid typed objects.
        let c = cfg();
        let _ = namespace(&c);
        let _ = service_account(&c);
        let _ = cluster_role(&c);
        let _ = tenant_cluster_role(&c);
        let _ = cluster_role_binding(&c);
        let _ = deployment(&c);
    }

    #[test]
    fn operator_role_can_manage_tenant_rbac() {
        let rules = cluster_role(&cfg()).rules.unwrap();
        // namespaces get/list/watch (the tenant-namespace watch source).
        let ns = rules
            .iter()
            .find(|r| r.resources.as_ref().unwrap().contains(&"namespaces".to_string()))
            .expect("namespaces rule");
        assert!(ns.verbs.contains(&"watch".to_string()));
        // rolebindings create (project the per-tenant cloud-api-platform binding).
        let rb = rules
            .iter()
            .find(|r| r.resources.as_ref().unwrap().contains(&"rolebindings".to_string()))
            .expect("rolebindings rule");
        assert!(rb.verbs.contains(&"create".to_string()));
        // clusterroles `bind` scoped by resourceName to the tenant ClusterRole only.
        let bind = rules
            .iter()
            .find(|r| r.verbs.contains(&"bind".to_string()))
            .expect("bind rule");
        assert_eq!(
            bind.resource_names.as_ref().unwrap(),
            &vec![TENANT_CLUSTER_ROLE.to_string()]
        );
    }

    #[test]
    fn tenant_cluster_role_grants_the_platform_deploy_surface() {
        let cr = tenant_cluster_role(&cfg());
        assert_eq!(cr.metadata.name.as_deref(), Some(TENANT_CLUSTER_ROLE));
        let rules = cr.rules.unwrap();
        // resourcequotas get — the exact op waitForTenantRBAC probes.
        let rq = rules
            .iter()
            .find(|r| r.resources.as_ref().unwrap().contains(&"resourcequotas".to_string()))
            .expect("resourcequotas rule");
        assert!(rq.verbs.contains(&"get".to_string()));
        // services under the operator's own api group (the app Service CR).
        let svc = rules
            .iter()
            .find(|r| {
                r.api_groups.as_ref().unwrap().contains(&"hanzo.ai".to_string())
                    && r.resources.as_ref().unwrap().contains(&"services".to_string())
            })
            .expect("services.hanzo.ai rule");
        assert!(svc.verbs.contains(&"create".to_string()));
    }

    #[test]
    fn deployment_carries_image_command_and_api_group_env() {
        let d = deployment(&cfg());
        let pod = d.spec.unwrap().template.spec.unwrap();
        let c = &pod.containers[0];
        assert_eq!(c.image.as_deref(), Some("ghcr.io/hanzoai/node:v1.2.3"));
        // Supervised entrypoint — `hanzod operator`.
        assert_eq!(
            c.command.as_ref().unwrap(),
            &vec!["hanzod".to_string(), "operator".to_string()]
        );
        assert_eq!(pod.service_account_name.as_deref(), Some(OPERATOR_NAME));
        // OPERATOR_API_GROUP is wired so the running operator honors the group.
        let env = c.env.as_ref().unwrap();
        let ag = env
            .iter()
            .find(|e| e.name == "OPERATOR_API_GROUP")
            .expect("OPERATOR_API_GROUP env");
        assert_eq!(ag.value.as_deref(), Some("hanzo.ai"));
    }

    #[test]
    fn cluster_role_covers_workloads_and_own_crd_group() {
        let cr = cluster_role(&cfg());
        let rules = cr.rules.unwrap();
        // apps/deployments is present with patch (adoption is load-bearing).
        let apps = rules
            .iter()
            .find(|r| r.api_groups.as_ref().unwrap().contains(&"apps".to_string()))
            .expect("apps rule");
        assert!(apps.resources.as_ref().unwrap().contains(&"deployments".to_string()));
        assert!(apps.verbs.contains(&"patch".to_string()));
        // The operator's own group + its kms.<group> get full authority.
        let own = rules
            .iter()
            .find(|r| {
                let g = r.api_groups.as_ref().unwrap();
                g.contains(&"hanzo.ai".to_string()) && g.contains(&"kms.hanzo.ai".to_string())
            })
            .expect("own CRD-group rule");
        assert!(own.resources.as_ref().unwrap().contains(&"*".to_string()));
    }

    #[test]
    fn render_is_multidoc_and_includes_crds_when_enabled() {
        let out = render(&cfg()).unwrap();
        // CRDs + 6 core objects, each with a `---` doc separator.
        assert!(out.contains("kind: CustomResourceDefinition"));
        assert!(out.contains("kind: Deployment"));
        assert!(out.contains("kind: ClusterRoleBinding"));
        // The scoped tenant ClusterRole cloud-api is bound to per tenant.
        assert!(out.contains("hanzo-cloud-platform-tenant"));
        assert!(out.matches("---\n").count() >= 7);
        // 28 managed CRDs land in the stream.
        assert_eq!(crds(&cfg()).len(), 28);
    }

    #[test]
    fn skipping_crds_renders_only_the_workload() {
        let mut c = cfg();
        c.apply_crds = false;
        let out = render(&c).unwrap();
        assert!(!out.contains("kind: CustomResourceDefinition"));
        assert!(out.contains("kind: Deployment"));
    }

    #[test]
    fn kms_group_follows_api_group() {
        let mut c = cfg();
        assert_eq!(c.kms_group(), "kms.hanzo.ai");
        c.api_group = "lux.cloud".to_string();
        assert_eq!(c.kms_group(), "kms.lux.cloud");
    }
}
