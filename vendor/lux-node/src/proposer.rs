// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! Who may propose at a height and a round: Go's proposer windower.
//!
//! `luxfi/node vms/proposervm/proposer/windower.go` names ONE validator per
//! (height, slot) — `expectedProposer` (windower.go:306): a stake-weighted
//! draw over the validators sorted by node id (windower.go:271-304), from a
//! 64-bit Mersenne Twister (gonum `prng.MT19937_64`) seeded with
//! `chainSource ^ height ^ bits.Reverse64(slot)` (windower.go:317), where
//! `chainSource` is the chain id's first eight bytes read little-endian
//! (windower.go:153). The draw itself is `luxfi/container/sampler`'s: a value
//! in `[0, total weight)` from `Uint64Inclusive` (rand.go), located in a
//! weighted heap (weighted_heap.go). Every step is ported rather than
//! approximated, so a Go validator and this one name the same proposer; the
//! tests hold it to the vectors `windower_test.go` holds Go to.
//!
//! THE SLOT IS COUNTED ON THE VALIDATOR'S CLOCK. Go reads it off the outer
//! block's timestamp — `TimeToSlot(parent, block)`, one slot per
//! `WindowDuration` after the parent's (windower.go:325). This chain has no
//! outer block in its accepted history, and its EVM timestamps are whole
//! seconds that run ahead of the clock under load (`max(now, parent + 1)`), so
//! neither can place a window. The engine counts the same windows from when it
//! took the height up, and a proposal states the slot it was made in, under the
//! proposer's signature. The rule is Go's: one proposer per window, and the
//! next one only once the window before it has lapsed.
//!
//! A PROPOSAL IS SIGNED, in the shape Go's proposervm signs a block under
//! strict PQ (`BuildMLDSA`, proposervm/block/build.go:69): ML-DSA-65 over a
//! header binding the chain, the parent and the body (`BuildHeader(chainID,
//! parentID, blockID)`, build.go:112). The bytes are not Go's — Go's body is its
//! ZAP arena block, and this node's wire has none — so the context is this
//! node's own, [`CONTEXT`], never Go's `lux-proposervm-block-v1`: a signature
//! over these bytes must never be readable as a proposervm block. The body is
//! SHA-256 over the validator-set root, parent, height, slot, the scheme byte
//! and ML-DSA key Go puts in its certificate slot (block.go:42), and the block.
//! A proposal is therefore proof of who made it, for which set, whoever relays
//! it.

use lux_consensus::cert::NodeId;
use sha2::{Digest, Sha256};

use crate::vm::Id;

/// This node's proposal context: a proposer's signature can never be read as
/// any other signature a validator makes, nor as a Go proposervm block's.
pub const CONTEXT: &[u8] = b"lux-node-proposal-v1";

/// Go's scheme byte for an ML-DSA-65 proposer identity
/// (`ids.NodeIDSchemeMLDSA65`).
pub const MLDSA65: u8 = 0x42;

/// The bytes a proposer signs: chain, parent, and the digest of the body —
/// set root, parent, height, slot, key and block.
pub fn header(
    chain: &Id,
    set: &Id,
    parent: &Id,
    height: u64,
    slot: u64,
    identity: &[u8],
    block: &[u8],
) -> [u8; 96] {
    let mut body = Sha256::new();
    body.update(set);
    body.update(parent);
    body.update(height.to_be_bytes());
    body.update(slot.to_be_bytes());
    body.update([MLDSA65]);
    body.update(identity);
    body.update(block);
    let mut h = [0u8; 96];
    h[..32].copy_from_slice(chain);
    h[32..64].copy_from_slice(parent);
    h[64..].copy_from_slice(&body.finalize());
    h
}

/// MT19937-64's state size, its middle word, and its constants — Matsumoto
/// and Nishimura's reference `mt19937-64.c`, as gonum states them.
const NN: usize = 312;
const MM: usize = 156;
const MATRIX: u64 = 0xB502_6F5A_A966_19E9;
const UPPER: u64 = 0xFFFF_FFFF_8000_0000;
const LOWER: u64 = 0x7FFF_FFFF;

/// The 64-bit Mersenne Twister, seeded by one word.
struct Twister {
    mt: [u64; NN],
    at: usize,
}

impl Twister {
    /// gonum `MT19937_64.Seed`.
    fn seeded(seed: u64) -> Self {
        let mut mt = [0u64; NN];
        mt[0] = seed;
        for i in 1..NN {
            mt[i] = 6_364_136_223_846_793_005u64
                .wrapping_mul(mt[i - 1] ^ (mt[i - 1] >> 62))
                .wrapping_add(i as u64);
        }
        Twister { mt, at: NN }
    }

    /// gonum `MT19937_64.Uint64`.
    fn next(&mut self) -> u64 {
        let mag = |x: u64| if x & 1 == 1 { MATRIX } else { 0 };
        if self.at >= NN {
            for i in 0..NN - MM {
                let x = (self.mt[i] & UPPER) | (self.mt[i + 1] & LOWER);
                self.mt[i] = self.mt[i + MM] ^ (x >> 1) ^ mag(x);
            }
            for i in NN - MM..NN - 1 {
                let x = (self.mt[i] & UPPER) | (self.mt[i + 1] & LOWER);
                self.mt[i] = self.mt[i - (NN - MM)] ^ (x >> 1) ^ mag(x);
            }
            let x = (self.mt[NN - 1] & UPPER) | (self.mt[0] & LOWER);
            self.mt[NN - 1] = self.mt[MM - 1] ^ (x >> 1) ^ mag(x);
            self.at = 0;
        }
        let mut x = self.mt[self.at];
        self.at += 1;
        x ^= (x >> 29) & 0x5555_5555_5555_5555;
        x ^= (x << 17) & 0x71D6_7FFF_EDA6_0000;
        x ^= (x << 37) & 0xFFF7_EEE0_0000_0000;
        x ^ (x >> 43)
    }

    /// `sampler.rng.Uint64Inclusive`: a value in `[0, n]`. Its note — "the
    /// result of this function is stored in chain state" — is why every branch
    /// is Go's, rejection loops included.
    fn inclusive(&mut self, n: u64) -> u64 {
        if n & n.wrapping_add(1) == 0 {
            return self.next() & n;
        }
        if n > i64::MAX as u64 {
            loop {
                let v = self.next();
                if v <= n {
                    return v;
                }
            }
        }
        let most = (1u64 << 63) - 1 - (1u64 << 63) % (n + 1);
        loop {
            let v = self.next() & i64::MAX as u64;
            if v <= most {
                return v % (n + 1);
            }
        }
    }
}

/// `sampler.weightedHeap`: the weights ordered heaviest first (index breaking
/// ties), each node carrying the weight of its whole subtree.
struct Heap {
    /// (weight, subtree weight, index into the validators).
    nodes: Vec<(u64, u64, usize)>,
}

impl Heap {
    fn new(weights: &[u64]) -> Option<Heap> {
        let mut nodes: Vec<(u64, u64, usize)> =
            weights.iter().enumerate().map(|(i, w)| (*w, *w, i)).collect();
        nodes.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.2.cmp(&b.2)));
        for i in (1..nodes.len()).rev() {
            let parent = (i - 1) >> 1;
            nodes[parent].1 = nodes[parent].1.checked_add(nodes[i].1)?;
        }
        Some(Heap { nodes })
    }

    fn total(&self) -> u64 {
        self.nodes.first().map_or(0, |n| n.1)
    }

    /// `weightedHeap.Sample`.
    fn sample(&self, mut value: u64) -> Option<usize> {
        if self.total() <= value {
            return None;
        }
        let mut i = 0;
        loop {
            let (weight, _, index) = *self.nodes.get(i)?;
            if value < weight {
                return Some(index);
            }
            value -= weight;
            i = i * 2 + 1;
            let left = self.nodes.get(i)?.1;
            if left <= value {
                value -= left;
                i += 1;
            }
        }
    }
}

/// One validator set's proposer schedule on one chain.
pub struct Windower {
    source: u64,
    /// The validators, sorted by node id — Go's canonical order, and the one a
    /// sample's index is into.
    ids: Vec<NodeId>,
    heap: Heap,
}

impl Windower {
    /// The schedule of `validators` (node id, weight) on `chain`.
    ///
    /// Refused for an empty set, a set whose weights sum to zero, and one whose
    /// weights overflow: Go answers the first two with `ErrAnyoneCanPropose`,
    /// and a chain where anyone can propose is the chain this schedule exists
    /// to prevent.
    pub fn new(chain: &Id, validators: &[(NodeId, u64)]) -> Result<Windower, String> {
        let mut sorted = validators.to_vec();
        sorted.sort_unstable_by_key(|(id, _)| *id);
        let weights: Vec<u64> = sorted.iter().map(|(_, w)| *w).collect();
        let heap = Heap::new(&weights).ok_or("the validators' weights overflow")?;
        if heap.total() == 0 {
            return Err("a proposer schedule needs a validator with weight".into());
        }
        let source = u64::from_le_bytes(chain[..8].try_into().expect("eight bytes"));
        Ok(Windower { source, ids: sorted.into_iter().map(|(id, _)| id).collect(), heap })
    }

    /// `ExpectedProposer(height, slot)`: the one validator whose window is
    /// `slot` at `height`.
    pub fn expected(&self, height: u64, slot: u64) -> NodeId {
        let mut rng = Twister::seeded(self.source ^ height ^ slot.reverse_bits());
        let value = rng.inclusive(self.heap.total() - 1);
        let index = self.heap.sample(value).expect("a value below the total weight is placed");
        self.ids[index]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ids.BuildTestNodeID([]byte{i + 1})`: the id's first byte, zeros after.
    fn validators(n: u8) -> Vec<NodeId> {
        (0..n)
            .map(|i| {
                let mut id = [0u8; 20];
                id[0] = i + 1;
                id
            })
            .collect()
    }

    /// `fixedChainID = ids.ID{0, 2}`.
    fn fixed() -> Id {
        let mut id = [0u8; 32];
        id[1] = 2;
        id
    }

    fn windower(n: u8) -> (Vec<NodeId>, Windower) {
        let ids = validators(n);
        let set: Vec<(NodeId, u64)> = ids.iter().map(|id| (*id, 1)).collect();
        (ids, Windower::new(&fixed(), &set).expect("windower"))
    }

    #[test]
    fn the_twister_is_the_reference_one() {
        // Seeded with 5489, the default gonum falls back to and C++'s
        // `mt19937_64` starts from: ISO C++ [rand.predef] fixes its 10000th
        // output, and its first is the reference generator's first.
        let mut t = Twister::seeded(5489);
        assert_eq!(t.next(), 14_514_284_786_278_117_030);
        let mut last = 0;
        for _ in 1..10_000 {
            last = t.next();
        }
        assert_eq!(last, 9_981_545_732_273_789_042);
    }

    /// `TestExpectedProposerChangeBySlot`, windower_test.go:257.
    #[test]
    fn the_proposer_per_slot_is_go_s() {
        let (ids, w) = windower(10);
        let go = [7, 9, 7, 0, 4, 8, 1, 8, 9, 9, 7, 3, 8, 6, 5, 4, 2, 9, 1, 4, 1, 4, 8, 3];
        for (slot, want) in go.iter().enumerate() {
            assert_eq!(w.expected(1, slot as u64), ids[*want], "slot {slot}");
        }
        assert_eq!(w.expected(1, 720), ids[3], "MaxLookAheadSlots");
        assert_eq!(w.expected(1, 721), ids[4], "MaxLookAheadSlots + 1");
    }

    /// `TestExpectedProposerChangeByHeight`, windower_test.go:197.
    #[test]
    fn the_proposer_per_height_is_go_s() {
        let (ids, w) = windower(10);
        assert_eq!(w.expected(1, 0), ids[7]);
        assert_eq!(w.expected(2, 0), ids[1]);
    }

    #[test]
    fn the_order_the_set_is_given_in_does_not_matter() {
        let (ids, w) = windower(5);
        let mut shuffled: Vec<(NodeId, u64)> = ids.iter().rev().map(|id| (*id, 1)).collect();
        shuffled.swap(1, 3);
        let v = Windower::new(&fixed(), &shuffled).expect("windower");
        for h in 0..50 {
            for s in 0..20 {
                assert_eq!(w.expected(h, s), v.expected(h, s), "height {h} slot {s}");
            }
        }
    }

    #[test]
    fn every_validator_gets_windows_and_weight_buys_more() {
        let ids = validators(4);
        let set = [(ids[0], 1), (ids[1], 1), (ids[2], 1), (ids[3], 5)];
        let w = Windower::new(&fixed(), &set).expect("windower");
        let mut count = [0usize; 4];
        for s in 0..8000 {
            let who = w.expected(7, s);
            count[ids.iter().position(|id| *id == who).expect("a member")] += 1;
        }
        assert!(count.iter().all(|c| *c > 0), "{count:?}");
        assert!(count[3] > 3 * count[0], "five times the weight: {count:?}");
    }

    #[test]
    fn a_header_binds_every_field() {
        let base = header(&[1; 32], &[6; 32], &[2; 32], 3, 4, &[5; 8], b"block");
        assert_eq!(&base[..32], &[1; 32]);
        assert_eq!(&base[32..64], &[2; 32]);
        for other in [
            header(&[9; 32], &[6; 32], &[2; 32], 3, 4, &[5; 8], b"block"),
            header(&[1; 32], &[9; 32], &[2; 32], 3, 4, &[5; 8], b"block"),
            header(&[1; 32], &[6; 32], &[9; 32], 3, 4, &[5; 8], b"block"),
            header(&[1; 32], &[6; 32], &[2; 32], 9, 4, &[5; 8], b"block"),
            header(&[1; 32], &[6; 32], &[2; 32], 3, 9, &[5; 8], b"block"),
            header(&[1; 32], &[6; 32], &[2; 32], 3, 4, &[9; 8], b"block"),
            header(&[1; 32], &[6; 32], &[2; 32], 3, 4, &[5; 8], b"other"),
        ] {
            assert_ne!(other, base);
        }
    }

    #[test]
    fn an_empty_or_weightless_set_has_no_schedule() {
        assert!(Windower::new(&fixed(), &[]).is_err());
        assert!(Windower::new(&fixed(), &[([1u8; 20], 0)]).is_err());
        assert!(Windower::new(&fixed(), &[([1u8; 20], u64::MAX), ([2u8; 20], 1)]).is_err());
    }
}
