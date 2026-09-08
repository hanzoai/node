// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! The networks this node runs, and the two numbers each one is.
//!
//! ONE NUMBER PER NETWORK PER ENVIRONMENT, because Hanzo is a sovereign L1.
//!
//! A validator joins a NETWORK and a transaction is signed against a CHAIN, and
//! on a network that carries many chains those are two questions with two
//! answers. Hanzo's primary network carries one: its EVM. So the two numbers
//! were free to differ, and differing is what went wrong — mainnet greeted
//! under id 1, which is what LUX mainnet greets under, and so does Zoo's. Three
//! sovereign networks introducing themselves by the same number is the
//! ambiguity a wallet cannot see through and a staking-key derivation path
//! cannot be unique under.
//!
//! So the primary network id IS the EVM chain id. One number per environment,
//! globally unique, and a peer that greets under 36963 is on Hanzo mainnet and
//! nowhere else.
//!
//! These three are the whole set. A network that is not here is not one this
//! binary can be pointed at by name — which is the point of compiling them in
//! rather than reading them from a file that can be edited into a fork.

/// One network: what it is called, the number its validators greet under, and
/// the chain this node serves on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Network {
    /// The name a person uses.
    pub name: &'static str,
    /// The network id. What peers greet each other under, and what a genesis
    /// document names when it names a network.
    pub id: u32,
    /// The EVM chain id a transaction on this network is signed against.
    pub chain: u64,
}

/// The settlement network.
pub const MAINNET: Network = Network { name: "mainnet", id: 36963, chain: 36963 };
/// The network that carries release candidates.
pub const TESTNET: Network = Network { name: "testnet", id: 36962, chain: 36962 };
/// The network the protocol itself is developed against.
pub const DEVNET: Network = Network { name: "devnet", id: 36964, chain: 36964 };

/// Every network this binary knows, in the order it lists them.
pub const ALL: [Network; 3] = [MAINNET, TESTNET, DEVNET];

impl Network {
    /// The network called `name`, or nothing.
    ///
    /// Nothing, rather than a default: a node that fell back to a network when
    /// it was asked for one it did not have would be a node that joins the
    /// wrong network on a typo, and it would look like it had started
    /// correctly.
    pub fn named(name: &str) -> Option<Network> {
        ALL.into_iter().find(|n| n.name == name)
    }

    /// The names, for a message that has to list them.
    pub fn names() -> String {
        ALL.iter().map(|n| n.name).collect::<Vec<_>>().join(" | ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_network_is_reachable_by_its_name() {
        for n in ALL {
            assert_eq!(Network::named(n.name), Some(n));
        }
    }

    #[test]
    fn an_unknown_name_is_not_a_network() {
        // The failure that matters: not a default, and not the first entry.
        assert_eq!(Network::named("mainet"), None);
        assert_eq!(Network::named(""), None);
        assert_eq!(Network::named("MAINNET"), None);
    }

    #[test]
    fn no_two_networks_share_a_chain() {
        // Two chains with one id is a replay of every signature across both.
        for (i, a) in ALL.iter().enumerate() {
            for b in &ALL[i + 1..] {
                assert_ne!(a.chain, b.chain, "{} and {} share a chain id", a.name, b.name);
                assert_ne!(a.id, b.id, "{} and {} share a network id", a.name, b.name);
            }
        }
    }

    #[test]
    fn a_network_greets_under_its_chain_id() {
        // Hanzo's primary network carries one chain, so it introduces itself by
        // that chain's number. The mistake this replaces: mainnet greeting under
        // 1, which Lux mainnet and Zoo mainnet also greet under — three networks,
        // one number, and a wallet with no way to tell which it reached.
        for n in ALL {
            assert_eq!(n.id as u64, n.chain, "{} greets under a number that is not its chain", n.name);
        }
    }
}
