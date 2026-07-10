// Copyright 2026 Hanzo AI Inc. All Rights Reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! `hanzod-operator` — the Kubernetes-operator surface of hanzod.
//!
//! Vision: one hanzod per cluster, a drop-in operator that reconciles the
//! `hanzoai/cloud` binary and every Hanzo system from `apps.hanzo.ai` CRs, and —
//! across clusters — forms a public, permissionless, leaderless blockchain where
//! each hanzod is one consensus participant. This crate is the FOUNDATION: the
//! reconcile loop plus the seam the leaderless decision plugs into.
//!
//! Modules (one concern each): [`crd`] (schema types), [`manifests`] (pure CR →
//! objects), [`reconcile`] (the controller), [`coordinator`] (leaderless seam).

pub mod coordinator;
pub mod crd;
pub mod manifests;
pub mod reconcile;

pub use coordinator::{
    check_singleton, is_owner, owner, CoordError, Coordinator, HrwCoordinator, Member, Membership,
    StaticCoordinator, StaticMembership,
};
pub use crd::{App, AppSpec, AppStatus};
pub use manifests::{plan, Plan};
pub use reconcile::run;

use kube::CustomResourceExt;

/// The `apps.hanzo.ai` CustomResourceDefinition generated from [`App`], with
/// `x-kubernetes-preserve-unknown-fields: true` at the spec root.
///
/// HIGH-2: hanzod's derived schema is a strict SUBSET of the authoritative
/// `crds.yaml` (missing 7 advanced fields + status.conditions). Applying a
/// pruning structural CRD over the live one would make the apiserver DELETE
/// those fields off every stored CR. Preserving unknown fields makes this emit
/// non-destructive. **`crds.yaml` remains the authoritative CRD** — this emit is
/// for reference/bootstrap only; see the `crd` subcommand's warning.
pub fn crd_definition(
) -> k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition {
    let mut crd = App::crd();
    if let Some(version) = crd.spec.versions.get_mut(0) {
        if let Some(schema) = version.schema.as_mut() {
            if let Some(root) = schema.open_api_v3_schema.as_mut() {
                if let Some(spec) = root.properties.as_mut().and_then(|p| p.get_mut("spec")) {
                    spec.x_kubernetes_preserve_unknown_fields = Some(true);
                }
            }
        }
    }
    crd
}
