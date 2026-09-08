// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! The networks this node runs, and what each one is called by number.
//!
//! A sovereign network is named by ONE number. It is the network id peers
//! greet each other with and it is the EVM chain id a transaction is signed
//! against, and they are the same number on purpose: two numbers for one
//! network is a way for a wallet and a validator to disagree about which
//! network they are on, and the disagreement is only visible after a
//! transaction has been signed for the wrong one.
//!
//! These three are the whole set. A network that is not here is not one this
//! binary can be pointed at by name — which is the point of compiling them in
//! rather than reading them from a file that can be edited into a fork.

/// One network: what it is called, and the number it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Network {
    /// The name a person uses.
    pub name: &'static str,
    /// The number the protocol uses — network id and EVM chain id, one value.
    pub id: u64,
}

/// The settlement network.
pub const MAINNET: Network = Network { name: "mainnet", id: 36963 };
/// The network that carries release candidates.
pub const TESTNET: Network = Network { name: "testnet", id: 36962 };
/// The network the protocol itself is developed against.
pub const DEVNET: Network = Network { name: "devnet", id: 36964 };

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
        ALL.iter()
            .map(|n| n.name)
            .collect::<Vec<_>>()
            .join(" | ")
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
    fn no_two_networks_share_a_number() {
        // Two networks with one id is a replay of every signature across both.
        for (i, a) in ALL.iter().enumerate() {
            for b in &ALL[i + 1..] {
                assert_ne!(a.id, b.id, "{} and {} share an id", a.name, b.name);
            }
        }
    }
}
