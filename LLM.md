# node — AI Assistant Context

<h1 align="center">
  <img src="files/icon.png"/><br/>
  Hanzo Node
</h1>
<p align="center">Hanzo allows you to create AI agents without touching code. Define tasks, schedule actions, and let Hanzo write custom code for you. Native crypto support included.<br/><br/> There is a companion repo called Hanzo Apps which contains the frontend that encapsulates this project, you can find it <a href="https://github.com/hanzoai/hanzo-apps">here</a>.</p><br/>

## Federation (lab mode)

Opt-in federated training via the `[federation]` block in `hanzo.toml`. The
`FederationManager` (`src/managers/federation.rs`) loads config at boot
(`runner::init_federation`), elects coordinator/worker against the
`(nic_gbps * tflops_hint)` score in `lab.yaml`, then spawns the lab task.

* `enabled=false` (default) — manager is a no-op.
* `role="auto"` — coordinator if this `hostname` wins the lab election.
* `role="coordinator"` — serves `/v1/federation/*` on `bind`.
* `role="worker"` — pushes deltas to `coordinator_url` (or synthesised from lab).

Runtime linkage to `~/work/hanzo/engine/hanzo-federation` is gated behind the
`federation-runtime` cargo feature. **On by default** as of the native-Rust
pipeline cutover — a stock `cargo build -p hanzo-node --release` ships
hanzod with `hanzo_federation::{Coordinator, Worker}` linked. Disable
with `--no-default-features` if you want the smallest binary.

## 100% Rust AI Pipeline

A fresh `hanzod` binary, no Python on the box, can: (1) serve zen5-family
models, (2) act as a federation coordinator or worker, (3) ingest
compressed LoRA deltas from peers and apply them. The shape:

```
hanzod
  ├─ FederationManager       (hanzo-federation, on by default)
  │     ├─ Coordinator       serves /v1/federation/* + /v1/rlhf/*
  │     └─ Worker            pushes BF16 deltas → trim-mean aggregation
  ├─ Zen5Registry            (hanzo-engine → hanzo-zen5)
  │     ├─ Zen5InferenceAdapter for each `<variant>.gguf`
  │     └─ model_id = sha256("<variant>:<absolute path>")
  ├─ Quantization runtime    (hanzo-quant for adapter merge:
  │                           BitDelta, DeltaQuant, DeltaSoup)
  └─ RLHF facade             (hanzo-rlhf — typed traits;
                              production runs bridge to Python TRL)
```

### Boot path

`runner::initialize_node`:

1. `install_zen5_engines()` — reads `[zen5]` in hanzo.toml. If `enabled=true`,
   loads each `<variant>.gguf` from `weights_dir` and registers a
   `Zen5Registry` as the process-wide inference engine.
2. `install_engine()` — fallback MistralEngine for HF repos. No-op if
   Zen5 already won the slot (first-writer wins, intentional).
3. `init_federation()` — reads `[federation]`, elects role, spawns the
   coordinator or worker task with `hanzo_federation` linked natively.

### `[zen5]` toml schema

```toml
[zen5]
enabled = false
weights_dir = "/var/lib/hanzo/zen5"
models = ["zen-5-flash", "zen-5-pro", "zen-5-mini", "zen-5-coder"]
backend = "auto"      # auto | metal | cuda | cpu
```

### What's Rust vs Python

| Concern                    | Rust                              | Python   |
| -------------------------- | --------------------------------- | -------- |
| Inference                  | hanzo-engine + hanzo-zen5         | none     |
| Embedding                  | hanzo-engine (mistralrs backend)  | none     |
| Federation transport       | hanzo-federation (axum + HMAC)    | none     |
| Delta encode/decode (BF16) | hanzo-federation::codec           | mirrored |
| Delta soup aggregation     | hanzo-federation::coordinator     | mirrored |
| Quantization (1-bit/INT4)  | hanzo-quant                       | mirrored |
| RLHF typed API             | hanzo-rlhf (traits + RunConfig)   | none     |
| RLHF backward pass         | scaffolded only (unimplemented!)  | TRL      |

Both sides share the canonical BF16 delta wire format so a Python
trainer can post deltas any Rust worker can ingest, and vice versa.

### Build matrix

| Target                                     | Features                              |
| ------------------------------------------ | ------------------------------------- |
| Stock hanzod                               | `default = ["federation-runtime"]`    |
| + Zen5 FFI (Metal/CUDA, vendored C)        | `--features zen5-ffi`                 |
| + Zen5 native (pure-Rust candle)           | `--features zen5-native`              |
| Minimal (no federation, no AI extensions)  | `--no-default-features`               |

## hanzod Kubernetes Operator (`hanzo-bin/hanzod-operator`)

hanzod's operator surface: one hanzod per cluster, a drop-in operator that
watches the `services.hanzo.ai/v1` CRD (`kind: Service`, shortname `hsvc`) and
reconciles each CR to a **Deployment + Service + optional Ingress**. It replaces
the decommissioned Go operator (`ghcr.io/hanzoai/operator`); the CRD it consumes
carries the `Auto-generated derived type for ServiceSpec` marker, i.e. the schema
was always generated from Rust `kube::CustomResource` types — hanzod is now their
home. Manifests: `~/work/hanzo/universe/infra/k8s/operator/{crds.yaml,deployment.yaml,rbac/}`.

Built on `kube-rs` 4.0 + `k8s-openapi` 0.28 + `schemars` 1.2. Self-contained
workspace leaf (only crates.io deps) so it builds and tests independently of the
inference-heavy `hanzo-node` binary; the node wires it in as a subcommand later.

### Layout — one concern per module (decomplected: values, not places)

| Module           | Concern                                                                 |
| ---------------- | ----------------------------------------------------------------------- |
| `crd.rs`         | `services.hanzo.ai` Rust types — the schema's source of truth.          |
| `manifests.rs`   | **Pure** CR → Deployment/Service/Ingress mapping (the *decision*).      |
| `reconcile.rs`   | The kube controller (the *effect*: gate → plan → SSA → status).         |
| `coordinator.rs` | The leaderless seam — who reconciles when N hanzods run.                |

The decision (`manifests::plan`) is separated from the effect (`reconcile::apply`)
so the whole CR → objects mapping is unit-tested directly with no cluster/client.
Reconcile is idempotent server-side-apply under field manager `hanzod`; it sets
`ownerReferences` (GC + `owns()` watch) and writes the `status` subresource
(`observedGeneration`, `readyReplicas`, `availableReplicas`, derived `phase`).

Run: `hanzod-operator` (ambient kubeconfig / in-cluster SA). `hanzod-operator crd`
prints the CRD (`| kubectl apply -f -`) — types generate the schema, one way, no
drift. Reconciled CR fields today: image, replicas, strategy, command/args, env
(+valueFrom/envFrom), ports, resources, liveness/readiness probes (exec > tcp >
http precedence), labels/annotations, serviceAccountName, imagePullSecrets,
fsGroup, init/sidecar containers, volumes/volumeMounts, ingress (class annotation
for `hanzoai/ingress` + cert-manager issuer), partOf/component. serde ignores
unknown CR fields, so advanced fields not yet reconciled (autoscaling→HPA,
pdb→PDB, networkPolicy, serviceMonitor, kmsSecrets, persistence) deserialize
cleanly and become their own reconcilers next; `crds.yaml` stays authoritative for
the full schema until the Rust types cover it end-to-end.

### Consensus seam — leaderless, mirrors `github.com/hanzoai/ha` + Lux ZAP

The vision: many hanzods across clusters forming a public, permissionless,
leaderless blockchain, each hanzod one consensus participant. The reconcile loop
asks exactly ONE question per object — `Coordinator::should_reconcile(key)` where
`key = "<namespace>/<name>"` — and everything about how that is answered lives
behind the trait. Four layers, composed not braided, deliberately mirroring `ha`
(already proven in visor/cloud):

1. **Membership** (`trait Membership`) — the live reconcile-eligible hanzod set +
   this node's stable id. `StaticMembership` (single node/tests) or a cluster
   source. Fail-closed on error/empty (no safe owner ⇒ stand aside).
2. **Owner** (`fn owner`) — pure Rendezvous/HRW hashing over `(key, members)`,
   `sha256(key ‖ 0x00 ‖ id)[..8]` — **byte-identical to `ha.weight`** (a golden
   test locks it), so a Rust hanzod and a Go `ha` elect the same owner. HRW today.
3. **Coordinator** (`trait Coordinator`) — `StaticCoordinator` (single-node
   default, always owner) and `HrwCoordinator<M>` (composes Membership + owner).
4. **Fencer / Round** (documented seam, not yet wired) — election answers who
   SHOULD write; it cannot make a deposed/partitioned node STOP. A monotone
   `Round` fencing token does. `ha` keeps this separate on purpose: the round is
   the output of a *linearizable* source, and folding that in re-complects
   election with consensus. Today: a single linearizable register. Tomorrow: a
   **ZAP-BFT-agreed round** — Lux `~/work/lux/consensus1` over `~/work/lux/zap`
   `conn_pq` PQ sessions (the quasar RSM) — plugged in behind `Fencer` with no
   change at any call site. The k8s API server itself is the fenced store (SSA
   with a per-round field manager / `resourceVersion` precondition rejects a stale
   writer's patch).

### What's left for full leaderless-BFT (follow-on)

- Wire a real `Membership`: `KubeMembership` (list hanzod Leases in
  `coordination.k8s.io`) or `ZapMembership` (mDNS/discovery peers over `zap`).
- Add a `FencedCoordinator` = `owner` + `Fencer::acquire(key)`; stamp the round
  onto every SSA (field manager `hanzod-r<round>`) so a deposed writer is fenced.
- Back the `Fencer` with the ZAP-agreed round so "who" is BFT-agreed across
  mutually-distrusting clusters, not just locally-HRW-agreed.
- Reconcilers for the advanced CR fields (HPA/PDB/NetworkPolicy/ServiceMonitor/
  KMSSecret/persistence) + apply-time pruning of orphaned owned objects.

Local build/test note: the repo `.cargo/config.toml` pins `-fuse-ld=lld` (CI has
lld). On a box without it: `CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=cc
CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="" cargo test -p hanzod-operator`.
