# node

[![Open in Hanzo](https://hanzo.app/badges/open-in-hanzo.svg)](https://hanzo.app/dev/open?repo=github.com/hanzoai/node&ref=main&action=open) [![Edit with Hanzo](https://hanzo.app/badges/edit-with-hanzo.svg)](https://hanzo.app/dev/open?repo=github.com/hanzoai/node&ref=main&action=edit) [![Deploy with Hanzo](https://hanzo.app/badges/deploy-with-hanzo.svg)](https://hanzo.app/dev/open?repo=github.com/hanzoai/node&ref=main&action=deploy)

`hanzo-chain` — the Hanzo network validator. `hanzod` is the per-machine daemon (`daemon/`, HIP-1330); the validator lives in `chain/` and runs as its child on opt-in.

It holds this validator's keys, joins the network's validator mesh over a
post-quantum handshake, decides blocks under the committee that network
published, executes them, and serves JSON-RPC.

A block is verified before this validator signs for it, certified before it is
accepted, and accepted only once a certificate exists. So this node never signs
for a state it has not computed and never holds a state nobody agreed to.

## Networks

| network | network id | chain id |
| ------- | ---------- | -------- |
| mainnet | 1          | 36963    |
| testnet | 2          | 36962    |
| devnet  | 3          | 36964    |

Two numbers, because there are two questions. The network id is what validators
greet each other under. The chain id is what a transaction is signed against.
Both are compiled in, with the genesis each network starts from. A name that is
not one of the three is an error, never a default.

## Build

```
cargo build --release
```

The binary is `target/release/hanzo-chain`.

## Run

A validator is named by the key it proves on every link, so there is no name to
claim without the key. The keys are made on first use and kept, readable only
by their owner. A committee is the list of what its validators published, so
bringing a network up takes two steps:

```
for n in 0 1 2 3 4; do hanzo-chain --data n$n --publish >> committee.txt; done
```

Each line is an ML-DSA-65 identity of 1952 bytes, the BLS key of 48 bytes that
validator votes with, and a 96-byte proof it holds that key.

```
for n in 0 1 2 3 4; do
  hanzod --network devnet --data n$n --committee committee.txt \
    --peers 127.0.0.1:19631,127.0.0.1:19633,127.0.0.1:19635,127.0.0.1:19637,127.0.0.1:19639 \
    --rpc 127.0.0.1:$((19630 + 2*n)) --mesh 127.0.0.1:$((19631 + 2*n)) &
done
```

The peer list is positions, not names: the names are already fixed by the
committee and the handshake proves them, so an address that turns out to belong
to someone else is refused rather than believed.

```
hanzo-chain 0.2.0
network     devnet
network id  3
chain id    36964
chain       3e093f8f46050497aa2585a4aa902700617ff3c843d52d3f80c168dfc1d87084  (derived from the genesis; none was issued)
genesis     this network's own, 3 accounts
data        n0
validator   374bd5de870afc4e7b4404f1a4e7705f363cd5c3  seat 0 of 5
committee   5 validators carrying 5
quorum      4 of 5
stake       more than 3 of 5
mesh        127.0.0.1:19631
rpc         http://127.0.0.1:19630/v1/chain/hanzo
peers       4 of 4
```

A certificate carries a supermajority of the seats: three of four, four of five.
Below four seats there is no supermajority that survives one compromised key,
and the rule refuses rather than pretend:

```
proposing: no certificate: Cert(MinCommittee { n: 2, need: 4 })
```

`SIGTERM` and `SIGINT` stop it cleanly. The loops see the flag, the server
stops, and the journal of what this validator has already signed is on disk.

## Deciding a block

A block is built when there is something to put in it. Send a transaction from
an account the genesis funds:

```
curl -sS -X POST -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"eth_sendRawTransaction","params":["0x02f87082906480843b9aca008506fc23ac00830186a094f7edc8fa1ecc32967f827c9043fcae6ba73afa5c8203e880c001a0d27635386b0ac521147cd869df5e9bf06ddfbfb830f70ffe4b47c2b9ffd3db4da001fc71001704e5ef976182bfdc3988d27cd9ab8db7cc847dcd968d8b969e8b6c"]}' \
  http://127.0.0.1:19630/v1/chain/hanzo
```

```
{"jsonrpc":"2.0","id":1,"result":"0xced7b41baf35955943cd22d8e8f5d98b97f64501543de768f9f2958273866757"}
```

The node that holds it proposes; the rest execute the block themselves and vote
on the state they arrive at:

```
produced  height 1  state 012a8b224669c814a5f0d9150d87ecd3cc53f32435dd9d0d88d25e659d891ee5  5 votes
accepted  height 1  state 012a8b224669c814a5f0d9150d87ecd3cc53f32435dd9d0d88d25e659d891ee5  5 votes
```

## Talking to it

```
curl -sS -X POST -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}' \
  http://127.0.0.1:19630/v1/chain/hanzo
```

```
{"jsonrpc":"2.0","id":1,"result":"0x9064"}
```

```
$ curl -sS http://127.0.0.1:19630/v1/health
{"healthy":true}
$ curl -sS http://127.0.0.1:19630/v1/chain/hanzo/health
{"chain":"C","healthy":true}
```

## What is refused

A name that is not a network, because a default would join the wrong network on
a typo and look like it had started correctly:

```
$ hanzo-chain --network mainet --data n0 --committee committee.txt
hanzo-chain: mainet is not a network; try mainnet | testnet | devnet
```

A genesis is checked, not trusted. It must be this network's chain, and when it
names a network it must name this one:

```
$ hanzo-chain --network devnet --data n0 --genesis genesis.json --committee committee.txt
hanzo-chain: genesis.json: this genesis is chain 36963, not devnet's chain 36964
```

A committee is admitted whole or not at all, so a file with one bad line never
leaves the node running against a set that is missing a member it believes is
there:

```
$ hanzo-chain --network devnet --data n0 --committee forged.txt
hanzo-chain: forged.txt: PopInvalid
```

A validator that is not in the committee it was handed, said plainly, because it
is the difference between a validator and a spectator:

```
$ hanzo-chain --network devnet --data n5 --committee committee.txt
hanzo-chain: this validator (50b9ed61715c94734d2aa9c54f66412917f0d794) is not in committee.txt; add the line --publish prints
```

Two things on one address, before either binds:

```
$ hanzo-chain --network devnet --data n0 --committee committee.txt --rpc 127.0.0.1:19700 --mesh 127.0.0.1:19700
hanzo-chain: --rpc and --mesh cannot be one address
```

## Flags

```
  --network <name>     mainnet | testnet | devnet          (default mainnet)
  --data <dir>         where this validator's keys live    (default ~/.hanzod)
  --committee <file>   the validators of this network, one published line each
  --peers <a,b,...>    every validator's mesh address, in committee order
  --rpc <addr>         JSON-RPC address                    (default 127.0.0.1:9630)
  --mesh <addr>        validator mesh address              (default 127.0.0.1:9631)
  --genesis <file>     a genesis document, instead of this network's own
  --chain <hex32>      the 32 bytes this chain was issued, in place of the ones
                       derived from its genesis
  --produce <ms>       how often to look for work                  (default 200)
  --archive <url>      serve state this node does not keep from an archive
  --publish            print this validator's committee line, and exit
  --version            print the version, and exit
  --help               print this text, and exit
```

## Daemon (`hanzod`)

`hanzod` (`daemon/`) is the local machine daemon exposing HTTP (`127.0.0.1:3690`) and ZAP-HTTP Unix socket routes (`/healthz`, `/v1/hanzod`, `/v1/lifecycle`, `/v1/engine`, `/v1/chain`).

### Build Prerequisites

- Rust 1.85+ (`cargo`, `rustc`)
- C/C++ compiler toolchain + `cmake` (`aws-lc-rs` and bundled `rusqlite`)
- On Windows (`x86_64-pc-windows-msvc`): `nasm` (when building `aws-lc-rs` assembly targets)

```bash
cargo build --locked -p hanzod
./target/debug/hanzod --version
```

The lifecycle supervisor (`POST /v1/lifecycle/reconcile` and the background 1s tick) monitors managed workloads and reconciles unexpected child exits back to desired state using bounded exponential backoff (`1s` initial, `30s` cap, `60s` healthy reset).

## Test

```
cargo test --locked -p hanzod -p steward -p zaphttp
cargo test --release
```

## License

Hanzo Ecosystem License — see [LICENSE](LICENSE). Third-party components are
listed in [NOTICE](NOTICE).

Computer route CI retry after lux-rs/node fetch failure.

Computer route CI retry after deploy token provisioning.
