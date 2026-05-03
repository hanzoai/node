# Hanzo EVM (C-Chain)

`hanzod`'s C-Chain runs the GPU-accelerated `cevm` from `luxcpp/cevm`
exposed via the `luxfi/cevm` Go cgo bridge. AI-specific precompiles
(AIVM range `0x0a01–0x0a08`) are layered on top via chain config.

> Reference papers: [`lp-009-gpu-native-evm`](../../../../lux/lps/),
> [`hanzo-ai-chain`](../../../papers/) — AIVM precompiles.

## Quick Start

```bash
hanzod --track-chains=C --network-id=mainnet

# Standard JSON-RPC.
curl -X POST http://127.0.0.1:9650/ext/bc/C/rpc \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}'
```

## Endpoints

| Endpoint | Path | Purpose |
| --- | --- | --- |
| HTTP RPC | `/ext/bc/C/rpc` | `eth_*`, `web3_*`, `net_*` |
| WebSocket | `/ext/bc/C/ws` | subscriptions |
| Admin | `/ext/admin` | `admin_*` (gated) |

## Standard EVM RPC

`eth_chainId`, `eth_blockNumber`, `eth_getBalance`, `eth_call`,
`eth_estimateGas`, `eth_sendRawTransaction`, `eth_getTransactionReceipt`,
`eth_getLogs`, `eth_subscribe` (newHeads, logs, newPendingTransactions).

## AI Precompiles (AIVM)

| Address | Purpose | Gas |
| --- | --- | --- |
| `0x0a01` | `AI_VERIFY_SIG` — ML-DSA inference attestation | 100k |
| `0x0a02` | `AI_TEE_QUOTE` — NVIDIA Confidential Compute quote | 50k |
| `0x0a03` | `AI_MODEL_REGISTRY` — register/lookup model hash | 25k |
| `0x0a04` | `AI_REWARD_CLAIM` — claim mining reward | 30k |
| `0x0a05` | `AI_INFERENCE_ATTEST` — pose-of-inference attestation | 75k |
| `0x0a06` | `AI_GPU_BENCH` — verifiable GPU benchmark | 50k |
| `0x0a07` | `AI_DATASET_PROOF` — dataset commitment | 40k |
| `0x0a08` | `AI_AGENT_BIND` — bind agent to wallet | 20k |

## Example Tx

```js
import { ChainClient } from "@hanzo/sdk/chain";
const c = new ChainClient("http://127.0.0.1:9650/ext/bc/C/rpc", fetch);
console.log(await c.chainId()); // 0x9039 (36921 hanzo mainnet)
```

## Triumvirate Note

The EVM is the orchestration layer of the **DEX + EVM + FHE** triumvirate.
Smart contracts on C-Chain emit DEX trades against D-Chain and request
encrypted-compute attestations from T-Chain.
