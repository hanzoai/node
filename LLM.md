# node

`hanzod` — the Hanzo network node. Rust. One binary.

## What this repository is

The Hanzo network's identity, as a program. A node is asked a handful of
questions before it can do anything — which network this is, what number that
network is, where its key lives, what it is called, what it binds, whether a
genesis document is this network's, and what rule its committee decides under —
and this answers them.

The answers are compiled in. A node that read its own identity from somewhere
editable is a node that can be pointed at a fork by editing it.

## Layout

    src/network.rs     the three networks, and nothing else about them
    src/validator.rs   the key this validator holds, and the line it publishes
    src/committee.rs   the lines others published, and the rule they decide under
    src/genesis.rs     a genesis document, and whether it is this network's
    src/main.rs        the flags, and what is printed

Each answers one question. `network` knows nothing about keys; `validator`
knows nothing about networks; `committee` states no rule of its own.

## The identity

`networkID == evmChainID`, per env, per LP-018. One number, because two numbers
for one network is a way for a wallet and a validator to disagree about which
network they are on, and the disagreement is only visible after a transaction
has been signed for the wrong one.

    mainnet 36963    testnet 36962    devnet 36964

Ports are 9630 (RPC) and 9631 (mesh) — what the deployed nodes bind. The data
directory is `~/.hanzod`.

## The rule comes from the standard

The consensus package named in `Cargo.toml` is the one dependency that decides
anything. The proof of possession a validator publishes, the admission rule a
committee is built by, and the thresholds a round is measured against are read
from it, so that what this node calls a quorum is what every other
implementation calls one. Nothing here restates a threshold.

`blst` is pinned to the version that crate verifies under. A second BLS in one
process is a second network.

## A validator is named by its key

The name is the head of the key's digest, so there is no name to claim without
the key and none to publish separately. This is why a published line carries a
key, a proof and a weight but no name: there is nothing on the line to lie
about. A line either proves possession of the key it carries or it is refused.

The key is made once with 32 bytes from the kernel and written `0600` at
creation — not created and then chmodded, because a key that is world readable
for the width of two syscalls has been world readable.

## What is refused

Refusal is the interesting half. Every one of these has a test.

- A network name that is not one of the three. Never a default: a default would
  join the wrong network on a typo and look like it had started correctly.
- A genesis stating another network's number, and a whole-network document
  whose two halves name different networks.
- A field that is present and unreadable — a quoted chain id, a negative one.
  Never a zero.
- A committee file with one bad line, refused whole, so the node never runs
  against a set missing a member it believes is there.
- The same line pasted in twice. One key belongs to one member.
- A weight of zero.
- `--rpc` and `--mesh` on one address.

## Building

    cargo build --release
    cargo test --release

Both clean, no warnings.

## Conventions

- Copyright is Hanzo AI, Inc.; new files carry `SPDX-License-Identifier:
  BSD-3-Clause-Eco`. Upstream attribution lives in `NOTICE`.
- The word for another network's brand does not appear in prose, help text, log
  lines, or anything a person reads. The dependency line in `Cargo.toml` names
  the package it depends on, which is the dependency and not a brand surface.
- `LLM.md` is the one document file. `CLAUDE.md`, `AGENTS.md` and `GEMINI.md`
  are gitignored symlinks to it.
