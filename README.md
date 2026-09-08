# node

`hanzod` — the Hanzo network node.

It holds this validator's key, resolves the network it runs, checks a genesis
document against that network, and reports the rule its committee decides
under.

## Networks

| network | number |
| ------- | ------ |
| mainnet | 36963  |
| testnet | 36962  |
| devnet  | 36964  |

One number per network: it is the network id peers greet each other with and
the EVM chain id a transaction is signed against. The three are compiled in. A
name that is not one of them is an error, never a default.

## Build

```
cargo build --release
```

The binary is `target/release/hanzod`.

## Run

A validator is named by the key it signs with — the name is that key's digest,
so there is no name to claim without the key. The key is made on first use and
kept, readable only by its owner.

A committee is the list of what its validators published, so bringing one up
takes two steps:

```
for n in 0 1 2 3; do hanzod --data n$n --publish >> committee.txt; done
hanzod --network devnet --data n0 --genesis genesis.json --committee committee.txt
```

```
hanzod 0.1.0
network    devnet
id         36964
data       n0
rpc        127.0.0.1:9630
mesh       127.0.0.1:9631
validator  b66ff1ffa9215c9cb98a6225e63d1eb232501be0
key        b2e1882c10fd6fdef3cb43dc8b529d739479284b021b2c9b241e705452062493f004814046c1f4a79600d239de3414e5
genesis    genesis.json
states     36964
allocates  2 accounts
committee  4 validators carrying 4
quorum     3 of 4
signers    3 distinct
depth      2 rounds
crash      1 fault survived
export     2 weight
seat       held
```

A certificate needs a committee of at least four: below that one compromised
key forges it, and the rule refuses rather than pretend.

## What is refused

A genesis is checked, not trusted. It must state the number of the network it
was named with, and a whole-network document must state that number in both of
the places it writes it:

```
$ hanzod --network mainnet --data n0 --genesis genesis.json
hanzod: genesis.json: this genesis is network 36964, not mainnet (36963)
```

A committee is admitted whole or not at all, so a file with one bad line never
leaves the node running against a set that is missing a member it believes is
there:

```
$ hanzod --network devnet --data n0 --committee forged.txt
hanzod: forged.txt: this committee is not one: PopInvalid
```

## Flags

```
  --network <name>     mainnet | testnet | devnet          (default mainnet)
  --data <dir>         where this validator's key lives    (default ~/.hanzod)
  --rpc <addr>         JSON-RPC address                    (default 127.0.0.1:9630)
  --mesh <addr>        validator mesh address              (default 127.0.0.1:9631)
  --weight <n>         the stake published behind this validator   (default 1)
  --genesis <file>     a genesis document to check against this network
  --committee <file>   the validators of this network, one published line each
  --publish            print this validator's committee line, and exit
  --version            print the version, and exit
  --help               print this text, and exit
```

## Test

```
cargo test --release
```

## License

Hanzo Ecosystem License — see [LICENSE](LICENSE). Third-party components are
listed in [NOTICE](NOTICE).
