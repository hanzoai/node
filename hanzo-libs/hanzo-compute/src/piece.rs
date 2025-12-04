//! Task piece distribution
//!
//! Large compute tasks are split into pieces that can be distributed
//! across multiple peers. This follows BitTorrent's piece model:
//! - Each piece is independently verifiable
//! - Pieces can be computed by different peers
//! - Redundant computation enables verification

use crate::{ComputeTask, PeerId, TaskId};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Unique identifier for a piece (task_id + piece_index)
pub type PieceId = String;

/// State of a piece
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PieceState {
    /// Piece is waiting to be assigned
    Pending,
    /// Piece is assigned to peer(s) for computation
    Assigned,
    /// Piece computation is in progress
    InProgress,
    /// Piece has been computed (waiting for verification)
    Computed,
    /// Piece has been verified
    Verified,
    /// Piece computation failed
    Failed,
}

/// A piece of a compute task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Piece {
    /// Task this piece belongs to
    pub task_id: TaskId,
    /// Index of this piece within the task
    pub index: usize,
    /// Current state
    pub state: PieceState,
    /// Hash of the input data for this piece
    pub input_hash: String,
    /// Peers currently computing this piece
    pub assigned_peers: HashSet<PeerId>,
    /// Results received from peers (peer_id -> result_hash)
    pub results: HashMap<PeerId, String>,
    /// Verified result data (if verified)
    pub verified_result: Option<Vec<u8>>,
    /// Number of required redundant computations
    pub redundancy: usize,
    /// Deadline for this piece (Unix timestamp)
    pub deadline: Option<u64>,
    /// Priority (higher = more urgent)
    pub priority: u32,
    /// Number of times this piece has been retried
    pub retry_count: usize,
}

impl Piece {
    /// Create a new piece
    pub fn new(task_id: TaskId, index: usize, input_data: &[u8], redundancy: usize) -> Self {
        let input_hash = blake3::hash(input_data).to_hex().to_string();

        Self {
            task_id,
            index,
            state: PieceState::Pending,
            input_hash,
            assigned_peers: HashSet::new(),
            results: HashMap::new(),
            verified_result: None,
            redundancy,
            deadline: None,
            priority: 0,
            retry_count: 0,
        }
    }

    /// Get the unique piece ID
    pub fn id(&self) -> PieceId {
        format!("{}:{}", self.task_id, self.index)
    }

    /// Check if piece needs more peers assigned
    pub fn needs_more_peers(&self) -> bool {
        self.assigned_peers.len() < self.redundancy
    }

    /// Check if piece has enough results for verification
    pub fn ready_for_verification(&self) -> bool {
        self.results.len() >= self.redundancy
    }

    /// Assign a peer to compute this piece
    pub fn assign_peer(&mut self, peer_id: PeerId) {
        self.assigned_peers.insert(peer_id);
        if self.state == PieceState::Pending {
            self.state = PieceState::Assigned;
        }
    }

    /// Record a result from a peer
    pub fn record_result(&mut self, peer_id: PeerId, result_hash: String) {
        self.results.insert(peer_id, result_hash);
        if self.results.len() >= self.redundancy {
            self.state = PieceState::Computed;
        }
    }

    /// Mark piece as verified
    pub fn mark_verified(&mut self, result: Vec<u8>) {
        self.verified_result = Some(result);
        self.state = PieceState::Verified;
    }

    /// Mark piece as failed
    pub fn mark_failed(&mut self) {
        self.state = PieceState::Failed;
        self.retry_count += 1;
    }

    /// Reset piece for retry
    pub fn reset_for_retry(&mut self) {
        self.state = PieceState::Pending;
        self.assigned_peers.clear();
        self.results.clear();
    }
}

/// Manages pieces for all tasks in the swarm
#[derive(Debug)]
pub struct PieceManager {
    /// All pieces indexed by piece_id
    pieces: DashMap<PieceId, Piece>,
    /// Pieces by task_id
    task_pieces: DashMap<TaskId, Vec<PieceId>>,
    /// Pieces by state (for efficient querying)
    pieces_by_state: DashMap<PieceState, HashSet<PieceId>>,
    /// Piece availability (piece_id -> count of peers that have computed it)
    availability: DashMap<PieceId, usize>,
}

impl PieceManager {
    /// Create a new piece manager
    pub fn new() -> Self {
        Self {
            pieces: DashMap::new(),
            task_pieces: DashMap::new(),
            pieces_by_state: DashMap::new(),
            availability: DashMap::new(),
        }
    }

    /// Create pieces for a task
    pub fn create_pieces_for_task(&self, task: &ComputeTask, input_chunks: Vec<Vec<u8>>) {
        let mut piece_ids = Vec::with_capacity(task.num_pieces);

        for (index, chunk) in input_chunks.iter().enumerate() {
            let mut piece = Piece::new(task.id.clone(), index, chunk, task.redundancy);
            piece.deadline = task.deadline;

            let piece_id = piece.id();
            piece_ids.push(piece_id.clone());

            self.pieces.insert(piece_id.clone(), piece);
            self.availability.insert(piece_id.clone(), 0);

            // Track by state
            self.pieces_by_state
                .entry(PieceState::Pending)
                .or_insert_with(HashSet::new)
                .insert(piece_id);
        }

        self.task_pieces.insert(task.id.clone(), piece_ids);
    }

    /// Get a piece by ID
    pub fn get_piece(&self, piece_id: &PieceId) -> Option<Piece> {
        self.pieces.get(piece_id).map(|p| p.clone())
    }

    /// Get all pieces for a task
    pub fn get_task_pieces(&self, task_id: &TaskId) -> Vec<Piece> {
        self.task_pieces
            .get(task_id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.pieces.get(id).map(|p| p.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get pending pieces sorted by rarity (rarest first)
    pub fn get_rarest_pending_pieces(&self, limit: usize) -> Vec<Piece> {
        let pending = self.pieces_by_state.get(&PieceState::Pending);

        let mut pieces: Vec<_> = pending
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| {
                        self.pieces.get(id).map(|p| {
                            let availability = self
                                .availability
                                .get(id)
                                .map(|a| *a)
                                .unwrap_or(0);
                            (p.clone(), availability)
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Sort by availability (rarest first), then by priority (highest first)
        pieces.sort_by(|(p1, a1), (p2, a2)| {
            a1.cmp(a2).then_with(|| p2.priority.cmp(&p1.priority))
        });

        pieces.into_iter().take(limit).map(|(p, _)| p).collect()
    }

    /// Assign a peer to a piece
    pub fn assign_peer(&self, piece_id: &PieceId, peer_id: PeerId) -> bool {
        if let Some(mut piece) = self.pieces.get_mut(piece_id) {
            let old_state = piece.state;
            piece.assign_peer(peer_id);

            // Update state tracking
            if old_state != piece.state {
                self.update_state_tracking(piece_id, old_state, piece.state);
            }
            true
        } else {
            false
        }
    }

    /// Record a result for a piece
    pub fn record_result(&self, piece_id: &PieceId, peer_id: PeerId, result_hash: String) -> bool {
        if let Some(mut piece) = self.pieces.get_mut(piece_id) {
            let old_state = piece.state;
            piece.record_result(peer_id.clone(), result_hash);

            // Update availability
            self.availability
                .entry(piece_id.clone())
                .and_modify(|a| *a += 1)
                .or_insert(1);

            // Update state tracking
            if old_state != piece.state {
                self.update_state_tracking(piece_id, old_state, piece.state);
            }
            true
        } else {
            false
        }
    }

    /// Mark a piece as verified
    pub fn mark_verified(&self, piece_id: &PieceId, result: Vec<u8>) -> bool {
        if let Some(mut piece) = self.pieces.get_mut(piece_id) {
            let old_state = piece.state;
            piece.mark_verified(result);
            self.update_state_tracking(piece_id, old_state, piece.state);
            true
        } else {
            false
        }
    }

    /// Check if all pieces for a task are verified
    pub fn is_task_complete(&self, task_id: &TaskId) -> bool {
        self.task_pieces
            .get(task_id)
            .map(|ids| {
                ids.iter().all(|id| {
                    self.pieces
                        .get(id)
                        .map(|p| p.state == PieceState::Verified)
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
    }

    /// Get task completion progress
    pub fn get_task_progress(&self, task_id: &TaskId) -> (usize, usize) {
        self.task_pieces
            .get(task_id)
            .map(|ids| {
                let total = ids.len();
                let verified = ids
                    .iter()
                    .filter(|id| {
                        self.pieces
                            .get(*id)
                            .map(|p| p.state == PieceState::Verified)
                            .unwrap_or(false)
                    })
                    .count();
                (verified, total)
            })
            .unwrap_or((0, 0))
    }

    /// Update state tracking indices
    fn update_state_tracking(&self, piece_id: &PieceId, old_state: PieceState, new_state: PieceState) {
        // Remove from old state set
        if let Some(mut old_set) = self.pieces_by_state.get_mut(&old_state) {
            old_set.remove(piece_id);
        }

        // Add to new state set
        self.pieces_by_state
            .entry(new_state)
            .or_insert_with(HashSet::new)
            .insert(piece_id.clone());
    }

    /// Get statistics
    pub fn get_stats(&self) -> PieceStats {
        let mut stats = PieceStats::default();

        for state in [
            PieceState::Pending,
            PieceState::Assigned,
            PieceState::InProgress,
            PieceState::Computed,
            PieceState::Verified,
            PieceState::Failed,
        ] {
            let count = self
                .pieces_by_state
                .get(&state)
                .map(|s| s.len())
                .unwrap_or(0);

            match state {
                PieceState::Pending => stats.pending = count,
                PieceState::Assigned => stats.assigned = count,
                PieceState::InProgress => stats.in_progress = count,
                PieceState::Computed => stats.computed = count,
                PieceState::Verified => stats.verified = count,
                PieceState::Failed => stats.failed = count,
            }
        }

        stats.total = self.pieces.len();
        stats
    }

    /// Remove all pieces for a task
    pub fn remove_task(&self, task_id: &TaskId) {
        if let Some((_, piece_ids)) = self.task_pieces.remove(task_id) {
            for piece_id in piece_ids {
                if let Some((_, piece)) = self.pieces.remove(&piece_id) {
                    // Remove from state tracking
                    if let Some(mut state_set) = self.pieces_by_state.get_mut(&piece.state) {
                        state_set.remove(&piece_id);
                    }
                }
                self.availability.remove(&piece_id);
            }
        }
    }
}

impl Default for PieceManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics about pieces
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PieceStats {
    pub total: usize,
    pub pending: usize,
    pub assigned: usize,
    pub in_progress: usize,
    pub computed: usize,
    pub verified: usize,
    pub failed: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_piece_creation() {
        let piece = Piece::new("task-123".to_string(), 0, b"test data", 3);

        assert_eq!(piece.task_id, "task-123");
        assert_eq!(piece.index, 0);
        assert_eq!(piece.state, PieceState::Pending);
        assert_eq!(piece.redundancy, 3);
        assert!(!piece.input_hash.is_empty());
    }

    #[test]
    fn test_piece_assignment() {
        let mut piece = Piece::new("task-123".to_string(), 0, b"test", 2);

        piece.assign_peer("peer-1".to_string());
        assert!(piece.needs_more_peers());
        assert_eq!(piece.state, PieceState::Assigned);

        piece.assign_peer("peer-2".to_string());
        assert!(!piece.needs_more_peers());
    }

    #[test]
    fn test_piece_results() {
        let mut piece = Piece::new("task-123".to_string(), 0, b"test", 2);

        piece.record_result("peer-1".to_string(), "hash1".to_string());
        assert!(!piece.ready_for_verification());

        piece.record_result("peer-2".to_string(), "hash2".to_string());
        assert!(piece.ready_for_verification());
        assert_eq!(piece.state, PieceState::Computed);
    }

    #[test]
    fn test_piece_manager() {
        let manager = PieceManager::new();

        let task = ComputeTask::new(
            crate::TaskType::Inference {
                model: "test".to_string(),
                prompt: "hello".to_string(),
                max_tokens: 10,
            },
            1.0,
        )
        .with_pieces(3)
        .with_redundancy(2);

        let chunks: Vec<Vec<u8>> = (0..3).map(|i| vec![i as u8; 100]).collect();
        manager.create_pieces_for_task(&task, chunks);

        let pieces = manager.get_task_pieces(&task.id);
        assert_eq!(pieces.len(), 3);

        let stats = manager.get_stats();
        assert_eq!(stats.total, 3);
        assert_eq!(stats.pending, 3);
    }

    #[test]
    fn test_rarest_first() {
        let manager = PieceManager::new();

        // Create a task with pieces
        let task = ComputeTask::new(
            crate::TaskType::Inference {
                model: "test".to_string(),
                prompt: "hello".to_string(),
                max_tokens: 10,
            },
            1.0,
        )
        .with_pieces(3)
        .with_redundancy(1);

        let chunks: Vec<Vec<u8>> = (0..3).map(|i| vec![i as u8; 100]).collect();
        manager.create_pieces_for_task(&task, chunks);

        // Simulate different availability
        let pieces = manager.get_task_pieces(&task.id);
        manager.availability.insert(pieces[0].id(), 5);
        manager.availability.insert(pieces[1].id(), 1);
        manager.availability.insert(pieces[2].id(), 3);

        // Get rarest pieces
        let rarest = manager.get_rarest_pending_pieces(10);

        // Piece with availability 1 should come first
        assert_eq!(rarest[0].index, 1);
    }
}
