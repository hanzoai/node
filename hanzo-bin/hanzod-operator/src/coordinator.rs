// Copyright 2026 Hanzo AI Inc. All Rights Reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! The leaderless coordination seam: *who reconciles object K when N hanzods
//! run?*
//!
//! One hanzod per cluster is the drop-in default (`StaticCoordinator` — always
//! the owner). The vision is many hanzods forming a public, permissionless,
//! leaderless blockchain across clusters, where each is one consensus
//! participant. This module is the SEAM that decision plugs into, not the
//! consensus itself — it deliberately mirrors `github.com/hanzoai/ha`
//! (Membership + Owner + Fencer) so the same value model already proven in
//! visor/cloud carries over, and so a Rust hanzod and a Go `ha` agree on the
//! same owner for the same key.
//!
//! ## Layers (each independently complete, composed not braided)
//!
//! 1. **Membership** — the live, reconcile-eligible hanzod set + this node's own
//!    id. The ONE injectable source. `StaticMembership` (single node / tests) or
//!    a cluster source (K8s Lease / ZAP mDNS discovery) in production.
//! 2. **Owner** ([`owner`]) — a pure function of `(key, members)` via Rendezvous
//!    (Highest-Random-Weight) hashing. Deterministic, order-independent, and
//!    byte-identical to `ha.Owner` (same `sha256(key ‖ 0x00 ‖ id)[..8]` weight),
//!    so cross-language agreement holds. This is "HRW today".
//! 3. **Coordinator** — the ONE question the reconcile loop asks:
//!    `should_reconcile(key)`. `StaticCoordinator` is the single-node default;
//!    [`HrwCoordinator`] composes a `Membership` with `owner`.
//! 4. **Fencer / Round** (documented seam, not yet wired) — election answers
//!    "who SHOULD write"; it cannot make a deposed/partitioned node STOP. A
//!    monotone [`Round`] fencing token does. `ha` keeps this as a separate seam
//!    on purpose: the round is the output of a *linearizable* source, and folding
//!    that in would re-complect election with consensus. Today: a single
//!    linearizable register. Tomorrow: a ZAP-BFT-agreed round (Lux `consensus1`
//!    over `zap` `conn_pq` PQ sessions), the Lux quasar RSM. Both plug in behind
//!    `Fencer` with no change at any call site — the k8s API server itself is the
//!    fenced store (server-side apply with a per-round field manager / a
//!    `resourceVersion` precondition rejects a stale writer's patch).
//!
//! ## How full leaderless-BFT lands (follow-on)
//!
//! - Wire a `KubeMembership` (list hanzod Leases in `coordination.k8s.io`) or a
//!   `ZapMembership` (mDNS/discovery peers over `zap`) behind [`Membership`].
//! - Replace [`HrwCoordinator`] with a `FencedCoordinator` that pairs `owner`
//!   with a `Fencer::acquire(key)` and stamps the round onto every apply.
//! - Back the `Fencer` with the ZAP-agreed round so the "who" is not just
//!   locally-HRW-agreed but BFT-agreed across mutually-distrusting clusters.
//!
//! Nothing above this module changes when that lands: [`reconcile`] only ever
//! asks `should_reconcile`.
//!
//! [`reconcile`]: crate::reconcile

use async_trait::async_trait;
use sha2::{Digest, Sha256};

/// One reconcile-eligible hanzod. `id` MUST be stable for the node's life (its
/// pod name / a persistent node id) — HRW weights derive from it. `addr` is the
/// reachable address for future write-forwarding (host:port); unused today.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub id: String,
    pub addr: String,
}

impl Member {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into(), addr: String::new() }
    }
}

/// HRW score for `(key, id)`: the first 8 bytes of `SHA-256(key ‖ 0x00 ‖ id)` as
/// a big-endian u64. The NUL separator stops `("ab","c")`/`("a","bc")` colliding.
/// Byte-identical to `ha.weight` — this is what makes cross-language owner
/// agreement hold.
fn weight(key: &str, id: &str) -> u64 {
    let mut h = Sha256::new();
    h.update(key.as_bytes());
    h.update([0u8]);
    h.update(id.as_bytes());
    let sum = h.finalize();
    u64::from_be_bytes(sum[..8].try_into().expect("sha256 >= 8 bytes"))
}

/// The member that owns the reconciler for `key`, or `None` when `members` is
/// empty (fail-closed — never a wrong owner). Deterministic and order-independent
/// (ties broken on the lexicographically smaller id), so every node computes the
/// same owner from the same set. Mirrors `ha.Owner`.
pub fn owner<'a>(key: &str, members: &'a [Member]) -> Option<&'a Member> {
    let mut best: Option<(&Member, u64)> = None;
    for m in members {
        let s = weight(key, &m.id);
        match best {
            Some((b, bs)) if s < bs || (s == bs && m.id >= b.id) => {}
            _ => best = Some((m, s)),
        }
    }
    best.map(|(m, _)| m)
}

/// Whether `self_id` owns the reconciler for `key` under `members` — the check
/// the reconcile loop runs per object. Mirrors `ha.IsOwner`.
pub fn is_owner(key: &str, self_id: &str, members: &[Member]) -> bool {
    owner(key, members).is_some_and(|o| o.id == self_id)
}

/// Errors from coordination. The reconcile loop MUST fail CLOSED on these: an
/// unknown/empty membership has no safe owner, so the node SKIPS rather than risk
/// two writers racing the k8s API.
#[derive(Debug, thiserror::Error)]
pub enum CoordError {
    #[error("membership unavailable: {0}")]
    Membership(String),
    #[error("StaticCoordinator with operator replicas={0}: split-brain. Set replicas:1 or wire a leaderless Coordinator (Lease/ZAP membership).")]
    SplitBrain(i32),
    #[error("in-cluster but could not verify the operator's own replica count (set POD_NAMESPACE + HANZOD_DEPLOYMENT_NAME and grant get deployments); refusing to start rather than risk split-brain")]
    Unverifiable,
}

/// The live, reconcile-eligible hanzod set plus this node's own id — the ONE
/// injectable source every coordinator shares. Mirrors `ha.Membership`.
///
/// A consumer MUST fail CLOSED on an error or an empty set: election over an
/// unknown or empty membership has no safe owner. A non-empty set with no error
/// is the only "proceed" signal.
#[async_trait]
pub trait Membership: Send + Sync {
    /// This node's stable id — the value HRW weights derive from.
    fn self_id(&self) -> &str;
    /// The live reconcile-eligible set, or an error the caller fails closed on.
    async fn members(&self) -> Result<Vec<Member>, CoordError>;
}

/// Single-process membership: one member, itself, never an error. The correct
/// source for one-hanzod-per-cluster, standalone runs, and tests, and the safe
/// default when no cluster source is wired (a single node is exactly-once by
/// construction). Mirrors `ha.Static`.
pub struct StaticMembership {
    id: String,
}

impl StaticMembership {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }
}

#[async_trait]
impl Membership for StaticMembership {
    fn self_id(&self) -> &str {
        &self.id
    }
    async fn members(&self) -> Result<Vec<Member>, CoordError> {
        Ok(vec![Member::new(self.id.clone())])
    }
}

/// The ONE question the reconcile loop asks before touching the k8s API for a
/// given object: *should THIS hanzod reconcile it right now?* Everything about
/// how that is decided — single node, HRW over a live set, or a future BFT round
/// — lives behind this trait, so [`reconcile`](crate::reconcile) never changes.
#[async_trait]
pub trait Coordinator: Send + Sync {
    /// `key` names the ownership unit (here `"<namespace>/<name>"` of the
    /// Service CR). Returns `Ok(true)` to reconcile, `Ok(false)` to stand aside
    /// (another node owns it), or an `Err` the caller treats as stand-aside
    /// (fail-closed).
    async fn should_reconcile(&self, key: &str) -> Result<bool, CoordError>;

    /// Whether this coordinator is only safe at ONE operator replica. A
    /// coordination-free default (`StaticCoordinator`) is: two replicas would
    /// both reconcile every object (split-brain). A real leaderless coordinator
    /// (HRW/ZAP over a live membership) returns false — it fences N replicas.
    /// `run()` hard-fails at boot if this is true and the operator Deployment
    /// has replicas > 1.
    fn requires_single_replica(&self) -> bool {
        false
    }
}

/// Boot guard for MED-5: reject an unsafe operator scale-out. Pure so it is
/// unit-tested without a cluster. `observed_replicas` is the operator's own
/// Deployment `spec.replicas`.
pub fn check_singleton(requires_single: bool, observed_replicas: i32) -> Result<(), CoordError> {
    if requires_single && observed_replicas > 1 {
        return Err(CoordError::SplitBrain(observed_replicas));
    }
    Ok(())
}

/// Fail-CLOSED boot decision for MED-3. `in_cluster` is whether the operator is
/// running under a k8s ServiceAccount (as opposed to a local/standalone run).
/// `resolved_replicas` is `Some` only when the operator could actually READ its
/// own Deployment's replica count; `None` means it could not verify.
///
/// - A coordinator that fences N replicas (`requires_single=false`) → always OK.
/// - Not in-cluster → a single process, single-replica by construction → OK.
/// - In-cluster + `requires_single` + could NOT verify replicas → REFUSE
///   (never proceed unverified — an unread replicas:2 is silent split-brain).
/// - In-cluster + verified → OK iff replicas == 1.
pub fn singleton_boot_decision(
    requires_single: bool,
    in_cluster: bool,
    resolved_replicas: Option<i32>,
) -> Result<(), CoordError> {
    if !requires_single || !in_cluster {
        return Ok(());
    }
    match resolved_replicas {
        Some(r) => check_singleton(true, r),
        None => Err(CoordError::Unverifiable),
    }
}

/// Single-node default: always the owner. This is the drop-in
/// one-hanzod-per-cluster behavior — no discovery, no election, exactly-once by
/// construction. Mirrors `ha.Static` + `ha.StaticFencer`.
#[derive(Clone, Default)]
pub struct StaticCoordinator;

#[async_trait]
impl Coordinator for StaticCoordinator {
    async fn should_reconcile(&self, _key: &str) -> Result<bool, CoordError> {
        Ok(true)
    }
    fn requires_single_replica(&self) -> bool {
        true
    }
}

/// HRW coordinator: composes a [`Membership`] with [`owner`] so N hanzods agree,
/// coordination-free, on one owner per key and fail over deterministically. This
/// is "HRW today". It has no fencing round yet (see the module docs): safe for a
/// stable membership, and the drop-in point for a `Fencer`-backed variant when
/// split-brain writes must be provably impossible.
pub struct HrwCoordinator<M: Membership> {
    membership: M,
}

impl<M: Membership> HrwCoordinator<M> {
    pub fn new(membership: M) -> Self {
        Self { membership }
    }
}

#[async_trait]
impl<M: Membership> Coordinator for HrwCoordinator<M> {
    async fn should_reconcile(&self, key: &str) -> Result<bool, CoordError> {
        let members = self.membership.members().await?;
        if members.is_empty() {
            // Fail closed: no safe owner over an empty set.
            return Ok(false);
        }
        Ok(is_owner(key, self.membership.self_id(), &members))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn members(ids: &[&str]) -> Vec<Member> {
        ids.iter().map(|i| Member::new(*i)).collect()
    }

    /// Locks the HRW weight to the byte-exact algorithm `ha` uses in Go, so a
    /// Rust hanzod and a Go `ha` elect the same owner. If this golden changes,
    /// cross-language agreement has silently broken.
    #[test]
    fn weight_matches_ha_golden() {
        assert_eq!(weight("hanzo/billing", "hanzod-a"), 1474527018049071440);
    }

    #[test]
    fn owner_is_the_highest_weight_member() {
        let m = members(&["hanzod-a", "hanzod-b", "hanzod-c"]);
        assert_eq!(owner("hanzo/billing", &m).unwrap().id, "hanzod-b");
        assert_eq!(owner("hanzo/cloud", &m).unwrap().id, "hanzod-a");
    }

    #[test]
    fn owner_is_order_independent() {
        let a = members(&["hanzod-a", "hanzod-b", "hanzod-c"]);
        let b = members(&["hanzod-c", "hanzod-a", "hanzod-b"]);
        assert_eq!(owner("k", &a).unwrap().id, owner("k", &b).unwrap().id);
    }

    #[test]
    fn owner_fails_closed_on_empty_set() {
        assert!(owner("k", &[]).is_none());
    }

    #[test]
    fn exactly_one_member_owns_a_key() {
        let m = members(&["hanzod-a", "hanzod-b", "hanzod-c"]);
        let owned: Vec<_> = m.iter().filter(|x| is_owner("k", &x.id, &m)).collect();
        assert_eq!(owned.len(), 1);
    }

    #[tokio::test]
    async fn static_coordinator_always_reconciles() {
        assert!(StaticCoordinator.should_reconcile("anything").await.unwrap());
    }

    #[tokio::test]
    async fn hrw_coordinator_reconciles_only_owned_keys() {
        // Single-member membership: this node owns every key.
        let solo = HrwCoordinator::new(StaticMembership::new("hanzod-a"));
        assert!(solo.should_reconcile("hanzo/billing").await.unwrap());

        // A membership where THIS node is not the owner stands aside. For key
        // "hanzo/billing" the owner is hanzod-b, so hanzod-a must NOT reconcile.
        struct ThreeSet;
        #[async_trait]
        impl Membership for ThreeSet {
            fn self_id(&self) -> &str {
                "hanzod-a"
            }
            async fn members(&self) -> Result<Vec<Member>, CoordError> {
                Ok(super::tests::members(&["hanzod-a", "hanzod-b", "hanzod-c"]))
            }
        }
        let coord = HrwCoordinator::new(ThreeSet);
        assert!(!coord.should_reconcile("hanzo/billing").await.unwrap());
        assert!(coord.should_reconcile("hanzo/cloud").await.unwrap());
    }

    #[tokio::test]
    async fn hrw_coordinator_fails_closed_on_empty_membership() {
        struct Empty;
        #[async_trait]
        impl Membership for Empty {
            fn self_id(&self) -> &str {
                "hanzod-a"
            }
            async fn members(&self) -> Result<Vec<Member>, CoordError> {
                Ok(vec![])
            }
        }
        assert!(!HrwCoordinator::new(Empty).should_reconcile("k").await.unwrap());
    }

    #[test]
    fn static_coordinator_requires_single_replica() {
        assert!(StaticCoordinator.requires_single_replica());
    }

    #[test]
    fn singleton_guard_rejects_operator_scale_out() {
        // StaticCoordinator (requires_single=true) at replicas>1 is split-brain.
        assert!(check_singleton(true, 2).is_err());
        assert!(check_singleton(true, 1).is_ok());
        // A real leaderless coordinator (requires_single=false) fences N replicas.
        assert!(check_singleton(false, 5).is_ok());
    }

    #[test]
    fn boot_decision_fails_closed_when_unverifiable() {
        // In-cluster + single-replica coordinator + can't read own replicas → REFUSE.
        assert!(singleton_boot_decision(true, true, None).is_err());
        // In-cluster + verified >1 → refuse; verified ==1 → ok.
        assert!(singleton_boot_decision(true, true, Some(2)).is_err());
        assert!(singleton_boot_decision(true, true, Some(1)).is_ok());
        // Not in-cluster (local/standalone) → single process → ok even if unread.
        assert!(singleton_boot_decision(true, false, None).is_ok());
        // Leaderless coordinator fences N → ok regardless.
        assert!(singleton_boot_decision(false, true, None).is_ok());
    }
}
