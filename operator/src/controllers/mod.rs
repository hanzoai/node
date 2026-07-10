//! Controllers for each CRD Kind.

use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use kube::Resource;

pub mod base;
pub mod datastore;
pub mod dns;
pub mod gateway;
pub mod ingress;
pub mod kms_zap;
pub mod mpc;
pub mod network;
pub mod service;

/// Build an OwnerReference pointing at a CR. The CR must have a UID set.
pub fn owner_ref_for<K>(cr: &K, api_version: &str, kind: &str) -> OwnerReference
where
    K: Resource<DynamicType = ()>,
{
    OwnerReference {
        api_version: api_version.to_string(),
        kind: kind.to_string(),
        name: cr.meta().name.clone().unwrap_or_default(),
        uid: cr.meta().uid.clone().unwrap_or_default(),
        controller: Some(true),
        block_owner_deletion: Some(true),
    }
}

// v0.3.2: new Kinds
pub mod function;
pub mod observability;
pub mod queue;
pub mod spa;
pub mod static_site;

// v0.3.3: facade Kinds (orphaned in v0.3.0 — controllers added here).
pub mod docdb;
pub mod explorer;
pub mod iam;
pub mod indexer;
pub mod kms;
pub mod kv;
pub mod llm;
pub mod s3;
pub mod sql;

// v0.3.4: union with go/ — LuxRuntime + NodeFleet blockchain Kinds.
pub mod luxruntime;
pub mod nodefleet;

// v0.4.x: apps-lifecycle DRIVE controller. Not a CRD Kind — its reconcile
// source is the platform `apps` table (read over `GET /v1/apps`), and it drives
// `declared_tag` → cluster by patching Deployments. Opt-in + fail-safe; see the
// module docs for the safety-gate model. Implements PR 5 of the platform's
// docs/APPS_LIFECYCLE.md.
pub mod apps;

// ManagedDatabase facade — per-tenant isolated Datastore workload.
pub mod managed_database;

// AgentDeployment — the autonomous-bot lifecycle (cloud Agent + visor-bound
// @hanzo/bot machine). Reconcile ACTIONS reach cloud /v1/agents + visor
// /v1/machines over HTTP; provisioning is opt-in + fail-safe (AGENT_DEPLOY_MODE).
pub mod agent_deployment;

// Tenant-RBAC — onboards platform-managed `tenant-<org>` namespaces (watches
// Namespaces labelled hanzo.ai/managed-by=platform). Projects cloud-api's
// per-tenant cloud-api-platform RoleBinding (the grant cloud's waitForTenantRBAC
// gates on) + the ghcr-pull image-pull Secret. Not a CRD Kind — its reconcile
// source is the tenant Namespace itself.
pub mod tenant_rbac;
