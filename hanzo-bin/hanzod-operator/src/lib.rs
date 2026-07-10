// Copyright 2026 Hanzo AI Inc. All Rights Reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! `hanzod-operator` — the Kubernetes-operator surface of hanzod.
//!
//! Vision: one hanzod per cluster, a drop-in operator that reconciles the
//! `hanzoai/cloud` binary and every Hanzo system from `services.hanzo.ai` CRs,
//! and — across clusters — forms a public, permissionless, leaderless blockchain
//! where each hanzod is one consensus participant. This crate is the FOUNDATION:
//! the reconcile loop plus the seam that the leaderless decision plugs into. It
//! is a self-contained leaf so it builds and tests independently of the
//! inference-heavy `hanzo-node` binary; the node wires it in as a subcommand
//! (`hanzod operator`) later.
//!
//! Structure (each concern in one place):
//! - [`crd`] — the `services.hanzo.ai` Rust types (the schema's source of truth).
//! - [`manifests`] — pure CR → Deployment/Service/Ingress mapping (the decision).
//! - [`reconcile`] — the kube controller (the effect: gate, apply, status).
//! - [`coordinator`] — the leaderless seam (who reconciles when N hanzods run).

pub mod coordinator;
pub mod crd;
pub mod manifests;
pub mod reconcile;

pub use coordinator::{
    is_owner, owner, Coordinator, CoordError, HrwCoordinator, Member, Membership,
    StaticCoordinator, StaticMembership,
};
pub use crd::{Service, ServiceSpec, ServiceStatus};
pub use manifests::{plan, Plan};
pub use reconcile::run;

use kube::CustomResourceExt;

/// The `services.hanzo.ai` CustomResourceDefinition generated from [`Service`].
/// `hanzod-operator crd | kubectl apply -f -` installs it — types generate the
/// schema (one way), so the CRD never drifts from the code that consumes it.
pub fn crd_definition() -> k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition {
    Service::crd()
}
