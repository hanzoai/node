# lux-rs/node

## What is here

`lux-node`: a running Lux validator, and the node every brand runs. It
executes real EVM blocks through revm, certifies the state root those blocks
actually produce, and serves JSON-RPC. Two binaries, because luxd is two: `luxd`
(the validator) and `lux-evm-plugin` (the C-chain, in its own process, reached
over ZAP).

A brand node is a `Spec` and one call — the whole of hanzod's `main`:

    const HANZO: Spec = Spec::new("hanzo", "hanzod", env!("CARGO_PKG_VERSION"), &[
        Network { name: "mainnet", id: 1, chain: 36963, genesis: include_str!("../genesis/mainnet.json") },
        ...
    ]);
    fn main() -> ExitCode { lux_node::run(&HANZO) }

`src/main.rs` is Lux's own: `lux_node::run(&lux_node::spec::LUX)`. The chain is
served at `/v1/chain/<name>` and `/v1/chain/<name>/rpc`; Lux's is `c`, and no
brand can answer there (a single-letter name is refused). `web3_clientVersion`
names the client (`hanzod/v0.3.0`), `info.getNetworkID` the network id.

Std only — no async runtime, a reader and a writer thread per peer.

- `src/spec.rs` — what a brand states: name, client, networks, genesis check.
- `src/run.rs` — flags, keys, committee, mesh, journal, RPC, the loop; `run`.
- `src/vm.rs` — the seam. Go's `block.ChainVM` / `block.Block`, object-safe.
- `src/evm.rs` — the C-chain on `lux-evm`: build, verify, accept, and its RPC.
  Blocks above the tip that are not decided are bounded (`UNDECIDED_MAX`, of
  them `UNVERIFIED_MAX` unexecuted, oldest let go); one that fails `verify` is
  dropped at once; one this node built gives its transactions back to the pool
  when it is dropped. An accepted block is found under the lock and read off
  disk after it. `eth_getLogs` range scans take a turn (`LOGS_SCANS`), read a
  bloom-negative block's header only, and stop at `LOGS_READ_MAX` bytes.
- `src/evm/ledger.rs` — the chain on disk. `Evm::open(genesis, dir)` is how a
  node starts a chain: `<dir>/blocks` (u32 BE length + block bytes, hanzod
  0.2.2's format, synced before accept, flock'd) is replayed; `<dir>/receipts`
  is derived and rewritten where it disagrees. Replay checks every block's
  state root: lux-evm's world is its own persistent tries, so a root costs
  the nodes a block changed.
- `src/engine.rs` — `Engine::step`, one pass of Go's B1 rule: windowed
  proposer, one signature per height on the converged winner (lowest id among
  live blocks after a settle window), clone recovery, retry; catch-up; the
  `Committee`.
- `src/proposer.rs` — `Windower`, Go's proposervm windower byte for byte, and
  `header`, what a proposer signs: ML-DSA-65 under this node's own context
  `lux-node-proposal-v1` (never Go's — the bytes are not Go's), over chain,
  set root, parent, height, slot, key and block.
- `src/held.rs` — `<data>/held`, the proposal a validator signed, synced before
  the vote; re-offered on restart, removed once its height is decided. A file
  that is not a proposal is moved to `held.damaged` and the node starts.
- `src/plugin.rs` — that seam across a socket, on `luxfi/api/zap`'s numbers.
- `src/pq.rs` — the peer post-quantum handshake, byte for byte the one
  `mesh/peer/handshake.go` runs: ML-DSA-65 identity, ML-KEM sessions, INIT/RESP.
- `src/keys.rs` — what a validator holds (ML-DSA + BLS) and what it publishes.
- `src/rpc.rs` — one bounded HTTP server, `/v1/chain/<chain>`.
- `src/genesis.rs` — reading a C-chain genesis document.
- `src/import.rs` — reading a chain back out of its RLP export, block by block.
- `src/gate.rs` — what an import leaves: `<data>/uncertified`, the first height
  accepted without a certificate. Read at every start; lifted by a certificate.
- `src/mesh.rs` — peers, frames, eviction. Votes, signed proposals (59, first
  byte `PROPOSAL_VERSION` so a v0.2 bare block is refused, never misread),
  catch-up questions (57, answer under the response flag) and transactions
  (58). What a peer sends waits in that peer's own queue, bounded
  (`ARRIVALS_MAX` blocks, `VOTES_MAX` votes, oldest let go), and is drained
  with the link it came on; what goes to a peer waits in its outbound queue
  (`OUTBOUND_MAX` frames, `OUTBOUND_BYTES`) for its writer thread, so no sender
  waits on a socket or holds the peers lock across a write. Questions are
  answered at a pace (`ASK_BURST`, then `ASK_RATE` a second).
- `src/ledger.rs` — every accepted block's certificate, in `<data>/certs`,
  keyed by height and block id, in Go's `cert_codec.go` bytes.
- `src/relay.rs` — the chain on the mesh: serves certified blocks to a peer
  that is behind, and relays transactions between pools.
- `src/journal.rs` — one statement per `Slot`, on disk, written before it is signed.
- `src/host.rs` — bind, the accept/dial sweep, the handshake, `vote`, `collect`.
- `tests/cluster.rs` — four hosts on loopback reaching a verifying certificate.
- `tests/catchup.rs` — a late validator catching up by certificate, forged
  certificates refused, a transaction relayed to the next proposer.

## Catching up

Nobody re-votes a decided height. A validator that fell behind asks peers for
accepted blocks from its tip (`Engine::catch_up`; `step` calls it when f+1
validators are heard at later heights, a certificate names a block it lacks,
or the retry gives up — formerly `follow` did, when a
pushed block lands beyond the tip) and takes each on the certificate the
network formed: Quasar, this committee, exactly this chain/height/block/parent,
checked before the block is parsed, then executed to the signed root. Served
only where the chain's id at a height equals the ledger's, so an in-memory EVM
serves its last `RETAINED` (512) heights and no further.

## Layout it assumes

`lux-node` depends on the published `lux-consensus`, which is
`luxfi/consensus/pkg/rust` and nothing else. There was a sibling checkout —
`lux-rs/consensus`, a filtered copy of that crate carrying a vote plane it
lacked — and the dep pointed at it by path. The vote plane is upstream now, so
the copy is gone and there is one crate answering the name.

## Joining a luxd network

`wire` + `luxd` + `bin/lux-join` make this node a peer of a real Go node.
Verified against luxd 1.36.181 on a localnet: TLS up on X25519MLKEM768, luxd
logs `[CONNECTED] PEER HANDSHAKE COMPLETED`, and its C-chain consensus acts on
our `AcceptedFrontier` — `frontier reply decision ... containerID=qAxcGTtMyZ...`
is the CB58 of the exact 32 bytes we sent.

Run it:

    ./build/luxd --network-id=local --plugin-dir=<node>/build/plugins \
      --staking-ephemeral-cert-enabled --staking-ephemeral-signer-enabled \
      --skip-bootstrap --sybil-protection-enabled=false --public-ip=127.0.0.1
    lux-join 127.0.0.1:<staking-port> 1337 <chainId-hex> <tip-hex>

- **The C-chain is a plugin.** luxd logs "chain VM plugin not loaded — opting
  out" and serves no C-chain RPC without `--plugin-dir`. The EVM plugin is a
  separate binary keyed by VM id.
- **TLS is the gate, and it offers one group.** `tls_config.go` pins
  `CurvePreferences: [X25519MLKEM768]`, so rustls needs the `aws-lc-rs`
  provider; ring cannot produce that group. Self-signed ECDSA P-256 only —
  `ValidateCertificate` takes P-256 or RSA and nothing else.
- **The signed IP is v6-mapped.** The handshake FIELD carries 4 bytes for a v4
  address; the SIGNATURE covers the mapped 16. Signing the field is a valid
  signature over the wrong message and the connection just closes.
- **`IpBlsSig` is not optional.** luxd parses it unconditionally; an empty one
  is a disconnect. DST `BLS_POP_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_`.
- **PeerList is what makes a peer count**, not the handshake. An empty one does.
- **No PQ handshake, no per-frame AEAD** unless a genesis `securityProfile.json`
  pins a strict profile — and even then luxd derives the AEAD key and never
  reads it back. A network without that shard logs "classical-compat mode".
  When it IS pinned, the handshake is one INIT and one RESP under a 4-byte
  big-endian length, on the raw connection, before any p2p frame. Guessing the
  axis desyncs the link on its first byte, in both directions.
- **luxd sends a ZERO chain commitment.** `mesh.go` builds its
  `HandshakeConfig` with a profile, a scheme and the refusal switch and leaves
  `ChainID` at its zero value, so that is what a peer must send back. It is a
  32-byte field in the transcript; filling it in is a chain mismatch.
- **Not protobuf.** `luxfi/proto node/zap/p2p` is a positional codec: 4-byte BE
  length, one tag byte, fields in declaration order, big-endian. A fixed-width
  id is still length-prefixed.

## Agreeing with luxd about blocks

`genesis::luxd_conformance` and `genesis::luxd_block_one` hold this down. Given
luxd's OWN documents this crate now computes:

- genesis state root `0x497bcca8…` and hash `0x6d6136e8…`
- the hash of block one luxd PRODUCED, `0x224357fa…`, at the same state root

Three things had to be right, and each was wrong first:

1. **Genesis state is the allocation plus the SYSTEM precompiles.**
   `ApplyPrecompileActivations` writes `SetNonce(addr,1)` + `SetCode(addr,{01})`
   for every module the transition crosses; with no parent timestamp that is
   `ActivationTime <= genesisTime`. `precompile::REGISTRY` carries every
   module's address, read out of a running `modules.RegisteredModules()`.
2. **`upgrade.json` is NOT genesis state.** Its 44 `precompileUpgrades` are
   upgrades — each applies at the transition crossing its timestamp, and
   genesis has none to cross. Applying them gives `0x8ccf141e…`, a chain nobody
   runs. Probing a live node settles it: exactly TWO addresses carry code at
   block zero — warp (spelled out in the allocation) and the DEX settlement
   precompile (AlwaysOn, named by no config).
3. **This localnet's blocks are the 20-field Cancun shape.** `cancunTime: 0`
   puts an empty withdrawals root, zero blob gas and a zero beacon root in the
   header. `eth_getBlockByNumber` does NOT report `withdrawalsRoot`, and it DOES
   report `blockGasCost` — which is not in the preimage. Believing the RPC on
   either count names every block wrongly.

Block one also differs from the genesis in `difficulty` (1, not 0) and carries
86 zero bytes of extraData.

## Reading a block the way Go does

`Sealed::read` is luxfi/geth `core/types.DecodeBlock`: a list of at least three
items — header, transactions, uncles — and whatever follows taken as raw items
(`rlp.Stream.Raw`: a canonical length within the list, the payload not looked
at) and ignored, except that a fourth turns a 17-field header's hash into its
withdrawals root. An uncle is refused there, as Go's `ParseBlock` refuses it. The header is `lux_evm::header::Lux::read`, which is
`DecodeHeader`: the field count names the candidate shapes (15–24 fields) and
the first that reads every position wins, in Go's order — coreth's ExtDataHash
at position 15 before Lux's base fee there, Lux's 20 and 21 before Ethereum's.
A block keeps the bytes it arrived as and is named by keccak256 of its
header's, as Go names it; the header re-encodes to exactly those bytes.

`evm::tests::every_header_shape_go_reads_is_read_as_go_reads_it` holds all 40
cases of luxfi/conformance `evm_header_layouts.json` — Go's own oracle — to
Go's fields and name, through the header reader and the block reader. Against
the reader before it, 28 of the 40 disagreed.

- **The ordinary block is `[baseFee, ""]`.** Seventeen fields, and the
  seventeenth is an ABSENT ExtDataHash, not a block gas cost: an integer there
  is a header Go refuses. So `sealed().block_gas_cost` is None for every block
  of the Go corpora; the rule reads an absent cost as the derived one, as Go's
  live VM fills it (`ensureBlockGasCost`, plugin/evm/block.go:288), keeps it as
  `Sealed.cost` / `Link.cost` for the child to derive from, and the RPC serves
  it. A block this node builds that owes a cost carries it ("19 lux", "23").
- **A base fee is 64 bits here.** revm's block environment holds a `u64`, so a
  wider one is refused while reading. Go reads any width and refuses past 256
  bits in SyntacticVerify; the two corpus cases with a 33-byte base fee are
  refused on both sides, at different steps.

## The shared two-validator network

`genesis::shared_network` holds it. A genesis minted from luxd's own localnet
C-chain document plus ONE account whose key this side holds, with BOTH nodes in
`initialStakers` — each with its BLS public key and proof of possession. luxd
boots on it and reports the state root and block hash this crate computes:
`0x0a507b6b…` / `0xb1a2d458…`.

Minting one needs three things this repo can now produce:

    lux-join --identity <dir>     # this node's NodeID + BLS signer block
    lux-join --pop <signer.key>   # the same block for luxd's own BLS key
    # then: cChainGenesis = localnet cchain.json + your funded account,
    #       initialStakers = both, networkID = something non-standard

- **NodeID = hash160(sha256(cert DER))** — RIPEMD-160 of SHA-256, not a
  truncation of either. The staking certificate is KEPT on disk, because a
  regenerated one is a different validator than the genesis names.
- **The proof of possession signs the COMPRESSED (48-byte) public key** under
  `BLS_POP_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_`.
- **`--track-all-chains=true` or there is no C-chain.** Without it luxd creates
  only P and the RPC 404s — the same symptom as a missing `--plugin-dir` and a
  different cause.
- A custom `--genesis-file` needs a NON-standard `--network-id`; the flag is
  ignored for mainnet/testnet/local.

## What is left before co-certification

The state half is done; the consensus half is not.

## Co-certification with luxd — REACHED

luxd 1.36.181 and three node.rs validators, one genesis, one C-chain block,
one Quasar certificate. From luxd's own log:

    quorum: vote RECEIVED blockID=TVv2trVunud… from=NodeID-MfFojYtgG6K9Z5yVGuVGzzUfevqsbbdCu inPendingBlocks=true
    quorum: vote RECEIVED blockID=TVv2trVunud… from=NodeID-koawEfEevZfTwyKjYXHiuTRbQGBMFdvB inPendingBlocks=true
    quorum: vote RECEIVED blockID=TVv2trVunud… from=NodeID-5otLkHqCWfj7w8BajzPqF5LYuzfvKsQnk inPendingBlocks=true
    quorum: CERT assembled and gossiped blockID=TVv2trVunud… sentToPeers=3

The C-chain went from height 0 to 1 on a REAL transaction (gasUsed 0x5208),
block `0x6718d858…`, state root `0x8d6d96fe…`, parent = the genesis both sides
compute.

### Four things it took

- **A committee of four.** `presetAlpha=3 presetK=4`: a certificate needs THREE
  distinct signers, so a two-validator network can never form one — the same
  floor `MIN_BFT_COMMITTEE = 4` states in the consensus crate. One luxd and
  three node.rs validators is the smallest set that certifies.
- **The effective weight, not the declared one.** The genesis said
  1_000_000_000_000; the P-chain computed **7_500_000_000**, because
  allocations contribute stake. The set root is over what the P-chain
  computed, and luxd serves it at
  `/v1/chain/<P>/ops/validators/at?height=0` — the platformvm's JSON-RPC
  surface is GONE in this build, but that op remains.
- **The key is hashed UNCOMPRESSED.** `/validators/at` returns 96 bytes
  (`0x0fb4c478…`, not the compressed `0xafb4c478…`), and that is what goes into
  `hashValidatorSet`.
- **TWO different ids, and using one for both is a vote that is never counted.**
  The SIGNATURE binds the canonical (inner execution) identity; the ENVELOPE
  names the OUTER container id, because that is the key luxd tracks pending
  blocks by. Addressed to the inner id, luxd logs
  "vote is for a block NOT in pendingBlocks — it will be buffered, never
  counted".

Also worth keeping: luxd's vote-guard is durable. Restarting a chain from
genesis with the same data directory leaves a binding at that height and luxd
refuses to sign a different block there — the same rule as our own journal.
Wipe `chainData` for a fresh network, never the staking keys. luxd also keeps
the genesis it first booted on in `<data>/genesis.bytes` and honours THAT over
`--genesis-file`; a changed genesis that appears to have no effect has not been
read.

### luxd's HTTP routes

    /v1/chain/<alias>/<endpoint>

`<alias>` is the chain: `C`, `P`, or the CB58 chain id. `<endpoint>` is what
the VM registers — `rpc` for the EVM's JSON-RPC, `ops/...` for the platform's
read ops. So:

    /v1/chain/C/rpc                                   eth_*
    /v1/chain/11111111111111111111111111111111P/ops/validators/at?height=0

There is no `/ext/` and no `/api/` prefix, and the platformvm's JSON-RPC
methods (`platform.getCurrentValidators` and the rest) are gone in 1.36.x — the
`ops` route is what remains, and it is the one this node reads its set from.

## Running as a validator

`lux-join <staking-addr> <network-id> <chain-hex> <tip-hex>` is a daemon. It
holds the link, answers polls, votes on every block it is queried about,
redials with a doubling backoff when the link breaks, and stops on SIGTERM.

    LUX_STAKER_DIR=<dir> LUX_RPC=127.0.0.1:9650 \
      lux-join 127.0.0.1:9651 <network-id> <chain-hex> <tip-hex>

- **The set root is derived, not configured.** `validators::at` reads
  `/v1/chain/<P>/ops/validators/at?height=0` and `validators::root` hashes it
  the way Go's `hashValidatorSet` does. It is re-read on every dial rather than
  cached, because a root computed once would keep signing for a set no longer
  in force. `LUX_SET_ROOT` still overrides, for reproducing an old run.
- **A quiet link is not a broken one.** luxd pings about every 22s and the
  socket deadline is 30s, so a validator with nothing to answer legitimately
  hears nothing. `fill` reports `Error::Quiet` only when the deadline fired
  having consumed NOTHING; a deadline part-way through a frame has lost stream
  sync and is a broken link, whatever kind the error carries. `read_exact`
  cannot tell those apart, which is why it is not used.
- Stopping takes up to one read deadline: SIGTERM sets a flag that is read at
  the top of the loop, so a silent link exits within 30s and a busy one at once.

### Reading luxd's own vote

`lux-join --solve <dumpdir> <chainid> <luxd-bls-pubkey>` recovers the
`VotePosition` luxd signed by checking luxd's OWN signature against candidate
positions. A candidate that verifies is not a guess that worked — it is the
only message that signature could have been made over. `LUX_DUMP_DIR` on a
normal run captures the PushQuery containers and the app-gossip votes it needs.

Settled by that loop and by source:

- **The signed message is byte-identical between the two implementations.**
  Go's `canonicalVoteMessageFor` and `lux_consensus::canonical_vote_message`
  agree on tag `LUX/chain/vote/v2\0`, version 3, type 1, and every field in
  order. Confirmed field by field, not assumed.
- **The outer ids are NOT signed.** Only the canonical (inner execution)
  identity is: `canonical_id`, `parent_canonical_id`, the two roots, the set
  root. The outer proposervm ids are transport cache keys.
- **`ExecutionStateRoot` and `PayloadRoot` are `ids.Empty`.** The proposervm is
  the ONLY implementor of `canonicalCommitter` in the tree, and it returns
  Empty for both. So a voter does NOT need to execute the block to vote.
- **The C-chain block id is the Ethereum block hash** (`ids.ID(ethBlock.Hash())`),
  which this crate already computes identically.
- **The container is ZAP's ARENA format**, not the transport frame: `ZAP\0`,
  three header words, then the root at offset 16 — kind, parentID at +1,
  timestamp at +33, PChainHeight at +41. The inner block is an ordinary
  Ethereum block RLP inside it (offset 208 in a 1001-byte container), and
  `header::Lux::read` parses it.
- **The vote DST is `…_NUL_`**, not the proof-of-possession `…_POP_` tag.
- **`SignVote` does not transform the message** — it signs the canonical bytes
  directly.
- **The C-chain validator set is EMPTY** on this network
  (`validators.getCurrentValidators` → `[]`), so `hashValidatorSet` returns
  `ids.Empty` and the set root is zeros.

- **The Rust encoder IS the Go encoder.** `luxd::vote_message_conformance` holds
  a vector printed by a Go program linked against `luxfi/consensus@v1.36.81`:
  226 bytes, matched exactly, including the degrade that binds the outer ids
  when the canonical ones are unset. This is no longer an argument.
- **The oracle is sound.** The solver self-tests by verifying luxd's own proof
  of possession from the genesis before trusting any negative result — a search
  that can never succeed looks exactly like a search over the wrong fields.
- **The container is matched by id**, not taken as the first one captured: a
  proposervm block's id is the hash of its bytes, so the right container is the
  one hashing to the id in the vote envelope.

WHAT IS LEFT IS ONE THING, not two. A 24,000-candidate product (chain id ×
canonical × parent × payload root × execution root × set root × round × height)
and a focused 20,000-round sweep at heights 0 and 1 with a ZERO set root both
fail to reproduce luxd's signature. Everything else in that message is either
proven or exhausted, so the conclusion is that **`validator_set_root` is NOT
zero** — luxd has a set-root source wired and its set is not empty.

That is the same artifact BLOCKER 2 needs. `hashValidatorSet` is already
byte-identical to `Committee::root`, so with the effective set — its members,
their weights, and whether Go hashes the compressed or uncompressed key — the
root becomes computable, the vote becomes signable, and node.rs is in the set
`VerifyVote` checks. One unknown closes both.

Note the C-chain's `validators.getCurrentValidators` returning `[]` is NOT that
set: it is the EVM's own L1-validator API. The set that matters is the primary
network's, read through `validators.State` at the block's P-chain height, and
this build does not route the P-chain's HTTP handlers (`/v1/chain/P` 404s), so
it has to come from luxd's state rather than its API.

Reached: a real transaction on the shared genesis drives luxd to
`SendPushQuery`, this node answers with **Chits that luxd receives**
(`received Chits message from=NodeID-PEPLwe…`), and luxd broadcasts its own
Quasar vote to us over app-gossip (`vote broadcast … sentToPeers=1
voteBytes=120`).

Not reached: a certificate with two signers. luxd stays at
`ourAcceptedHeight=0`, still *"behind on the CERT"*, and every
`vote signature verified` names only its own node id.

What is missing is this node EMITTING its own quorum vote. The envelope and the
payload are written and tested, and the signature is over
`CanonicalVoteMessage(position)` — but the vote does not carry the position,
both sides DERIVE it from the block. So node.rs has to parse the proposervm
container luxd pushes (1001 bytes), unwrap the EVM block inside, execute it, and
build the same `Position` — chain id, height, block id, parent, canonical id,
execution state root, payload root, validator set root — before its signature
can be over the same bytes luxd verifies against.

- `anvil_mine` is NOT a way around this: it builds EVM blocks while the engine
  stays at height 0. Only a real transaction drives a PushQuery.

The wire for it is written and tested: `luxd::quorum` frames the `LXQ\x01`
envelope and the `nodeID || u32 len || sig` vote payload exactly as
`chains/quorum.go` and `engine/chain/topology.go` do, and the signature covers
`CanonicalVoteMessage(position)` — the same bytes
`lux_consensus::finality::canonical_vote_message` produces.

## Importing a chain from its RLP export

`--import-chain-data <file>`, the same flag name and the same startup timing as
Go's (`luxfi/node config/keys.go` → the C-chain's `import-chain-data`, read in
`luxfi/evm plugin/evm/vm.go` right after the chain initialises, before the RPC
is up). One runbook covers all three runtimes.

    luxd --data d --committee committee.txt --network testnet \
         --import-chain-data ~/work/lux/state/rlp/lux-testnet/lux-testnet-96368.rlp

The export is a bare concatenation of RLP blocks — genesis first, no envelope
and no index. `import::from_reader` streams it (a mainnet export is gigabytes),
skips block zero and anything at or below the current head, checks each block's
parent against the block just accepted, and then hands it to the same
`verify` → accept → `set_preference` a peer's block goes through. Nothing is
copied out of a header: the state root reported is the one revm arrived at.

**History is synced in runs, not per block.** A certified block is on the
device before `Vm::accept` returns. An imported one goes through
`Vm::accept_history`, which writes the record and leaves the wait to
`Vm::sync`: the import syncs every `SYNC_EVERY` (4096) blocks and before it
returns, failed or not, and it runs before the mesh and the RPC are up — so
nothing is built, signed or served on a block the device does not hold. The
ledger says so on the device: the first unwaited record of a run puts
`<dir>/blocks.loose` (the run's start offset, synced with its name) beside
`blocks`, and the sync that ends the run removes it. A reopen that finds it
reads the chain up to the first record past that offset that is not whole —
a crash can leave the tail torn or zeroed — and cuts there; before the offset,
as without a mark, a damaged record refuses the chain. Replay then syncs, which
ends the run. A chain behind a plugin keeps the trait's defaults: every
history block accepted as a certified one is, synced each time.

Idempotent, because Go's flag stays set across restarts: a second run of the
same export imports nothing and is not an error.

**An import does not make a validator.** The export carries no certificates, so
every block it accepts is history no quorum spoke about. Go names this exact
wound from the other side — `vms/proposervm/vm.go` finds an inner tip with no
outer envelope beneath it and says the chain "MUST NOT be treated as a
caught-up validator until the outer index is rebuilt from certified peer
state". Here that is `gate::Gate`: `<data>/uncertified`, holding the first
imported height, written and synced before the import accepts its first block
and read at every start whether or not the flag is passed — a restart without
`--import-chain-data` is still gated. While it stands `propose` refuses before
it builds and `certify` refuses before the key is touched —
`Failed::Uncertified`. The way out is `Engine::anchor`, reached through
`follow`, or a certified catch-up onto the imported tip: the node executes a
peer's block on top of the imported tip and stays SILENT while the other
validators certify it. A certificate whose block names our tip as its parent is
a quorum saying the history we read off a file is the history they are
building on; the gate file is removed (and the removal synced) only after that
block is accepted. Voting there would be manufacturing the agreement we are
waiting for, so the node does not.

The gate is STATED, never inferred from the files. hanzod 0.2.2 recorded no
certificates at all, so "blocks with no certificate" is the shape of every seat
of the Hanzo network that ran it; a node that read the gate off that shape would
halt the network on upgrade. No `uncertified` file, no gate.

Checked against Go's own import of the same file (`tests/import.rs`, with
`LUX_RLP_TESTNET` pointing at the export): lux-testnet 96368, 218 blocks, tip
`0x722e2b39…381a5`, state root `0x4e19366f…137f35`, timestamp 1746815479 —
identical to what `luxd --import-chain-data` reports for it.

`tests/chains.rs` is the chains plane of luxfi/conformance (`LUX_CONFORMANCE` =
its `vectors/`, `LUX_STATE` = a luxfi/state checkout): every chain it names —
lux-mainnet, lux-testnet, zoo-mainnet, zoo-testnet — imported onto its genesis
and answering the Go node's sampled blocks, receipts and tip balances with the
same strings. An export is `rlp.parts`, read end to end as one stream, hashed
against `rlp.sha256` and then imported; nothing holds more than a buffer of it.
lux-mainnet is 1,082,780 blocks, all four chains equal to Go: 473 s in a
release build on the build pod (7,412 s before lux-evm 0.1.1's tries, synced
runs and in-place recovery), 318 MiB peak. The peak is the chain's indexes — a
link per height, every block id and transaction hash — not the world, which
was the same size before. Where the time goes now, by stage timers over the
whole import: sender recovery 47%, execution 24%, the ledger's writes 9%, the
state root 8%. The per-block fdatasync and full trie root that were 41% and
49% are gone, and a copy of the world is a pointer. Run it with `--release`.

## Decisions worth not relitigating

- **The state root is computed, never asserted.** The cluster used to vote on a
  hardcoded `execution_state_root` of `0xe5`. `Engine::position` now fills it
  from the block revm produced, and `Vm::verify` re-executes a peer's block and
  refuses a header whose claimed root is not the one it arrives at. A
  certificate over a root nobody computed certifies nothing.

- **chainId is genesis data.** Each network's document is compiled in from
  `genesis/`: Lux localnet 31337 (network 1337), mainnet 96369 (network 1),
  testnet 96368 (network 2). `--genesis <file>` replaces the document and is
  checked against the network named — its `config.chainId`, its `networkID`
  when it states one, and the block zero it makes, which must hash to the
  compiled-in document's — so a file cannot move a node to another network,
  nor to another allocation under the same ids. It can only restate the same
  genesis in another shape. Mainnet and localnet hash to `lux_evm::genesis`'s values; Go's
  `configs/{mainnet,localnet}/cchain.json` at HEAD carry seven allocations,
  not two, and are NOT what this node runs.

- **L1 or L2.** `--committee <file>` is an L1's set. `--pchain <addr>` is the
  L2 source (the default for a brand) and is refused with `run::L2`: the
  P-chain serves node id, weight and BLS key per net but no proof of
  possession, `lux-consensus` admits no signer without one, and the answer is
  unauthenticated. The node id does map: luxd's ML-DSA node id is
  `pq::name`, byte for byte.

- **`lux-consensus` is the PUBLISHED 1.22.7.** The sibling `lux-rs/consensus`
  is 1.23.0 with `publish = false` and has no `cert`/`pop` modules, so it
  cannot satisfy this crate's imports at all. Same name, higher number,
  different code — do not let the path dep come back.

- **A Quasar certificate needs four validators.** `MIN_BFT_COMMITTEE` is 4:
  below it `f = ⌊(n−1)/3⌋` is zero, so the certificate absorbs no fault and one
  key forges it. A solo node builds blocks and cannot certify them. This is the
  rule being right, not a bug to route around.

- **Two keys, because they answer two questions.** ML-DSA-65 is WHO (the node
  id is a digest of its public key, proved on every link); BLS is WHAT IT SAID
  (votes, aggregated into the certificate). Both are made once and kept at
  0600. Nothing is derived from a shared seed — that is one validator wearing
  four hats and no fault tolerance at all.

- **A node id is not keccak of the key.** It is
  `SHAKE256(left_encode ‖ "NODE_ID_V1" ‖ left_encode ‖ chain ‖ left_encode ‖
  0x42 ‖ left_encode ‖ key)[..20]` with an all-zero chain — `luxfi/ids`
  `NodeIDScheme.DeriveMLDSA`, the derivation luxd's `verifyPQIdentityBinding`
  re-runs on every peer. A node that names itself any other way is refused by
  every Go peer whatever it signs.

- **The lattices are `ml-dsa` and `ml-kem`, not libluxcrypto.** The handshake
  is defined over a FIPS 204 context string and deterministic signing, and
  libluxcrypto's Rust wrapper exposes neither — its C ABI grew
  `mldsa65_sign_ctx` for the C++ node and `lux-pq` never followed, so a
  signature made through it is over a different message. It also wanted a
  static archive that `lux/crypto/dist` does not ship, which is why this crate
  would not build at all. Both replacements are published crates; the vectors
  are what says they agree with Go.

- **How luxd starts a VM plugin**, which is written down nowhere else and was
  read out of `vms/rpcchainvm/{factory.go,runtime/subprocess/runtime_zap.go}`
  and confirmed against the shipped binary:
  1. the node binds a listener and puts its address in the child's environment
     under `VM_RUNTIME_ENGINE_ADDR` *and* `LUX_VM_RUNTIME_ENGINE_ADDR` (the
     name before the rename — every shipped plugin reads only the old one),
     with `VM_TRANSPORT=zap` and the rest of the environment inherited;
  2. the child dials back and writes ONE frame,
     `[u32 len][u32 protocol][the address it is listening on]`, where `len`
     counts the protocol word;
  3. the node checks the protocol — 42 today, no negotiation — answers a
     single `0x01` byte and closes;
  4. the node dials the address the child named. That connection is the chain.

- **The shipped plugin dials TCP, whatever the address says.** luxd's
  `NewListener` opens a unix socket only under `LUXD_VM_UNIX_SOCKET=1` and
  otherwise binds TCP loopback, because a plugin built against the older
  api/zap hardcodes `dial tcp`. Hand `ghcr.io/luxfi/node:v1.36.133`'s EVM
  plugin a socket path and it dies with `dial tcp: address …/vm.sock: missing
  port in address`. So the bootstrap listener is TCP; the socket transport is
  for the address the child names back, and for this node's own plugin.

- **A Go plugin's link is plaintext.** No handshake, no AEAD — `Conn.Call` is
  a zap frame whose payload starts with a `u32` request id, and that is all.
  This node's own plugin link proves both ends and seals every frame; a Go
  plugin cannot, so `Child` does not pretend to.

- **A VM that has not been told it is ready does not build**, and says so by
  dereferencing a nil pointer rather than by refusing: `MsgBuildBlock` against
  a freshly initialised EVM plugin answers `panic in handler: invalid memory
  address`. `MsgSetState`(Ready=4) first, and it answers an empty block
  instead — which is a chain with nothing to put in one, not an error.

- **Go's `BlockResponse` carries no state root.** It is id, parent, bytes,
  height, timestamp and two flags; the root is the fourth field of the RLP
  header inside the bytes. A node that puts a plugin's block into a
  certificate has to read it out of there — `execution_state_root` is what the
  certificate quotes, so a zero taken on faith would be a certificate over
  nothing.

- **`tests/vectors/luxd_localnet_upgrade.json` is not an upgrade config a
  plugin accepts.** It carries a `_comment` key and the plugin decodes upgrade
  json with unknown fields disallowed; strip that and it refuses the
  precompile list for re-enabling at timestamp 0 what the genesis already
  enabled. The genesis alone is what a plugin needs.

- **The journal is load-bearing across runs.** Restarting a chain from genesis
  with the same data directory is equivocation as far as the journal is
  concerned — same chain id, same height, a different block — and it refuses.
  Delete `<data>/votes` to start a new network; never the keys.

- **The wire is `luxfi/api/zap`, not `luxfi/zap`.** Two protocols share the ZAP
  name. The transport frame — big-endian length, type byte, payload — is what
  Go's `luxfi/api/zap` writes and what C++'s `luxcpp/zap` reads, and it is what
  a vote travels under. `hanzo-zap` speaks the *other* one (the arena message
  format: `ZAP\0` magic, 16-byte header, little-endian framing, no type byte).
  Wiring `hanzo-zap` in here would produce frames no Go or C++ peer can read.
- **No async runtime.** A frame is read with `read_exact`; a read deadline that
  expired mid-frame would consume a header and leave the stream one frame out
  of step forever. Blocking, one thread per peer, means a stalled peer holds
  only its own thread, and `close` shuts the socket down to return the read.
- **`Host` is `Send` but not `Sync`.** One thread drives one host, as in the C++
  lane. Move the host into its thread; do not share `&Host`.
- **Identity is the 20-byte node id, not a seat number.** It decides who dials,
  and it is what the validator set resolves a key for.
- **Write first, sign second.** `Journal::admit` reaches the device before the
  key is used. A crash in between records a statement that was never made, which
  costs one vote; the other order makes a statement that was never recorded,
  which costs the stake.
- **The record is the signed bytes.** Each entry is one canonical vote message,
  not a digest and not the fields to rebuild it from. The slot is read back out
  by the same function a receiver uses, so the journal cannot key an entry under
  a slot the message does not claim.
- **A journal is not optional.** `Host::vote` takes `&mut Journal`. A validator
  that can sign without a record is a validator that can equivocate, and a test
  cannot stand one up by forgetting an argument.

## Open, and not this crate's to close

Block ingestion and validator lifecycle. Until those exist this is a harness,
and the crate docs say so.

Block ingestion needs a ZAP message id, and there is none free in the block Go
reserves for Quasar: Go holds 60 and 61 and the vote took 62. The handshake
gave 63 back when it moved to Go's protocol — it runs on its own framing,
before anything on the link speaks zap — so 63 is free, and taking it for
blocks still needs the registry's owner to say so.
Allocating a fourth id somewhere else in Go's space without Go allocating it is
how the C++ lane ended up on `0x11`, which is Go's `MsgResponse`. The id has to
come from the registry's owner.

## One signature a height (fix/live)

v0.2.1 let every seat holding relayed txs propose at once; each signed its own
block and, one signature a height, could sign nothing else — the height never
certified. The engine now runs Go's current rule (luxfi/consensus engine/chain
B1; the Tendermint round layer was deleted in d13b4559 and must not come back):

- Windower (vms/proposervm/proposer/windower.go) picks one proposer per
  (height, slot); a slot is `--produce` × 5 (1 s) on the seat's clock since it
  took the height up. A seat builds only in its slot and only while no block is
  live; proposals are ML-DSA-signed (proposervm block/build.go:69) and relayed.
- One signature per height = `<data>/votes` (engine.go:2489, vote_guard.go:60).
- After a settle window (window/5 in Go's 150 ms–2 s clamp, engine.go:1699)
  since the latest live block arrived, sign the lowest id among live blocks
  (engine.go:2829/2922, integration.go:1002). Clone recovery re-sends the
  bound statement (integration.go:1018). Retry from 2×settle doubling to 16 s,
  8 tries then catch-up (engine.go:1575-1586, 1784).
- Every validly signed sibling is live, two from one proposer included
  (engine.go:2829), within `EACH` (16) a proposer and `SIBLINGS_MAX` a
  height; past either the HIGHEST unheld id goes (and is vm.reject'ed), so
  nobody drops the winner and the chain's 128 undecided slots never fill.
  A frame is "seen" only as a live sibling's digest, so a displaced block is
  taken back when pushed again. A refused frame (not a block, did not run) is
  never remembered — a local chain error may be the cause — and spends one of
  `JUNK_EACH` turns per (proposer, link) per settle window.
- Settle runs from the latest block to arrive, kept or not (Go tracks every
  sibling, engine.go:2988), until ONE deadline per height: `SETTLE_MAX` after
  the first arrived. A streaming proposer holds a vote 2 s at most.
- Slots: a local clock from entering the height. A proposal is taken up to
  `SKEW` (2) windows past this seat's wall-clock slot (windows since the tip's
  timestamp); once it has run, the local clock moves up to it but never past
  the seat's own wall-clock slot. A restarted seat in a quiet height takes the
  proposer's block at once; nothing a peer sends puts a seat ahead of itself.
- A block's header is held to Go's rule before a sender is recovered
  (`Evm::rule` → `lux_evm::block::rule`, luxfi/evm v1.104.56): everything is
  DERIVED from the parent and the genesis config — gas limit
  `feeConfig.gasLimit`; 86 bytes of extra (the rolled fee window, then six
  zero predicate-result bytes); base fee from the window; block gas cost
  (step 200,000 a second off the 2 s target, 0–1M), carried in the header
  when not zero and, when absent, the derived one — Go's live VM, not its
  import path, which reads absent as zero; coinbase `0x0100…00` unless
  `allowFeeRecipients`; difficulty 1, nonce 0, mix 0; time ≥ parent, ≤ now +
  10 s. Execution then holds the tips to the cost (`verifyBlockFee`), the
  roots, bloom and gas. Build derives the same (`Env::next`, time
  max(now, parent)); a build whose tips do not cover the cost is refused and
  its transactions stay in the pool. Replay holds every ledger block to the
  same rule, by no clock.
- The genesis config is read as luxd's `parseGenesis` reads it and
  `SetEthUpgrades` aligns it: `feeConfig` (else `DefaultFeeConfig`),
  `allowFeeRecipients`, `blobSchedule`, `skipPostMergeFields`,
  `evmTimestamp`/`durangoTimestamp` (default 0), `feeSplitTimestamp`;
  Shanghai at Durango, Cancun at Quasar where scheduled else `cancunTime`,
  both 0 when unnamed; every config key luxd v1.37.9 registers
  (`precompile::REGISTRY`, 63, read from luxfi/evm v1.104.56 with its
  registry linked) becomes an activation, plus the AlwaysOn DEX settlement
  precompile. Refused: pre-EVM, a non-zero block fork, Prague or later, and
  the precompiles whose `Configure` writes state or can refuse (fee and reward
  manager, stake weight, the allow lists, native minter, model registry, dead,
  FHE, bridge registrar). A transaction that names warp in its access list, or
  that calls any active precompile but the BLS12-381 modules, is refused
  (lux-evm), in a block built here and in one verified here alike; a module
  answers only once on, and none is warm (Go warms Ethereum's set only).
- `--upgrade FILE` reads the network's `upgrade.json` (`genesis::upgrade`) and
  FAILS CLOSED: any entry but enabling a BLS12-381 module — another
  precompile, a disable, `stateUpgrades`, `networkUpgradeOverrides`, an
  unknown field — refuses the start, as does what luxd's
  `verifyPrecompileUpgrades` refuses (an entry dated before the one ahead of
  it, a second enable). An upgrade added under blocks already accepted is
  caught by replay: the marker it writes changes a recorded root. The node, and `lux-evm-plugin`, refuse
  what luxd refuses (`Genesis::verify`: the fee schedule, and a
  `feeSplitTimestamp` without `allowFeeRecipients`, fee_split.go:38 — which
  makes `--network mainnet`, Lux's document, refuse as luxd v1.37.9 does), and
  print at startup every active precompile they refuse calls to
  (`genesis::refused`: warp and DEX settlement on Lux mainnet).
- **A Rust node must not join a validator set with Go validators on a chain
  where any precompile it refuses is active.** Go executes the call and this
  node refuses the transaction, so the first Go-built block with one splits
  them. Hanzo restarts from its same genesis with Rust-only validators (chain
  data wiped, keys kept); porting warp, DEX settlement and the rest is the
  next project, and comes before any mixed set.
- `eth_call` runs in the tip's own context, on the tip's state
  (`Env::of`, as Go's `NewEVMBlockContext` of the header it names):
  NUMBER is the tip's, BLOCKHASH answers the 256 before it.
- Cancun proof: Hanzo's own genesis (hanzoai/node genesis/mainnet.json, Cancun
  from 0, with throwaway keys funded) and a 300-block chain built here that
  deploys and calls a probe (TSTORE/TLOAD within and across frames, MCOPY,
  BLOBBASEFEE, BLOBHASH, NUMBER, BLOCKHASH, PREVRANDAO …), a factory whose
  children are created and destroyed in one transaction, and a contract
  destroyed long after it was made, crossing the DEX settlement activation —
  imported with equal roots by Go's `InsertChain` (luxd's precompile registry
  linked) and Go's live VM. cevm refuses it at the first BLOBBASEFEE: it
  answers 0 where Go answers 1.
- Precompile proof (round 3): both chains carry a toucher that hits
  0x0b–0x11 by BALANCE, EXTCODESIZE and a 256-byte CALL and stores the cost,
  size and result. On Hanzo (BLS off) every one is cold (2,607 with the
  opcodes around it), size 0, call empty. On the 430-block chain an
  `upgrade.json` turns the seven BLS modules on at block 143: before, the
  same; in the crossing block and after, still cold, size 1 (the marker),
  G1Add answers 128 bytes and the rest refuse their input. Go's
  `InsertChain` (given the same upgrade.json) and Go's live VM accept both
  chains with equal roots (Hanzo 0x0debdf2f…, tip 300; lux 0xb3fde2e1…,
  tip 430); without the upgrade.json Go refuses block 143 on gas. cevm
  refuses both at the first toucher call, before any module is on (Hanzo
  block 5: gas 1,125,104 against Go's 681,529) — it charges as if something
  answered at 0x0b–0x11 where Go has nothing.
- Proof (2026-09-24, AWS build pod): every block of lux-mainnet (1,082,780),
  lux-testnet, zoo-mainnet and zoo-testnet passes the rule; a 430-block chain
  built here (202 blocks under 2 s apart, 252 carrying a cost up to 1M, base
  fee up to 5,167 gwei and back down, the fee split turning on mid-chain, a
  contract storing PREVRANDAO) is imported with equal roots by Go's
  `InsertChain`, by Go's live VM (the luxd EVM plugin: parse, verify, accept)
  and by cevm. Rust refuses every single-field mutation either Go path
  refuses. Go's two paths disagree with each other on three: import accepts
  difficulty 0 and an extra with no predicate results, which the VM refuses;
  the VM accepts a header that omits a cost it owes, which import refuses.
  cevm accepts the missing predicate results. Hanzo mainnet's blocks (12,615
  checked) were built under the old rule and are ALL refused (block 1:
  difficulty 0, coinbase 0x0, gas limit, empty extra).
- Test chains that build several blocks a second run `tests_support::
  UNMETERED` (no step, a target no test reaches): they test agreement, not
  fees; the fee tests run Go's default schedule.
- A refused frame is remembered by digest for the height when the fault is
  the block's; `VmError::Unavailable` (a chain link that failed or timed out)
  is retried after a wait doubling from a settle window to `RETRY_MAX`.
- The winner skips blocks the verified votes shut out of α (its votes + not
  yet heard from < α) — not Go's: Go's winner is vote-blind, so a returning
  seat can sign its own block beside one three others signed and halt the
  height (engine.go:2829 winner, :4545 builds regardless, :1853 backoff).
- No build while a block is live, while bound, or while f+1 verified votes
  name a block this seat lacks (until the retry gives up). A new peer, a new
  sibling beside the one signed, or the first live block brings the retry
  forward; such a push is not a retry attempt. Votes for later heights:
  `AHEAD_EACH` checks per link per settle window, only verified ones reach
  the catch-up tracker and the per-link next-height buffer.
- Catch-up asks every peer at once (`Mesh::ask(&[peers])`), one round a
  settle window at most inside `step`; a silent peer costs one wait.
- `<data>/votes` is held under an exclusive flock; a write that fails and
  cannot be cut back stops the node with a non-zero exit, so it is restarted
  and the journal reopened.
- Test builds optimize dependencies (`[profile.dev.package."*"]`): the probes
  run adversaries against 150 ms settle windows, which unoptimized ML-DSA and
  revm cannot keep pace with.
- The voted block goes to `<data>/held` before the vote; a whole-committee
  restart mid-height re-offers it and completes the height.
- A split height with no block able to reach α stays uncertified (Go:
  "unrecoverable", engine.go:1716; left to Nova, which this node does not
  run); a split with a block at α resumes once someone holds that block.
- ACCEPTED: the per-height settle deadline bounds a streaming proposer to
  2 s, and past it the same proposer can land its lowest block at two seats
  just before their vote instant while the others have voted: a 2–2 split,
  the height stuck. Go's engine has the same exposure (its settle restarts on
  every tracked sibling, engine.go:2988, so the attacker times the landing
  instead).
- UPGRADING v0.2: v0.2 kept undecided blocks in memory only. Stop it at rest
  — no seat's journal binding a height above the tip, or at most n−α seats —
  or that height can never certify. The node logs "signed block … and kept no
  copy of it" when it starts bound to a block it lacks.
- Bench (5 seats, shipped hanzod): ~2,000 tx/s sustained at the default
  window (234 ms heights: the settle window is the floor), 2,760 tx/s at
  `--produce 150`; a killed seat costs its windows, nothing more.
