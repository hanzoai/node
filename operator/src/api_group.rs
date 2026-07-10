//! Runtime-configurable API group for CRDs.
//!
//! kube-rs's `CustomResource` derive bakes the API group at compile time. To
//! support per-universe customization without forking the binary, we accept
//! `--api-group` / `OPERATOR_API_GROUP` at startup and rewrite the CRD YAML
//! group field at install time via the `generate-crd-yaml` binary.
//!
//! For controller runtime, all reconcilers receive the resolved API group
//! and use it when watching dynamic CRs (legacy compat Kinds and KMSSecret).
//!
//! ## Defaults per universe
//!
//! | Universe   | Default API group |
//! |------------|-------------------|
//! | hanzo      | `hanzo.ai`        |
//! | lux        | `lux.cloud`       |
//! | zoo        | `zoo.cloud`       |
//! | osage      | `osage.cloud`     |

use std::env;

/// The compile-time default API group. Other universes override via
/// `--api-group` flag or `OPERATOR_API_GROUP` env var.
pub const DEFAULT_API_GROUP: &str = "hanzo.ai";

/// API version. All CRDs are at v1.
pub const API_VERSION: &str = "v1";

/// API group/version pair used for owner references on managed K8s
/// objects (Deployments, Services, etc.). Computed once at startup.
#[derive(Clone, Debug)]
pub struct ApiGroup {
    pub group: String,
    pub version: String,
}

impl ApiGroup {
    pub fn new(group: String) -> Self {
        Self {
            group,
            version: API_VERSION.to_string(),
        }
    }

    /// Resolve the API group from (in order): CLI arg, env var,
    /// compile-time default.
    pub fn resolve(cli_override: Option<&str>) -> Self {
        let group = cli_override
            .map(str::to_string)
            .or_else(|| env::var("OPERATOR_API_GROUP").ok())
            .unwrap_or_else(|| DEFAULT_API_GROUP.to_string());
        Self::new(group)
    }

    pub fn api_version(&self) -> String {
        format!("{}/{}", self.group, self.version)
    }

    /// The KMS API group is `secrets.<group>` by convention; for `hanzo.ai`
    /// this is `kms.hanzo.ai` (legacy). We honor the legacy when group is
    /// the default, otherwise compute the standard form.
    pub fn kms_group(&self) -> String {
        if self.group == DEFAULT_API_GROUP {
            "kms.hanzo.ai".to_string()
        } else {
            format!("kms.{}", self.group)
        }
    }
}
