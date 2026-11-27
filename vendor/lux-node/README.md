# lux-node

A running Lux consensus node in Rust: a validator mesh over ZAP frames,
driving the finality rule from [`lux-consensus`](https://github.com/luxfi/consensus).

Three layers and no fourth.

| Layer     | What it knows                                                     |
|-----------|-------------------------------------------------------------------|
| `mesh`    | sockets, ZAP frames, the vote codec. No quorum, no signature check.|
| `journal` | what this validator has already signed, on disk.                  |
| `host`    | a listener, the peer sweep, and driving one `Tally`.              |

The rule stays in `lux-consensus`. A node that held its own copy of the rule
would be a second implementation of it.

## The mesh is not the quorum

`connect` returns how many peers it reached inside one deadline. It never
demands all of them. The finality rule already says how many votes are enough;
saying it a second time as "every peer must connect" would only be a way to
disagree with it, and one absent validator would take down a cluster the ⅔ rule
calls healthy.

## One pair, one connection

The lower node id dials, the higher accepts. The rule is the identity itself,
so there is no seat number to keep in step with the validator set and both ends
derive the same answer without agreeing on anything first.

The handshake is a ZAP frame like everything else on the link. What it carries
is a *claim* of identity, not a proof — and it is not asked to be one: every
vote is verified against the validator set on its own, so a peer that lies about
who it is gains a socket and no vote.

## One statement per slot, across a restart

A validator may make one statement per slot — one chain, one height, one round.
Making two is equivocation: it is what a slashing rule punishes and what a fork
is built out of. In one process a map is enough. Across a restart it is not — a
node that forgets comes back willing to contradict itself, and the network
cannot tell the amnesia from the attack.

`Host::vote` takes `&mut Journal`, so there is no way to sign without a record.
The journal is written and flushed to the device *before* the key is used: a
crash in between records a statement that was never made, which costs one vote,
where the other order makes a statement that was never recorded, which costs the
stake.

Each entry is one canonical vote message, exactly as signed — not a digest, not
the fields to rebuild it from. The slot is read back out of the message by the
same function a receiver uses, so an entry cannot be filed under a slot the
message does not claim. Entries are fixed width, so a torn tail is arithmetic
rather than a guess.

Re-admitting the identical statement is not equivocation and is not an error: a
signer that crashed between recording and sending has to be able to send.

## A peer is a member, not a wall

One eviction rule: a peer whose stream ended, whose framing was violated, or
whose socket will not take a write is dropped. A structurally invalid *payload*
is not a framing violation — the lie is dropped and the peer keeps its seat.

## What this is not

There is no block ingestion and no validator lifecycle here. This carries votes
over a real network, refuses to contradict itself across a restart, and turns
votes into a certificate. Until those two exist it is a consensus harness that
speaks the wire, not a node that can join a chain.

Block ingestion is waiting on an id, not on code: the ZAP block Go reserves for
Quasar is 60..63, Go holds 60 and 61, the vote took 62 and the handshake 63.
Allocating a fourth id elsewhere in Go's space without Go allocating it is how
the C++ lane ended up carrying votes under `0x11`, which is Go's `MsgResponse`.

## Test

```
cargo test
```

`tests/cluster.rs` binds four real listeners on loopback, forms a real mesh,
signs with real BLS keys, and checks the certificate with the same rule that
checks one off the network. `tests/journal.rs` ends the process and reopens the
record, tears a write in half, and hands it a corrupt file.
