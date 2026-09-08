// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! The rule a committee decides under.
//!
//! Every number here is READ from the consensus standard rather than restated,
//! so this node's idea of a quorum is the network's idea of one. A second
//! spelling anywhere is how two implementations come to disagree about whether
//! a block was decided, which is the one disagreement a network cannot have.
//!
//! The tier is the one the engine drives. Reporting the thresholds of a rung
//! this node does not attest would be a number that is true of something else.

use lux_consensus::cert::ValidatorSet;
use lux_consensus::finality::{self, Finality};

/// The tier a certificate here attests. Export finality: the rung that
/// authorises settlement, and the rung the engine assembles.
pub const TIER: Finality = Finality::Quasar;

/// What a set of this size and stake decides under.
pub struct Rule {
    /// Seats.
    pub members: usize,
    /// Total weight the set carries.
    pub weight: u64,
    /// Distinct signatures a certificate must carry, whatever the stake
    /// distribution.
    pub signers: i64,
    /// The weight a certificate must STRICTLY exceed.
    pub stake: u64,
}

impl Rule {
    /// Read the rule `set` decides under.
    pub fn of(set: &ValidatorSet) -> Self {
        let weight = set.carried();
        Self {
            members: set.len(),
            weight,
            signers: finality::signer_floor(TIER, set.len() as i64),
            stake: finality::two_thirds_stake_floor(weight),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lux_node::engine::{Committee, Engine};
    use lux_node::keys::Keys;

    /// A committee of `n` validators, built the way a real one is: each makes
    /// its own keys and publishes a line, and the file is what they published.
    fn committee(n: usize, tag: &str) -> (Committee, Vec<Keys>, Vec<std::path::PathBuf>) {
        let mut held = Vec::new();
        let mut dirs = Vec::new();
        let mut lines = Vec::new();
        for i in 0..n {
            let dir = std::env::temp_dir()
                .join(format!("hanzod-rule-{}-{tag}-{i}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let keys = Keys::open(&dir).expect("keys");
            lines.push(keys.publish());
            held.push(keys);
            dirs.push(dir);
        }
        (Committee::read(&lines.join("\n")).expect("a committee"), held, dirs)
    }

    fn clean(dirs: &[std::path::PathBuf]) {
        for d in dirs {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn the_rule_is_the_standards_rule() {
        for n in [1usize, 2, 4, 5, 7] {
            let (c, _held, dirs) = committee(n, &format!("std{n}"));
            let rule = Rule::of(&c.set().expect("admitted"));
            assert_eq!(rule.members, n);
            assert_eq!(rule.weight, n as u64);
            assert_eq!(rule.signers, finality::signer_floor(TIER, n as i64));
            assert_eq!(rule.stake, finality::two_thirds_stake_floor(n as u64));
            clean(&dirs);
        }
    }

    #[test]
    fn the_rule_reported_is_the_rule_the_engine_drives() {
        // The guard against two spellings: what this node PRINTS as its quorum
        // has to be the number the engine actually holds a round to.
        for n in [1usize, 4, 5] {
            let (c, held, dirs) = committee(n, &format!("engine{n}"));
            let me = held[0].identity.node();
            let engine = Engine::new(me, held[0].consensus.clone(), &c, 1, [0u8; 32])
                .expect("an engine over its own committee");
            assert_eq!(Rule::of(&c.set().expect("admitted")).signers as usize, engine.quorum());
            clean(&dirs);
        }
    }

    #[test]
    fn a_supermajority_is_strictly_more_than_two_thirds() {
        // The arithmetic an operator sizes a network by. Four seats need three,
        // five need four: three is not more than two thirds of five.
        for (n, need) in [(4usize, 3i64), (5, 4), (7, 5)] {
            let (c, _held, dirs) = committee(n, &format!("super{n}"));
            assert_eq!(Rule::of(&c.set().expect("admitted")).signers, need);
            clean(&dirs);
        }
    }

    #[test]
    fn the_weight_a_certificate_must_exceed_is_not_the_weight_it_may_equal() {
        // Strictly exceed: four seats carrying four need more than two, so
        // three. A certificate that only equalled the floor would be two thirds
        // and not more than it.
        let (c, _held, dirs) = committee(4, "stake");
        let rule = Rule::of(&c.set().expect("admitted"));
        assert_eq!(rule.stake, 2);
        assert!(rule.signers as u64 > rule.stake);
        clean(&dirs);
    }
}
