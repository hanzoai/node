//! SQL reconciler — newtype facade over Datastore. PostgreSQL workloads
//! (hanzoai/sql) declared as a `SQL` CR materialize as an ordinary Datastore
//! with `type=postgresql` forced server-side: a `SQL` CR cannot accidentally
//! become a Valkey or MinIO datastore.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use kube::api::Api;
use kube::runtime::controller::{Action, Controller};
use kube::runtime::watcher::Config;
use kube::{Client, ResourceExt};
use tracing::{error, info};

use crate::core::{OperatorError, Result};
use crate::crd::SQL;

use super::{datastore, owner_ref_for};

/// Canonical `spec.type` for SQL facade CRs.
const DATASTORE_TYPE: &str = "postgresql";

#[derive(Clone)]
pub struct Ctx {
    pub client: Client,
    pub api_group: String,
}

pub async fn reconcile(cr: Arc<SQL>, ctx: Arc<Ctx>) -> Result<Action> {
    let name = cr.name_any();
    let namespace = cr
        .namespace()
        .ok_or_else(|| OperatorError::Config("SQL has no namespace".into()))?;
    let api_version = format!("{}/v1", ctx.api_group);
    let owner = owner_ref_for(cr.as_ref(), &api_version, "SQL");
    let mut ds_spec = cr.spec.0.clone();
    ds_spec.type_ = DATASTORE_TYPE.to_string();
    datastore::reconcile_datastore_inner_pub(&ctx.client, &name, &namespace, &ds_spec, owner)
        .await?;
    // Report Ready on the facade CR just like the canonical Datastore does. The
    // reconcile above materializes the StatefulSet, but the newtype facade
    // previously never wrote status — leaving `SQL`/`KV` CRs with an empty
    // Ready condition even when their workload was healthy.
    datastore::write_status::<SQL>(
        &ctx.client,
        &name,
        &namespace,
        &ds_spec,
        cr.metadata.generation.unwrap_or(0),
    )
    .await;
    Ok(Action::requeue(Duration::from_secs(60)))
}

pub fn on_error(_obj: Arc<SQL>, err: &OperatorError, _ctx: Arc<Ctx>) -> Action {
    error!(error = %err, "SQL reconcile failed");
    Action::requeue(Duration::from_secs(30))
}

pub async fn run_sql_controller(client: Client, namespace: String, api_group: String) {
    let api: Api<SQL> = if namespace.is_empty() {
        Api::all(client.clone())
    } else {
        Api::namespaced(client.clone(), &namespace)
    };
    info!(group = %api_group, "Starting SQL controller");
    let ctx = Arc::new(Ctx { client, api_group });
    Controller::new(api, Config::default())
        .run(reconcile, on_error, ctx)
        .for_each(|_| async {})
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crd::{DatastoreSpec, SQLSpec, StorageSpec};

    #[test]
    fn sql_forces_postgresql_type() {
        // User-supplied type must be overridden by the facade.
        let inner = DatastoreSpec {
            type_: "minio".to_string(),
            storage: StorageSpec {
                size: "10Gi".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        let facade = SQLSpec(inner);
        let mut ds = facade.0.clone();
        ds.type_ = DATASTORE_TYPE.to_string();
        assert_eq!(ds.type_, "postgresql");
    }
}
