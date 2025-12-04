//! BitTorrent-style Compute Swarm
//!
//! The ComputeSwarm is the main entry point for distributed compute operations.
//! It manages:
//! - Peer discovery and connection
//! - Task submission and distribution
//! - Piece scheduling using rarest-first strategy
//! - Result verification and consensus

use crate::error::{ComputeError, ComputeResult};
use crate::peer::{Peer, PeerId, PeerState};
use crate::piece::{PieceManager, PieceStats};
use crate::scheduler::{Scheduler, SchedulerStats, SchedulingStrategy};
use crate::verifier::{ResultVerifier, VerificationMethod};
use crate::{ComputeResult as TaskResult, ComputeTask, TaskId, TaskType};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

/// Configuration for the compute swarm
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmConfig {
    /// Local peer ID (our identity)
    pub local_peer_id: PeerId,
    /// Address to listen on
    pub listen_address: String,
    /// Scheduling strategy
    pub scheduling_strategy: SchedulingStrategy,
    /// Verification method
    pub verification_method: VerificationMethod,
    /// Maximum concurrent tasks to process
    pub max_concurrent_tasks: usize,
    /// Minimum reputation to accept tasks from
    pub min_reputation: f64,
    /// Default redundancy for task verification
    pub default_redundancy: usize,
    /// Task timeout in seconds
    pub task_timeout_secs: u64,
    /// Peer discovery interval in seconds
    pub discovery_interval_secs: u64,
    /// Maximum peers to maintain connections with
    pub max_peers: usize,
}

impl Default for SwarmConfig {
    fn default() -> Self {
        Self {
            local_peer_id: uuid::Uuid::new_v4().to_string(),
            listen_address: "0.0.0.0:3691".to_string(),
            scheduling_strategy: SchedulingStrategy::Hybrid,
            verification_method: VerificationMethod::MajorityConsensus,
            max_concurrent_tasks: 10,
            min_reputation: 10.0,
            default_redundancy: 3,
            task_timeout_secs: 300,
            discovery_interval_secs: 30,
            max_peers: 50,
        }
    }
}

/// Events emitted by the swarm
#[derive(Debug, Clone)]
pub enum SwarmEvent {
    /// New peer connected
    PeerConnected(PeerId),
    /// Peer disconnected
    PeerDisconnected(PeerId),
    /// Task submitted
    TaskSubmitted(TaskId),
    /// Task piece assigned to peer
    PieceAssigned { task_id: TaskId, piece_index: usize, peer_id: PeerId },
    /// Piece result received
    PieceResultReceived { task_id: TaskId, piece_index: usize, peer_id: PeerId },
    /// Piece verified
    PieceVerified { task_id: TaskId, piece_index: usize },
    /// Task completed
    TaskCompleted(TaskId),
    /// Task failed
    TaskFailed { task_id: TaskId, reason: String },
}

/// The main compute swarm
pub struct ComputeSwarm {
    /// Configuration
    config: SwarmConfig,
    /// Connected peers
    peers: DashMap<PeerId, Peer>,
    /// Active tasks
    tasks: DashMap<TaskId, ComputeTask>,
    /// Piece manager
    piece_manager: Arc<PieceManager>,
    /// Scheduler
    scheduler: Arc<Scheduler>,
    /// Result verifier
    verifier: Arc<ResultVerifier>,
    /// Results storage: task_id -> piece_index -> results
    results: DashMap<TaskId, DashMap<usize, Vec<TaskResult>>>,
    /// Event sender
    event_tx: Option<mpsc::UnboundedSender<SwarmEvent>>,
    /// Running flag
    running: Arc<RwLock<bool>>,
    /// Completed task results
    completed_results: DashMap<TaskId, Vec<Vec<u8>>>,
}

impl ComputeSwarm {
    /// Create a new compute swarm
    pub async fn new(config: SwarmConfig) -> ComputeResult<Self> {
        let scheduler = Arc::new(Scheduler::new(config.scheduling_strategy));
        let verifier = Arc::new(ResultVerifier::with_method(config.verification_method));

        Ok(Self {
            config,
            peers: DashMap::new(),
            tasks: DashMap::new(),
            piece_manager: Arc::new(PieceManager::new()),
            scheduler,
            verifier,
            results: DashMap::new(),
            event_tx: None,
            running: Arc::new(RwLock::new(false)),
            completed_results: DashMap::new(),
        })
    }

    /// Start the swarm
    pub async fn start(&self) -> ComputeResult<()> {
        *self.running.write().await = true;
        tracing::info!("Compute swarm started: {}", self.config.local_peer_id);
        Ok(())
    }

    /// Stop the swarm
    pub async fn stop(&self) -> ComputeResult<()> {
        *self.running.write().await = false;
        tracing::info!("Compute swarm stopped");
        Ok(())
    }

    /// Subscribe to swarm events
    pub fn subscribe(&mut self) -> mpsc::UnboundedReceiver<SwarmEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.event_tx = Some(tx);
        rx
    }

    /// Add a peer to the swarm
    pub fn add_peer(&self, peer: Peer) -> ComputeResult<()> {
        let peer_id = peer.id.clone();

        if self.peers.contains_key(&peer_id) {
            return Err(ComputeError::PeerAlreadyExists(peer_id));
        }

        if self.peers.len() >= self.config.max_peers {
            return Err(ComputeError::CapacityExceeded {
                peer_id: self.config.local_peer_id.clone(),
                current: self.peers.len(),
                max: self.config.max_peers,
            });
        }

        self.scheduler.register_peer(
            peer_id.clone(),
            peer.capabilities.max_concurrent_tasks,
        );
        self.peers.insert(peer_id.clone(), peer);

        self.emit_event(SwarmEvent::PeerConnected(peer_id));
        Ok(())
    }

    /// Remove a peer from the swarm
    pub fn remove_peer(&self, peer_id: &PeerId) -> ComputeResult<()> {
        self.peers
            .remove(peer_id)
            .ok_or_else(|| ComputeError::PeerNotFound(peer_id.clone()))?;

        self.scheduler.unregister_peer(peer_id);
        self.emit_event(SwarmEvent::PeerDisconnected(peer_id.clone()));
        Ok(())
    }

    /// Get a peer by ID
    pub fn get_peer(&self, peer_id: &PeerId) -> Option<Peer> {
        self.peers.get(peer_id).map(|p| p.clone())
    }

    /// Get all connected peers
    pub fn get_peers(&self) -> Vec<Peer> {
        self.peers.iter().map(|e| e.value().clone()).collect()
    }

    /// Submit a task to the swarm
    pub async fn submit_task(&self, mut task: ComputeTask) -> ComputeResult<TaskId> {
        let task_id = task.id.clone();

        if self.tasks.contains_key(&task_id) {
            return Err(ComputeError::TaskAlreadyExists(task_id));
        }

        // Set defaults from config
        if task.redundancy == 0 {
            task.redundancy = self.config.default_redundancy;
        }
        task.creator = self.config.local_peer_id.clone();

        // Create input chunks for pieces
        let input_chunks = self.create_input_chunks(&task);

        // Create pieces
        self.piece_manager.create_pieces_for_task(&task, input_chunks);

        // Store task
        self.tasks.insert(task_id.clone(), task);
        self.results.insert(task_id.clone(), DashMap::new());

        // Schedule initial piece assignments
        self.schedule_pieces(&task_id).await?;

        self.emit_event(SwarmEvent::TaskSubmitted(task_id.clone()));
        Ok(task_id)
    }

    /// Create input chunks from task data
    fn create_input_chunks(&self, task: &ComputeTask) -> Vec<Vec<u8>> {
        // Serialize task type as input data
        let input = serde_json::to_vec(&task.task_type).unwrap_or_default();

        if task.num_pieces == 1 {
            return vec![input];
        }

        // Split into chunks
        let chunk_size = (input.len() + task.num_pieces - 1) / task.num_pieces;
        input
            .chunks(chunk_size.max(1))
            .map(|c| c.to_vec())
            .collect()
    }

    /// Schedule pieces to available peers
    async fn schedule_pieces(&self, task_id: &TaskId) -> ComputeResult<()> {
        let task = self
            .tasks
            .get(task_id)
            .ok_or_else(|| ComputeError::TaskNotFound(task_id.clone()))?;

        // Get model requirement from task
        let required_model = match &task.task_type {
            TaskType::Inference { model, .. } => Some(model.as_str()),
            TaskType::Embedding { model, .. } => Some(model.as_str()),
            TaskType::Reranking { model, .. } => Some(model.as_str()),
            TaskType::Training { model, .. } => Some(model.as_str()),
            TaskType::Custom { .. } => None,
        };

        // Get available peers
        let peers: Vec<_> = self.peers.iter().map(|e| e.value().clone()).collect();
        let available_peers = self.scheduler.get_available_peers(
            &peers,
            task.min_reputation,
            required_model,
        );

        if available_peers.is_empty() {
            return Err(ComputeError::NoPeersAvailable(task_id.clone()));
        }

        // Select pieces and assign to peers
        let assignments = self.scheduler.select_pieces(
            &self.piece_manager,
            &available_peers,
            self.config.max_concurrent_tasks,
        );

        for (piece_id, peer_id) in assignments {
            self.piece_manager.assign_peer(&piece_id, peer_id.clone());

            // Extract piece index from piece_id
            if let Some(piece) = self.piece_manager.get_piece(&piece_id) {
                self.emit_event(SwarmEvent::PieceAssigned {
                    task_id: task_id.clone(),
                    piece_index: piece.index,
                    peer_id,
                });
            }
        }

        Ok(())
    }

    /// Submit a result for a piece
    pub async fn submit_result(&self, result: TaskResult) -> ComputeResult<()> {
        let piece_id = format!("{}:{}", result.task_id, result.piece_index);

        // Record result in piece manager
        self.piece_manager.record_result(
            &piece_id,
            result.computed_by.clone(),
            result.result_hash.clone(),
        );

        // Store result
        if let Some(task_results) = self.results.get(&result.task_id) {
            task_results
                .entry(result.piece_index)
                .or_insert_with(Vec::new)
                .push(result.clone());
        }

        self.emit_event(SwarmEvent::PieceResultReceived {
            task_id: result.task_id.clone(),
            piece_index: result.piece_index,
            peer_id: result.computed_by.clone(),
        });

        // Check if piece is ready for verification
        if let Some(piece) = self.piece_manager.get_piece(&piece_id) {
            if piece.ready_for_verification() {
                self.verify_piece(&result.task_id, result.piece_index).await?;
            }
        }

        // Check if task is complete
        if self.piece_manager.is_task_complete(&result.task_id) {
            self.complete_task(&result.task_id).await?;
        }

        Ok(())
    }

    /// Verify a piece
    async fn verify_piece(&self, task_id: &TaskId, piece_index: usize) -> ComputeResult<()> {
        let piece_id = format!("{}:{}", task_id, piece_index);

        let piece = self
            .piece_manager
            .get_piece(&piece_id)
            .ok_or_else(|| ComputeError::PieceNotFound {
                task_id: task_id.clone(),
                piece_index,
            })?;

        // Get results for this piece
        let results: Vec<TaskResult> = self
            .results
            .get(task_id)
            .and_then(|r| r.get(&piece_index).map(|v| v.clone()))
            .unwrap_or_default();

        // Verify
        let verification = self.verifier.verify_piece(&piece, &results);

        if verification.success {
            // Find the matching result data
            if let Some(verified_hash) = &verification.verified_hash {
                if let Some(result) = results.iter().find(|r| &r.result_hash == verified_hash) {
                    self.piece_manager.mark_verified(&piece_id, result.data.clone());
                }
            }

            // Update peer reputations
            let adjustments = self.verifier.calculate_reputation_adjustments(&verification);
            for (peer_id, adjustment) in adjustments {
                if let Some(mut peer) = self.peers.get_mut(&peer_id) {
                    peer.reputation = (peer.reputation + adjustment).clamp(0.0, 100.0);
                }
            }

            self.emit_event(SwarmEvent::PieceVerified {
                task_id: task_id.clone(),
                piece_index,
            });
        } else {
            // Piece verification failed - penalize peers and retry
            for peer_id in &verification.non_matching_peers {
                if let Some(mut peer) = self.peers.get_mut(peer_id) {
                    peer.record_task_failure();
                }
            }

            // TODO: Implement retry logic
            tracing::warn!(
                "Piece verification failed: task={}, piece={}",
                task_id,
                piece_index
            );
        }

        Ok(())
    }

    /// Complete a task
    async fn complete_task(&self, task_id: &TaskId) -> ComputeResult<()> {
        // Collect verified results
        let pieces = self.piece_manager.get_task_pieces(task_id);
        let mut results: Vec<Vec<u8>> = Vec::with_capacity(pieces.len());

        for piece in pieces {
            if let Some(data) = piece.verified_result {
                results.push(data);
            } else {
                return Err(ComputeError::VerificationFailed {
                    task_id: task_id.clone(),
                    reason: format!("Piece {} not verified", piece.index),
                });
            }
        }

        // Store completed results
        self.completed_results.insert(task_id.clone(), results);

        // Update peer stats for successful completion
        if let Some(task_results) = self.results.get(task_id) {
            for piece_results in task_results.iter() {
                for result in piece_results.value() {
                    if let Some(mut peer) = self.peers.get_mut(&result.computed_by) {
                        // Calculate reward share
                        if let Some(task) = self.tasks.get(task_id) {
                            let reward_per_piece = task.reward / task.num_pieces as f64;
                            peer.record_task_success(result.compute_time_ms, reward_per_piece);
                        }
                    }
                }
            }
        }

        self.emit_event(SwarmEvent::TaskCompleted(task_id.clone()));
        Ok(())
    }

    /// Wait for a task result
    pub async fn await_result(&self, task_id: &TaskId) -> ComputeResult<Vec<Vec<u8>>> {
        // Check if already completed
        if let Some(results) = self.completed_results.get(task_id) {
            return Ok(results.clone());
        }

        // Wait for completion
        let timeout = tokio::time::Duration::from_secs(self.config.task_timeout_secs);
        let start = std::time::Instant::now();

        while start.elapsed() < timeout {
            if let Some(results) = self.completed_results.get(task_id) {
                return Ok(results.clone());
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        Err(ComputeError::Timeout(task_id.clone()))
    }

    /// Get task progress
    pub fn get_task_progress(&self, task_id: &TaskId) -> Option<(usize, usize)> {
        if self.tasks.contains_key(task_id) {
            Some(self.piece_manager.get_task_progress(task_id))
        } else {
            None
        }
    }

    /// Get swarm statistics
    pub fn get_stats(&self) -> SwarmStats {
        let piece_stats = self.piece_manager.get_stats();
        let scheduler_stats = self.scheduler.get_stats();

        let connected_peers = self
            .peers
            .iter()
            .filter(|p| matches!(p.state, PeerState::Connected | PeerState::Busy))
            .count();

        SwarmStats {
            local_peer_id: self.config.local_peer_id.clone(),
            total_peers: self.peers.len(),
            connected_peers,
            active_tasks: self.tasks.len(),
            completed_tasks: self.completed_results.len(),
            piece_stats,
            scheduler_stats,
        }
    }

    /// Emit a swarm event
    fn emit_event(&self, event: SwarmEvent) {
        if let Some(tx) = &self.event_tx {
            let _ = tx.send(event);
        }
    }
}

/// Statistics about the swarm
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmStats {
    pub local_peer_id: PeerId,
    pub total_peers: usize,
    pub connected_peers: usize,
    pub active_tasks: usize,
    pub completed_tasks: usize,
    pub piece_stats: PieceStats,
    pub scheduler_stats: SchedulerStats,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peer::PeerCapabilities;

    async fn create_test_swarm() -> ComputeSwarm {
        ComputeSwarm::new(SwarmConfig::default()).await.unwrap()
    }

    fn create_test_peer(id: &str) -> Peer {
        let mut peer = Peer::new(id.to_string(), format!("127.0.0.1:8{}", id.len()));
        peer.state = PeerState::Connected;
        peer.capabilities = PeerCapabilities {
            max_concurrent_tasks: 5,
            ..Default::default()
        };
        peer
    }

    #[tokio::test]
    async fn test_swarm_creation() {
        let swarm = create_test_swarm().await;
        assert!(!swarm.config.local_peer_id.is_empty());
    }

    #[tokio::test]
    async fn test_peer_management() {
        let swarm = create_test_swarm().await;

        // Add peer
        let peer = create_test_peer("peer-1");
        swarm.add_peer(peer.clone()).unwrap();

        assert_eq!(swarm.peers.len(), 1);
        assert!(swarm.get_peer(&"peer-1".to_string()).is_some());

        // Remove peer
        swarm.remove_peer(&"peer-1".to_string()).unwrap();
        assert_eq!(swarm.peers.len(), 0);
    }

    #[tokio::test]
    async fn test_task_submission() {
        let swarm = create_test_swarm().await;
        swarm.start().await.unwrap();

        // Add a peer
        let mut peer = create_test_peer("peer-1");
        peer.capabilities.supported_models.insert("test-model".to_string());
        swarm.add_peer(peer).unwrap();

        // Submit task
        let task = ComputeTask::new(
            TaskType::Inference {
                model: "test-model".to_string(),
                prompt: "Hello".to_string(),
                max_tokens: 100,
            },
            1.0,
        );

        let task_id = swarm.submit_task(task).await.unwrap();
        assert!(!task_id.is_empty());
        assert_eq!(swarm.tasks.len(), 1);
    }

    #[tokio::test]
    async fn test_result_submission() {
        let swarm = create_test_swarm().await;
        swarm.start().await.unwrap();

        // Add peers
        for i in 0..3 {
            let mut peer = create_test_peer(&format!("peer-{}", i));
            peer.capabilities.supported_models.insert("test-model".to_string());
            swarm.add_peer(peer).unwrap();
        }

        // Submit task
        let task = ComputeTask::new(
            TaskType::Inference {
                model: "test-model".to_string(),
                prompt: "Hello".to_string(),
                max_tokens: 100,
            },
            1.0,
        )
        .with_pieces(1)
        .with_redundancy(3);

        let task_id = swarm.submit_task(task).await.unwrap();

        // Submit results from each peer
        for i in 0..3 {
            let result = TaskResult::new(
                task_id.clone(),
                0,
                vec![1, 2, 3, 4], // Same data = will verify
                format!("peer-{}", i),
            );
            swarm.submit_result(result).await.unwrap();
        }

        // Check task completion
        assert!(swarm.piece_manager.is_task_complete(&task_id));
    }

    #[tokio::test]
    async fn test_swarm_stats() {
        let swarm = create_test_swarm().await;

        // Add some peers
        for i in 0..5 {
            let peer = create_test_peer(&format!("peer-{}", i));
            swarm.add_peer(peer).unwrap();
        }

        let stats = swarm.get_stats();
        assert_eq!(stats.total_peers, 5);
        assert_eq!(stats.connected_peers, 5);
    }
}
