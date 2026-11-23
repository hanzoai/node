# node

`hanzod` — the single unified Hanzo daemon and sole Desktop, Bot, and Team backend per machine (v0.4.0, HIP-0106 / HIP-1330 / HIP-1400). Rust. One binary.

`hanzod` (`daemon/`) owns `<data_local_dir>/hanzo/hanzod.zap.sock` (0600) and
`127.0.0.1:3690` as the sole local backend for Hanzo Desktop, Hanzo Bot, and Hanzo Team:
- Native Rust Dev runtime (`/v1/dev`, `dev-protocol` + `dev-core` + `dev-orchestrator` + WAL SQLite in `daemon/src/dev_native.rs`)
- Native Hanzo Bot backend (`/v1/bot`, crew presets, persistent bots, scheduled `hanzoai/tasks` runs, optional `hanzo-bot` gateway proxy)
- Native Hanzo Team backend (`/v1/team`, spaces, channels, human+bot threads, Kanban board tasks dispatched via `hanzoai/tasks`, optional `hanzo-team` transactor / Cloud sync)
- Native Hanzo Dō research subsystem (`/v1/research`, `hanzo-vm` microVM / workspace bounded execution + SHA-256 artifacts)
- Reactive `hanzoai/tasks` linked-machine worker (`/v1/hanzod/worker` in `daemon/src/tasks_worker.rs` — outbound SSE `/v1/workflow/events` + instant `Notify` busy-to-idle transitions + `ReleaseActivity` deferral across `dgx`, `evo`, and Desktop machines)
- Automated local/edge Kubernetes & unified Cloud orchestrator (`/v1/cluster` & `/v1/k3s` — `hanzo-vm` k3s, native `k3s`, `hanzo-cloud`, and `tasksd`)
- Local inference (`/v1/engine` + `/v1/engine/catalog`), outbound WebTransport/ZAP relay (`link.rs`), machine identity (`hanzo-identity`), mobile/iOS staticlib C-ABI (`hanzod_ffi`), and opt-in blockchain validator (`/v1/chain` — embedded directly with `--features embedded-chain` via `HANZOD_MODE=chain` self-exec, or via `chain/steward`). `zap/` is the ZAP-HTTP codec both `hanzod` and its clients use.

## What this repository is

A validator on this network, and the Hanzo chain it serves.

The host it runs on is a dependency, at a tag. That host is where identity, the
post-quantum handshake, the validator mesh, the block engine, the interpreter
and the JSON-RPC server live. Every node on the network — whatever language it
is written in — runs the same host, because two nodes that disagree about what
a block means are two networks.

What this repository holds is what is Hanzo's: the networks and their numbers,
the genesis each starts from, the name this node answers under, the data
directory, the ports, and the command line. Nothing here re-implements anything
the host already states.

## Layout

    src/main.rs        Hanzo's spec — name, client, three networks — and one call
    genesis/*.json     one chain genesis per network

Everything else — flags, keys, committee, mesh, the chain on disk, catch-up and
JSON-RPC — is `lux_node::run`. The chain answers at `/v1/chain/hanzo` and
`/v1/chain/hanzo/rpc`; the C-Chain name belongs to Lux alone.

## Two numbers, not one

A validator joins a NETWORK. A transaction is signed against a CHAIN. They are
different numbers and collapsing them is how a wallet and a validator come to
disagree about what they are on — visible only after something has been signed
for the wrong thing.

    network id    1 mainnet     2 testnet     3 devnet
    chain id      36963         36962         36964

The chain ids are LP-018's canonical map for this brand. The network ids are the
network whose validator set carries the chain: an operator running `hanzod`
holds a seat in that set and serves this chain on it.

The SDK's default ports are 9650 (RPC) and 9651 (mesh); a deployment passes
`--rpc` and `--mesh`. The data directory is `~/.hanzod`.

## The rule comes from the standard

The consensus package named in `Cargo.toml` is what decides anything. The proof
of possession a validator publishes, the admission rule a committee is built by,
and the thresholds a certificate is measured against are read from it, so what
this node calls a quorum is what every other implementation calls one. Nothing
here restates a threshold.

A certificate needs a supermajority of the SEATS, strictly more than two thirds:
three of four, four of five, five of seven. And it needs at least four seats at
all — below that the fault budget is zero and one compromised key forges it. The
standard refuses it by name:

    proposing: no certificate: Cert(MinCommittee { n: 2, need: 4 })

## A validator is named by its key

Two keys, because they answer two questions. ML-DSA-65 is WHO: it names the
validator and is proved on every link. BLS12-381 is WHAT IT SAID: it signs votes
and is what a certificate aggregates. Both are made on first use and kept `0600`.

A published line is the ML-DSA identity, the BLS key, and the proof of
possession — 1952, 48 and 96 bytes. No name, because there is nothing on the
line to lie about: the name is derived from the identity, and the proof binds
the voting key to it.

Nothing is derived from a shared seed. A network brought up by handing every
node one seed is one validator wearing several hats.

## The name this node answers to

`web3_clientVersion` and every version surface name this node — `hanzod/v…` —
and the chain is served under `hanzo`. Execution is the SDK's, untouched.

## What is refused

Refusal is the interesting half. Every one of these has a test or a run.

- A network name that is not one of the three. Never a default.
- A genesis whose chain is not this network's chain.
- A genesis that names a network, when that is not this network. A chain
  document carried inside another network's genesis belongs to that network.
- A field that is present and unreadable — a quoted chain id, a negative one.
  Never a zero.
- A committee file with one bad line, refused whole (`PopInvalid`), so the node
  never runs against a set missing a member it believes is there.
- A validator that is not in the committee it was handed.
- `--rpc` and `--mesh` on one address.
- A `--chain` that is not exactly 32 bytes of hexadecimal.

## Bringing a network up

A committee is the list of what its validators published, so it takes two steps
and nothing can publish before the file exists:

    for n in 0 1 2 3 4; do hanzod --data n$n --publish >> committee.txt; done
    hanzod --network devnet --data n0 --committee committee.txt --peers <addresses>

`--peers` is positions, not names: the names are fixed by the committee file and
the handshake proves them, so an address belonging to someone else is refused
rather than believed. The mesh is not the quorum — `peers` reports who was
reached and the rule already says how many are enough.

## Building

    cargo build --release
    cargo test --release

Both clean, no warnings.

## Conventions

- Copyright is Hanzo AI, Inc.; new files carry `SPDX-License-Identifier:
  BSD-3-Clause-Eco`. Upstream attribution lives in `NOTICE`.
- The word for another network's brand does not appear in prose, help text, log
  lines, or anything a person reads. The dependency lines in `Cargo.toml` and
  the attribution in `NOTICE` name the packages this depends on, which is the
  dependency and not a brand surface.
- No path dependencies and no vendored copies: the host resolves to a tag.
- `LLM.md` is the one document file. `CLAUDE.md`, `AGENTS.md` and `GEMINI.md`
  are gitignored symlinks to it.
