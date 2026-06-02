# Hanzo FHE (T-Chain)

`hanzod`'s T-Chain (ThresholdVM) provides Fully Homomorphic Encryption
backed by `luxcpp/fhe` (CKKS for arithmetic, TFHE for boolean circuits).
Threshold decryption is performed by validators using the lattice-based
DKG from `luxfi/lattice`.

> Reference papers: [`lp-013-fhe`](../../../../lux/lps/),
> [`hanzo-fhe-inference`](../../../papers/).

## Quick Start

```bash
hanzod --track-chains=T --network-id=mainnet
```

## Endpoints

| Endpoint | Path | Purpose |
| --- | --- | --- |
| RPC | `/ext/bc/T/rpc` | encrypted-tx submission |
| Public Key | `/ext/bc/T/publicKey` | network FHE encryption key |

## RPC Methods

| Method | Params | Returns |
| --- | --- | --- |
| `fhe_publicKey` | — | network FHE pubkey (CKKS or TFHE) |
| `fhe_submit` | `[{ciphertext, op}]` | `txHash` |
| `fhe_result` | `[txHash]` | encrypted result bytes |
| `fhe_decrypt` | `[ciphertext]` | (validators only) threshold-decrypt |

## Precompiles (C-Chain bridge)

| Address | Purpose |
| --- | --- |
| `0x0700` | FHE main entry — invoke from EVM contracts |
| `0x0701` | ECIES |
| `0x0702` | Ring signatures |
| `0x0703` | HPKE |

## Example: Encrypted Inference

```python
import parsdao_sdk  # or @hanzo/sdk
from parsdao_sdk import ParsClient, default_local, EncryptedTx

with ParsClient(default_local()) as c:
    pk = c.fhe.public_key()
    cipher = encrypt_with_ckks(pk, my_input_vector)
    h = c.fhe.submit(EncryptedTx(ciphertext=cipher, op="inference"))
    enc_result = c.fhe.result(h)
```

## Triumvirate Note

FHE is the privacy pillar of the **DEX + EVM + FHE** triumvirate.
Encrypted inputs flow through C-Chain contracts that delegate the
homomorphic step to T-Chain, then settle public results back on
C-Chain or DEX outcomes on D-Chain.
