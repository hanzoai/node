//! Additive ZAP-native KMSSecret reconciler.
//!
//! Projects KMS secrets into k8s Secrets over the ZAP binary protocol
//! (`crate::zapclient`) for `kms.hanzo.ai/v1alpha1 KMSSecret` CRs explicitly
//! marked ZAP-native (`spec.transport == "zap"`). Purely ADDITIVE and OPT-IN
//! (`KMS_ZAP_CONTROLLER=true`): CRs without that marker are IGNORED, so the
//! legacy REST projector remains the only secret path until cutover.
//!
//! One CRD family — the same `kms.hanzo.ai/v1alpha1 KMSSecret` the operator
//! already writes (`controllers::service::reconcile_kms_secret`), watched
//! group-erased as a DynamicObject so it works under every universe api-group.
//!
//! Fail-closed: any ZAP fetch error → no Secret write (never a plaintext
//! default). Reuses the canonical guards in `core::secret` (strict
//! hijack-protection + control-byte rejection).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::Api;
use kube::core::{ApiResource, DynamicObject, GroupVersionKind};
use kube::runtime::controller::{Action, Controller};
use kube::runtime::watcher::Config;
use kube::Client;
use serde::Deserialize;
use tracing::{info, warn};

use crate::apply;
use crate::core::secret::{
    is_operator_managed, owner_ref, validate_secret_value, MANAGED_BY_LABEL,
};
use crate::zapclient::ZapClient;

/// managed-by value for Secrets this controller owns — distinct from the REST
/// projector's so the two never adopt each other's Secrets.
pub const KMS_ZAP_MANAGER: &str = "hanzo-operator-kms-zap";

/// Canonical, centralized KMS CRD family. KMS is one service for every
/// universe, so the group is fixed (not api-group-rewritten); we watch it
/// group-erased as a DynamicObject.
const KMS_GROUP: &str = "kms.hanzo.ai";
const KMS_VERSION: &str = "v1alpha1";
const KMS_KIND: &str = "KMSSecret";

// Input bounds (mirror the proven KMS-bridge limits).
const MAX_ADDR: usize = 256;
const MAX_FIELD: usize = 256;
const MAX_KEY: usize = 128;
const MAX_KEYS: usize = 128;
const MAX_NAME: usize = 253;

/// The ZAP-native subset of a `KMSSecret` spec. Only CRs whose spec carries
/// `transport: "zap"` are handled here.
#[derive(Debug, Clone, Deserialize, Default)]
struct ZapKmsSpec {
    #[serde(default)]
    transport: String,
    #[serde(default, rename = "zapAddr")]
    zap_addr: String,
    #[serde(default, rename = "projectSlug")]
    project_slug: String,
    #[serde(default, rename = "envSlug")]
    env_slug: String,
    #[serde(default, rename = "secretsPath")]
    secrets_path: String,
    #[serde(default)]
    keys: Vec<String>,
    #[serde(default, rename = "managedSecretName")]
    managed_secret_name: String,
    #[serde(default, rename = "allowedNamespaces")]
    allowed_namespaces: Vec<String>,
    #[serde(default, rename = "clusterName")]
    cluster_name: String,
}

/// True iff this CR opts into the ZAP-native path.
fn is_zap_native(spec: &ZapKmsSpec) -> bool {
    spec.transport == "zap"
}

/// Validate a ZAP-native spec before any I/O. Pure.
fn validate_spec(s: &ZapKmsSpec) -> Result<(), String> {
    if !is_zap_native(s) {
        return Err("not zap-native".into());
    }
    if s.zap_addr.is_empty() || s.zap_addr.len() > MAX_ADDR {
        return Err("zapAddr empty or too long".into());
    }
    if s.project_slug.is_empty() || s.project_slug.len() > MAX_FIELD {
        return Err("projectSlug empty or too long".into());
    }
    if s.env_slug.is_empty() || s.env_slug.len() > MAX_FIELD {
        return Err("envSlug empty or too long".into());
    }
    if s.secrets_path.len() > MAX_FIELD {
        return Err("secretsPath too long".into());
    }
    if s.managed_secret_name.is_empty() || s.managed_secret_name.len() > MAX_NAME {
        return Err("managedSecretName empty or too long".into());
    }
    if s.keys.is_empty() || s.keys.len() > MAX_KEYS {
        return Err("keys empty or too many".into());
    }
    for k in &s.keys {
        if k.is_empty() || k.len() > MAX_KEY {
            return Err("key empty or too long".into());
        }
    }
    Ok(())
}

/// Namespace allow-list: target must be the CR's own namespace, or explicitly
/// allow-listed. No cross-namespace by default.
fn namespace_allowed(s: &ZapKmsSpec, cr_ns: &str, target_ns: &str) -> bool {
    target_ns == cr_ns || s.allowed_namespaces.iter().any(|n| n == target_ns)
}

struct Ctx {
    client: Client,
}

#[derive(Debug, thiserror::Error)]
enum ReconcileError {
    #[error("{0}")]
    Msg(String),
}
impl From<String> for ReconcileError {
    fn from(s: String) -> Self {
        ReconcileError::Msg(s)
    }
}

async fn reconcile(obj: Arc<DynamicObject>, ctx: Arc<Ctx>) -> Result<Action, ReconcileError> {
    let name = obj.metadata.name.clone().unwrap_or_default();
    let cr_ns = obj.metadata.namespace.clone().unwrap_or_default();

    // Decode the spec; IGNORE non-zap-native CRs (left to the REST projector).
    let spec: ZapKmsSpec = obj
        .data
        .get("spec")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| format!("spec decode: {e}"))?
        .unwrap_or_default();
    if !is_zap_native(&spec) {
        return Ok(Action::await_change());
    }
    validate_spec(&spec)?;

    // The managed Secret lives in the CR's namespace.
    let target_ns = cr_ns.clone();
    if !namespace_allowed(&spec, &cr_ns, &target_ns) {
        return Err(format!("namespace {target_ns} not allowed").into());
    }

    // Fetch every key over ZAP — FAIL-CLOSED: any error aborts with no write.
    let mut zap = ZapClient::connect(&spec.zap_addr, &spec.cluster_name)
        .await
        .map_err(|e| format!("zap connect {}: {e}", spec.zap_addr))?;
    let mut data: BTreeMap<String, String> = BTreeMap::new();
    for key in &spec.keys {
        let val = zap
            .get_secret(&spec.secrets_path, key, &spec.env_slug)
            .await
            .map_err(|e| format!("zap get {key}: {e}"))?;
        validate_secret_value(val.as_bytes())?;
        data.insert(key.clone(), val);
    }

    // Strict hijack guard: never overwrite a Secret we don't own.
    let secrets: Api<Secret> = Api::namespaced(ctx.client.clone(), &target_ns);
    let cr_uid = obj.metadata.uid.clone().unwrap_or_default();
    if let Some(existing) = secrets
        .get_opt(&spec.managed_secret_name)
        .await
        .map_err(|e| format!("get secret: {e}"))?
    {
        if !is_operator_managed(&existing, KMS_ZAP_MANAGER, &cr_uid) {
            return Err(format!(
                "refuse to overwrite unmanaged Secret {target_ns}/{}",
                spec.managed_secret_name
            )
            .into());
        }
    }

    // Project via SSA. managed-by label + ownerRef so only we adopt it.
    let mut labels = BTreeMap::new();
    labels.insert(MANAGED_BY_LABEL.to_string(), KMS_ZAP_MANAGER.to_string());
    let owner = owner_ref(
        &format!("{KMS_GROUP}/{KMS_VERSION}"),
        KMS_KIND,
        &name,
        &cr_uid,
    );
    let secret = Secret {
        metadata: ObjectMeta {
            name: Some(spec.managed_secret_name.clone()),
            namespace: Some(target_ns.clone()),
            labels: Some(labels),
            owner_references: Some(vec![owner]),
            ..Default::default()
        },
        string_data: Some(data),
        ..Default::default()
    };
    apply::apply(&secrets, &secret)
        .await
        .map_err(|e| format!("apply secret: {e}"))?;

    info!(
        name,
        namespace = target_ns,
        keys = spec.keys.len(),
        "KMSSecret (zap) reconciled"
    );
    Ok(Action::requeue(Duration::from_secs(300)))
}

fn on_error(_obj: Arc<DynamicObject>, err: &ReconcileError, _ctx: Arc<Ctx>) -> Action {
    warn!(error = %err, "KMS ZAP reconcile error");
    Action::requeue(Duration::from_secs(30))
}

/// Opt-in entrypoint. When `enabled` is false this returns immediately — the
/// controller never watches anything, so the REST projector stays the only
/// secret path until a deliberate cutover. Matches the call shape of the
/// other controllers so it slots into `run_all_controllers`' `join!`.
pub async fn run_kms_zap_controller(client: Client, namespace: String, enabled: bool) {
    if !enabled {
        info!("KMS ZAP controller disabled (set KMS_ZAP_CONTROLLER=true to enable)");
        return;
    }
    let gvk = GroupVersionKind::gvk(KMS_GROUP, KMS_VERSION, KMS_KIND);
    let ar = ApiResource::from_gvk(&gvk);
    let api: Api<DynamicObject> = if namespace.is_empty() {
        Api::all_with(client.clone(), &ar)
    } else {
        Api::namespaced_with(client.clone(), &namespace, &ar)
    };
    info!(
        group = KMS_GROUP,
        "Starting KMS ZAP controller (additive, zap-native CRs only)"
    );
    let ctx = Arc::new(Ctx { client });
    Controller::new_with(api, Config::default(), ar)
        .run(reconcile, on_error, ctx)
        .for_each(|res| async move {
            if let Err(e) = res {
                warn!(error = ?e, "KMS ZAP controller stream error");
            }
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_spec() -> ZapKmsSpec {
        ZapKmsSpec {
            transport: "zap".into(),
            zap_addr: "kms.hanzo.svc:9000".into(),
            project_slug: "hanzo-goproxy".into(),
            env_slug: "prod".into(),
            secrets_path: "/".into(),
            keys: vec!["GH_PROXY_TOKEN".into()],
            managed_secret_name: "goproxy-secrets".into(),
            allowed_namespaces: vec![],
            cluster_name: "hanzo".into(),
        }
    }

    #[test]
    fn only_transport_zap_is_native() {
        let mut s = base_spec();
        s.transport = String::new();
        assert!(!is_zap_native(&s));
        s.transport = "rest".into();
        assert!(!is_zap_native(&s));
        s.transport = "zap".into();
        assert!(is_zap_native(&s));
    }

    #[test]
    fn validate_spec_accepts_well_formed() {
        assert!(validate_spec(&base_spec()).is_ok());
    }

    #[test]
    fn validate_spec_rejects_non_zap() {
        let mut s = base_spec();
        s.transport = "rest".into();
        assert!(validate_spec(&s).is_err());
    }

    #[test]
    fn validate_spec_rejects_empty_required_fields() {
        let mut a = base_spec();
        a.zap_addr = String::new();
        assert!(validate_spec(&a).is_err());
        let mut b = base_spec();
        b.project_slug = String::new();
        assert!(validate_spec(&b).is_err());
        let mut c = base_spec();
        c.env_slug = String::new();
        assert!(validate_spec(&c).is_err());
        let mut d = base_spec();
        d.managed_secret_name = String::new();
        assert!(validate_spec(&d).is_err());
    }

    #[test]
    fn validate_spec_rejects_empty_or_oversize_keys() {
        let mut empty = base_spec();
        empty.keys = vec![];
        assert!(validate_spec(&empty).is_err());

        let mut long_key = base_spec();
        long_key.keys = vec!["x".repeat(MAX_KEY + 1)];
        assert!(validate_spec(&long_key).is_err());

        let mut too_many = base_spec();
        too_many.keys = (0..(MAX_KEYS + 1)).map(|i| format!("k{i}")).collect();
        assert!(validate_spec(&too_many).is_err());
    }

    #[test]
    fn validate_spec_rejects_oversize_fields() {
        let mut s = base_spec();
        s.zap_addr = "a".repeat(MAX_ADDR + 1);
        assert!(validate_spec(&s).is_err());
        let mut p = base_spec();
        p.secrets_path = "p".repeat(MAX_FIELD + 1);
        assert!(validate_spec(&p).is_err());
    }

    #[test]
    fn namespace_same_ns_allowed_by_default() {
        let s = base_spec();
        assert!(namespace_allowed(&s, "hanzo", "hanzo"));
        assert!(!namespace_allowed(&s, "hanzo", "kube-system"));
    }

    #[test]
    fn namespace_allow_list_honored() {
        let mut s = base_spec();
        s.allowed_namespaces = vec!["lux".into()];
        assert!(namespace_allowed(&s, "hanzo", "lux"));
        assert!(!namespace_allowed(&s, "hanzo", "zoo"));
    }
}
