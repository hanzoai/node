//! Result verification for distributed compute tasks
//!
//! Verification ensures that compute results are correct through:
//! - Hash verification: Compare result hashes across redundant computations
//! - Consensus: Require multiple peers to agree on the same result
//! - TEE attestation: Verify results from Trusted Execution Environments

use crate::peer::PeerId;
use crate::piece::Piece;
use crate::ComputeResult as TaskResult;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Method used for verifying compute results
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerificationMethod {
    /// Simple hash comparison (all results must match)
    HashMatch,
    /// Majority consensus (>50% of results must match)
    MajorityConsensus,
    /// Supermajority consensus (>66% of results must match)
    SupermajorityConsensus,
    /// Byzantine fault tolerant (>2/3 honest nodes)
    ByzantineFaultTolerant,
    /// TEE attestation required
    TeeAttestation,
    /// No verification (trust single result)
    None,
}

impl Default for VerificationMethod {
    fn default() -> Self {
        Self::MajorityConsensus
    }
}

/// Result of verification process
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    /// Whether verification succeeded
    pub success: bool,
    /// Verification method used
    pub method: VerificationMethod,
    /// The verified result hash (if successful)
    pub verified_hash: Option<String>,
    /// Number of matching results
    pub matching_count: usize,
    /// Total number of results checked
    pub total_count: usize,
    /// Peers that submitted matching results
    pub matching_peers: Vec<PeerId>,
    /// Peers that submitted non-matching results (potentially malicious)
    pub non_matching_peers: Vec<PeerId>,
    /// Confidence score (0.0 - 1.0)
    pub confidence: f64,
}

impl VerificationResult {
    /// Create a successful verification result
    pub fn success(
        method: VerificationMethod,
        verified_hash: String,
        matching_count: usize,
        total_count: usize,
        matching_peers: Vec<PeerId>,
        non_matching_peers: Vec<PeerId>,
    ) -> Self {
        let confidence = if total_count > 0 {
            matching_count as f64 / total_count as f64
        } else {
            0.0
        };

        Self {
            success: true,
            method,
            verified_hash: Some(verified_hash),
            matching_count,
            total_count,
            matching_peers,
            non_matching_peers,
            confidence,
        }
    }

    /// Create a failed verification result
    pub fn failure(
        method: VerificationMethod,
        matching_count: usize,
        total_count: usize,
        non_matching_peers: Vec<PeerId>,
    ) -> Self {
        Self {
            success: false,
            method,
            verified_hash: None,
            matching_count,
            total_count,
            matching_peers: Vec::new(),
            non_matching_peers,
            confidence: 0.0,
        }
    }
}

/// Verifier for compute task results
pub struct ResultVerifier {
    /// Default verification method
    default_method: VerificationMethod,
    /// Minimum confidence threshold
    min_confidence: f64,
    /// TEE attestation verifier (placeholder for actual implementation)
    tee_verifier: Option<Box<dyn TeeVerifier>>,
}

impl ResultVerifier {
    /// Create a new verifier with default settings
    pub fn new() -> Self {
        Self {
            default_method: VerificationMethod::default(),
            min_confidence: 0.5,
            tee_verifier: None,
        }
    }

    /// Create a verifier with a specific method
    pub fn with_method(method: VerificationMethod) -> Self {
        Self {
            default_method: method,
            min_confidence: 0.5,
            tee_verifier: None,
        }
    }

    /// Set the minimum confidence threshold
    pub fn set_min_confidence(&mut self, confidence: f64) {
        self.min_confidence = confidence.clamp(0.0, 1.0);
    }

    /// Set the TEE verifier
    pub fn set_tee_verifier(&mut self, verifier: Box<dyn TeeVerifier>) {
        self.tee_verifier = Some(verifier);
    }

    /// Verify results for a piece
    pub fn verify_piece(&self, piece: &Piece, results: &[TaskResult]) -> VerificationResult {
        if results.is_empty() {
            return VerificationResult::failure(self.default_method, 0, 0, Vec::new());
        }

        match self.default_method {
            VerificationMethod::HashMatch => self.verify_hash_match(results),
            VerificationMethod::MajorityConsensus => self.verify_consensus(results, 0.5),
            VerificationMethod::SupermajorityConsensus => self.verify_consensus(results, 0.67),
            VerificationMethod::ByzantineFaultTolerant => self.verify_bft(results),
            VerificationMethod::TeeAttestation => self.verify_tee(piece, results),
            VerificationMethod::None => self.verify_none(results),
        }
    }

    /// Verify using exact hash match
    fn verify_hash_match(&self, results: &[TaskResult]) -> VerificationResult {
        if results.is_empty() {
            return VerificationResult::failure(VerificationMethod::HashMatch, 0, 0, Vec::new());
        }

        // Group results by hash
        let hash_groups = self.group_by_hash(results);

        // All results must have the same hash
        if hash_groups.len() == 1 {
            let (hash, peers) = hash_groups.into_iter().next().unwrap();
            VerificationResult::success(
                VerificationMethod::HashMatch,
                hash,
                peers.len(),
                results.len(),
                peers,
                Vec::new(),
            )
        } else {
            // Find non-matching peers (all peers since no consensus)
            let non_matching: Vec<_> = results.iter().map(|r| r.computed_by.clone()).collect();
            VerificationResult::failure(
                VerificationMethod::HashMatch,
                0,
                results.len(),
                non_matching,
            )
        }
    }

    /// Verify using consensus threshold
    fn verify_consensus(&self, results: &[TaskResult], threshold: f64) -> VerificationResult {
        if results.is_empty() {
            let method = if threshold > 0.6 {
                VerificationMethod::SupermajorityConsensus
            } else {
                VerificationMethod::MajorityConsensus
            };
            return VerificationResult::failure(method, 0, 0, Vec::new());
        }

        let hash_groups = self.group_by_hash(results);
        let total = results.len();
        let required = (total as f64 * threshold).ceil() as usize;

        // Find the most common hash
        let (best_hash, matching_peers) = hash_groups
            .into_iter()
            .max_by_key(|(_, peers)| peers.len())
            .unwrap();

        let method = if threshold > 0.6 {
            VerificationMethod::SupermajorityConsensus
        } else {
            VerificationMethod::MajorityConsensus
        };

        if matching_peers.len() >= required {
            // Find non-matching peers
            let non_matching: Vec<_> = results
                .iter()
                .filter(|r| !matching_peers.contains(&r.computed_by))
                .map(|r| r.computed_by.clone())
                .collect();

            VerificationResult::success(
                method,
                best_hash,
                matching_peers.len(),
                total,
                matching_peers,
                non_matching,
            )
        } else {
            let non_matching: Vec<_> = results.iter().map(|r| r.computed_by.clone()).collect();
            VerificationResult::failure(method, matching_peers.len(), total, non_matching)
        }
    }

    /// Verify using Byzantine fault tolerance (requires > 2/3 agreement)
    fn verify_bft(&self, results: &[TaskResult]) -> VerificationResult {
        if results.len() < 4 {
            // Need at least 4 nodes for BFT (3f+1 where f=1)
            return VerificationResult::failure(
                VerificationMethod::ByzantineFaultTolerant,
                0,
                results.len(),
                Vec::new(),
            );
        }

        // BFT requires > 2/3 honest nodes
        self.verify_consensus(results, 0.67)
    }

    /// Verify using TEE attestation
    fn verify_tee(&self, _piece: &Piece, results: &[TaskResult]) -> VerificationResult {
        // If no TEE verifier, fall back to consensus
        let verifier = match &self.tee_verifier {
            Some(v) => v,
            None => return self.verify_consensus(results, 0.5),
        };

        let mut verified_results = Vec::new();
        let mut non_verified = Vec::new();

        for result in results {
            if verifier.verify_attestation(&result.computed_by, &result.data) {
                verified_results.push(result.clone());
            } else {
                non_verified.push(result.computed_by.clone());
            }
        }

        if verified_results.is_empty() {
            return VerificationResult::failure(
                VerificationMethod::TeeAttestation,
                0,
                results.len(),
                non_verified,
            );
        }

        // Among TEE-verified results, check for consensus
        let hash_groups = self.group_by_hash(&verified_results);

        if let Some((hash, peers)) = hash_groups.into_iter().max_by_key(|(_, peers)| peers.len()) {
            VerificationResult::success(
                VerificationMethod::TeeAttestation,
                hash,
                peers.len(),
                results.len(),
                peers,
                non_verified,
            )
        } else {
            VerificationResult::failure(
                VerificationMethod::TeeAttestation,
                0,
                results.len(),
                non_verified,
            )
        }
    }

    /// No verification - trust first result
    fn verify_none(&self, results: &[TaskResult]) -> VerificationResult {
        if let Some(result) = results.first() {
            VerificationResult::success(
                VerificationMethod::None,
                result.result_hash.clone(),
                1,
                results.len(),
                vec![result.computed_by.clone()],
                Vec::new(),
            )
        } else {
            VerificationResult::failure(VerificationMethod::None, 0, 0, Vec::new())
        }
    }

    /// Group results by their hash
    fn group_by_hash(&self, results: &[TaskResult]) -> HashMap<String, Vec<PeerId>> {
        let mut groups: HashMap<String, Vec<PeerId>> = HashMap::new();

        for result in results {
            groups
                .entry(result.result_hash.clone())
                .or_default()
                .push(result.computed_by.clone());
        }

        groups
    }

    /// Calculate reputation adjustments based on verification
    pub fn calculate_reputation_adjustments(
        &self,
        verification: &VerificationResult,
    ) -> HashMap<PeerId, f64> {
        let mut adjustments = HashMap::new();

        // Reward matching peers
        for peer in &verification.matching_peers {
            let reward = verification.confidence * 5.0; // Up to +5 reputation
            adjustments.insert(peer.clone(), reward);
        }

        // Penalize non-matching peers
        for peer in &verification.non_matching_peers {
            let penalty = (1.0 - verification.confidence) * -10.0; // Up to -10 reputation
            adjustments.insert(peer.clone(), penalty);
        }

        adjustments
    }
}

impl Default for ResultVerifier {
    fn default() -> Self {
        Self::new()
    }
}

/// Trait for TEE attestation verification
pub trait TeeVerifier: Send + Sync {
    /// Verify TEE attestation for a result
    fn verify_attestation(&self, peer_id: &PeerId, result_data: &[u8]) -> bool;

    /// Get supported TEE types
    fn supported_tee_types(&self) -> Vec<String>;
}

/// Mock TEE verifier for testing
pub struct MockTeeVerifier {
    /// Set of peer IDs that are considered to have valid attestations
    valid_peers: std::collections::HashSet<PeerId>,
}

impl MockTeeVerifier {
    pub fn new() -> Self {
        Self {
            valid_peers: std::collections::HashSet::new(),
        }
    }

    pub fn add_valid_peer(&mut self, peer_id: PeerId) {
        self.valid_peers.insert(peer_id);
    }
}

impl TeeVerifier for MockTeeVerifier {
    fn verify_attestation(&self, peer_id: &PeerId, _result_data: &[u8]) -> bool {
        self.valid_peers.contains(peer_id)
    }

    fn supported_tee_types(&self) -> Vec<String> {
        vec!["mock".to_string()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_result(task_id: &str, piece_index: usize, data: &[u8], peer_id: &str) -> TaskResult {
        TaskResult::new(
            task_id.to_string(),
            piece_index,
            data.to_vec(),
            peer_id.to_string(),
        )
    }

    #[test]
    fn test_hash_match_success() {
        let verifier = ResultVerifier::with_method(VerificationMethod::HashMatch);

        let results = vec![
            create_result("task-1", 0, b"hello", "peer-1"),
            create_result("task-1", 0, b"hello", "peer-2"),
            create_result("task-1", 0, b"hello", "peer-3"),
        ];

        let verification = verifier.verify_piece(
            &Piece::new("task-1".to_string(), 0, b"input", 3),
            &results,
        );

        assert!(verification.success);
        assert_eq!(verification.matching_count, 3);
        assert_eq!(verification.confidence, 1.0);
    }

    #[test]
    fn test_hash_match_failure() {
        let verifier = ResultVerifier::with_method(VerificationMethod::HashMatch);

        let results = vec![
            create_result("task-1", 0, b"hello", "peer-1"),
            create_result("task-1", 0, b"world", "peer-2"),
        ];

        let verification = verifier.verify_piece(
            &Piece::new("task-1".to_string(), 0, b"input", 2),
            &results,
        );

        assert!(!verification.success);
    }

    #[test]
    fn test_majority_consensus() {
        let verifier = ResultVerifier::with_method(VerificationMethod::MajorityConsensus);

        let results = vec![
            create_result("task-1", 0, b"hello", "peer-1"),
            create_result("task-1", 0, b"hello", "peer-2"),
            create_result("task-1", 0, b"world", "peer-3"),
        ];

        let verification = verifier.verify_piece(
            &Piece::new("task-1".to_string(), 0, b"input", 3),
            &results,
        );

        assert!(verification.success);
        assert_eq!(verification.matching_count, 2);
        assert_eq!(verification.non_matching_peers.len(), 1);
    }

    #[test]
    fn test_supermajority_failure() {
        let verifier = ResultVerifier::with_method(VerificationMethod::SupermajorityConsensus);

        // 2 out of 3 is not > 67%
        let results = vec![
            create_result("task-1", 0, b"hello", "peer-1"),
            create_result("task-1", 0, b"hello", "peer-2"),
            create_result("task-1", 0, b"world", "peer-3"),
        ];

        let verification = verifier.verify_piece(
            &Piece::new("task-1".to_string(), 0, b"input", 3),
            &results,
        );

        assert!(!verification.success);
    }

    #[test]
    fn test_reputation_adjustments() {
        let verifier = ResultVerifier::new();

        let verification = VerificationResult::success(
            VerificationMethod::MajorityConsensus,
            "hash123".to_string(),
            2,
            3,
            vec!["peer-1".to_string(), "peer-2".to_string()],
            vec!["peer-3".to_string()],
        );

        let adjustments = verifier.calculate_reputation_adjustments(&verification);

        assert!(adjustments.get("peer-1").unwrap() > &0.0);
        assert!(adjustments.get("peer-2").unwrap() > &0.0);
        assert!(adjustments.get("peer-3").unwrap() < &0.0);
    }

    #[test]
    fn test_no_verification() {
        let verifier = ResultVerifier::with_method(VerificationMethod::None);

        let results = vec![
            create_result("task-1", 0, b"hello", "peer-1"),
            create_result("task-1", 0, b"world", "peer-2"),
        ];

        let verification = verifier.verify_piece(
            &Piece::new("task-1".to_string(), 0, b"input", 1),
            &results,
        );

        // Should succeed with just the first result
        assert!(verification.success);
        assert_eq!(verification.matching_peers, vec!["peer-1"]);
    }

    #[test]
    fn test_tee_verification() {
        let mut verifier = ResultVerifier::with_method(VerificationMethod::TeeAttestation);

        let mut mock_tee = MockTeeVerifier::new();
        mock_tee.add_valid_peer("peer-1".to_string());
        mock_tee.add_valid_peer("peer-2".to_string());
        verifier.set_tee_verifier(Box::new(mock_tee));

        let results = vec![
            create_result("task-1", 0, b"hello", "peer-1"),
            create_result("task-1", 0, b"hello", "peer-2"),
            create_result("task-1", 0, b"hello", "peer-3"), // No TEE attestation
        ];

        let verification = verifier.verify_piece(
            &Piece::new("task-1".to_string(), 0, b"input", 3),
            &results,
        );

        assert!(verification.success);
        assert_eq!(verification.matching_count, 2);
        assert_eq!(verification.non_matching_peers, vec!["peer-3"]);
    }
}
