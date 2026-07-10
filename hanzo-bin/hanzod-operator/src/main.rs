// Copyright 2026 Hanzo AI Inc. All Rights Reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! `hanzod-operator` binary. Runs the controller against the ambient kubeconfig
//! / in-cluster service account.
//!
//! `hanzod-operator crd` prints hanzod's derived CRD for REFERENCE only. It does
//! NOT replace `universe/infra/k8s/operator/crds.yaml`, which is the
//! authoritative superset — never pipe this into `kubectl apply` over the live
//! CRD.

use std::sync::Arc;

use hanzod_operator::{crd_definition, reconcile, StaticCoordinator};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    if std::env::args().nth(1).as_deref() == Some("crd") {
        eprintln!(
            "# REFERENCE ONLY — the authoritative CRD is universe/infra/k8s/operator/crds.yaml\n\
             # (a superset). Do NOT apply this over the live CRD."
        );
        println!("{}", serde_json::to_string_pretty(&crd_definition())?);
        return Ok(());
    }

    let client = kube::Client::try_default().await?;
    // Single-node default. The leaderless coordinator (HRW today, ZAP-BFT round
    // tomorrow) plugs in HERE with no change to the reconcile loop.
    let coordinator = Arc::new(StaticCoordinator);
    reconcile::run(client, coordinator).await
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,hanzod_operator=debug"));
    fmt().with_env_filter(filter).init();
}
