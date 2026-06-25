# Cloud ↔ Compute Broker — Decentralized Compute Marketplace

Status: design + first tested slice (branch `feat/compute-broker-integration`).
Owner: node + desktop. Cloud-side hooks are specified here as a contract; the
cloud repo is changed by its own owner, not in this branch.

This supersedes the original "ComputeDEX" framing. The compute market is **not**
an AMM: `ComputeDEX.sol` (a constant-product `x*y=k` pool that paid on faith) is
deleted. The canonical marketplace is (a) **HMM pricing** off-chain
(`hanzo-hmm::compute_price`), (b) a **canonical-proof-gated escrow** on-chain
(`ComputeSettlement` → `@luxfi/standard/ai`, LP-302, Freivalds, **no TEE**), and
(c) a **broker** that matches requests to participant nodes. See §9–§11 for the
broker home, desktop participant mode, and honest phasing.

## 1. Goal

Anyone runs `hanzo/node` + `hanzo/desktop`, registers their GPU/CPU, serves AI
backends, and earns HANZO via Proof-of-AI. A `cloud.hanzo.ai` inference request
that the central pool can't (or doesn't want to) serve is *brokered* onto a
participant node, executed there, proven, and settled in HANZO — with the cloud
trusting the result through attestation + redundancy, never blind faith.

## 2. What already exists (do not rebuild)

The primitives are canonical and shared by Hanzo/Lux/Zoo EVMs. The integration
*wires* them; it must not redefine any of them.

| Primitive | Source of truth | Semantics |
|-----------|-----------------|-----------|
| Work-proof wire format | `github.com/luxfi/precompile/ai` `BuildWorkProof` / `ParseWorkProof` | `deviceId[32] ‖ nonce[32] ‖ ts(u64 BE) ‖ privacy(u16 BE) ‖ computeMins(u32 BE) ‖ teeQuote[..]`, min 78B |
| Work-id | `ai.ComputeWorkId(deviceId,nonce,chainId)` | `BLAKE3(deviceId ‖ nonce ‖ chainId_be)` |
| Reward | `ai.CalculateReward(proof,chainId)` | `1e18 × computeMins × privacyMult/10000`; mult = {Public 2500, Private 5000, Confidential 10000, Sovereign 15000} |
| Canonical compute proof | `@luxfi/standard/ai` `ComputeProofLib` / `ComputeVerifier` (LP-302) | `ComputeProof{proofType, reportData, evidence}`; `verify` enforces binding (`reportData == expectedReportData`) + governance-accepted runtime + evidence backend. **proofType 3 = OptimisticEvidence (Freivalds re-exec) — NO TEE, NO GPU attestation.** This is the ONE proof system; there is no second implementation. |
| Compute settlement | `contracts/src/ComputeSettlement.sol` | escrow + canonical-proof-gated + replay-guarded payout (`openJob`/`settle`/`slashJob`/`refundExpired`). `settle` pays the escrow to the operator iff `ComputeVerifier.verify` accepts the proof for the job's binding; replay guard keys on the canonical `reportData`. |
| Compute pricing (HMM) | `hanzo/net` `hanzo-hmm` `src/compute_price.rs` (`price(job, market)`) | Hamiltonian Market Maker, **NOT** `x*y=k`. Prices a heterogeneous, SLA-bound job from the Hamiltonian energy-modulated equilibrium + perishability + tier; bounded, deterministic. The broker calls this to set the escrow amount. |
| PQ signature (transport) | `ai.VerifyMLDSA(pk,msg,sig)` | ML-DSA 44/65/87 (FIPS 204). Used for the broker↔provider dispatch envelope and provider identity, **not** as the compute proof (the compute proof is Freivalds, above). |
| Participant proof engine | desktop `apps/hanzo-desktop/src-tauri/src/mining/` | builds work-proofs, signs ML-DSA-65, verifies via `luxprecompile_sys` FFI, persists history. Phase-2 work: emit the canonical `ComputeProof` (Freivalds opening from `hanzo-engine`'s int8 accumulator) and submit on-chain. |
| Token | `contracts/HANZOToken.sol` | ERC20; the escrow + settlement medium. |

**Removed, deliberately.** The earlier design listed `ComputeDEX.sol` (a
constant-product AMM that settled `fillOrder` on faith) and a `VerifyTEE` gate.
Both are **deleted**: LP-302 (Final) makes the compute proof a recomputable
Freivalds side-effect with **NO TEE**, and `x*y=k` is the wrong model for
perishable/heterogeneous/SLA-bound compute (that is what the HMM is for). The
canonical settlement is `ComputeSettlement` gating on `@luxfi/standard/ai`.

The settlement contract's `_reportData` delegates *directly* to the canonical
`ComputeProofLib.expectedReportData` — same domains (`lux/aivm/compute-{challenge,
report}/v1`), same keccak chain — so a proof a participant produces verifies
byte-for-byte in the broker, in `ComputeSettlement`, and in the A-Chain VM.

## 3. The real gaps (what this work closes)

1. **Faith-paying settlement → CLOSED.** `ComputeDEX.fillOrder` paid the provider
   with no proof of delivery and no double-spend guard. ComputeDEX is **deleted**;
   `ComputeSettlement` pays only against a valid canonical compute proof, with a
   `reportData`-keyed replay guard (P1, done).
2. **Wrong pricing model → CLOSED.** A constant-product (`x*y=k`) AMM cannot price
   perishable/heterogeneous/SLA-bound compute. Pricing is the HMM
   (`hanzo-hmm::compute_price`), priced from the Hamiltonian equilibrium +
   perishability + tier, bounded and deterministic (P1, done).
3. **Mint stub → CLOSED.** `AITestMiner.sol` (100 tokens for any `bytes32`, zero
   verification) is **deleted**. The reward path is the canonical proof-gated
   `AIMiner` (`@luxfi/standard/ai`) + `ComputeSettlement` (P1, done).
4. **No registry of *who can serve what* → OPEN (P2).** The broker needs each
   provider's models, endpoint identity (ML-DSA pubkey), capacity, tier, liveness.
5. **No broker → OPEN (P2).** Nothing yet maps a cloud request → a registered
   node → an escrowed job → a proven settlement. The broker home and shape are
   §9–§10; status is §11.

## 4. Architecture

```
            ┌──────────────────────────────────────────────────────────────┐
            │ cloud.hanzo.ai (owned elsewhere; integrates via §7 contract)   │
            │  LLM request → can central pool serve?  ── yes ─→ serve locally│
            │                         │ no / overflow / cheaper-on-edge      │
            └─────────────────────────┼──────────────────────────────────────┘
                                      ▼
                          ┌───────────────────────┐
                          │   Compute Broker (Go)  │  pkg: compute/broker
                          │  - registry (capacity) │
                          │  - match(req)→provider  │
                          │  - escrow open (on-chain│
                          │    ComputeSettlement)   │
                          │  - dispatch job         │
                          │  - verify proof+result  │
                          │  - settle / slash       │
                          └───────────┬────────────┘
                  register/heartbeat  │  job dispatch (signed)        proof
            ┌───────────────────────┐ │ ┌──────────────────────────┐  ▲
            │ desktop participant   │◄┘ └►│ desktop participant      │──┘
            │  node identity (ML-DSA│     │  hanzo-engine :36900     │
            │  capacity: GPU/model  │     │  runs inference, returns │
            │  /participant panel   │     │  result + work-proof     │
            └───────────────────────┘     └──────────────────────────┘
                                      │
                                      ▼
                    ┌────────────────────────────────────────┐
                    │  EVM (C-Chain / future A-Chain VM)       │
                    │  ComputeSettlement.sol  (escrow + proof) │
                    │  ai precompile 0x0300 (verify/workid/    │
                    │  reward/spent-set)  HANZOToken (pay)     │
                    └──────────────────────────────────────────┘
```

### 4.1 Components

- **Registry** — providers register a `Provider{ nodeId, mldsaPubKey, models[],
  resourceType, capacity, pricePerUnit, privacyTier, endpoint }` and heartbeat.
  `nodeId = BLAKE3(mldsaPubKey)[:20]` (mirrors the desktop device-id derivation
  and the KMS service-identity convention). Stale providers (no heartbeat within
  TTL) are not matched.
- **Matcher** — given an inference request `{model, maxPriceWei, privacyTier,
  redundancy}`, returns N eligible providers ranked by `price ↑, reputation ↓,
  privacyTier ≥ requested`. N = redundancy factor (1 for trusted-attested, ≥2
  for redundancy-verified — see §5).
- **Escrow** — broker opens a job on `ComputeSettlement.sol`: buyer's HANZO is
  locked, `workId = ComputeWorkId(providerDeviceId, jobNonce, chainId)` is bound
  to the job. One job ⇒ one workId ⇒ one settlement (spent-set enforced).
- **Dispatch** — broker sends the job to the provider's endpoint with a
  broker-signed envelope (reuse the KMS envelope shape: ML-DSA over
  `SHAKE256(domain ‖ digest ‖ canonical-json)`, 5-min skew). Provider runs
  inference on `hanzo-engine`, returns `{result, workProof, mldsaSig}`.
- **Verify** — broker checks: (a) `VerifyMLDSA(providerPub, workIdMsg, sig)`,
  (b) privacy/TEE tier if claimed, (c) result acceptance per §5. On pass it calls
  `ComputeSettlement.settle(workId, proof, sig)`; the contract re-derives the
  workId, checks the spent-set via the precompile, pays the provider from escrow,
  marks spent. On fail → `slash`/refund.

### 4.2 Why a broker and not pure on-chain matching

Inference is sub-second and high-frequency; on-chain order matching per request
is economically absurd (gas) and too slow. The broker does match + dispatch
off-chain at request latency; the chain does only what must be trustless:
**escrow, proof verification, double-spend prevention, settlement**. This is the
same split the economics doc already prescribes ("commit-reveal with timeout").

## 5. Trust / verification model

The cloud must trust a participant's result. The **primary** gate is the
canonical compute proof (LP-302); redundancy and reputation are
defense-in-depth, not the basis of trust.

1. **Compute-proven (canonical, the default).** The provider returns the result
   plus a canonical `ComputeProof` (proofType 3 = Freivalds re-exec over the
   exact int8 accumulator of `hanzo-engine`). `ComputeVerifier.verify` checks the
   proof binds to *this* job (`reportData == expectedReportData`) under a
   governance-accepted runtime, then the evidence backend attests it. Soundness
   is information-theoretic (`≤ 2^-61` per challenge vector) and **post-quantum by
   construction** — no TEE, no GPU attestation, no hardware trust. Redundancy = 1
   because the proof, not a second opinion, is the guarantee. This is the steady
   state.
2. **Redundancy-verified (transition / non-quantized tiers).** Where a workload
   is not on the exact-integer Freivalds path (e.g. fp16/bf16 today), the broker
   dispatches the *same* job to N≥2 independent providers (distinct nodeIds,
   ideally distinct ASNs) and compares: byte-equality for deterministic
   (temp=0, fixed seed) work; embedding-cosine ≥ threshold for stochastic.
   Agreeing providers split the reward; a dissenter is slashed. Needs no TEE.
3. **Reputation-weighted sampling.** Optimization over (2): trust a
   high-reputation provider at redundancy 1 but spot-check a random ε fraction
   with a second provider. Reputation is the EWMA of agreement rate; a
   spot-check disagreement slashes and resets it. Defends a build-trust-then-
   defect attacker.

TEE attestation is **not** a tier here — LP-302 removed it from the proof path.
A TEE may still raise a provider's *reputation* off-chain, but it never
substitutes for the compute proof. All paths settle through the **same**
`ComputeSettlement` + canonical-`reportData` spent-set, so one computation is
paid exactly once. Slashing maps to the economics doc: SLA 1–5%, proof failure
10%, malicious 100%.

### 5.1 Sybil / collusion notes

- nodeId is pubkey-derived, free to mint → identity alone is not scarce.
  Scarcity comes from **provider stake** (economics doc: min stake to serve;
  slashing burns it). Redundancy across distinct stakes raises collusion cost.
- Redundancy-verified is vulnerable to a provider colluding with itself across
  Sybil identities. Mitigation: stake per identity + ASN/IP diversity in matcher
  + random pairing (a provider can't choose its verifier).

## 6. Settlement contract (`ComputeSettlement.sol`)

Closes gap #1. Escrowed, **canonical-proof-gated**, double-spend-safe. It does
**not** implement a second proof system — it consumes `@luxfi/standard/ai`
(LP-302) as a foundry submodule (`contracts/lib/standard`) and gates on it.

Lifecycle: `openJob(jobId, escrow, deadline, Binding)` (buyer escrows HANZO and
binds the canonical proof fields — `taskId, intentID, modelSpecHash, promptHash,
openBlockHash, operator, outputHash, runtimeMeasurement`) → provider executes
off-chain and produces a `ComputeProof` → `settle(jobId, ComputeProof)`
(recompute `reportData = ComputeProofLib.expectedReportData(binding)`; require
`ComputeVerifier.verify(proof, reportData, runtimeMeasurement) == true`; pay the
escrow to `binding.operator`; mark `reportData` spent) OR
`slashJob`/`refundExpired` (failure / timeout → buyer refunded, provider
reputation penalized; the binding is **not** marked spent so a retry can re-bind).

Why this shape: the contract owns only what must be trustless — escrow, the
canonical proof gate, the replay guard, and pay/slash/refund. The proof's
correctness lives in `@luxfi/standard/ai` (one implementation, every chain); the
*runtime-acceptance* policy lives in the verifier's registry; the *price* is
computed off-chain by the broker via the HMM. Each concern in exactly one place.

Tests: `contracts/test/ComputeSettlement.t.sol` — a test-only ERC20 mock for
HANZO and a `MockVerifier` (returns true iff `reportData == expected`,
simulating a valid Freivalds binding deterministically) exercise the full
lifecycle: escrow, settle-on-valid-binding (exact payout + spent), revert on
wrong binding (nobody paid), double-settle/replay guard, refundExpired, slash.
The genuine Freivalds `OptimisticEvidence` backend is exercised in
`luxfi/standard`'s own suite; at deploy time `verifier` points at a deployed
`ComputeVerifier` with that backend registered for proofType 3.

## 7. Cloud-side integration contract (for the cloud repo owner)

The cloud repo is **not** edited here. It integrates by implementing one
outbound interface and one inbound webhook:

- **Outbound** (cloud → broker): `POST /v1/broker/jobs`
  `{ model, inputRef, maxPriceWei, privacyTier, redundancy, deadlineMs }`
  → `{ jobId, workId, providers[], escrowTx }`. The broker is reachable in-cluster
  (`broker.hanzo.svc`) and authenticates the caller via IAM JWT (`owner` claim =
  org; jobs are org-scoped). Identity headers per the X-* convention; the gateway
  injects them.
- **Inbound** (broker → cloud): the broker streams/returns the chosen provider's
  result so the cloud can relay it to the end user. For overflow-from-central, the
  cloud's existing model-router gains a "broker" backend that looks identical to
  any other upstream (OpenAI-compatible `/v1/chat/completions`), so no cloud
  scheduler rewrite — the broker *is* an OpenAI-compatible upstream that happens
  to fan out to participant nodes.

This keeps separation of concerns: cloud owns request routing and the user
session; broker owns provider selection, escrow, proof, settlement; node/desktop
own execution and proof generation.

## 8. Security / ops

- No secrets in code. Broker reads its ML-DSA signing identity from a
  KMS-provisioned `LUX_MNEMONIC` (the consensus-native service-identity recipe),
  derives via `luxfi/keys.NewServiceIdentity`. Same mnemonic ⇒ same nodeId.
- Provider payments mint/transfer HANZO only through the settlement contract's
  escrow — never a direct broker-held key paying out.
- Broker is stateless-restartable: registry is rebuildable from on-chain
  registration events + heartbeats; in-flight jobs are recoverable from
  `ComputeSettlement` job state (escrow is the source of truth).
- Health/readiness, Prometheus metrics, structured logs, graceful shutdown —
  standard service contract.

## 9. Broker home — `hanzo/net` (Rust), not new Go

The broker is a service in the existing Hanzo network workspace
`~/work/hanzo/net`, **not** a new Go package. That workspace already holds the
real, composable primitives the broker needs, so building it elsewhere would
create a second compute system (the exact duplication this work removes):

- `hanzo-hmm` — the canonical HMM; `compute_price::price(job, market)` is the
  broker's pricing call (one home for pricing).
- `hanzo-compute` — "BitTorrent-style Decentralized Compute Protocol"
  (peer/piece/scheduler/swarm/verifier): the provider registry + job dispatch
  substrate.
- `hanzo-mining` — `bridge`/`evm`/`ledger`/`wallet`: the on-chain seam that
  opens the escrow and submits `settle` to `ComputeSettlement`.
- `hanzo-jobs`, `hanzo-job-queue-manager` — job lifecycle + queueing.
- `hanzo-pqc`, `hanzo-did`, `hanzo-identity` — the ML-DSA service identity for
  the dispatch envelope and provider nodeId.

The broker's KMS-provisioned `LUX_MNEMONIC` service identity (§8) derives via
`luxfi/keys.NewServiceIdentity`, consistent with the rest of the stack.

## 10. Desktop participant mode — "contribute compute & earn"

The user-facing participant is `~/work/hanzo/desktop` (a dcSpark/Shinkai fork,
Tauri + React). It already mines locally
(`apps/hanzo-desktop/src-tauri/src/mining/`): builds work-proofs, signs ML-DSA-65,
verifies via `luxprecompile_sys` FFI, persists history; and it already serves
OpenAI-compatible inference via the embedded `hanzo-engine` on `:36900`. The
participant-mode work turns this from local-only into an earning provider:

1. **Register.** A "Contribute compute" toggle in the desktop UI registers the
   node with the broker: `Provider{ nodeId = BLAKE3(mldsaPubKey)[:20], mldsaPubKey,
   models[], resourceKind, capacity, pricePerUnit, tier, endpoint }`, then
   heartbeats. nodeId is derived, never typed.
2. **Receive + execute.** The broker dispatches a signed job envelope to the
   node's `endpoint`; the node verifies the broker signature, runs inference on
   the embedded engine, and — the Phase-2 deliverable — emits a canonical
   `ComputeProof` (proofType 3): the Freivalds opening derived from the engine's
   **exact int8 accumulator** (the same `reportData` binding the chain checks),
   not a TEE quote. It returns `{result, computeProof}`.
3. **Settle + earn.** The broker calls `ComputeSettlement.settle(jobId, proof)`;
   on a valid proof the escrowed HANZO pays the node's wallet. The desktop's
   existing balance/history UI shows realized (not just pending) earnings.

Separation of concerns: cloud owns request routing + user session; broker owns
provider selection, HMM pricing, escrow, proof check, settlement; desktop/node
own execution and proof generation. The cloud repo is **not** edited here — it
integrates via the §7 contract (the broker is an OpenAI-compatible upstream).

## 11. Phasing — honest status

| Phase | Scope | Status |
|-------|-------|--------|
| P0 | Canonical proof layer (`@luxfi/standard/ai`, Freivalds, no TEE) | **done** (luxfi/standard; consumed here) |
| P0 | Dedup: delete dead ComputeDEX (4 repos); declare canonical HMM home | **done** |
| P1 | `ComputeSettlement` gated on canonical `ComputeVerifier` + tests | **done** (this branch) |
| P1 | HMM compute pricing `hanzo-hmm::compute_price` + property tests | **done** (`hanzo/net`) |
| P2 | Broker service in `hanzo/net` (registry, matcher, dispatch, settle) | **not started** |
| P2 | Desktop: register/heartbeat + canonical `ComputeProof` emission + on-chain submit | **not started** |
| P2 | Wire HMM price → broker escrow amount → `openJob` end-to-end on a live devnet | **not started** |
| P3 | Cloud-side §7 "broker" upstream backend (owned by the cloud repo) | **not started** (other owner) |

The first vertical slice that is *proven by tests today*: **a heterogeneous,
SLA-bound job is priced by the HMM (`hanzo-hmm::compute_price`, NOT `x*y=k`), and
that price settles in HANZO only against a valid canonical compute proof
(`ComputeSettlement` → `ComputeVerifier`, Freivalds, no TEE), with a
replay-guarded, escrow-safe lifecycle.** P2/P3 (the live broker service, desktop
provider mode, and cloud upstream) are multi-week and explicitly not yet built.
