// Copyright (C) 2026, Hanzo AI, Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

//! The validators of a network, and the rule they decide under.
//!
//! A committee is the list of what its validators published, and nothing more.
//! Each line carries a key, a proof that its holder possesses it, and the
//! weight staked behind it. The NAME is not on the line: it is the key's
//! digest, so there is nothing on a line to lie about — a line either proves
//! possession of the key it carries or it is refused.
//!
//! Admission is all-or-nothing. A committee read from a file this node did not
//! write is admitted as a whole or not at all, so a file with one bad line
//! never leaves the node running against a set that is missing a member it
//! believes is there.

use lux_consensus::cert::{CertError, Registration, ValidatorSet};
use lux_consensus::finality;

use crate::validator::name_of;

/// The rule a committee of this size and stake decides under.
///
/// Every number here is read from the consensus standard rather than restated,
/// so this node's idea of a quorum is the network's idea of one.
pub struct Rule {
    /// Members.
    pub members: usize,
    /// Total weight the set carries.
    pub weight: u64,
    /// Seats that must agree before a preference can ignite.
    pub quorum: i64,
    /// Distinct signers required whatever the stake distribution.
    pub signers: i64,
    /// Consecutive agreeing rounds required to ignite.
    pub depth: i64,
    /// Simultaneous crash faults ignition survives.
    pub crash: i64,
    /// Weight a certificate needs before it authorises export.
    pub export: u64,
}

impl Rule {
    /// Read the rule a set decides under.
    pub fn of(set: &ValidatorSet) -> Self {
        let members = set.len();
        let weight = set.carried();
        let n = members as i64;
        Self {
            members,
            weight,
            quorum: finality::nova_quorum(n),
            signers: finality::nova_signer_floor(n),
            depth: finality::nova_beta(n),
            crash: finality::crash_tolerance(n),
            export: finality::two_thirds_stake_floor(weight),
        }
    }
}

/// Read a committee out of the lines validators published.
///
/// Blank lines and `#` comments are skipped so a committee file can say who is
/// who. Everything else must be a line this crate wrote.
pub fn read(text: &str) -> Result<ValidatorSet, String> {
    let mut registrations = Vec::new();
    for (number, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        registrations.push(parse(line).map_err(|e| format!("line {}: {e}", number + 1))?);
    }
    if registrations.is_empty() {
        return Err("no validators in it".into());
    }
    ValidatorSet::register(registrations).map_err(refusal)
}

/// One published line.
fn parse(line: &str) -> Result<Registration, String> {
    let mut field = line.split_whitespace();
    let key = field.next().ok_or("no key")?;
    let proof = field.next().ok_or("no proof")?;
    let weight = field.next().ok_or("no weight")?;
    if field.next().is_some() {
        return Err("more than a key, a proof and a weight".into());
    }
    let public_key = bytes(key).map_err(|e| format!("key: {e}"))?;
    let proof = bytes(proof).map_err(|e| format!("proof: {e}"))?;
    let weight: u64 = weight
        .parse()
        .map_err(|_| format!("weight {weight} is not a number"))?;
    if weight == 0 {
        return Err("a weight of zero is a validator that cannot carry a vote".into());
    }
    Ok(Registration {
        node: name_of(&public_key),
        public_key,
        proof,
        weight,
    })
}

/// Read hexadecimal, refusing anything that is not exactly that.
fn bytes(text: &str) -> Result<Vec<u8>, String> {
    if text.len() % 2 != 0 {
        return Err("an odd number of hexadecimal digits".into());
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(|_| "not hexadecimal".to_string()))
        .collect()
}

/// Why the standard refused a set, said plainly.
fn refusal(e: CertError) -> String {
    format!("this committee is not one: {e:?}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validator::Validator;

    fn validators(n: usize, tag: &str) -> (Vec<Validator>, Vec<std::path::PathBuf>) {
        let mut held = Vec::new();
        let mut dirs = Vec::new();
        for i in 0..n {
            let d = std::env::temp_dir()
                .join(format!("hanzod-c-{}-{tag}-{i}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            held.push(Validator::open(&d).expect("made"));
            dirs.push(d);
        }
        (held, dirs)
    }

    fn clean(dirs: &[std::path::PathBuf]) {
        for d in dirs {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn a_file_of_published_lines_is_a_committee() {
        let (held, dirs) = validators(4, "read");
        let file = held
            .iter()
            .map(|v| v.line(1))
            .collect::<Vec<_>>()
            .join("\n");
        let set = read(&file).expect("four validators");
        assert_eq!(set.len(), 4);
        assert_eq!(set.carried(), 4);
        for v in &held {
            assert!(set.contains(&v.name()), "a validator it published is in it");
            assert!(set.can_verify(&v.name()), "and its key came with it");
        }
        clean(&dirs);
    }

    #[test]
    fn comments_and_blank_lines_are_not_validators() {
        let (held, dirs) = validators(4, "comments");
        let mut file = String::from("# the validators of this network\n\n");
        for v in &held {
            file.push_str(&format!("{}   # a node\n", v.line(1)));
        }
        assert_eq!(read(&file).expect("four").len(), 4);
        clean(&dirs);
    }

    #[test]
    fn one_bad_line_refuses_the_whole_file() {
        let (held, dirs) = validators(4, "bad");
        let mut lines: Vec<String> = held.iter().map(|v| v.line(1)).collect();
        // A proof that does not bind the key it sits beside.
        let broken = lines[2].clone();
        let mut part = broken.split_whitespace();
        let key = part.next().unwrap().to_string();
        let mut proof = part.next().unwrap().to_string();
        proof.replace_range(0..1, if proof.starts_with('a') { "b" } else { "a" });
        lines[2] = format!("{key} {proof} 1");
        let err = read(&lines.join("\n")).expect_err("a forged proof is refused");
        assert!(err.contains("not one"), "{err}");
        clean(&dirs);
    }

    #[test]
    fn a_line_cannot_be_pasted_in_twice() {
        let (held, dirs) = validators(4, "twice");
        let mut lines: Vec<String> = held.iter().map(|v| v.line(1)).collect();
        lines.push(lines[0].clone());
        // One key belongs to one member; a repeat is not a second seat.
        assert!(read(&lines.join("\n")).is_err());
        clean(&dirs);
    }

    #[test]
    fn zero_weight_is_refused_at_the_line() {
        assert!(parse(&format!("{} {} 0", "aa".repeat(48), "bb".repeat(96))).is_err());
    }

    #[test]
    fn an_empty_file_is_not_a_committee() {
        assert!(read("").is_err());
        assert!(read("# only a comment\n").is_err());
    }

    #[test]
    fn the_rule_is_the_standards_rule() {
        let (held, dirs) = validators(4, "rule");
        let file = held.iter().map(|v| v.line(1)).collect::<Vec<_>>().join("\n");
        let rule = Rule::of(&read(&file).expect("four"));
        assert_eq!(rule.members, 4);
        assert_eq!(rule.weight, 4);
        assert_eq!(rule.quorum, finality::nova_quorum(4));
        assert_eq!(rule.signers, finality::nova_signer_floor(4));
        assert_eq!(rule.depth, finality::nova_beta(4));
        assert_eq!(rule.crash, finality::crash_tolerance(4));
        assert_eq!(rule.export, finality::two_thirds_stake_floor(4));
        clean(&dirs);
    }
}
