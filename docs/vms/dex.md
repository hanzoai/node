# Hanzo DEX (D-Chain)

The Hanzo DEX is the on-chain CLOB+AMM that ships natively in `hanzod`.
It runs as the `dexvm` VM (D-Chain), with order matching executed by
`luxcpp/dex` via cgo bindings.

> Reference papers: [`lightspeed-dex`](../../../papers/), [`lp-9010-dex-precompile`](../../../../lux/lps/).
> See also [`@hanzo/sdk` (JS)](../../../js-sdk/) and [`hanzo-sdk` (Go)](../../../go-sdk/).

## Quick Start

```bash
# 1. Run hanzod with the D-Chain tracked.
hanzod --track-chains=D --network-id=mainnet

# 2. Place an order via the SDK.
curl -X POST http://127.0.0.1:9650/ext/bc/D/rpc \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"dex_placeOrder","params":[{
        "pair":"AI/USDC","side":"buy","price":"0.50","size":"100"
      }]}'
```

## Endpoints

| Endpoint | Path | Purpose |
| --- | --- | --- |
| RPC | `http://127.0.0.1:9650/ext/bc/D/rpc` | JSON-RPC |
| WebSocket | `ws://127.0.0.1:9650/ext/bc/D/ws` | live book / fill streams |

## RPC Methods

| Method | Params | Returns |
| --- | --- | --- |
| `dex_placeOrder` | `{pair, side, price, size}` | `{orderID, txHash}` |
| `dex_cancelOrder` | `[orderID]` | `txHash` |
| `dex_getBook` | `[pair, depth]` | `{bids: [{price,size}], asks: [{price,size}]}` |
| `dex_getOrder` | `[orderID]` | `{state, fills, ...}` |
| `dex_listFills` | `[pair, since]` | `[{price,size,time}]` |

## Example Tx (place + cancel)

```jsonc
// place
{"method":"dex_placeOrder","params":[{
  "pair":"AI/USDC","side":"buy","price":"0.50","size":"100"
}]}
// returns
{"orderID":"0xabc...","txHash":"0x..."}

// cancel
{"method":"dex_cancelOrder","params":["0xabc..."]}
```

## Triumvirate Note

Hanzo's three-pillar VM stack — **DEX + EVM + FHE** — is one
coherent triumvirate, not a trio of separate services. Orders placed
on D-Chain settle against AI-mining rewards on C-Chain (cevm) and
can carry encrypted side-information executed on T-Chain (FHE).
