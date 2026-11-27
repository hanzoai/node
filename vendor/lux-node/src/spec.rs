// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! What a network is, to the node that runs it.
//!
//! A brand node is a [`Spec`] and one call to [`crate::run`]. The spec says
//! four things: the brand's name, what its client is called, the client's
//! version, and the networks it runs — each a name, a network id, a chain id
//! and the genesis document compiled in for it. Everything else a node does is
//! the same on every network, so it lives here once and not in every brand.
//!
//! THE NAME IS THE CHAIN'S NAME. A brand's chain is served at
//! `/v1/chain/<name>`. The one exception is Lux's own: the C-chain is the Lux
//! primary network's, it answers at `c`, and no brand can take that name —
//! [`Spec::new`] gives a spec its own name as its chain's, and a single letter
//! is refused, because single letters are the primary network's chains.
//!
//! TWO NUMBERS, BECAUSE THERE ARE TWO QUESTIONS. A validator joins a NETWORK,
//! and the network id is what peers greet each other under. A transaction is
//! signed against a CHAIN, and the chain id is what an EVM signature is bound
//! to. They are stated separately so a wallet and a validator cannot disagree
//! about which one they are on.
//!
//! WHAT IS COMPILED IN IS COMPILED IN. A node pointed at a network by name
//! starts from the document its binary carries. A document read off disk is
//! checked against that network before it is used ([`Network::check`]): a
//! genesis is data, and data from outside is checked, not trusted.

use serde_json::Value;

/// One network a brand runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Network {
    /// The name a person uses: `mainnet`, `testnet`.
    pub name: &'static str,
    /// What peers greet each other under, and what a genesis document names
    /// when it names a network.
    pub id: u32,
    /// The EVM chain id a transaction on this network is signed against.
    pub chain: u64,
    /// The chain's genesis document, compiled in.
    pub genesis: &'static str,
}

/// A brand: its name, its client, and the networks it runs.
///
/// Built with [`Spec::new`]. The chain name is not a field a brand sets: it is
/// the brand's own name, so no brand can answer as another.
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    name: &'static str,
    client: &'static str,
    version: &'static str,
    networks: &'static [Network],
    chain: &'static str,
}

/// How deep a `cChainGenesis` chain is followed before it is called a loop.
/// A document that wraps itself would otherwise recurse until the stack ends.
const MAX_NESTING: u8 = 8;

/// Lux: its C-chain, on the three networks this node runs.
///
/// Localnet is first, so a Lux node given no `--network` runs a local network
/// and never joins mainnet by omission. Mainnet's document is Go's own record
/// of it (`luxfi/genesis configs/mainnet/cchain.canonical.json`); testnet's is
/// the C-chain document the Go testnet node is started with, whose block zero
/// `tests/import.rs` checks against the Go node's.
pub const LUX: Spec = Spec {
    name: "lux",
    client: "lux-node",
    version: env!("CARGO_PKG_VERSION"),
    networks: &[
        Network {
            name: "localnet",
            id: 1337,
            chain: 31337,
            genesis: include_str!("../genesis/localnet.json"),
        },
        Network {
            name: "mainnet",
            id: 1,
            chain: 96369,
            genesis: include_str!("../genesis/mainnet.json"),
        },
        Network {
            name: "testnet",
            id: 2,
            chain: 96368,
            genesis: include_str!("../genesis/testnet.json"),
        },
    ],
    chain: "c",
};

impl Spec {
    /// A brand's spec. `version` is the client's own, usually
    /// `env!("CARGO_PKG_VERSION")`. The first network is the default.
    pub const fn new(
        name: &'static str,
        client: &'static str,
        version: &'static str,
        networks: &'static [Network],
    ) -> Self {
        Self { name, client, version, networks, chain: name }
    }

    /// The brand's name.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// What the client is called: the binary, and the first word of every
    /// version string it reports.
    pub fn client(&self) -> &'static str {
        self.client
    }

    pub fn version(&self) -> &'static str {
        self.version
    }

    /// `hanzod/v0.3.0` — what `web3_clientVersion` answers.
    pub fn agent(&self) -> String {
        format!("{}/v{}", self.client, self.version)
    }

    /// The name this brand's chain is served under: `/v1/chain/<chain>`.
    pub fn chain(&self) -> &'static str {
        self.chain
    }

    pub fn networks(&self) -> &'static [Network] {
        self.networks
    }

    /// The network called `name`, or nothing.
    ///
    /// Nothing, rather than a default: a node that fell back to a network when
    /// asked for one it does not have would join the wrong network on a typo
    /// and look like it had started correctly.
    pub fn network(&self, name: &str) -> Option<&'static Network> {
        self.networks.iter().find(|n| n.name == name)
    }

    /// The network a node runs when none is named: the first.
    pub fn default_network(&self) -> Option<&'static Network> {
        self.networks.first()
    }

    /// `localnet | mainnet | testnet`, for a message that lists them.
    pub fn names(&self) -> String {
        self.networks.iter().map(|n| n.name).collect::<Vec<_>>().join(" | ")
    }

    /// Refuse a spec no node can run under.
    ///
    /// Checked at startup rather than trusted, because a spec is a brand's
    /// constant and a mistake in one is a node that joins something else.
    pub fn check(&self) -> Result<(), String> {
        let name = self.name;
        if name.len() < 2 || name.len() > 32 {
            return Err(format!(
                "a brand is named in 2 to 32 characters, and {name:?} is {}; single letters \
                 are the Lux primary network's chains",
                name.len()
            ));
        }
        if !name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) {
            return Err(format!("{name:?} is not lower-case letters and digits"));
        }
        for (what, word) in [("client", self.client), ("version", self.version)] {
            if word.is_empty() || word.contains(|c: char| c.is_whitespace() || c == '/') {
                return Err(format!("{name}'s {what} {word:?} is not one word"));
            }
        }
        if self.networks.is_empty() {
            return Err(format!("{name} runs no network"));
        }
        for (i, a) in self.networks.iter().enumerate() {
            if a.name.is_empty() {
                return Err(format!("{name} has a network with no name"));
            }
            // Two networks with one of these is a replay of every signature, or
            // every greeting, across both.
            for b in &self.networks[i + 1..] {
                if a.name == b.name || a.id == b.id || a.chain == b.chain {
                    return Err(format!("{name}'s {} and {} are not two networks", a.name, b.name));
                }
            }
            a.check(a.genesis).map_err(|e| format!("{name} {}: {e}", a.name))?;
        }
        Ok(())
    }
}

impl Network {
    /// Refuse `raw` if it is not this network's genesis.
    ///
    /// THE TWO NUMBERS FIRST. The chain id must be this network's, because it
    /// is what every transaction is signed against. The network id, when the
    /// document states one, must be this network's, because a chain document
    /// carried inside another network's genesis belongs to that network. A
    /// field that is present and unreadable is an error, never a default: a
    /// document that fell back to zero for a number it could not parse would
    /// agree with nothing and look like it had been checked.
    ///
    /// THEN THE BLOCK. Two numbers are not a chain: a document with this
    /// network's ids and another allocation is another block zero, and a node
    /// started on it forks on its first block. So where this network carries
    /// its own document, `raw` must make the same genesis block — the same
    /// hash, whatever shape the file is written in.
    pub fn check(&self, raw: &str) -> Result<(), String> {
        let doc: Value = serde_json::from_str(raw).map_err(|e| format!("not JSON: {e}"))?;
        if !doc.is_object() {
            return Err("not a genesis document".into());
        }

        // A whole-network document carries the chain's own inside it, as an
        // object or as a string holding the document. Both are on disk.
        let mut held;
        let mut chain = &doc;
        let mut depth = 0;
        while let Some(inner) = chain.get("cChainGenesis") {
            depth += 1;
            if depth > MAX_NESTING {
                return Err("cChainGenesis nests too deeply".into());
            }
            chain = match inner {
                Value::String(text) => {
                    held = serde_json::from_str(text)
                        .map_err(|e| format!("cChainGenesis is not JSON: {e}"))?;
                    &held
                }
                other => other,
            };
        }
        if !chain.is_object() {
            return Err("cChainGenesis is not a document".into());
        }

        let config = chain.get("config").filter(|c| c.is_object()).ok_or("no config")?;
        let id = number(config, "chainId")?;
        if id != self.chain {
            return Err(format!(
                "this genesis is chain {id}, not {}'s chain {}",
                self.name, self.chain
            ));
        }
        if doc.get("networkID").is_some() {
            let stated = number(&doc, "networkID")?;
            if stated != u64::from(self.id) {
                return Err(format!(
                    "this genesis is network {stated}, not {} ({})",
                    self.name, self.id
                ));
            }
        }
        if !self.genesis.is_empty() && raw != self.genesis {
            let theirs = crate::genesis::parse(raw)?.hash();
            let ours = crate::genesis::parse(self.genesis)
                .map_err(|e| format!("{}'s own genesis: {e}", self.name))?
                .hash();
            if theirs != ours {
                return Err(format!(
                    "this genesis is block {theirs}, not {}'s block zero {ours}",
                    self.name
                ));
            }
        }
        Ok(())
    }
}

/// How many accounts a document allocates.
pub fn accounts(raw: &str) -> usize {
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|doc| doc.get("alloc").and_then(|a| a.as_object()).map(|a| a.len()))
        .unwrap_or(0)
}

/// A number that must be a number. A quoted or absent value is an error here
/// rather than a zero three lines later.
fn number(at: &Value, field: &str) -> Result<u64, String> {
    at.get(field)
        .ok_or_else(|| format!("no {field}"))?
        .as_u64()
        .ok_or_else(|| format!("{field} is not a whole number"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: Network = Network {
        name: "mainnet",
        id: 7,
        chain: 7777,
        genesis: r#"{"config":{"chainId":7777},"gasLimit":"0xb71b00"}"#,
    };
    const TWO: Network = Network {
        name: "testnet",
        id: 8,
        chain: 7778,
        genesis: r#"{"config":{"chainId":7778},"gasLimit":"0xb71b00"}"#,
    };

    fn net(chain: u64) -> Network {
        Network { name: "mainnet", id: 1, chain, genesis: "" }
    }

    #[test]
    fn lux_is_a_spec_a_node_can_run() {
        LUX.check().expect("lux");
        assert_eq!(LUX.chain(), "c", "Lux keeps its C-chain's name");
        assert_eq!(LUX.agent(), format!("lux-node/v{}", env!("CARGO_PKG_VERSION")));
        assert_eq!(LUX.default_network().map(|n| n.name), Some("localnet"));
    }

    #[test]
    fn luxs_documents_are_the_chains_it_has_always_run() {
        // Mainnet and localnet are the values `lux_evm` carries, to the hash;
        // testnet is the Go testnet's own block zero.
        let hash = |name: &str| {
            let n = LUX.network(name).expect(name);
            crate::genesis::parse(n.genesis).expect("parses").hash()
        };
        assert_eq!(hash("mainnet"), lux_evm::genesis::mainnet().hash());
        assert_eq!(hash("localnet"), lux_evm::genesis::localnet().hash());
        assert_eq!(
            hash("testnet").to_string(),
            "0x1c5fe37764b8bc146dc88bc1c2e0259cd8369b07a06439bcfa1782b5d4fb0995"
        );
    }

    #[test]
    fn a_brand_answers_under_its_own_name() {
        let s = Spec::new("zeta", "zetad", "9.9.9", &[ONE, TWO]);
        s.check().expect("a spec");
        assert_eq!(s.chain(), "zeta");
        assert_eq!(s.agent(), "zetad/v9.9.9");
        assert_eq!(s.network("testnet"), Some(&TWO));
        assert_eq!(s.network("mainet"), None, "a typo is not a network");
        assert_eq!(s.network("MAINNET"), None);
        assert_eq!(s.names(), "mainnet | testnet");
    }

    #[test]
    fn no_brand_can_be_the_c_chain() {
        // A single letter is the primary network's; a brand spelled `c` would
        // answer where only Lux's C-chain may.
        for name in ["c", "C", "p", "x", ""] {
            assert!(Spec::new(name, "d", "1", &[ONE]).check().is_err(), "{name:?}");
        }
        for name in ["Zeta", "ze ta", "ze/ta", "ze-ta"] {
            assert!(Spec::new(name, "d", "1", &[ONE]).check().is_err(), "{name:?}");
        }
    }

    #[test]
    fn a_spec_that_names_two_networks_as_one_is_refused() {
        const TWIN: Network = Network { name: "testnet", ..ONE };
        const SAME: Network = Network { id: 9, chain: 9999, ..ONE };
        const WRONG: Network = Network { genesis: TWO.genesis, ..ONE };
        assert!(Spec::new("zeta", "d", "1", &[ONE, TWIN]).check().is_err(), "one id");
        assert!(Spec::new("zeta", "d", "1", &[ONE, SAME]).check().is_err(), "one name");
        assert!(Spec::new("zeta", "d", "1", &[]).check().is_err(), "no network");
        assert!(Spec::new("zeta", "d", "1", &[WRONG]).check().is_err(), "another's genesis");
        assert!(Spec::new("zeta", "zeta d", "1", &[ONE]).check().is_err(), "a client of two words");
    }

    #[test]
    fn one_networks_genesis_is_not_anothers() {
        for (i, a) in LUX.networks().iter().enumerate() {
            a.check(a.genesis).unwrap_or_else(|e| panic!("{}: {e}", a.name));
            assert!(accounts(a.genesis) > 0, "{} allocates nothing", a.name);
            for b in &LUX.networks()[i + 1..] {
                assert!(b.check(a.genesis).is_err(), "{}'s genesis passed for {}", a.name, b.name);
                assert!(a.check(b.genesis).is_err(), "{}'s genesis passed for {}", b.name, a.name);
            }
        }
    }

    #[test]
    fn a_document_naming_another_network_is_refused() {
        // The chain is right and the network is not.
        let raw = r#"{"networkID":9,"cChainGenesis":{"config":{"chainId":36963}}}"#;
        let err = net(36963).check(raw).expect_err("network 9 is not network 1");
        assert!(err.contains("network 9"), "{err}");
        // And the network is right, in both shapes it is written.
        assert!(net(36962)
            .check(r#"{"networkID":1,"cChainGenesis":{"config":{"chainId":36962}}}"#)
            .is_ok());
        assert!(net(36964)
            .check(r#"{"networkID":1,"cChainGenesis":"{\"config\":{\"chainId\":36964}}"}"#)
            .is_ok());
        // The common shape: the chain's own document, with no network around it.
        assert!(net(36963).check(r#"{"config":{"chainId":36963}}"#).is_ok());
    }

    #[test]
    fn nothing_unreadable_becomes_a_default() {
        let n = net(36963);
        assert!(n.check("").is_err(), "an empty file");
        assert!(n.check(r#"{"config":{"chainId":36963"#).is_err(), "truncated JSON");
        assert!(n.check("[1,2,3]").is_err(), "not an object");
        assert!(n.check(r#"{"alloc":{}}"#).is_err(), "no config");
        assert!(n.check(r#"{"config":{}}"#).is_err(), "no chain id");
        assert!(n.check(r#"{"config":{"chainId":"36963"}}"#).is_err(), "a quoted chain id");
        assert!(n.check(r#"{"config":{"chainId":-1}}"#).is_err(), "a negative chain id");
        assert!(
            n.check(r#"{"networkID":"1","cChainGenesis":{"config":{"chainId":36963}}}"#).is_err(),
            "a quoted network id"
        );
    }

    #[test]
    fn a_document_with_this_networks_numbers_and_another_body_is_refused() {
        // The ids agree and the allocation does not: another block zero.
        let other = r#"{"config":{"chainId":7777},"gasLimit":"0xb71b00",
            "alloc":{"0x9011E888251AB053B7bD1cdB598Db4f9DEd94714":{"balance":"0x1"}}}"#;
        let err = ONE.check(other).expect_err("another genesis block");
        assert!(err.contains("not mainnet's block zero"), "{err}");
        let other = r#"{"config":{"chainId":7777},"gasLimit":"0xb71b01"}"#;
        assert!(ONE.check(other).is_err(), "a gas limit is part of block zero");

        // The same block zero, written the other way it comes: accepted.
        let wrapped =
            r#"{"networkID":7,"cChainGenesis":{"config":{"chainId":7777},"gasLimit":"0xb71b00"}}"#;
        ONE.check(wrapped).expect("the same genesis, in the network's wrapper");
        let text = r#"{"networkID":7,"cChainGenesis":"{\"config\":{\"chainId\":7777},\"gasLimit\":\"0xb71b00\"}"}"#;
        ONE.check(text).expect("and as the string Go writes it as");
    }

    #[test]
    fn a_document_nested_past_the_limit_is_refused() {
        let mut wrapped = r#"{"config":{"chainId":36963}}"#.to_string();
        for _ in 0..12 {
            wrapped = format!(r#"{{"cChainGenesis":{wrapped}}}"#);
        }
        assert!(net(36963).check(&wrapped).is_err());
    }
}
