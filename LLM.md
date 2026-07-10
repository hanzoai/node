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

## Kubernetes operator — hanzod IS the operator

The canonical Rust k8s operator is merged into this repo at `./operator`
(crate/binary `operator`; the standalone `hanzoai/operator` repo is retired).
The same `hanzod` binary fronts it:

```
hanzod operator [args]   # run the reconcile loop (28 CRD Kinds, all universes)
hanzod install  [args]   # install hanzod as the in-cluster operator
```

- **Boundary:** supervised multi-binary in ONE image, over the k8s API — not
  FFI. `hanzod operator`/`hanzod install` (`main.go` switch →
  `operator_dispatch.go`) `execve` into the `operator` binary. The Go node
  (luxfi/node) and the Rust operator never share an address space; the only
  cross-process contract is the CRD schema. Two orthogonal build graphs, one
  image.
- **Build (own workspace):** `cd operator && cargo build` — the operator has its
  own empty `[workspace]`, so it is excluded from the node's Rust workspace and
  its kube/k8s-openapi deps stay decoupled from the node's blockchain/AI graph.
- **Docs:** `operator/LLM.md` (subcommands, wire contract = live `0.6.19`,
  cutover + k3s/ConsensusSet follow-ups).
