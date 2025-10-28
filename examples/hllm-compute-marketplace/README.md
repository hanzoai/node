# HLLM Compute Marketplace Examples

Practical examples showing how advanced AI features work on Hanzo's decentralized compute network.

## Examples

### 1. HLLM Regime Routing (`regime_routing.rs`)
Shows how a Hamiltonian Hidden-Markov LLM routes requests to specialized models based on context regimes (medical, legal, code, creative).

### 2. Hamiltonian Market Maker (`market_maker.rs`)
Demonstrates dynamic pricing for compute resources using Hamiltonian mechanics to balance supply/demand.

### 3. HANZO Token Payments (`token_payments.rs`)
Complete payment flow for compute jobs using HANZO tokens with the ComputeDEX AMM.

### 4. Compute Verification (`verification.rs`)
Proof generation and verification for compute jobs using TEE attestations and zero-knowledge proofs.

### 5. Simulation Marketplace (`simulation_market.rs`)
End-to-end example of the Zoo-1 simulation tiers being traded on the marketplace.

## Quick Start

```bash
# Run all examples
cargo run --example regime_routing
cargo run --example market_maker
cargo run --example token_payments
cargo run --example verification
cargo run --example simulation_market

# Run with local Hanzo node
export HANZO_NODE_URL=http://localhost:3690
export HANZO_WALLET_KEY=your-private-key
cargo run --example simulation_market -- --live
```

## Architecture

```
User Request → HLLM Router → Regime Selection → Compute Market
                                ↓
                        Hamiltonian Pricing
                                ↓
                        Provider Matching
                                ↓
                        HANZO Payment
                                ↓
                        Job Execution
                                ↓
                        Proof Generation
                                ↓
                        Settlement
```

## Key Concepts

### HLLM (Hamiltonian + Hidden Markov LLM)
- **Hidden Markov Model**: Tracks latent regimes (contexts) over time
- **Regime-specific Adapters**: Small LoRA/BitDelta weights per domain
- **Hamiltonian Dynamics**: Governs resource allocation and pricing

### Compute Tiers (Zoo-1 Simulation)
- **Tier A**: V-JEPA2 latent simulation (~$0.001/request)
- **Tier B**: WebXR/WebGPU browser simulation (~$0.01/request)
- **Tier C**: MuJoCo/Brax high-fidelity (~$0.10/request)

### Market Mechanics
- **Supply**: GPU providers post capacity and price curves
- **Demand**: Users submit jobs with SLOs and budgets
- **Clearing**: Hamiltonian market maker finds equilibrium
- **Settlement**: Smart contracts handle payment on completion