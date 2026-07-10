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
watches the `apps.hanzo.ai/v1` CRD (`kind: App`, short `app`) and reconciles each
CR to **Deployment + Service + optional Ingress + optional PVC + optional HPA**.
Replaces the decommissioned Go operator (`ghcr.io/hanzoai/operator`). Kind is
`App` (renamed from `Service`) to avoid colliding with core k8s `Service`:
`kubectl get apps.hanzo.ai`.

Built on `kube-rs` 4.0 + `k8s-openapi` 0.28 + `schemars` 1.2. Self-contained
workspace leaf (crates.io deps only) so it builds/tests independently of the
inference-heavy `hanzo-node` binary.

### Layout — one concern per module (decomplect: decision vs effect)

| Module           | Concern                                                                 |
| ---------------- | ----------------------------------------------------------------------- |
| `crd.rs`         | `apps.hanzo.ai` Rust types (the modeled subset + a reject catch-all).   |
| `manifests.rs`   | **Pure** CR → owned-objects mapping (the *decision*), unit-tested.      |
| `reconcile.rs`   | The kube controller (the *effect*: gate → plan → apply/prune → status). |
| `coordinator.rs` | The leaderless seam — who reconciles when N hanzods run.                |

### CRD authority (HIGH-2)

**`universe/infra/k8s/operator/crds.yaml` is the authoritative CRD.** hanzod's
derived schema is a strict SUBSET (missing pdb/networkPolicy/serviceMonitor/
kmsSecrets/surgeColocation + status.conditions). hanzod must NEVER apply its
narrower structural schema over the live one — that would make the apiserver
PRUNE those fields off every stored CR. Two guards: `crd_definition()` injects
`x-kubernetes-preserve-unknown-fields: true` at the spec root (non-destructive),
and `hanzod-operator crd` is REFERENCE-only (prints a stderr warning; never pipe
to `kubectl apply` over the live CRD).

### Fail-closed on unmodeled fields (HIGH-1)

serde captures any spec key hanzod does not model into `AppSpec::extra`; a
non-empty `extra` makes the CR `status.phase=Rejected` (terminal until the CR
changes) — NEVER a silent no-op that would drop e.g. a `persistence` PVC + WAL
backup → data loss. Modeled today: image, replicas, strategy, command/args, env
(+valueFrom/envFrom), ports, resources, probes (exec>tcp>http), labels/
annotations, serviceAccountName, imagePullSecrets, fsGroup, init/sidecar
containers, volumes/mounts, ingress, **persistence**, **autoscaling**, partOf/
component. Rejected until modeled: pdb (11 CRs), surgeColocation (3),
networkPolicy/serviceMonitor/kmsSecrets (unused) — model these next to unblock
their CRs; the reject path keeps them fail-closed-safe meanwhile.

### persistence (durable SQLite via hanzoai/replicate)

`persistence.enabled` wires: a retained PVC (`<app>-<volume>`, RWO, sized from
`storage.size`) or an emptyDir; the volume mounted at `dataDir` in the main
container; a `replicate restore -if-db-not-exists <dataDir/db>` init container
APPENDED after user init containers (paas rationale: a migrate-first init would
create an empty DB and skip the snapshot); and a continuous `replicate replicate`
WAL sidecar. Config via `REPLICATE_*` env (BUCKET/PATH/REPLICA_URL/ENDPOINT/
REGION/FORCE_PATH_STYLE/ALLOW_PLAINTEXT, ACCESS_KEY_ID+SECRET_ACCESS_KEY from
`credentialsSecret`, AGE_IDENTITY on restore / AGE_RECIPIENT on replicate from
`ageSecret`). Required before chat/paas/dataroom migrate.

### Prune, autoscaling, backoff

- MED-3: a disabled Ingress/HPA is DELETED by name (not orphaned). A data PVC is
  never deleted.
- MED-6: under `autoscaling.enabled`, Deployment.replicas is left unset (hanzod
  never fights the HPA) and an autoscaling/v2 HPA is emitted (CPU 80% default).
- MED-8: reconcile failures back off exponentially (5s → 600s cap); after 6
  consecutive failures the CR is quarantined `status.phase=Invalid` (stops the
  hot-loop until the CR changes).

Reconcile is idempotent server-side-apply under field manager `hanzod`, sets
`ownerReferences` (GC + `owns()` watch), writes the `status` subresource.

### Consensus seam — leaderless, mirrors `github.com/hanzoai/ha` + Lux ZAP

The reconcile loop asks ONE question per object —
`Coordinator::should_reconcile("<ns>/<name>")` — everything about how it is
answered lives behind the trait. Mirrors `ha` (Membership + Owner + Fencer):

1. **Membership** — live reconcile-eligible hanzod set + self id; fail-closed on
   error/empty. `StaticMembership` default.
2. **Owner** (`fn owner`) — pure HRW `sha256(key ‖ 0x00 ‖ id)[..8]`,
   byte-identical to `ha.weight` (golden-locked) → Rust hanzod and Go `ha` agree.
3. **Coordinator** — `StaticCoordinator` (single-node default) + `HrwCoordinator`.
   MED-5: `StaticCoordinator::requires_single_replica()==true`; `run()` reads the
   operator's own Deployment replicas and hard-fails at boot if >1 (split-brain
   guard) — the real fix is a leaderless `Membership` (Lease/ZAP) that fences N.
4. **Fencer / Round** (seam, not wired) — a monotone `Round` fences a deposed
   writer. Tomorrow: a ZAP-BFT-agreed round (`lux/consensus1` over `lux/zap`
   `conn_pq`), the quasar RSM; plugs in behind `Fencer` with the k8s API server
   as the fenced store, no call-site change.

### What's left for full leaderless-BFT

- A real `Membership` (K8s Lease over the granted `coordination.k8s.io` RBAC, or
  ZAP mDNS discovery), retiring the boot-time single-replica guard.
- A `FencedCoordinator` = `owner` + `Fencer::acquire(key)` stamping the round on
  every apply; back the `Fencer` with the ZAP-agreed round.
- Model pdb + surgeColocation (unblock 11+3 CRs), then networkPolicy/
  serviceMonitor/kmsSecrets; apply-time prune of orphaned owned objects.

Local build/test (repo `.cargo/config.toml` pins lld, absent on some boxes):
`CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=cc
CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="" cargo test -p hanzod-operator`.
