// Copyright 2026 Hanzo AI Inc. All Rights Reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! `hanzod-operator` binary. Runs the controller against the ambient kubeconfig
//! / in-cluster service account. Two one-shot subcommands:
//!
//! - `crd` — print hanzod's derived CRD for REFERENCE only (never `kubectl apply`
//!   it over the live authoritative CRD).
//! - `plan` — read an App/Service CR as JSON on stdin, print the owned objects
//!   hanzod WOULD server-side-apply as a JSON array on stdout. Diff that against
//!   the live objects to prove cutover equivalence offline, before migrating a CR.

use std::io::Read;
use std::sync::Arc;

use hanzod_operator::{crd_definition, manifests, reconcile, App, StaticCoordinator};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    match std::env::args().nth(1).as_deref() {
        Some("crd") => {
            eprintln!(
                "# REFERENCE ONLY — the authoritative CRD is universe/infra/k8s/operator/crds.yaml\n\
                 # (a superset). Do NOT apply this over the live CRD."
            );
            println!("{}", serde_json::to_string_pretty(&crd_definition())?);
            return Ok(());
        }
        Some("plan") => return render_plan(),
        _ => {}
    }

    let client = kube::Client::try_default().await?;
    // Single-node default. The leaderless coordinator (HRW today, ZAP-BFT round
    // tomorrow) plugs in HERE with no change to the reconcile loop.
    let coordinator = Arc::new(StaticCoordinator);
    reconcile::run(client, coordinator).await
}

/// `hanzod-operator plan` — render the owned objects for a CR read as JSON on
/// stdin. A `kind: Service` CR being migrated deserializes the same as `kind:
/// App` (the ignored `kind` field aside), so the proof runs directly on a live
/// Service CR: `kubectl get service.hanzo.ai <n> -o json | hanzod-operator plan`.
/// Objects go to stdout (clean, diffable); an unmodeled field the CR carries is
/// reported on stderr as the CR hanzod WOULD Reject (it applies nothing then).
fn render_plan() -> anyhow::Result<()> {
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    let app: App = serde_json::from_str(&buf)?;

    if !app.spec.extra.is_empty() {
        let mut keys: Vec<&String> = app.spec.extra.keys().collect();
        keys.sort();
        let keys: Vec<&str> = keys.iter().map(|k| k.as_str()).collect();
        eprintln!(
            "# WOULD BE REJECTED (status.phase=Rejected): unmodeled spec fields: {}\n\
             # hanzod applies NOTHING while Rejected; objects below are the hypothetical mapping.",
            keys.join(", ")
        );
    }

    let objects = manifests::plan(&app)?.objects()?;
    println!("{}", serde_json::to_string_pretty(&objects)?);
    Ok(())
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,hanzod_operator=debug"));
    fmt().with_env_filter(filter).init();
}
