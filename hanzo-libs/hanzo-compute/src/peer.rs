//! Peer management for the compute swarm
//!
//! Peers are compute nodes that can execute tasks. Each peer has:
//! - Capabilities (GPU, CPU, memory, supported models)
//! - Reputation score based on task completion history
//! - Connection state

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Unique identifier for a peer (typically a DID or public key)
pub type PeerId = String;

/// State of a peer in the swarm
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeerState {
    /// Peer is being discovered/connected
    Connecting,
    /// Peer is connected and ready to receive tasks
    Connected,
    /// Peer is currently executing tasks
    Busy,
    /// Peer is temporarily unavailable
    Unavailable,
    /// Peer has disconnected
    Disconnected,
    /// Peer has been banned due to bad behavior
    Banned,
}

/// Capabilities of a compute peer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerCapabilities {
    /// GPU compute power in TFLOPS
    pub gpu_tflops: f32,
    /// CPU compute power in GFLOPS
    pub cpu_gflops: f32,
    /// Available RAM in GB
    pub ram_gb: f32,
    /// Available VRAM in GB
    pub vram_gb: f32,
    /// Network bandwidth in Mbps
    pub network_mbps: f32,
    /// Supported model names
    pub supported_models: HashSet<String>,
    /// Maximum concurrent tasks
    pub max_concurrent_tasks: usize,
    /// Whether this peer supports TEE (Trusted Execution Environment)
    pub supports_tee: bool,
    /// TEE attestation quote (if available)
    pub tee_attestation: Option<String>,
}

impl Default for PeerCapabilities {
    fn default() -> Self {
        Self {
            gpu_tflops: 0.0,
            cpu_gflops: 0.0,
            ram_gb: 0.0,
            vram_gb: 0.0,
            network_mbps: 0.0,
            supported_models: HashSet::new(),
            max_concurrent_tasks: 1,
            supports_tee: false,
            tee_attestation: None,
        }
    }
}

/// A peer in the compute swarm
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peer {
    /// Unique peer identifier
    pub id: PeerId,
    /// Peer's address (IP:port or multiaddr)
    pub address: String,
    /// Current state
    pub state: PeerState,
    /// Compute capabilities
    pub capabilities: PeerCapabilities,
    /// Reputation score (0.0 - 100.0)
    pub reputation: f64,
    /// Number of tasks completed successfully
    pub tasks_completed: u64,
    /// Number of tasks failed
    pub tasks_failed: u64,
    /// Number of tasks currently in progress
    pub tasks_in_progress: usize,
    /// Average task completion time in milliseconds
    pub avg_completion_time_ms: f64,
    /// Last seen timestamp (Unix)
    pub last_seen: u64,
    /// First seen timestamp (Unix)
    pub first_seen: u64,
    /// Total AI coins earned by this peer
    pub total_earned: f64,
}

impl Peer {
    /// Create a new peer
    pub fn new(id: PeerId, address: String) -> Self {
        let now = chrono::Utc::now().timestamp() as u64;

        Self {
            id,
            address,
            state: PeerState::Connecting,
            capabilities: PeerCapabilities::default(),
            reputation: 50.0, // Start with neutral reputation
            tasks_completed: 0,
            tasks_failed: 0,
            tasks_in_progress: 0,
            avg_completion_time_ms: 0.0,
            last_seen: now,
            first_seen: now,
            total_earned: 0.0,
        }
    }

    /// Create a peer with known capabilities
    pub fn with_capabilities(mut self, capabilities: PeerCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Check if peer can accept more tasks
    pub fn can_accept_task(&self) -> bool {
        matches!(self.state, PeerState::Connected | PeerState::Busy)
            && self.tasks_in_progress < self.capabilities.max_concurrent_tasks
    }

    /// Check if peer meets minimum reputation requirement
    pub fn meets_reputation(&self, min_reputation: f64) -> bool {
        self.reputation >= min_reputation
    }

    /// Check if peer supports a specific model
    pub fn supports_model(&self, model: &str) -> bool {
        self.capabilities.supported_models.contains(model)
    }

    /// Update reputation after task completion
    pub fn record_task_success(&mut self, completion_time_ms: u64, reward: f64) {
        self.tasks_completed += 1;
        self.tasks_in_progress = self.tasks_in_progress.saturating_sub(1);
        self.total_earned += reward;

        // Update average completion time
        let total_tasks = self.tasks_completed + self.tasks_failed;
        self.avg_completion_time_ms = (self.avg_completion_time_ms * (total_tasks as f64 - 1.0)
            + completion_time_ms as f64)
            / total_tasks as f64;

        // Increase reputation (asymptotic approach to 100)
        let delta = (100.0 - self.reputation) * 0.01;
        self.reputation = (self.reputation + delta).min(100.0);

        self.last_seen = chrono::Utc::now().timestamp() as u64;
    }

    /// Update reputation after task failure
    pub fn record_task_failure(&mut self) {
        self.tasks_failed += 1;
        self.tasks_in_progress = self.tasks_in_progress.saturating_sub(1);

        // Decrease reputation more aggressively for failures
        self.reputation = (self.reputation - 5.0).max(0.0);

        self.last_seen = chrono::Utc::now().timestamp() as u64;
    }

    /// Start a task on this peer
    pub fn start_task(&mut self) {
        self.tasks_in_progress += 1;
        if self.tasks_in_progress >= self.capabilities.max_concurrent_tasks {
            self.state = PeerState::Busy;
        }
    }

    /// Calculate peer score for scheduling (higher is better)
    pub fn scheduling_score(&self) -> f64 {
        // Combine multiple factors:
        // - Reputation (most important)
        // - Available capacity
        // - Performance (inverse of avg completion time)
        // - Reliability (success rate)

        let capacity_factor = if self.capabilities.max_concurrent_tasks > 0 {
            1.0 - (self.tasks_in_progress as f64 / self.capabilities.max_concurrent_tasks as f64)
        } else {
            0.0
        };

        let total_tasks = self.tasks_completed + self.tasks_failed;
        let reliability = if total_tasks > 0 {
            self.tasks_completed as f64 / total_tasks as f64
        } else {
            0.5 // Neutral for new peers
        };

        let performance = if self.avg_completion_time_ms > 0.0 {
            1000.0 / self.avg_completion_time_ms // Higher score for faster peers
        } else {
            0.5
        };

        // Weighted combination
        (self.reputation * 0.5) + (capacity_factor * 20.0) + (reliability * 20.0) + (performance * 10.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_peer_creation() {
        let peer = Peer::new("peer-123".to_string(), "127.0.0.1:8080".to_string());

        assert_eq!(peer.id, "peer-123");
        assert_eq!(peer.state, PeerState::Connecting);
        assert_eq!(peer.reputation, 50.0);
        assert_eq!(peer.tasks_completed, 0);
    }

    #[test]
    fn test_peer_task_success() {
        let mut peer = Peer::new("peer-123".to_string(), "127.0.0.1:8080".to_string());
        peer.state = PeerState::Connected;

        peer.start_task();
        assert_eq!(peer.tasks_in_progress, 1);

        peer.record_task_success(1000, 1.0);
        assert_eq!(peer.tasks_completed, 1);
        assert_eq!(peer.tasks_in_progress, 0);
        assert_eq!(peer.total_earned, 1.0);
        assert!(peer.reputation > 50.0);
    }

    #[test]
    fn test_peer_task_failure() {
        let mut peer = Peer::new("peer-123".to_string(), "127.0.0.1:8080".to_string());
        peer.state = PeerState::Connected;

        peer.start_task();
        peer.record_task_failure();

        assert_eq!(peer.tasks_failed, 1);
        assert_eq!(peer.tasks_in_progress, 0);
        assert!(peer.reputation < 50.0);
    }

    #[test]
    fn test_can_accept_task() {
        let mut peer = Peer::new("peer-123".to_string(), "127.0.0.1:8080".to_string());
        peer.capabilities.max_concurrent_tasks = 2;

        // Connecting peers can't accept tasks
        assert!(!peer.can_accept_task());

        peer.state = PeerState::Connected;
        assert!(peer.can_accept_task());

        peer.start_task();
        assert!(peer.can_accept_task()); // Still has capacity

        peer.start_task();
        assert!(!peer.can_accept_task()); // At capacity
    }

    #[test]
    fn test_scheduling_score() {
        let mut peer = Peer::new("peer-123".to_string(), "127.0.0.1:8080".to_string());
        peer.state = PeerState::Connected;
        peer.reputation = 80.0;
        peer.capabilities.max_concurrent_tasks = 4;

        let score1 = peer.scheduling_score();

        // After successful tasks, score should increase
        peer.record_task_success(500, 1.0);
        peer.record_task_success(500, 1.0);

        let score2 = peer.scheduling_score();
        assert!(score2 > score1);
    }
}
