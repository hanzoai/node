//! Task scheduling with BitTorrent-inspired strategies
//!
//! Implements various scheduling strategies:
//! - Rarest-first: Prioritize pieces that are least available
//! - Priority-based: Prioritize high-priority pieces
//! - Deadline-aware: Prioritize pieces with approaching deadlines
//! - Capability-matching: Match pieces to peers with appropriate capabilities

use crate::peer::{Peer, PeerId};
use crate::piece::{Piece, PieceManager};
use crate::TaskId;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Scheduling strategy for piece assignment
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SchedulingStrategy {
    /// Prioritize pieces with lowest availability (BitTorrent-style)
    RarestFirst,
    /// Prioritize pieces with highest priority score
    PriorityFirst,
    /// Prioritize pieces with nearest deadlines
    DeadlineFirst,
    /// Round-robin across available pieces
    RoundRobin,
    /// Random selection
    Random,
    /// Hybrid: combine rarest-first with priority and deadline awareness
    Hybrid,
}

impl Default for SchedulingStrategy {
    fn default() -> Self {
        Self::Hybrid
    }
}

/// Scheduler for assigning pieces to peers
pub struct Scheduler {
    /// Scheduling strategy
    strategy: SchedulingStrategy,
    /// Peer assignments: task_id -> peer_id -> assigned pieces
    assignments: DashMap<TaskId, DashMap<PeerId, HashSet<String>>>,
    /// Peer capacities: peer_id -> (current load, max capacity)
    peer_loads: DashMap<PeerId, (usize, usize)>,
    /// Round-robin index
    round_robin_index: std::sync::atomic::AtomicUsize,
}

impl Scheduler {
    /// Create a new scheduler with the given strategy
    pub fn new(strategy: SchedulingStrategy) -> Self {
        Self {
            strategy,
            assignments: DashMap::new(),
            peer_loads: DashMap::new(),
            round_robin_index: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Set the scheduling strategy
    pub fn set_strategy(&mut self, strategy: SchedulingStrategy) {
        self.strategy = strategy;
    }

    /// Register a peer with its capacity
    pub fn register_peer(&self, peer_id: PeerId, max_capacity: usize) {
        self.peer_loads.insert(peer_id, (0, max_capacity));
    }

    /// Unregister a peer
    pub fn unregister_peer(&self, peer_id: &PeerId) {
        self.peer_loads.remove(peer_id);

        // Remove all assignments for this peer
        for task_entry in self.assignments.iter() {
            if let Some(mut peer_assignments) = task_entry.value().get_mut(peer_id) {
                peer_assignments.clear();
            }
        }
    }

    /// Get available peers for a piece based on requirements
    pub fn get_available_peers(
        &self,
        peers: &[Peer],
        min_reputation: f64,
        required_model: Option<&str>,
    ) -> Vec<PeerId> {
        peers
            .iter()
            .filter(|p| {
                // Check if peer can accept tasks
                if !p.can_accept_task() {
                    return false;
                }

                // Check reputation
                if !p.meets_reputation(min_reputation) {
                    return false;
                }

                // Check model support
                if let Some(model) = required_model {
                    if !p.supports_model(model) {
                        return false;
                    }
                }

                // Check capacity in scheduler
                if let Some(load) = self.peer_loads.get(&p.id) {
                    if load.0 >= load.1 {
                        return false;
                    }
                }

                true
            })
            .map(|p| p.id.clone())
            .collect()
    }

    /// Select pieces to assign to peers
    pub fn select_pieces(
        &self,
        piece_manager: &PieceManager,
        available_peers: &[PeerId],
        max_assignments: usize,
    ) -> Vec<(String, PeerId)> {
        if available_peers.is_empty() {
            return Vec::new();
        }

        let mut assignments = Vec::new();

        // Get pieces based on strategy
        let pieces = match self.strategy {
            SchedulingStrategy::RarestFirst => {
                piece_manager.get_rarest_pending_pieces(max_assignments * 2)
            }
            SchedulingStrategy::PriorityFirst => {
                self.get_priority_sorted_pieces(piece_manager, max_assignments * 2)
            }
            SchedulingStrategy::DeadlineFirst => {
                self.get_deadline_sorted_pieces(piece_manager, max_assignments * 2)
            }
            SchedulingStrategy::RoundRobin => {
                self.get_round_robin_pieces(piece_manager, max_assignments * 2)
            }
            SchedulingStrategy::Random => {
                self.get_random_pieces(piece_manager, max_assignments * 2)
            }
            SchedulingStrategy::Hybrid => {
                self.get_hybrid_sorted_pieces(piece_manager, max_assignments * 2)
            }
        };

        // Assign pieces to peers
        let mut peer_index = 0;
        for piece in pieces {
            if assignments.len() >= max_assignments {
                break;
            }

            // Check if piece needs more assignments
            if !piece.needs_more_peers() {
                continue;
            }

            // Find an available peer that isn't already assigned to this piece
            let mut assigned = false;
            for _ in 0..available_peers.len() {
                let peer_id = &available_peers[peer_index % available_peers.len()];
                peer_index += 1;

                if !piece.assigned_peers.contains(peer_id) {
                    // Check peer capacity
                    if let Some(mut load) = self.peer_loads.get_mut(peer_id) {
                        if load.0 < load.1 {
                            load.0 += 1;
                            assignments.push((piece.id(), peer_id.clone()));
                            assigned = true;
                            break;
                        }
                    } else {
                        assignments.push((piece.id(), peer_id.clone()));
                        assigned = true;
                        break;
                    }
                }
            }

            if !assigned {
                // No available peer for this piece
                continue;
            }
        }

        assignments
    }

    /// Record that a peer completed a piece
    pub fn record_completion(&self, task_id: &TaskId, peer_id: &PeerId, piece_id: &str) {
        // Update peer load
        if let Some(mut load) = self.peer_loads.get_mut(peer_id) {
            load.0 = load.0.saturating_sub(1);
        }

        // Remove from assignments
        if let Some(task_assignments) = self.assignments.get(task_id) {
            if let Some(mut peer_pieces) = task_assignments.get_mut(peer_id) {
                peer_pieces.remove(piece_id);
            }
        }
    }

    /// Record that a piece assignment failed
    pub fn record_failure(&self, task_id: &TaskId, peer_id: &PeerId, piece_id: &str) {
        // Same as completion - frees up the peer
        self.record_completion(task_id, peer_id, piece_id);
    }

    // Helper methods for different sorting strategies

    fn get_priority_sorted_pieces(&self, piece_manager: &PieceManager, limit: usize) -> Vec<Piece> {
        let mut pieces: Vec<_> = piece_manager
            .get_rarest_pending_pieces(limit * 2)
            .into_iter()
            .collect();

        pieces.sort_by(|a, b| b.priority.cmp(&a.priority));
        pieces.truncate(limit);
        pieces
    }

    fn get_deadline_sorted_pieces(&self, piece_manager: &PieceManager, limit: usize) -> Vec<Piece> {
        let mut pieces: Vec<_> = piece_manager
            .get_rarest_pending_pieces(limit * 2)
            .into_iter()
            .collect();

        pieces.sort_by(|a, b| {
            let deadline_a = a.deadline.unwrap_or(u64::MAX);
            let deadline_b = b.deadline.unwrap_or(u64::MAX);
            deadline_a.cmp(&deadline_b)
        });

        pieces.truncate(limit);
        pieces
    }

    fn get_round_robin_pieces(&self, piece_manager: &PieceManager, limit: usize) -> Vec<Piece> {
        let pieces = piece_manager.get_rarest_pending_pieces(limit * 2);
        let pieces_len = pieces.len();
        let start = self.round_robin_index.fetch_add(limit, std::sync::atomic::Ordering::Relaxed);

        if pieces_len == 0 {
            return Vec::new();
        }

        pieces
            .into_iter()
            .cycle()
            .skip(start % pieces_len)
            .take(limit)
            .collect()
    }

    fn get_random_pieces(&self, piece_manager: &PieceManager, limit: usize) -> Vec<Piece> {
        use rand::seq::SliceRandom;

        let mut pieces: Vec<_> = piece_manager
            .get_rarest_pending_pieces(limit * 2)
            .into_iter()
            .collect();

        let mut rng = rand::thread_rng();
        pieces.shuffle(&mut rng);
        pieces.truncate(limit);
        pieces
    }

    fn get_hybrid_sorted_pieces(&self, piece_manager: &PieceManager, limit: usize) -> Vec<Piece> {
        let mut pieces: Vec<_> = piece_manager
            .get_rarest_pending_pieces(limit * 2)
            .into_iter()
            .collect();

        let now = chrono::Utc::now().timestamp() as u64;

        // Score each piece based on multiple factors
        pieces.sort_by(|a, b| {
            let score_a = self.calculate_hybrid_score(a, now);
            let score_b = self.calculate_hybrid_score(b, now);
            score_b.partial_cmp(&score_a).unwrap_or(std::cmp::Ordering::Equal)
        });

        pieces.truncate(limit);
        pieces
    }

    fn calculate_hybrid_score(&self, piece: &Piece, now: u64) -> f64 {
        let mut score = 0.0;

        // Priority component (0-100)
        score += piece.priority as f64;

        // Deadline urgency (0-100)
        if let Some(deadline) = piece.deadline {
            if deadline > now {
                let time_remaining = deadline - now;
                // Higher score for pieces with closer deadlines
                // Full 100 points if deadline is within 1 minute
                score += (60.0 / (time_remaining as f64 + 1.0)) * 100.0;
            } else {
                // Already past deadline - highest priority
                score += 200.0;
            }
        }

        // Retry penalty (-10 per retry)
        score -= piece.retry_count as f64 * 10.0;

        score
    }

    /// Get scheduling statistics
    pub fn get_stats(&self) -> SchedulerStats {
        let mut total_assignments = 0;
        let mut peer_assignment_counts = HashMap::new();

        for task_entry in self.assignments.iter() {
            for peer_entry in task_entry.value().iter() {
                let count = peer_entry.value().len();
                total_assignments += count;
                *peer_assignment_counts.entry(peer_entry.key().clone()).or_insert(0) += count;
            }
        }

        let active_peers = self
            .peer_loads
            .iter()
            .filter(|e| e.value().0 > 0)
            .count();

        SchedulerStats {
            strategy: self.strategy,
            total_assignments,
            active_peers,
            peer_assignment_counts,
        }
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new(SchedulingStrategy::default())
    }
}

/// Statistics about the scheduler
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerStats {
    pub strategy: SchedulingStrategy,
    pub total_assignments: usize,
    pub active_peers: usize,
    pub peer_assignment_counts: HashMap<PeerId, usize>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peer::PeerCapabilities;

    fn create_test_peer(id: &str, max_tasks: usize) -> Peer {
        let mut peer = Peer::new(id.to_string(), format!("127.0.0.1:{}", 8000 + id.len()));
        peer.capabilities = PeerCapabilities {
            max_concurrent_tasks: max_tasks,
            ..Default::default()
        };
        peer.state = crate::peer::PeerState::Connected;
        peer
    }

    #[test]
    fn test_scheduler_creation() {
        let scheduler = Scheduler::new(SchedulingStrategy::RarestFirst);
        assert_eq!(scheduler.strategy, SchedulingStrategy::RarestFirst);
    }

    #[test]
    fn test_peer_registration() {
        let scheduler = Scheduler::new(SchedulingStrategy::RarestFirst);

        scheduler.register_peer("peer-1".to_string(), 5);
        scheduler.register_peer("peer-2".to_string(), 3);

        assert!(scheduler.peer_loads.contains_key("peer-1"));
        assert!(scheduler.peer_loads.contains_key("peer-2"));

        scheduler.unregister_peer(&"peer-1".to_string());
        assert!(!scheduler.peer_loads.contains_key("peer-1"));
    }

    #[test]
    fn test_get_available_peers() {
        let scheduler = Scheduler::new(SchedulingStrategy::RarestFirst);

        let peers = vec![
            create_test_peer("peer-1", 5),
            create_test_peer("peer-2", 3),
            create_test_peer("peer-3", 1),
        ];

        // Register peers
        for peer in &peers {
            scheduler.register_peer(peer.id.clone(), peer.capabilities.max_concurrent_tasks);
        }

        let available = scheduler.get_available_peers(&peers, 0.0, None);
        assert_eq!(available.len(), 3);
    }

    #[test]
    fn test_scheduling_strategies() {
        let strategies = vec![
            SchedulingStrategy::RarestFirst,
            SchedulingStrategy::PriorityFirst,
            SchedulingStrategy::DeadlineFirst,
            SchedulingStrategy::RoundRobin,
            SchedulingStrategy::Random,
            SchedulingStrategy::Hybrid,
        ];

        for strategy in strategies {
            let scheduler = Scheduler::new(strategy);
            assert_eq!(scheduler.strategy, strategy);
        }
    }

    #[test]
    fn test_hybrid_score() {
        let scheduler = Scheduler::new(SchedulingStrategy::Hybrid);
        let now = chrono::Utc::now().timestamp() as u64;

        // Piece with deadline soon should score higher
        let mut piece_urgent = Piece::new("task-1".to_string(), 0, b"data", 1);
        piece_urgent.deadline = Some(now + 30); // 30 seconds from now

        let mut piece_normal = Piece::new("task-1".to_string(), 1, b"data", 1);
        piece_normal.deadline = Some(now + 3600); // 1 hour from now

        let score_urgent = scheduler.calculate_hybrid_score(&piece_urgent, now);
        let score_normal = scheduler.calculate_hybrid_score(&piece_normal, now);

        assert!(score_urgent > score_normal);
    }
}
