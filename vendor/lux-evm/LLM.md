# lux-rs/evm

## What is here

`lux-evm`: the Lux EVM in Rust. revm is the machine; the Lux rules are values on
top of it. It executes real blocks, computes the roots a header commits to, and
names a block the way the network names it.

- `src/rules.rs` — the four rules that make Lux not Ethereum, as one value.
- `src/handler.rs` — those rules in the two places revm asks about them.
- `src/gas.rs` — what a transaction owes before it runs.
- `src/precompile.rs` — which precompile answers where, and at what price.
- `src/state.rs` — the world the roots are taken over, kept as its tries.
- `src/trie.rs` — a Merkle-Patricia trie whose nodes never change once made.
- `src/fee.rs` — the fee window, base fee, block gas cost, extra and block
  fee: every header field Go derives from the parent.
- `src/block.rs` — `run` (the ONE interpreter call), `begin` (what a block does
  before its first transaction), `apply`, `execute`, `call`; `Env::next` (a
  child's environment, derived) and `rule` (Go's header rule).
- `src/header.rs` — every header shape Go's `DecodeHeader` reads, and their RLP.
- `src/genesis.rs` — block zero, as a document.
- `tests/headers.rs` — 21 real mainnet headers, decoded and re-named, and Go's
  own reading of every shape (`evm_header_layouts.json`).

## Decisions worth not relitigating

- **A root is a Merkle-Patricia trie, never a digest over a sorted list.** Two
  rules a naive store gets wrong and that both move the root: a zero slot is an
  ABSENT slot, and an empty account (EIP-161) is an ABSENT account.

- **The world IS its tries, and they are persistent.** `state::World` keeps
  accounts in a `trie::Trie` keyed by `keccak(address)`, each account's
  storage in its own keyed by `keccak(slot)`. A change makes new nodes along
  its path and shares the rest, and a node hashes itself once, the first time
  it is asked, into a `OnceLock` — it can never go stale because the node can
  never change. So a copy of the world is a pointer, a root costs the nodes
  made since the last root, and there is no cache beside the state to
  invalidate. The random insert/remove differential in `trie.rs` holds every
  intermediate root to `alloy-trie`'s `HashBuilder` over the same leaves,
  including keys with long shared prefixes (inline children, extensions,
  collapses). The price is a `keccak` per first read of an account or slot in
  a transaction, and iteration is in trie order, not address order.

- **The genesis is checked against Go, not against ourselves.** `luxfi/genesis`
  records mainnet's state root `0x2d1cedac…` and hash `0x3f4fa2a0…`; both come
  out of `World` unchanged. A live node's own numbers are checked too — see the
  node repo's `genesis::luxd_conformance`.

- **A header is read the way Go's `DecodeHeader` reads it.** Its field count
  names the candidate shapes and they are tried in `luxfi/geth`
  `core/types/decode.go`'s order — coreth's (ExtDataHash at position 15) before
  Lux's (base fee there), Lux's 20 and 21 before Ethereum's — the first that
  reads every position winning. That order is data: `header::LAYOUTS`, 15 to
  24 fields, plus the two shapes `DecodeBlock` makes of a 17-field header when
  its block has a fourth item (the hash becomes the withdrawals root). A header
  keeps its `Layout` and re-encodes to the bytes it arrived as. Held to Go by
  `evm_header_layouts.json` (luxfi/conformance, Go's own oracle, 40 cases) here
  and in the node.

- **The ordinary block is `[baseFee, ""]` — seventeen fields, the seventeenth
  an ABSENT ExtDataHash.** Not `blockGasCost`: an integer there is a header Go
  refuses ("input string too short for common.Hash"). Every Lux, Zoo and SPC
  block on record is this shape, the genesis is 16, and mainnet from 1,082,781
  is the 20-field Ethereum Cancun shape. `Lux::block` writes "17" for a zero
  cost and, for a non-zero one, "19 lux" — the shape Go's `EncodeRLP19Lux`
  writes. A base fee is 64 bits here (revm's block environment); Go reads any
  width and refuses past 256 bits in SyntacticVerify, so a 65–256-bit base fee
  is refused here and read there. No fee schedule these chains run reaches it.

- **The RPC lies in both directions about the header.**
  `eth_getBlockByNumber` REPORTS `blockGasCost` for a block whose preimage does
  not contain it, and does NOT report `withdrawalsRoot`, which is in the
  preimage. Believing it on either count names every block wrongly.
  `difficulty` is 0 at genesis and 1 after it.

- **`seal_bytes` hashes what arrived.** Re-encoding a decoded header to name it
  is a chance to name it differently — a field this crate does not model would
  round-trip to a different preimage.

- **`Checks` is a value, not a flag.** Consensus asks "may this be in a block",
  where paying is the point; a simulation asks "what would this do", where the
  caller is not paying. `Checks::Simulated` sets aside the three rules about
  paying and NOTHING else, and is only reachable through `block::call`, which
  takes the world by value so discarding the result is structural.

- **A block's header is derived from its parent, never chosen.** `Env::next`
  is the one way to a child's environment and `block::rule` the one check of a
  header, both against luxfi/evm v1.104.56 (luxd v1.37.9): gas limit =
  `feeConfig.gasLimit`; extra = the fee window rolled from the parent's (80
  bytes), then from Durango `predicate.Results{}.Bytes()` = six zero bytes
  (86 in all — Go's live VM refuses 80); base fee from the window
  (`baseFeeFromWindow`, the minimum after the genesis); block gas cost
  `clamp(parent ± step·|target − elapsed|)`, the minimum after the genesis;
  coinbase `0x0100…00` unless `allowFeeRecipients`; difficulty 1, nonce 0, mix
  0, empty uncle root; time ≥ parent and ≤ now + 10 s. `finish` holds the tips
  to the cost (`verifyBlockFee`) on BOTH the proposer's and verifier's path.
  Held to Go two ways: Go's `InsertChain` and Go's live VM (the luxd EVM
  plugin) import a 430-block chain this crate built with equal roots — BLS
  turned on mid-chain by an upgrade.json, and 0x0b–0x11 touched by BALANCE,
  EXTCODESIZE and CALL before, at and after — and every block of the four Go
  corpora passes it. cevm took the chain until it touched 0x0b–0x11, where it
  charges gas Go does not.

- **A block's cost is the live VM's: carried, or filled from the parent.**
  Go's VM fills an absent cost from the parent before verifying
  (`ensureBlockGasCost`, plugin/evm/block.go:288-319), and Go's builder never
  writes one into the header; `block::rule` does the same — a carried cost
  must be the derived one, an absent one is the derived one, and `Env::next`
  takes the parent's cost (carried or filled; `None` for the genesis). What
  this crate builds carries its cost whenever it is not zero ("19 lux", "23"),
  so Go's import path, which reads an absent cost as zero
  (`consensus.go:169-175`), and cevm accept it too. The Go paths part only on a
  Go-built block under two seconds after its parent that owes a cost: the live
  VM and this crate take it, the import path and cevm refuse it. No block of
  the corpora is one: all are at least two seconds apart. Go also keeps the
  cost in a map by header hash, persisted under `bgc`; a parent whose cost it
  never persisted (genesis, state-synced tip, pre-`bgc` luxd) derives its child
  from nil after a restart.

- **PREVRANDAO is the header's difficulty.** Go's block context sets Random
  to `header.Difficulty` from Shanghai on (core/evm.go:82-89), so the opcode
  answers 1 on every Lux block; revm's default, the mix digest, answers 0.

- **The fork is the block's, from the genesis schedule.** `Chain::spec`:
  Cancun from `cancunTime`, else Shanghai from `shanghaiTime`, else Paris —
  with Shanghai = Durango and Cancun = Quasar where those are scheduled
  (`SetEthUpgrades`). A Cancun block runs TSTORE/TLOAD, MCOPY, EIP-6780 and
  the blob opcodes under revm's CANCUN; BLOBBASEFEE is `CalcBlobFee` of the
  header's excess (1 wei — no Lux block carries a blob); a blob transaction is
  refused (`Invalid::Blob`). Its header carries what Go's builder fills: an
  empty withdrawals root (Shanghai), zero blob gas used, the derived excess
  and a zero parent beacon root — "20 eth", or "23" when it owes a cost; a
  Shanghai-only one is "17, withdrawals" in a block with its empty
  withdrawals. `begin` hands the parent beacon root to EIP-4788's contract
  whenever an account is there (`ProcessBeaconBlockRoot`: a call to no account
  returns early, and an existing EMPTY one is touched and deleted under
  EIP-161, as Go deletes it) and remembers the parent's hash. A block's
  NUMBER reaches revm (it did not before: NUMBER answered 0 and BLOCKHASH
  nothing).

- **A stateful precompile is marked when it turns on, and a call to one
  this crate does not run is refused.** `Env.activations` (config keys and
  the AlwaysOn DEX settlement precompile at 1766704800): the block whose
  transition crosses one writes nonce 1 and code 0x01, at the genesis when not
  after it (before the allocation, which wins). Go then answers calls there;
  this crate answers the BLS12-381 `MODULES` and makes any other call — warp,
  DEX settlement — an error that refuses the transaction, on the builder's
  path and the verifier's alike. A module answers only once `Env::active`
  says its precompile is on (`Precompiles::serving`); before that 0x0b–0x11
  are accounts like any other. `Env::refused` lists the active ones this
  crate does not run, for a node to print at startup.

- **The warm set is Go's: Ethereum's precompiles for the fork, nothing
  else.** Go warms `vm.ActivePrecompiles(rules)` (core/state_transition.go:
  524), which holds no Lux module, so BALANCE, EXTCODESIZE or CALL on
  0x0b–0x11 costs the cold price the first time in a transaction, on or off.

- **A Rust node must not join a Go validator set on a chain where an active
  precompile is one it does not run** (warp, 0x…9999 DEX settlement, any
  `upgrade.json` activation outside `MODULES`). Go executes a call there and
  this crate refuses the transaction, so a Go-built block holding one is a
  block the Rust node rejects: the two sides split. Hanzo restarts from its
  genesis with Rust-only validators; porting those precompiles comes first
  for any mixed set.

- **Go refuses to start a fee split with nowhere for the kept half.**
  `Genesis::verify` refuses `feeSplitTimestamp` without `allowFeeRecipients`
  (`verifyFeeSplit`, params/extras/fee_split.go:38-58; the reward manager is
  not run here). Lux mainnet's document as `mainnet()` holds it is such a
  config: luxd v1.37.9 refuses it, and so does this crate.

- **A warp predicate is refused, not verified.** Once `warpConfig` is on, a
  transaction whose access list names 0x0200…05 is a predicate Go prices and
  verifies against the P-chain; `apply` refuses it (`Invalid::Predicate`), so
  a block's predicate results are always the six zero bytes.

- **chainId is genesis data.** Mainnet 96369, localnet 31337. A node that
  compiles one in replays onto whichever network it was built for.

- **The C-chain's BLS12-381 is the PRE-FINAL EIP-2537 assignment.** G1Add,
  G1Mul, G1MSM, G2Add, G2Mul, G2MSM, Pairing at 0x0b–0x11
  (`luxfi/precompile@v0.19.21 bls12381/contract.go`). revm's set is the final
  one, so from 0x0c onward the same address names a different operation in
  each: 0x0d is a G1 multi-scalar multiplication to the chain and a G2 addition
  to revm. The chain has no map-to-curve precompile at all. Mainnet has had all
  seven since 2025-12-26 and changing them is a hard fork, so the port follows
  the chain rather than the EIP.

- **A module SHADOWS the Ethereum precompile at its address.** Go asks
  `LuxPrecompileOverrider` before the standard table, so at an address both
  claim the standard table never answers.

- **Registered is not enabled.** The Go registry holds ~56 modules; a chain
  serves only the ones its `upgrade.json` lists under `precompileUpgrades`.
  Every Lux network lists the seven BLS keys; none lists `secp256r1Config`, so
  0x0100 answers nothing on any Lux chain and is deliberately absent from
  `MODULES`. The precompile differential's Go evaluator asks the registry
  rather than an enabled set and therefore serves 0x0100 at 3450 — a price no
  Lux chain charges. That is a fact about the harness, not about a chain.

- **A module is outside the classical gate.** `opForPrecompile` type-switches
  on the implementation and has no case for the `precompileAdapter` an
  overrider installs, so every module falls to `Op::Unknown`, which no profile
  refuses. The consequence is real and is Go's: a strict-PQ chain still serves
  BLS12-381 at 0x0b–0x11.

- **Price and work are stated apart, because Go deducts before it reads.**
  `contract.DeductGas` runs at the top of every module's `Run`, so a call that
  reads its input and then refuses it has still paid. revm computes a
  precompile's price inside the function that does the work and its error
  carries no charge, so `Module` keeps `gas` and `run` as two fields.

- **Go checks the BLS subgroup on the additions and the final EIP-2537 does
  not.** One `decodeG1`/`decodeG2` serves all seven Go operations and both
  check `IsInSubGroup`; revm's `g1_add`/`g2_add` follow the final text and skip
  it. An on-curve point outside the r-order subgroup is therefore accepted by
  revm and refused by the chain. `subgroup_g1`/`subgroup_g2` close that by
  handing the point to revm's MSM with a zero scalar, which validates it and
  then computes nothing.
