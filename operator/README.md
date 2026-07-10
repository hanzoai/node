<p align="center"><img src=".github/hero.svg" alt="operator" width="880"></p>

# operator

Canonical **Rust** Kubernetes operator for the Hanzo platform — and, per
the cross-impl parity goal, eventually all of Lux too. One binary, N CRD
Kinds, configurable API group at install time. See [`LLM.md`](./LLM.md)
for the agent-friendly overview.

## Canonical homes

| Impl  | Repo                              | Image                         |
|-------|-----------------------------------|-------------------------------|
| Rust  | `hanzoai/operator` (this repo)    | `ghcr.io/hanzoai/operator`    |
| Go    | `luxfi/operator`                  | `ghcr.io/luxfi/operator`      |

Both implementations target **full feature parity** against a shared CRD
wire contract: each k8s cluster deploys whichever language fits its
operating context (Hanzo web2 ↔ Lux web3). Any change to the CRD wire
shape must land in both impls.

## What it manages

Kinds at `<api-group>/v1`. No compat aliases — the v1 Kinds are the one
way.

| Kind        | Purpose                                                   | Materializes |
|-------------|-----------------------------------------------------------|--------------|
| Service     | Stateless service (most common)                           | Deployment, Service, Ingress, HPA, PDB, NetworkPolicy, KMSSecret |
| Datastore   | Generic stateful service (dispatches on `spec.type`)      | StatefulSet, ClusterIP + headless Service, PVC, KMSSecret |
| SQL / KV / DocDB / S3 | Thin facades over Datastore for each engine     | Same as Datastore |
| Gateway     | KrakenD-based API gateway                                 | Deployment, Service, ConfigMap (krakend.json), Ingress |
| MPC         | Multi-party computation threshold cluster                 | StatefulSet, ClusterIP + headless Service |
| Network     | Blockchain validator network (mode derived from networkID + validators) | StatefulSet (validators), Services, PVC |
| LuxRuntime  | Lux primary-network validators + tenant chain imports     | StatefulSet, Services, PVC, CronJob, Jobs |
| NodeFleet   | Pinned node fleet                                         | StatefulSet, Services |
| Ingress     | Multi-domain routing with cert-manager TLS                | Multiple Ingress resources |
| DNS         | Multi-tenant CoreDNS deployment                           | Deployment, Service |
| Base        | hanzoai/base-ha cluster (Quasar-pinned writer)            | StatefulSet, headless + ClusterIP Services |
| IAM / KMS / LLM / Indexer / Explorer | Thin facades over Service          | Same as Service |
| SPA / Static / Queue / Observability / Function | App-shaped facades            | Service / Datastore facades |
| Chain / Validator | Sub-resources of Network (NoOp stubs)               | — |

## Critical invariant

`spec.env`, `spec.volumes`, `spec.volumeMounts` MUST be honored on every
generated Deployment.

```bash
cargo test --lib controllers::service::tests
# env_is_carried_to_main_container ... ok
# volume_mounts_are_carried_to_main_container ... ok
# deployment_carries_volumes ... ok
```

When a Service CR sets `autoscaling.enabled = true`, the operator emits
the Deployment with **no** `spec.replicas` so the HPA is the sole field
manager for that field (server-side apply would otherwise fight the HPA
on every reconcile cycle).

## Build / Test

```bash
cargo build --release
cargo test --lib                      # unit tests
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## API group rebinding

kube-rs's `CustomResource` derive bakes the API group at compile time —
the default is `hanzo.ai`. To target another universe's API group,
regenerate the CRD YAML via the `generate-crd-yaml` binary:

```bash
generate-crd-yaml --api-group lux.cloud   --out k8s/crds/all-lux.cloud.yaml
generate-crd-yaml --api-group hanzo.ai    --out k8s/crds/all-hanzo.ai.yaml
generate-crd-yaml --api-group zoo.cloud   --out k8s/crds/all-zoo.cloud.yaml
generate-crd-yaml --api-group osage.cloud --out k8s/crds/all-osage.cloud.yaml
```

The running binary accepts `--api-group X.Y` / `OPERATOR_API_GROUP=X.Y`
and uses the resolved group for owner references and dynamic KMSSecret
references.

## Layout

```
src/
  main.rs              clap args, leader election, controller spawn
  lib.rs               library facade (re-exports)
  crd.rs               CRD types
  crd_types.rs         JsonSchema wrappers for k8s-openapi types
  manifests.rs         pure K8s object builders
  apply.rs             server-side apply (typed + DynamicObject)
  api_group.rs         runtime API-group resolution
  controllers/         one module per Kind
  core/                shared reconciler primitives
    error.rs           OperatorError + Result
    leader.rs          coordination.k8s.io/v1 lease loop
    iam_admin.rs       POST /v1/iam/admin/applications/upsert
    secret.rs          KMSSecret hijack guard + NUL-byte rejection
    status.rs          status.conditions helpers
    reconciler.rs      Action requeue cadence + clamp_resync
  bin/
    generate_crd_yaml.rs   CRD YAML generator with --api-group rewriter
scripts/                migration scripts (v0.2.x → v0.3.0)
k8s/crds/               generated CRD YAMLs per universe
```

## Rules

- Never `:latest`, `:main`, `:dev` — semver tags only (`vX.Y.Z`).
- amd64 only (arm64 paused per global LLM.md 2026-04-27).
- Honor `spec.env/volumes/volumeMounts` — the load-bearing assertion.
- Edition `2021`, Rust 1.79+, kube 4, k8s-openapi 0.28 (v1_33), schemars 1, jiff (not chrono).

## License

BSD-3-Clause (see [`LICENSE`](./LICENSE) — header in source files).
