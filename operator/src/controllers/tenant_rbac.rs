//! Tenant-RBAC reconciler — onboards each platform-managed tenant namespace so
//! cloud-api can deploy into it.
//!
//! Cloud's deploy path BLOCKS (`waitForTenantRBAC` — a SelfSubjectAccessReview
//! poll) until this controller has projected, into every `tenant-<org>`
//! namespace (labelled `hanzo.ai/managed-by=platform`):
//!
//!   1. a namespaced RoleBinding `cloud-api-platform` → ClusterRole
//!      `hanzo-cloud-platform-tenant`, bound to the `cloud-api` ServiceAccount,
//!      so cloud-api may act (resourcequotas / limitranges / services.hanzo.ai)
//!      inside that ONE tenant — never cluster-wide (a ClusterRoleBinding would
//!      turn the per-tenant mount into a cross-tenant deploy hole), and
//!   2. a `ghcr-pull` dockerconfigjson image-pull Secret so tenant pods can pull
//!      the private per-tenant build image (`ghcr.io/hanzoai/tenant-<org>/…`).
//!      cloud-api holds NO `secrets` grant; the operator is the designated
//!      K8s-secret handler (KMS-only secrets model), copying the payload from
//!      the operator namespace's own `ghcr-pull` Secret.
//!
//! The namespace's creation is exactly the trigger: the controller watches
//! Namespaces filtered to `hanzo.ai/managed-by=platform`, so a fresh tenant is
//! onboarded the moment cloud creates its namespace. Both projected objects are
//! owner-referenced to the Namespace and labelled `hanzo.ai/managed-by=platform`
//! so they GC with the tenant and are identifiable as platform-managed. Every
//! write is an idempotent SSA round through the shared [`crate::apply::apply`].

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use k8s_openapi::api::core::v1::{Namespace, Secret};
use k8s_openapi::api::rbac::v1::{RoleBinding, RoleRef, Subject};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, OwnerReference};
use k8s_openapi::ByteString;
use kube::api::Api;
use kube::runtime::controller::{Action, Controller};
use kube::runtime::watcher::Config;
use kube::{Client, ResourceExt};
use tracing::{error, info, warn};

use crate::apply::apply;
use crate::core::{OperatorError, Result};

use super::owner_ref_for;

/// The cloud-api identity the per-tenant RoleBinding authorizes (cloud-api's
/// ServiceAccount lives in the platform namespace, never in the tenant's).
const CLOUD_API_SA_NAME: &str = "cloud-api";
const CLOUD_API_SA_NAMESPACE: &str = "hanzo";

/// The per-tenant RoleBinding and the ClusterRole it references. The RoleBinding
/// name is the one cloud's `waitForTenantRBAC` waits on; the ClusterRole is the
/// scoped `/v1/platform` grant (`hanzo-cloud-platform-tenant`).
const ROLEBINDING_NAME: &str = "cloud-api-platform";
/// The scoped `/v1/platform` ClusterRole cloud-api is bound to per tenant. Named
/// once here; `install.rs` renders the ClusterRole itself, the controller binds
/// to it — one source for the name.
pub(crate) const TENANT_CLUSTER_ROLE: &str = "hanzo-cloud-platform-tenant";

/// The image-pull Secret every tenant namespace needs. The name matches the one
/// the Service CR references (cloud `tenantPullSecretName`) and the one this
/// controller reads from the operator namespace as its source — one name, one
/// dockerconfigjson key, throughout.
const PULL_SECRET_NAME: &str = "ghcr-pull";
const DOCKERCONFIG_KEY: &str = ".dockerconfigjson";
const DOCKERCONFIG_TYPE: &str = "kubernetes.io/dockerconfigjson";

/// The label cloud stamps on a platform-managed tenant namespace — the watch
/// filter AND the marker this controller re-stamps on every child for GC.
const MANAGED_BY_KEY: &str = "hanzo.ai/managed-by";
const MANAGED_BY_VALUE: &str = "platform";

/// Tenant namespaces are named `tenant-<org>`; the defensive name guard keeps a
/// mis-labelled namespace from ever being written into.
const TENANT_NS_PREFIX: &str = "tenant-";

#[derive(Clone)]
pub struct Ctx {
    pub client: Client,
    /// The operator's own namespace — the source of the `ghcr-pull` Secret.
    pub operator_namespace: String,
}

pub async fn reconcile(ns: Arc<Namespace>, ctx: Arc<Ctx>) -> Result<Action> {
    let name = ns.name_any();

    // Defensive: only onboard `tenant-<org>` namespaces even though the watch is
    // already label-filtered — never write into a mis-labelled namespace.
    if !name.starts_with(TENANT_NS_PREFIX) {
        return Ok(Action::await_change());
    }
    // A terminating namespace rejects writes and needs none — its children GC
    // with it. Skip until it is gone (the watch drops it, no requeue needed).
    if ns.metadata.deletion_timestamp.is_some() {
        return Ok(Action::await_change());
    }

    let owner = owner_ref_for(ns.as_ref(), "v1", "Namespace");

    // 1. The deploy-authorization RoleBinding — what cloud's readiness gate polls.
    let rb = tenant_role_binding(&name, owner.clone());
    apply(&Api::<RoleBinding>::namespaced(ctx.client.clone(), &name), &rb).await?;

    // 2. The image-pull Secret, copied from the operator namespace's source. A
    // missing source is logged, not fatal: the RoleBinding still lands (the
    // deploy gate opens) and the Secret converges once the source exists.
    ensure_pull_secret(&ctx.client, &ctx.operator_namespace, &name, owner).await?;

    info!(
        namespace = %name,
        "Tenant onboarded (cloud-api-platform RoleBinding + ghcr-pull Secret)"
    );
    // Re-assert periodically so a rotated source dockerconfig or an out-of-band
    // edit reverts within the window.
    Ok(Action::requeue(Duration::from_secs(300)))
}

/// Copy the operator namespace's `ghcr-pull` dockerconfigjson into `tenant_ns`.
/// A missing source Secret (or missing key) is a warning, not an error, so it
/// never blocks the RoleBinding that gates the deploy.
async fn ensure_pull_secret(
    client: &Client,
    operator_ns: &str,
    tenant_ns: &str,
    owner: OwnerReference,
) -> Result<()> {
    let src_api: Api<Secret> = Api::namespaced(client.clone(), operator_ns);
    let Some(src) = src_api.get_opt(PULL_SECRET_NAME).await? else {
        warn!(
            source_namespace = %operator_ns,
            secret = PULL_SECRET_NAME,
            "source image-pull Secret absent; tenant pods cannot pull private images until it exists"
        );
        return Ok(());
    };
    let Some(dockercfg) = src.data.as_ref().and_then(|d| d.get(DOCKERCONFIG_KEY)).cloned() else {
        warn!(
            source_namespace = %operator_ns,
            secret = PULL_SECRET_NAME,
            key = DOCKERCONFIG_KEY,
            "source image-pull Secret has no dockerconfigjson; skipping tenant copy"
        );
        return Ok(());
    };
    let secret = tenant_pull_secret(tenant_ns, dockercfg, owner);
    apply(&Api::<Secret>::namespaced(client.clone(), tenant_ns), &secret).await?;
    Ok(())
}

/// The `cloud-api-platform` RoleBinding → ClusterRole `hanzo-cloud-platform-tenant`
/// for one tenant namespace. Pure so it is unit-testable without a cluster.
fn tenant_role_binding(ns: &str, owner: OwnerReference) -> RoleBinding {
    RoleBinding {
        metadata: managed_meta(ROLEBINDING_NAME, ns, owner),
        role_ref: RoleRef {
            api_group: "rbac.authorization.k8s.io".to_string(),
            kind: "ClusterRole".to_string(),
            name: TENANT_CLUSTER_ROLE.to_string(),
        },
        subjects: Some(vec![Subject {
            api_group: None, // core-group ServiceAccount
            kind: "ServiceAccount".to_string(),
            name: CLOUD_API_SA_NAME.to_string(),
            namespace: Some(CLOUD_API_SA_NAMESPACE.to_string()),
        }]),
    }
}

/// The `ghcr-pull` dockerconfigjson Secret for one tenant namespace. Pure.
fn tenant_pull_secret(ns: &str, dockerconfigjson: ByteString, owner: OwnerReference) -> Secret {
    Secret {
        metadata: managed_meta(PULL_SECRET_NAME, ns, owner),
        type_: Some(DOCKERCONFIG_TYPE.to_string()),
        data: Some(BTreeMap::from([(DOCKERCONFIG_KEY.to_string(), dockerconfigjson)])),
        ..Default::default()
    }
}

/// Namespaced metadata carrying the platform-managed label + the Namespace owner
/// reference — the single ObjectMeta builder both projected children share.
fn managed_meta(name: &str, ns: &str, owner: OwnerReference) -> ObjectMeta {
    ObjectMeta {
        name: Some(name.to_string()),
        namespace: Some(ns.to_string()),
        labels: Some(BTreeMap::from([(
            MANAGED_BY_KEY.to_string(),
            MANAGED_BY_VALUE.to_string(),
        )])),
        owner_references: Some(vec![owner]),
        ..Default::default()
    }
}

pub fn on_error(_obj: Arc<Namespace>, err: &OperatorError, _ctx: Arc<Ctx>) -> Action {
    error!(error = %err, "Tenant-RBAC reconcile failed");
    Action::requeue(Duration::from_secs(30))
}

/// Watch platform-managed tenant namespaces and project cloud-api's deploy RBAC
/// plus the ghcr-pull Secret into each. Namespaces are cluster-scoped, so the
/// watch is [`Api::all`] narrowed by the `hanzo.ai/managed-by=platform` label.
pub async fn run_tenant_rbac_controller(client: Client, operator_namespace: String) {
    let api: Api<Namespace> = Api::all(client.clone());
    let cfg = Config::default().labels(&format!("{MANAGED_BY_KEY}={MANAGED_BY_VALUE}"));
    info!("Starting Tenant-RBAC controller");
    let ctx = Arc::new(Ctx {
        client,
        operator_namespace,
    });
    Controller::new(api, cfg)
        .run(reconcile, on_error, ctx)
        .for_each(|res| async move {
            if let Err(e) = res {
                warn!(error = %e, "Tenant-RBAC reconcile error");
            }
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner() -> OwnerReference {
        OwnerReference {
            api_version: "v1".to_string(),
            kind: "Namespace".to_string(),
            name: "tenant-acme".to_string(),
            uid: "uid-1".to_string(),
            controller: Some(true),
            block_owner_deletion: Some(true),
        }
    }

    #[test]
    fn rolebinding_binds_cloud_api_to_the_tenant_cluster_role() {
        let rb = tenant_role_binding("tenant-acme", owner());
        assert_eq!(rb.metadata.name.as_deref(), Some("cloud-api-platform"));
        assert_eq!(rb.metadata.namespace.as_deref(), Some("tenant-acme"));
        // roleRef → the scoped tenant ClusterRole (never a cluster-wide binding).
        assert_eq!(rb.role_ref.kind, "ClusterRole");
        assert_eq!(rb.role_ref.name, "hanzo-cloud-platform-tenant");
        // Subject is cloud-api's SA in the platform namespace, not the tenant's.
        let s = &rb.subjects.as_ref().unwrap()[0];
        assert_eq!(s.kind, "ServiceAccount");
        assert_eq!(s.name, "cloud-api");
        assert_eq!(s.namespace.as_deref(), Some("hanzo"));
    }

    #[test]
    fn pull_secret_is_a_labelled_owned_dockerconfig() {
        let s = tenant_pull_secret("tenant-acme", ByteString(b"{}".to_vec()), owner());
        assert_eq!(s.metadata.name.as_deref(), Some("ghcr-pull"));
        assert_eq!(s.metadata.namespace.as_deref(), Some("tenant-acme"));
        assert_eq!(s.type_.as_deref(), Some("kubernetes.io/dockerconfigjson"));
        assert_eq!(
            s.data.as_ref().unwrap().get(".dockerconfigjson"),
            Some(&ByteString(b"{}".to_vec()))
        );
    }

    #[test]
    fn children_are_gc_labelled_and_namespace_owned() {
        // Both projected children carry the platform GC label + the Namespace
        // owner reference, so they reap with the tenant.
        for meta in [
            tenant_role_binding("tenant-acme", owner()).metadata,
            tenant_pull_secret("tenant-acme", ByteString(vec![]), owner()).metadata,
        ] {
            assert_eq!(
                meta.labels.as_ref().unwrap().get("hanzo.ai/managed-by"),
                Some(&"platform".to_string())
            );
            let or = &meta.owner_references.as_ref().unwrap()[0];
            assert_eq!(or.kind, "Namespace");
            assert_eq!(or.name, "tenant-acme");
        }
    }
}
