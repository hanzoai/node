//! Idempotent server-side apply for managed K8s objects.
//!
//! Wraps `Api::patch` with `PatchParams::apply("hanzo-operator")` so each
//! reconcile is an SSA round — the operator owns its declared fields, and
//! anything edited out-of-band reverts on next loop.

use k8s_openapi::api::core::v1::ConfigMap;
use kube::api::{Api, DeleteParams, Patch, PatchParams};
use kube::core::DynamicObject;
use kube::Resource;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fmt::Debug;

use crate::core::{OperatorError, Result};

pub const FIELD_MANAGER: &str = "hanzo-operator";

/// Server-side apply a typed K8s object. Returns the live resource after
/// the apply. `obj` MUST have `metadata.name` set.
pub async fn apply<K>(api: &Api<K>, obj: &K) -> Result<K>
where
    K: Resource<DynamicType = ()> + Serialize + DeserializeOwned + Clone + Debug,
{
    let name =
        obj.meta().name.clone().ok_or_else(|| {
            crate::core::OperatorError::Config("apply: missing metadata.name".into())
        })?;
    let pp = PatchParams::apply(FIELD_MANAGER).force();
    let out = api.patch(&name, &pp, &Patch::Apply(obj)).await?;
    Ok(out)
}

/// Server-side apply a ConfigMap, refusing to clobber a populated ConfigMap
/// with empty data.
///
/// A ConfigMap whose `data` AND `binaryData` are both empty carries no config.
/// Force-applying it strips every key the `hanzo-operator` field manager owns,
/// blanking the workload's mounted config file → CrashLoopBackOff (the hanzo.id
/// 30-min auth outage: an empty CR config source regenerated `iam-conf` empty →
/// `panic: unable to open database file`; likewise `otel-collector-config`). The
/// operator NEVER emits an empty ConfigMap: we skip the apply and leave any
/// existing content untouched. Skipping is also correct on first create — an
/// empty ConfigMap has no legitimate use. Returns `true` when applied, `false`
/// when skipped.
pub async fn apply_configmap(api: &Api<ConfigMap>, cm: &ConfigMap) -> Result<bool> {
    let name = cm.metadata.name.clone().ok_or_else(|| {
        crate::core::OperatorError::Config("apply_configmap: missing metadata.name".into())
    })?;
    if crate::manifests::configmap_is_empty(cm) {
        tracing::warn!(
            configmap = %name,
            namespace = cm.metadata.namespace.as_deref().unwrap_or_default(),
            "refusing to apply empty-data ConfigMap; leaving any existing content untouched"
        );
        return Ok(false);
    }
    apply(api, cm).await?;
    Ok(true)
}

/// Server-side apply, escalating to delete+recreate when the LIVE child holds a
/// structural conflict that SSA-merge cannot reconcile.
///
/// While adopting objects a raw manifest / ArgoCD created, the apply is
/// rejected with HTTP 422 (`Invalid`) in two situations:
///   * a stale field SSA cannot clear — a server-defaulted `emptyDir` left over
///     from a past source-less volume, or a duplicate probe handler — surfaces
///     as `may not specify more than 1 {volume,handler} type`;
///   * an immutable field must change — `field is immutable` / `updates to
///     statefulset spec ... are forbidden`.
///
/// The operator's DESIRED object is always structurally valid (codegen emits
/// exactly one source per volume and one handler per probe), so this error
/// class is a property of the LIVE object, not the desired one — deleting and
/// recreating from the desired spec is the deterministic, non-flapping cure.
///
/// Guards (so it never flaps and never destroys a healthy object for a
/// desired-side bug):
///   * only recreate an object that already exists (an update). A first CREATE
///     that is Invalid is a genuine spec bug — surface it.
///   * a recreate whose CREATE is itself rejected returns the error (no loop);
///     the next reconcile re-applies and, finding the object gone, re-creates.
///   * a successful recreate leaves the child matching desired, so the next
///     apply is a clean no-op.
///
/// Intended for stateless / standalone-PVC children (Deployments). NOT used for
/// StatefulSets, whose delete would strand `volumeClaimTemplate` PVCs (data
/// loss); those adopt via a matching `storage.volumeName` instead.
pub async fn apply_or_recreate<K>(api: &Api<K>, obj: &K) -> Result<K>
where
    K: Resource<DynamicType = ()> + Serialize + DeserializeOwned + Clone + Debug,
{
    let name =
        obj.meta().name.clone().ok_or_else(|| {
            OperatorError::Config("apply_or_recreate: missing metadata.name".into())
        })?;
    let pp = PatchParams::apply(FIELD_MANAGER).force();
    match api.patch(&name, &pp, &Patch::Apply(obj)).await {
        Ok(out) => Ok(out),
        Err(e) => {
            let err = OperatorError::from(e);
            if !is_structural_conflict(&err) {
                return Err(err);
            }
            // Only recreate an existing object (the update path). A first-create
            // Invalid is a real spec bug and must surface, not delete anything.
            if api.get_opt(&name).await?.is_none() {
                return Err(err);
            }
            tracing::warn!(
                object = %name,
                error = %err,
                "apply rejected as Invalid (structural conflict on live object); deleting and recreating from desired spec"
            );
            // Background delete retains any standalone PVC the pod mounts.
            api.delete(&name, &DeleteParams::default()).await?;
            wait_until_gone(api, &name).await?;
            let out = api.patch(&name, &pp, &Patch::Apply(obj)).await?;
            Ok(out)
        }
    }
}

/// True for the 422/`Invalid` responses that a fresh recreate cures: a stale
/// duplicate source/handler SSA cannot remove, or an immutable-field change.
/// Pure over the error so it is unit-testable without a cluster.
pub(crate) fn is_structural_conflict(err: &OperatorError) -> bool {
    let OperatorError::KubeApi(kube::Error::Api(resp)) = err else {
        return false;
    };
    if resp.code != 422 {
        return false;
    }
    let m = resp.message.as_str();
    m.contains("may not specify more than 1")
        || m.contains("field is immutable")
        || m.contains("updates to statefulset spec")
}

/// Poll until the named object is gone (deletion finalized), bounded to ~30s.
/// Falls through on timeout — the subsequent apply surfaces any real problem.
async fn wait_until_gone<K>(api: &Api<K>, name: &str) -> Result<()>
where
    K: Resource<DynamicType = ()> + Serialize + DeserializeOwned + Clone + Debug,
{
    for _ in 0..60 {
        if api.get_opt(name).await?.is_none() {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    Ok(())
}

/// Apply a DynamicObject (used for CRDs whose types are not statically known,
/// like KMSSecret). `api` carries the ApiResource on its DynamicType.
pub async fn apply_dynamic(api: &Api<DynamicObject>, obj: &DynamicObject) -> Result<DynamicObject> {
    let name = obj.metadata.name.clone().ok_or_else(|| {
        crate::core::OperatorError::Config("apply_dynamic: missing metadata.name".into())
    })?;
    let pp = PatchParams::apply(FIELD_MANAGER).force();
    let out = api.patch(&name, &pp, &Patch::Apply(obj)).await?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kube::core::Status;

    fn api_err(code: u16, message: &str) -> OperatorError {
        OperatorError::KubeApi(kube::Error::Api(Box::new(Status {
            code,
            message: message.into(),
            reason: "Invalid".into(),
            ..Default::default()
        })))
    }

    // A stale duplicate volume source (server-defaulted emptyDir beside our PVC)
    // is a live-side conflict a fresh recreate cures → recreate.
    #[test]
    fn duplicate_volume_type_is_structural_conflict() {
        assert!(is_structural_conflict(&api_err(
            422,
            "Deployment.apps \"search-fts5\" is invalid: spec.template.spec.volumes[0].persistentVolumeClaim: Forbidden: may not specify more than 1 volume type"
        )));
    }

    // An immutable StatefulSet spec change is recreate-fixable too.
    #[test]
    fn immutable_statefulset_is_structural_conflict() {
        assert!(is_structural_conflict(&api_err(
            422,
            "StatefulSet.apps \"sql\" is invalid: spec: Forbidden: updates to statefulset spec for fields other than 'replicas' ... are forbidden"
        )));
    }

    // A desired-side bug with NO duplicate/immutable signal (e.g. a bare missing
    // volume) must NOT trigger a destructive delete — surface the error instead.
    #[test]
    fn plain_not_found_is_not_a_structural_conflict() {
        assert!(!is_structural_conflict(&api_err(
            422,
            "Deployment.apps \"x\" is invalid: spec.template.spec.containers[0].volumeMounts[0].name: Not found: \"data\""
        )));
    }

    // Only 422/Invalid qualifies — a 409 conflict or 404 is retried/surfaced,
    // never recreated.
    #[test]
    fn non_422_is_not_a_structural_conflict() {
        assert!(!is_structural_conflict(&api_err(
            409,
            "Operation cannot be fulfilled: the object has been modified"
        )));
        assert!(!is_structural_conflict(&OperatorError::Config("x".into())));
    }
}
