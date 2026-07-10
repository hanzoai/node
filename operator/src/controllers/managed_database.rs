//! ManagedDatabase reconciler — per-tenant isolated data workload.
//!
//! `ManagedDatabase` is the paid/isolated-tenant facade over `Datastore`. It
//! carries a full inner `DatastoreSpec` (the tenant picks the engine —
//! postgresql / valkey / docdb / minio) and reuses the exact StatefulSet +
//! Service + headless + PVC machinery the `Datastore` controller emits. Unlike
//! the `SQL`/`KV`/`DocDB`/`S3` facades it does NOT force a fixed `spec.type`.
//!
//! The reconcile stamps tenant identity onto the workload labels
//! (`app.kubernetes.io/part-of` + `<api-group>/tenant`) so the control plane
//! can scope discovery to one tenant, and writes the in-cluster DSN to
//! `status.connection_string`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use kube::api::Api;
use kube::runtime::controller::{Action, Controller};
use kube::runtime::watcher::Config;
use kube::{Client, ResourceExt};
use tracing::{error, info, warn};

use crate::core::{OperatorError, Result};
use crate::crd::ManagedDatabase;

use super::{datastore, owner_ref_for};

#[derive(Clone)]
pub struct Ctx {
    pub client: Client,
    pub api_group: String,
}

pub async fn reconcile(cr: Arc<ManagedDatabase>, ctx: Arc<Ctx>) -> Result<Action> {
    let name = cr.name_any();
    let namespace = cr
        .namespace()
        .ok_or_else(|| OperatorError::Config("ManagedDatabase has no namespace".into()))?;
    let api_version = format!("{}/v1", ctx.api_group);
    let owner = owner_ref_for(cr.as_ref(), &api_version, "ManagedDatabase");

    // Inner DatastoreSpec flows through verbatim — the tenant's chosen engine
    // (postgresql/valkey/docdb/minio) is honored, not overridden.
    let mut ds_spec = cr.spec.0.clone();
    // Group every tenant database under one part-of for fleet-wide selection,
    // unless the tenant set an explicit override.
    if ds_spec.part_of.is_empty() {
        ds_spec.part_of = "managed-database".to_string();
    }
    // Stamp tenant identity. The namespace is the isolation boundary, so it is
    // the tenant key; the label prefix tracks the active universe's API group
    // (hanzo.ai / lux.cloud / zoo.cloud / osage.cloud) to stay white-label.
    let mut extra_labels = BTreeMap::new();
    extra_labels.insert(format!("{}/tenant", ctx.api_group), namespace.clone());

    datastore::reconcile_datastore_inner_labeled(
        &ctx.client,
        &name,
        &namespace,
        &ds_spec,
        owner,
        &extra_labels,
    )
    .await?;

    // Publish the in-cluster DSN so the control plane can discover the workload.
    datastore::write_status::<ManagedDatabase>(
        &ctx.client,
        &name,
        &namespace,
        &ds_spec,
        cr.metadata.generation.unwrap_or(0),
    )
    .await;

    Ok(Action::requeue(Duration::from_secs(60)))
}

pub fn on_error(_obj: Arc<ManagedDatabase>, err: &OperatorError, _ctx: Arc<Ctx>) -> Action {
    error!(error = %err, "ManagedDatabase reconcile failed");
    Action::requeue(Duration::from_secs(30))
}

pub async fn run_managed_database_controller(client: Client, namespace: String, api_group: String) {
    let api: Api<ManagedDatabase> = if namespace.is_empty() {
        Api::all(client.clone())
    } else {
        Api::namespaced(client.clone(), &namespace)
    };
    info!(group = %api_group, "Starting ManagedDatabase controller");
    let ctx = Arc::new(Ctx { client, api_group });
    Controller::new(api, Config::default())
        .run(reconcile, on_error, ctx)
        .for_each(|res| async move {
            if let Err(e) = res {
                warn!(error = %e, "ManagedDatabase reconcile error");
            }
        })
        .await;
}

#[cfg(test)]
mod tests {
    use crate::controllers::datastore::connection_string_for;
    use crate::crd::{DatastoreSpec, StorageSpec};

    fn spec(type_: &str) -> DatastoreSpec {
        DatastoreSpec {
            type_: type_.to_string(),
            storage: StorageSpec {
                size: "10Gi".to_string(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn dsn_scheme_and_port_track_engine() {
        assert_eq!(
            connection_string_for(&spec("postgresql"), "tenant-a", "hanzo"),
            "postgresql://tenant-a.hanzo.svc:5432"
        );
        assert_eq!(
            connection_string_for(&spec("valkey"), "tenant-a", "hanzo"),
            "redis://tenant-a.hanzo.svc:6379"
        );
        assert_eq!(
            connection_string_for(&spec("docdb"), "tenant-a", "hanzo"),
            "mongodb://tenant-a.hanzo.svc:27017"
        );
        assert_eq!(
            connection_string_for(&spec("minio"), "tenant-a", "hanzo"),
            "http://tenant-a.hanzo.svc:9000"
        );
    }

    #[test]
    fn dsn_never_carries_credentials() {
        let mut s = spec("postgresql");
        s.credentials_secret = "tenant-a-db".to_string();
        let dsn = connection_string_for(&s, "tenant-a", "hanzo");
        assert!(!dsn.contains('@'), "DSN must not embed credentials: {dsn}");
    }
}
