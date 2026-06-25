# Cloud ↔ ComputeDEX Integration — Decentralized Compute Broker

Status: design + first working slice (this branch: `feat/compute-broker-integration`).
Owner: node + desktop. Cloud-side hooks are specified here as a contract; the
cloud repo is changed by its own owner, not in this branch.

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
| PQ signature | `ai.VerifyMLDSA(pk,msg,sig)` | ML-DSA 44/65/87 (FIPS 204), level auto-detected from pk size |
| Double-spend guard | `ai.IsSpent`/`MarkSpent(stateDB, workId)` | per-chain spent set, key `BLAKE3("spnt" ‖ workId)` at precompile addr `0x0300` |
| TEE gate | `ai.VerifyTEE(receipt,sig)` | structural today; platform SDK (NVTrust/SGX) is the upgrade path. **Not consensus-critical** — gates reward eligibility only |
| Participant proof engine | desktop `apps/hanzo-desktop/src-tauri/src/mining/` | already builds byte-exact proofs, signs ML-DSA-65, verifies via `luxprecompile_sys` FFI, persists history. On-chain submit was "Phase 2" — this design is Phase 2 |
| Compute market | `contracts/ComputeDEX.sol`, `contracts/OrderBook.sol` | AMM + CLOB over resource types; settle in `HANZOToken` |
| Token | `contracts/HANZOToken.sol` | ERC20, `rewardProvider` mint path, provider claim cooldown |

The desktop's `proof.rs` header is explicit: *"Byte layout MUST match
`lux/precompile/ai/ai_mining.go`"*. That file is the contract. This design
honors it on the cloud/broker/on-chain sides too, so a proof a desktop produces
verifies identically in the broker, in the settlement contract, and in the
A-Chain VM later.

## 3. The real gap (what this work closes)

1. **`ComputeDEX.fillOrder` settles on faith.** It transfers payment to the
   provider and credits the buyer an abstract `userResources` counter — there is
   no proof the job ran, and no double-spend guard. A provider can be paid for
   work never delivered; a buyer can be charged for nothing.
2. **No registry of *who can serve what*.** `createOrder` advertises a price for
   a `ResourceType`, but the cloud broker needs: model(s) served, endpoint
   identity (the node's ML-DSA pubkey), capacity, privacy tier, liveness.
3. **No broker.** Nothing maps a cloud inference request → a registered node →
   an escrowed job → a proven settlement.
4. **`AITestMiner.sol` is a mint stub** (100 tokens for any unused `bytes32`,
   zero verification). It is a placeholder, not the reward path. Real rewards
   flow through proof-gated settlement.

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

The cloud must trust a participant's result. Three tiers, selectable per request:

1. **Attested (privacy tier ≥ CPU/GPU TEE).** Provider returns a TEE quote in
   the work-proof `teeQuote` tail. `VerifyTEE` gates eligibility; reward
   multiplier rises (1.5x–2.0x). Redundancy = 1. This is the target steady state
   once platform TEE SDKs are wired (NVTrust/SGX). Today `VerifyTEE` is
   structural, so attested-tier is *advisory* until the SDK lands (roadmap P3).
2. **Redundancy-verified.** Broker dispatches the *same* job to N≥2 independent
   providers (distinct nodeIds, ideally distinct ASNs) and compares results. For
   deterministic workloads (temp=0, fixed seed) byte-equality; for stochastic,
   a semantic-equivalence check (embedding cosine ≥ threshold via the local
   embedding engine on :36901). Agreeing providers split the reward; a dissenter
   is slashed. This needs no TEE and works today.
3. **Reputation-weighted sampling.** Steady-state optimization: trust a
   high-reputation provider at redundancy 1 but spot-check a random ε fraction of
   its jobs with a second provider. Reputation is the EWMA of agreement rate;
   disagreement on a spot-check slashes and resets reputation. Defends against a
   provider that builds trust then defects.

All three settle through the **same** `ComputeSettlement` + spent-set, so a
work-proof can never be paid twice regardless of tier. Slashing maps to the
economics doc: SLA 1–5%, attestation failure 10%, malicious 100%.

### 5.1 Sybil / collusion notes

- nodeId is pubkey-derived, free to mint → identity alone is not scarce.
  Scarcity comes from **provider stake** (economics doc: min stake to serve;
  slashing burns it). Redundancy across distinct stakes raises collusion cost.
- Redundancy-verified is vulnerable to a provider colluding with itself across
  Sybil identities. Mitigation: stake per identity + ASN/IP diversity in matcher
  + random pairing (a provider can't choose its verifier).

## 6. Settlement contract (`ComputeSettlement.sol`)

Closes gap #1. Escrowed, proof-gated, double-spend-safe. Mirrors the precompile
semantics in pure Solidity so it is testable without the precompile, and so the
A-Chain VM can later swap the in-contract checks for `0x0300` precompile calls
(identical results — same BLAKE3 work-id, same reward formula, same spent set).

Lifecycle: `openJob` (buyer escrows HANZO, binds workId) → provider executes
off-chain → `settle(workId, workProof, mldsaSig)` (verify → pay provider from
escrow, mark workId spent) OR `slashJob`/`refundExpired` (timeout → buyer
refunded, provider reputation penalized). See the contract + tests in
`contracts/`.

The contract verifies ML-DSA via the `0x0300` precompile when deployed on a
Hanzo/Lux/Zoo EVM (production); on a vanilla EVM (CI/foundry) it accepts an
operator-attested settlement path guarded by the broker key, so the escrow,
work-id binding, reward math, and spent-set are all exercised in tests. The
precompile path and the operator path produce identical state transitions.

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

## 9. Phasing — see ROADMAP section at end of this doc and the slice below.
