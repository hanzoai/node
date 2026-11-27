// Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
// See the file LICENSE for licensing terms.

//! The transactions waiting for a block, bounded.
//!
//! Anyone with a socket submits here — over RPC, or through a peer that relays
//! what its own RPC admitted — so what the pool holds is bounded three ways,
//! each by the rule Go's `txpool` states for it:
//!
//! - in all, [`POOL_MAX`]. A full pool takes a new transaction only if it
//!   pays more than the cheapest one it would push out, and what it pushes
//!   out is the cheapest transaction nothing else waits on — a sender's last
//!   nonce — so an eviction never strands a later one behind a gap;
//! - per sender, [`SENDER_MAX`], so one account cannot fill it;
//! - per gap, [`GAP_MAX`]: a transaction at most that far past the next nonce
//!   its sender can run, so nonces that may never be filled are not held.
//!
//! A second transaction at a nonce already waiting replaces the first only if
//! it raises both its fee cap and its tip by [`BUMP`] percent (`PriceBump`),
//! so a replacement costs something and cannot be a free way to churn the pool.
//!
//! Within a sender, transactions are kept in nonce order: a block is built by
//! walking the pool front to back, and a nonce met before the one below it is
//! a transaction that cannot run.
//!
//! THE SENDER IS RECOVERED ONCE. A signature recovery is the dearest thing a
//! transaction costs, so it is done at intake and kept, by the transaction's
//! hash — which covers the signature, so one hash is one signer — for as long
//! as the transaction is here. A block that carries it is then verified
//! without recovering it again: [`Pool::sender`].

use std::cmp::Reverse;
use std::collections::{BTreeSet, HashMap};

use alloy_consensus::{Transaction as _, TxEnvelope};
use alloy_primitives::{Address, B256, U256};

use crate::vm::Error;

/// The most transactions waiting in all.
pub const POOL_MAX: usize = 4096;

/// The most transactions waiting from one sender.
pub const SENDER_MAX: usize = 64;

/// How far past the next nonce its sender can run a transaction may be.
pub const GAP_MAX: u64 = 16;

/// The percentage a replacement raises its fee cap and its tip by, at least.
pub const BUMP: u128 = 10;

/// A transaction waiting for a block, and the sender its signature recovered
/// to. The recovery is done once, when it arrives, because doing it again per
/// build attempt is a cost a peer could make us pay repeatedly.
#[derive(Clone, Debug)]
pub(super) struct Pending {
    pub tx: TxEnvelope,
    pub sender: Address,
}

impl Pending {
    fn hash(&self) -> &B256 {
        self.tx.tx_hash()
    }

    /// What it pays a block over the base fee, per gas: its tip, or what its
    /// cap leaves above the base fee, whichever is less.
    fn tip(&self, base_fee: u128) -> u128 {
        let cap = self.tx.max_fee_per_gas();
        self.tx.max_priority_fee_per_gas().unwrap_or(cap).min(cap.saturating_sub(base_fee))
    }

    /// The most it can take from its sender: its value, and its gas at its
    /// fee cap.
    pub fn cost(&self) -> U256 {
        U256::from(self.tx.gas_limit())
            .saturating_mul(U256::from(self.tx.max_fee_per_gas()))
            .saturating_add(self.tx.value())
    }

    /// Its fee cap and its tip cap, as a replacement is measured.
    fn caps(&self) -> (u128, u128) {
        let cap = self.tx.max_fee_per_gas();
        (cap, self.tx.max_priority_fee_per_gas().unwrap_or(cap))
    }
}

/// The transactions waiting for a block.
pub(super) struct Pool {
    txs: Vec<Pending>,
    /// The sender of every transaction in `txs`, by hash.
    senders: HashMap<B256, Address>,
    most: usize,
    each: usize,
    gap: u64,
    /// What a block charges per gas. A transaction is ranked by what it pays
    /// above this.
    base: u128,
}

impl Pool {
    /// A pool held to [`POOL_MAX`], [`SENDER_MAX`] and [`GAP_MAX`], on a
    /// chain whose base fee is `base`.
    pub fn new(base: u128) -> Self {
        Self::bounded(POOL_MAX, SENDER_MAX, GAP_MAX, base)
    }

    /// A pool held to `most` in all, `each` per sender, and nonces at most
    /// `gap` past the next one a sender can run.
    pub fn bounded(most: usize, each: usize, gap: u64, base: u128) -> Self {
        Self { txs: Vec::new(), senders: HashMap::new(), most, each, gap, base }
    }

    pub fn len(&self) -> usize {
        self.txs.len()
    }

    /// Front to back: the order a block is built in.
    pub fn iter(&self) -> std::slice::Iter<'_, Pending> {
        self.txs.iter()
    }

    pub fn get(&self, hash: &B256) -> Option<&Pending> {
        if !self.senders.contains_key(hash) {
            return None;
        }
        self.txs.iter().find(|p| p.hash() == hash)
    }

    /// The sender of the waiting transaction `hash`, recovered when it came in.
    pub fn sender(&self, hash: &B256) -> Option<Address> {
        self.senders.get(hash).copied()
    }

    fn insert(&mut self, at: usize, p: Pending) {
        self.senders.insert(*p.hash(), p.sender);
        self.txs.insert(at, p);
    }

    fn remove(&mut self, at: usize) -> Pending {
        let p = self.txs.remove(at);
        self.senders.remove(p.hash());
        p
    }

    /// The nonce `who`'s next transaction should carry, from `from`, the
    /// world's: advanced past every transaction of theirs waiting here in
    /// unbroken order.
    pub fn next(&self, who: &Address, from: u64) -> u64 {
        let waiting: BTreeSet<u64> =
            self.txs.iter().filter(|p| &p.sender == who).map(|p| p.tx.nonce()).collect();
        let mut n = from;
        while waiting.contains(&n) {
            n += 1;
        }
        n
    }

    /// What `who` has waiting here could cost it, leaving out its transaction
    /// at `nonce` — the one a new transaction at that nonce would replace.
    pub fn committed(&self, who: &Address, nonce: u64) -> U256 {
        self.txs
            .iter()
            .filter(|p| &p.sender == who && p.tx.nonce() != nonce)
            .fold(U256::ZERO, |sum, p| sum.saturating_add(p.cost()))
    }

    /// Take `p` in, or refuse it. `nonce` is its sender's in the world the
    /// next block is built on.
    pub fn admit(&mut self, p: Pending, nonce: u64) -> Result<(), Error> {
        let base_fee = self.base;
        if self.senders.contains_key(p.hash()) {
            return Ok(());
        }
        let n = p.tx.nonce();
        if n < nonce {
            return Err(Error::Invalid(format!("nonce too low: {n}, the next is {nonce}")));
        }
        if p.tx.max_fee_per_gas() < base_fee {
            return Err(Error::Invalid(format!(
                "max fee per gas {} is below the base fee {base_fee}",
                p.tx.max_fee_per_gas()
            )));
        }

        // A replacement: the same sender and nonce, paying more by the bump.
        if let Some(at) = self.txs.iter().position(|q| q.sender == p.sender && q.tx.nonce() == n) {
            let (cap, tip) = self.txs[at].caps();
            let (new_cap, new_tip) = p.caps();
            let bumped = |old: u128, new: u128| {
                new > old && new.saturating_mul(100) >= old.saturating_mul(100 + BUMP)
            };
            if !(bumped(cap, new_cap) && bumped(tip, new_tip)) {
                return Err(Error::Invalid(format!(
                    "replacement transaction underpriced: it must raise its fee cap and its tip \
                     by {BUMP}%"
                )));
            }
            self.remove(at);
            self.insert(at, p);
            return Ok(());
        }

        let next = self.next(&p.sender, nonce);
        if n > next.saturating_add(self.gap) {
            return Err(Error::Invalid(format!(
                "nonce {n} is more than {} past the next one this sender can run, {next}",
                self.gap
            )));
        }
        if self.txs.iter().filter(|q| q.sender == p.sender).count() >= self.each {
            return Err(Error::Invalid(format!(
                "{} transactions from this sender are already waiting",
                self.each
            )));
        }
        if self.txs.len() >= self.most {
            let cheapest = self.cheapest(Some(&p.sender));
            match cheapest {
                Some(at) if self.txs[at].tip(base_fee) < p.tip(base_fee) => {
                    self.remove(at);
                }
                _ => {
                    return Err(Error::Invalid(format!(
                        "the pool holds {} transactions and this one pays no more than the \
                         cheapest of them",
                        self.most
                    )))
                }
            }
        }

        // After what its sender already has below it, before what it has
        // above: nonce order within a sender, whatever order they came in.
        let at = self
            .txs
            .iter()
            .position(|q| q.sender == p.sender && q.tx.nonce() > n)
            .unwrap_or(self.txs.len());
        self.insert(at, p);
        Ok(())
    }

    /// The cheapest transaction nothing else waits on — each sender's highest
    /// nonce — leaving out `spare`'s. Of two that pay the same, the one
    /// further back goes.
    fn cheapest(&self, spare: Option<&Address>) -> Option<usize> {
        let mut tails: HashMap<&Address, usize> = HashMap::new();
        for (i, p) in self.txs.iter().enumerate() {
            if Some(&p.sender) == spare {
                continue;
            }
            let tail = tails.entry(&p.sender).or_insert(i);
            if self.txs[*tail].tx.nonce() < p.tx.nonce() {
                *tail = i;
            }
        }
        tails.into_values().min_by_key(|i| (self.txs[*i].tip(self.base), Reverse(*i)))
    }

    /// Take out the transactions at `at`, ascending positions.
    pub fn take(&mut self, at: &[usize]) {
        for i in at.iter().rev() {
            self.remove(*i);
        }
    }

    /// Put `back` in front, ahead of anything newer, as it was taken out —
    /// what a block this node built carried, when that block is let go — and
    /// hold the pool to its bound again, cheapest out first.
    pub fn restore(&mut self, back: Vec<Pending>) {
        let back: Vec<Pending> =
            back.into_iter().filter(|p| !self.senders.contains_key(p.hash())).collect();
        for p in &back {
            self.senders.insert(*p.hash(), p.sender);
        }
        self.txs.splice(0..0, back);
        while self.txs.len() > self.most {
            match self.cheapest(None) {
                Some(at) => {
                    self.remove(at);
                }
                None => break,
            }
        }
    }

    /// Let go of every transaction whose nonce its sender has already used,
    /// as `nonce` says.
    pub fn prune(&mut self, nonce: impl Fn(&Address) -> u64) {
        let senders = &mut self.senders;
        self.txs.retain(|p| {
            let keep = p.tx.nonce() >= nonce(&p.sender);
            if !keep {
                senders.remove(p.hash());
            }
            keep
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evm::tests_support::raw_paying;
    use alloy_consensus::transaction::SignerRecoverable as _;
    use alloy_eips::eip2718::Decodable2718 as _;

    const BASE: u128 = 25_000_000_000;
    const CAP: u128 = 100_000_000_000;
    const GWEI: u128 = 1_000_000_000;

    fn paying(from: u8, nonce: u64, cap: u128, tip: u128) -> Pending {
        let raw = raw_paying(from, nonce, 1, cap, tip);
        let tx = TxEnvelope::decode_2718(&mut &raw[..]).expect("a transaction");
        let sender = tx.recover_signer().expect("a sender");
        Pending { tx, sender }
    }

    fn tx(from: u8, nonce: u64, tip: u128) -> Pending {
        paying(from, nonce, CAP, tip)
    }

    fn holds(pool: &Pool, p: &Pending) -> bool {
        pool.get(p.hash()).is_some()
    }

    #[test]
    fn a_replacement_raises_both_caps_by_ten_percent() {
        let mut pool = Pool::bounded(8, 8, 4, BASE);
        pool.admit(tx(1, 0, GWEI), 0).expect("admitted");
        for (cap, tip) in
            [(CAP, 2 * GWEI), (CAP * 105 / 100, 2 * GWEI), (2 * CAP, GWEI * 109 / 100)]
        {
            let why = pool.admit(paying(1, 0, cap, tip), 0).expect_err("not enough of a bump");
            assert!(matches!(&why, Error::Invalid(w) if w.contains("underpriced")), "{why:?}");
        }
        let bumped = paying(1, 0, CAP * 11 / 10, GWEI * 11 / 10);
        pool.admit(bumped.clone(), 0).expect("ten percent on both");
        assert_eq!(pool.len(), 1, "a replacement, not a second transaction");
        assert!(holds(&pool, &bumped));
    }

    #[test]
    fn what_no_block_can_carry_is_refused() {
        let mut pool = Pool::bounded(8, 8, 4, BASE);
        assert!(pool.admit(tx(1, 0, GWEI), 1).is_err(), "a nonce already used");
        assert!(pool.admit(paying(1, 0, BASE - 1, 0), 0).is_err(), "below the base fee");
        assert!(pool.admit(tx(1, 5, GWEI), 0).is_err(), "five past the next, with a gap of four");
        pool.admit(tx(1, 4, GWEI), 0).expect("four past it");
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn one_sender_is_held_to_its_share() {
        let mut pool = Pool::bounded(64, 4, 16, BASE);
        for nonce in 0..20 {
            let _ = pool.admit(tx(1, nonce, GWEI), 0);
        }
        assert_eq!(pool.len(), 4);
        assert_eq!(pool.next(&tx(1, 0, GWEI).sender, 0), 4, "nonces 0 to 3, in order");
    }

    #[test]
    fn many_senders_stay_under_the_cap_and_the_richest_stay() {
        let mut pool = Pool::bounded(16, 4, 4, BASE);
        for from in 1..=50u8 {
            for nonce in 0..6 {
                let _ = pool.admit(tx(from, nonce, GWEI + u128::from(from)), 0);
                assert!(pool.len() <= 16, "{} waiting", pool.len());
            }
        }
        assert_eq!(pool.len(), 16);
        // The four highest bidders, four each.
        for from in 47..=50u8 {
            for nonce in 0..4 {
                assert!(holds(&pool, &tx(from, nonce, GWEI + u128::from(from))), "{from}/{nonce}");
            }
        }
        // And one that does not outbid the cheapest of them is refused.
        let why = pool.admit(tx(51, 0, GWEI), 0).expect_err("cheaper than all");
        assert!(matches!(&why, Error::Invalid(w) if w.contains("cheapest")), "{why:?}");
    }

    #[test]
    fn an_eviction_never_strands_a_later_nonce() {
        let mut pool = Pool::bounded(3, 3, 4, BASE);
        let low = tx(1, 0, GWEI);
        let high = tx(1, 1, 9 * GWEI);
        let other = tx(2, 0, 5 * GWEI);
        for p in [&low, &high, &other] {
            pool.admit(p.clone(), 0).expect("admitted");
        }
        // The cheapest is sender 1's nonce 0, and nonce 1 waits on it; so what
        // goes is the cheapest LAST nonce, sender 2's.
        pool.admit(tx(3, 0, 6 * GWEI), 0).expect("outbids sender 2");
        assert!(holds(&pool, &low) && holds(&pool, &high));
        assert!(!holds(&pool, &other));
    }

    #[test]
    fn a_sender_is_known_for_exactly_as_long_as_its_transaction_waits() {
        let mut pool = Pool::bounded(2, 2, 4, BASE);
        let (a, b, c) = (tx(1, 0, GWEI), tx(2, 0, 2 * GWEI), tx(3, 0, 3 * GWEI));
        pool.admit(a.clone(), 0).expect("admitted");
        pool.admit(b.clone(), 0).expect("admitted");
        assert_eq!(pool.sender(a.hash()), Some(a.sender));
        pool.admit(c.clone(), 0).expect("outbids a");
        assert_eq!(pool.sender(a.hash()), None, "evicted, and forgotten");
        let bumped = paying(3, 0, CAP * 2, 6 * GWEI);
        pool.admit(bumped.clone(), 0).expect("a replacement");
        assert_eq!(pool.sender(c.hash()), None, "replaced, and forgotten");
        assert_eq!(pool.sender(bumped.hash()), Some(bumped.sender));
        pool.take(&[0]);
        pool.prune(|_| 1);
        assert_eq!(pool.len(), 0);
        assert_eq!(pool.senders.len(), 0, "the index is the pool, and no bigger");
    }

    #[test]
    fn a_sender_is_in_nonce_order_whatever_order_it_came_in() {
        let mut pool = Pool::bounded(8, 8, 4, BASE);
        for nonce in [2, 0, 3, 1] {
            pool.admit(tx(1, nonce, GWEI), 0).expect("admitted");
        }
        let order: Vec<u64> = pool.iter().map(|p| p.tx.nonce()).collect();
        assert_eq!(order, vec![0, 1, 2, 3]);
    }

    #[test]
    fn what_comes_back_goes_in_front_and_the_bound_holds() {
        let mut pool = Pool::bounded(2, 2, 4, BASE);
        pool.admit(tx(1, 0, GWEI), 0).expect("admitted");
        pool.admit(tx(2, 0, 2 * GWEI), 0).expect("admitted");
        let back = tx(3, 0, 3 * GWEI);
        pool.restore(vec![back.clone()]);
        assert_eq!(pool.len(), 2, "held to the bound");
        assert!(pool.iter().next().is_some_and(|p| p.hash() == back.hash()), "in front");
        assert!(!holds(&pool, &tx(1, 0, GWEI)), "the cheapest went");
    }
}
