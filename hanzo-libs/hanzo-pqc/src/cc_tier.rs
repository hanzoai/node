//! Confidential Compute Tier Classification per LP-5610
//!
//! This module implements the 3-tier CC classification system for AI workloads
//! as specified in LP-5610: AI Confidential Compute Tier Specification.
//!
//! Tier 1 — "GPU-native CC": NVIDIA Blackwell, Hopper, RTX PRO 6000 with NVTrust
//! Tier 2 — "Confidential VM + GPU": AMD SEV-SNP, Intel TDX, Arm CCA + GPU
//! Tier 3 — "Device TEE + AI engine": Qualcomm TrustZone/SPU, Apple Secure Enclave
//! Tier 4 — "Standard" (non-CC): Consumer GPUs, stake-based soft attestation
//!
//! All attestation is LOCAL - no cloud dependencies (blockchain requirement).
//! See: <https://github.com/luxfi/lps/blob/main/LPs/lp-5610-ai-confidential-compute-tiers.md>

use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::privacy_tiers::{PrivacyTier, VendorCapabilities};

/// CC Tier classification per LP-5610
/// Lower number = higher security (opposite of PrivacyTier for compatibility)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum CCTier {
    /// Tier 1: GPU-native Confidential Compute
    /// NVIDIA Blackwell, Hopper, RTX PRO 6000 with NVTrust
    /// Trust Score: 90-100
    Tier1GpuNativeCC = 1,

    /// Tier 2: Confidential VM + GPU
    /// AMD EPYC + SEV-SNP, Intel Xeon + TDX, Arm Neoverse + CCA
    /// Trust Score: 70-89
    Tier2ConfidentialVM = 2,

    /// Tier 3: Device TEE + AI Engine
    /// Qualcomm Snapdragon + TrustZone/SPU, Apple Silicon + Secure Enclave
    /// Trust Score: 50-69
    Tier3DeviceTEE = 3,

    /// Tier 4: Standard (no hardware CC)
    /// Consumer GPUs (RTX 4090/5090), Cloud VMs without CC
    /// Trust Score: 10-49
    Tier4Standard = 4,

    /// Unknown/Invalid tier
    Unknown = 0,
}

impl CCTier {
    /// Get the human-readable name
    pub fn name(&self) -> &'static str {
        match self {
            Self::Tier1GpuNativeCC => "GPU-Native-CC",
            Self::Tier2ConfidentialVM => "Confidential-VM",
            Self::Tier3DeviceTEE => "Device-TEE",
            Self::Tier4Standard => "Standard",
            Self::Unknown => "Unknown",
        }
    }

    /// Get detailed description
    pub fn description(&self) -> &'static str {
        match self {
            Self::Tier1GpuNativeCC => "Full GPU-level hardware confidential compute with NVTrust attestation",
            Self::Tier2ConfidentialVM => "CPU-level VM isolation (SEV-SNP/TDX/CCA) with GPU passthrough",
            Self::Tier3DeviceTEE => "Edge device TEE with integrated AI accelerator",
            Self::Tier4Standard => "Software/stake-based attestation without hardware CC",
            Self::Unknown => "Unknown or invalid tier classification",
        }
    }

    /// Get the base trust score for this tier
    pub fn base_trust_score(&self) -> u8 {
        match self {
            Self::Tier1GpuNativeCC => 90,
            Self::Tier2ConfidentialVM => 70,
            Self::Tier3DeviceTEE => 50,
            Self::Tier4Standard => 10,
            Self::Unknown => 0,
        }
    }

    /// Get the maximum trust score achievable for this tier
    pub fn max_trust_score(&self) -> u8 {
        match self {
            Self::Tier1GpuNativeCC => 100,
            Self::Tier2ConfidentialVM => 89,
            Self::Tier3DeviceTEE => 69,
            Self::Tier4Standard => 49,
            Self::Unknown => 0,
        }
    }

    /// Get the reward multiplier for this tier
    /// Tier 1: 1.5x, Tier 2: 1.0x, Tier 3: 0.75x, Tier 4: 0.5x
    pub fn reward_multiplier(&self) -> f64 {
        match self {
            Self::Tier1GpuNativeCC => 1.5,
            Self::Tier2ConfidentialVM => 1.0,
            Self::Tier3DeviceTEE => 0.75,
            Self::Tier4Standard => 0.5,
            Self::Unknown => 0.0,
        }
    }

    /// Get the minimum stake required for this tier (in LUX tokens)
    pub fn min_stake_lux(&self) -> u64 {
        match self {
            Self::Tier1GpuNativeCC => 100_000,
            Self::Tier2ConfidentialVM => 50_000,
            Self::Tier3DeviceTEE => 10_000,
            Self::Tier4Standard => 1_000,
            Self::Unknown => 0,
        }
    }

    /// Get the attestation validity period for this tier
    pub fn attestation_validity(&self) -> Duration {
        match self {
            Self::Tier1GpuNativeCC => Duration::from_secs(6 * 3600), // 6 hours
            Self::Tier2ConfidentialVM => Duration::from_secs(24 * 3600), // 24 hours
            Self::Tier3DeviceTEE => Duration::from_secs(7 * 24 * 3600), // 7 days
            Self::Tier4Standard => Duration::from_secs(30 * 24 * 3600), // 30 days
            Self::Unknown => Duration::ZERO,
        }
    }

    /// Check if this tier meets or exceeds the required tier
    /// Lower number = higher capability, so Tier 1 meets all requirements
    pub fn meets_requirement(&self, required: CCTier) -> bool {
        if *self == Self::Unknown {
            return false;
        }
        *self as u8 <= required as u8
    }

    /// Convert from PrivacyTier (existing hanzo-pqc system)
    pub fn from_privacy_tier(tier: PrivacyTier) -> Self {
        match tier {
            PrivacyTier::AccessGpuTeeIoMax => Self::Tier1GpuNativeCC,
            PrivacyTier::AccessCpuTeePlusGpuCc => Self::Tier1GpuNativeCC, // H100 CC = Tier 1
            PrivacyTier::AccessCpuTee => Self::Tier2ConfidentialVM,
            PrivacyTier::AccessAtRest => Self::Tier3DeviceTEE, // SIM/FileVault ~ Device TEE
            PrivacyTier::AccessOpen => Self::Tier4Standard,
        }
    }

    /// Convert to PrivacyTier (existing hanzo-pqc system)
    pub fn to_privacy_tier(&self) -> PrivacyTier {
        match self {
            Self::Tier1GpuNativeCC => PrivacyTier::AccessGpuTeeIoMax, // Map to highest
            Self::Tier2ConfidentialVM => PrivacyTier::AccessCpuTee,
            Self::Tier3DeviceTEE => PrivacyTier::AccessAtRest,
            Self::Tier4Standard | Self::Unknown => PrivacyTier::AccessOpen,
        }
    }

    /// Determine CC tier from VendorCapabilities
    pub fn from_capabilities(caps: &VendorCapabilities) -> Self {
        // Tier 1: GPU-native CC (Blackwell TEE-IO or H100 CC with NRAS)
        if caps.gpu_blackwell_tee_io && caps.nvidia_nras_ok {
            return Self::Tier1GpuNativeCC;
        }
        if caps.gpu_h100_cc && caps.nvidia_nras_ok {
            return Self::Tier1GpuNativeCC;
        }

        // Tier 2: Confidential VM (CPU TEE + GPU)
        if caps.cpu_sev_snp || caps.cpu_tdx || caps.cpu_arm_cca {
            return Self::Tier2ConfidentialVM;
        }

        // Tier 3: Device TEE (SIM, SGX without full CC)
        if caps.sim_available || caps.hsm_available || caps.cpu_sgx {
            return Self::Tier3DeviceTEE;
        }

        // Tier 4: Standard (no CC)
        Self::Tier4Standard
    }

    /// Parse tier from u8
    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Tier1GpuNativeCC,
            2 => Self::Tier2ConfidentialVM,
            3 => Self::Tier3DeviceTEE,
            4 => Self::Tier4Standard,
            _ => Self::Unknown,
        }
    }
}

impl Default for CCTier {
    fn default() -> Self {
        Self::Unknown
    }
}

impl std::fmt::Display for CCTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Trust score calculation weights per LP-5610 Section 5
#[derive(Debug, Clone, Copy)]
pub struct TrustScoreWeights {
    /// Hardware tier and features (40% default)
    pub hardware: f64,
    /// Attestation freshness and verification (30% default)
    pub attestation: f64,
    /// Historical performance (20% default)
    pub reputation: f64,
    /// Availability (10% default)
    pub uptime: f64,
}

impl Default for TrustScoreWeights {
    fn default() -> Self {
        Self {
            hardware: 0.40,
            attestation: 0.30,
            reputation: 0.20,
            uptime: 0.10,
        }
    }
}

/// Input for trust score calculation
#[derive(Debug, Clone, Default)]
pub struct TrustScoreInput {
    // Hardware inputs
    pub tier: CCTier,
    pub gpu_generation: u8,      // 1-10, higher = newer
    pub cc_features_enabled: bool,
    pub tee_io_enabled: bool,
    pub rim_verified: bool,

    // Attestation inputs
    pub attestation_age_secs: u64,
    pub attestation_method: AttestationMethod,
    pub local_verification: bool,
    pub cert_chain_valid: bool,

    // Reputation inputs
    pub tasks_completed: u64,
    pub tasks_failed: u64,
    pub slashing_events: u64,
    pub reputation_score: f64,    // 0.0-1.0

    // Uptime inputs
    pub uptime_percentage: f64,   // 0.0-100.0
    pub last_seen_secs: u64,      // Seconds since last heartbeat
    pub consecutive_heartbeats: u64,
}

/// Attestation method types
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttestationMethod {
    /// NVIDIA NVTrust local verification (best for Tier 1)
    NvTrust,
    /// AMD SEV-SNP attestation
    SevSnp,
    /// Intel TDX attestation
    Tdx,
    /// ARM CCA attestation
    Cca,
    /// Apple Secure Enclave
    SecureEnclave,
    /// Software-only attestation
    #[default]
    Software,
}

/// Result of trust score calculation
#[derive(Debug, Clone)]
pub struct TrustScoreResult {
    pub total_score: u8,
    pub hardware_score: u8,
    pub attestation_score: u8,
    pub reputation_score: u8,
    pub uptime_score: u8,
    pub tier: CCTier,
    pub meets_minimum: bool,
    pub warnings: Vec<String>,
}

impl TrustScoreInput {
    /// Calculate trust score with default weights
    pub fn calculate(&self) -> TrustScoreResult {
        self.calculate_with_weights(TrustScoreWeights::default())
    }

    /// Calculate trust score with custom weights
    pub fn calculate_with_weights(&self, weights: TrustScoreWeights) -> TrustScoreResult {
        let hardware_score = self.calculate_hardware_score();
        let attestation_score = self.calculate_attestation_score();
        let reputation_score = self.calculate_reputation_score();
        let uptime_score = self.calculate_uptime_score();

        let total = (hardware_score as f64 * weights.hardware)
            + (attestation_score as f64 * weights.attestation)
            + (reputation_score as f64 * weights.reputation)
            + (uptime_score as f64 * weights.uptime);

        // Clamp to tier limits
        let min_score = self.tier.base_trust_score();
        let max_score = self.tier.max_trust_score();
        let mut warnings = Vec::new();

        let total_score = if total < min_score as f64 {
            warnings.push("Score clamped to tier minimum".to_string());
            min_score
        } else if total > max_score as f64 {
            max_score
        } else {
            total as u8
        };

        TrustScoreResult {
            total_score,
            hardware_score,
            attestation_score,
            reputation_score,
            uptime_score,
            tier: self.tier,
            meets_minimum: total_score >= min_score,
            warnings,
        }
    }

    fn calculate_hardware_score(&self) -> u8 {
        let mut score: f64 = match self.tier {
            CCTier::Tier1GpuNativeCC => 35.0 + (self.gpu_generation as f64 * 0.5).min(5.0),
            CCTier::Tier2ConfidentialVM => 25.0 + (self.gpu_generation as f64 * 0.5).min(5.0),
            CCTier::Tier3DeviceTEE => 15.0 + (self.gpu_generation as f64 * 0.5).min(5.0),
            CCTier::Tier4Standard => 5.0,
            CCTier::Unknown => 0.0,
        };

        // CC feature bonuses
        if self.cc_features_enabled {
            score += 3.0;
        }
        if self.tee_io_enabled {
            score += 2.0;
        }
        if self.rim_verified {
            score += 2.0;
        }

        score.min(100.0) as u8
    }

    fn calculate_attestation_score(&self) -> u8 {
        let mut score: f64 = 70.0; // Base for valid attestation

        // Freshness bonus
        let max_age = self.tier.attestation_validity().as_secs();
        if max_age > 0 {
            let age_ratio = self.attestation_age_secs as f64 / max_age as f64;
            if age_ratio < 0.25 {
                score += 15.0;
            } else if age_ratio < 0.50 {
                score += 10.0;
            } else if age_ratio < 0.75 {
                score += 5.0;
            }
        }

        // Method bonus
        score += match self.attestation_method {
            AttestationMethod::NvTrust => 10.0,
            AttestationMethod::SevSnp | AttestationMethod::Tdx => 8.0,
            AttestationMethod::Cca => 6.0,
            AttestationMethod::SecureEnclave => 5.0,
            AttestationMethod::Software => 2.0,
        };

        // Local verification bonus (blockchain requirement)
        if self.local_verification {
            score += 5.0;
        }

        // Certificate chain validation
        if self.cert_chain_valid {
            score += 3.0;
        }

        score.min(100.0) as u8
    }

    fn calculate_reputation_score(&self) -> u8 {
        let mut score: f64 = 50.0;

        // Task completion rate
        if self.tasks_completed > 0 {
            let total_tasks = self.tasks_completed + self.tasks_failed;
            let success_rate = self.tasks_completed as f64 / total_tasks as f64;
            score += success_rate * 30.0;

            // Volume bonus
            if total_tasks > 1000 {
                score += 5.0;
            } else if total_tasks > 100 {
                score += 3.0;
            } else if total_tasks > 10 {
                score += 1.0;
            }
        }

        // Slashing penalty
        if self.slashing_events > 0 {
            let penalty = (self.slashing_events as f64 * 10.0).min(30.0);
            score -= penalty;
        }

        // Historical reputation contribution
        if self.reputation_score > 0.0 {
            score += self.reputation_score * 15.0;
        }

        score.max(0.0).min(100.0) as u8
    }

    fn calculate_uptime_score(&self) -> u8 {
        let mut score: f64 = 0.0;

        // Uptime percentage (0-70 points)
        score += self.uptime_percentage * 0.7;

        // Heartbeat freshness (0-15 points)
        if self.last_seen_secs < 60 {
            score += 15.0;
        } else if self.last_seen_secs < 300 {
            score += 12.0;
        } else if self.last_seen_secs < 900 {
            score += 8.0;
        } else if self.last_seen_secs < 3600 {
            score += 4.0;
        }

        // Consecutive heartbeats bonus (0-15 points)
        if self.consecutive_heartbeats > 1000 {
            score += 15.0;
        } else if self.consecutive_heartbeats > 100 {
            score += 10.0;
        } else if self.consecutive_heartbeats > 10 {
            score += 5.0;
        }

        score.min(100.0) as u8
    }
}

/// Quick trust score calculation with minimal inputs
pub fn quick_trust_score(tier: CCTier, caps: &VendorCapabilities) -> u8 {
    let gpu_gen = if caps.gpu_blackwell_tee_io {
        10
    } else if caps.gpu_h100_cc {
        9
    } else {
        5
    };

    let input = TrustScoreInput {
        tier,
        gpu_generation: gpu_gen,
        cc_features_enabled: caps.gpu_h100_cc || caps.gpu_blackwell_tee_io,
        tee_io_enabled: caps.gpu_blackwell_tee_io,
        attestation_method: if caps.nvidia_nras_ok {
            AttestationMethod::NvTrust
        } else if caps.cpu_sev_snp {
            AttestationMethod::SevSnp
        } else if caps.cpu_tdx {
            AttestationMethod::Tdx
        } else {
            AttestationMethod::Software
        },
        local_verification: true,
        uptime_percentage: 100.0,
        reputation_score: 0.5,
        ..Default::default()
    };

    input.calculate().total_score
}

/// Tier attestation with on-chain binding
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierAttestation {
    pub tier: CCTier,
    pub provider_id: String,
    pub hardware_id: String,
    pub evidence_hash: [u8; 32],
    pub trust_score: u8,
    pub issued_at: u64,
    pub expires_at: u64,
    pub chain_id: u64,
    pub block_height: u64,
}

impl TierAttestation {
    /// Create a new attestation
    pub fn new(
        tier: CCTier,
        provider_id: String,
        hardware_id: String,
        evidence_hash: [u8; 32],
        trust_score: u8,
        chain_id: u64,
        block_height: u64,
    ) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let validity = tier.attestation_validity().as_secs();

        Self {
            tier,
            provider_id,
            hardware_id,
            evidence_hash,
            trust_score,
            issued_at: now,
            expires_at: now + validity,
            chain_id,
            block_height,
        }
    }

    /// Check if attestation is currently valid
    pub fn is_valid(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        self.tier != CCTier::Unknown && now >= self.issued_at && now < self.expires_at
    }

    /// Check if this attestation meets a tier requirement
    pub fn meets_requirement(&self, required: CCTier) -> bool {
        self.is_valid() && self.tier.meets_requirement(required)
    }

    /// Get time until expiry
    pub fn time_until_expiry(&self) -> Option<Duration> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        if now >= self.expires_at {
            None
        } else {
            Some(Duration::from_secs(self.expires_at - now))
        }
    }

    /// Create a valid attestation for testing
    /// The attestation will be valid for the specified duration
    pub fn new_valid(tier: CCTier, validity: Duration) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        Self {
            tier,
            provider_id: "test-provider".to_string(),
            hardware_id: "test-hardware".to_string(),
            evidence_hash: [0u8; 32],
            trust_score: tier.base_trust_score(),
            issued_at: now,
            expires_at: now + validity.as_secs(),
            chain_id: 1,
            block_height: 1,
        }
    }
}

/// Tier requirement for task execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierRequirement {
    pub min_tier: CCTier,
    pub require_valid_attestation: bool,
    pub max_attestation_age_secs: u64,
    pub min_trust_score: u8,
    pub require_specific_vendor: Option<String>,
    pub require_min_memory_bytes: Option<u64>,
}

impl TierRequirement {
    /// Create default requirements for a tier
    pub fn for_tier(tier: CCTier) -> Self {
        Self {
            min_tier: tier,
            require_valid_attestation: true,
            max_attestation_age_secs: tier.attestation_validity().as_secs(),
            min_trust_score: tier.base_trust_score(),
            require_specific_vendor: None,
            require_min_memory_bytes: None,
        }
    }

    /// Check if an attestation meets these requirements
    pub fn is_met(&self, attestation: &TierAttestation) -> Result<(), TierError> {
        if !attestation.meets_requirement(self.min_tier) {
            return Err(TierError::TierNotMet {
                have: attestation.tier,
                need: self.min_tier,
            });
        }

        if self.require_valid_attestation && !attestation.is_valid() {
            return Err(TierError::AttestationExpired);
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let age = now.saturating_sub(attestation.issued_at);
        if age > self.max_attestation_age_secs {
            return Err(TierError::AttestationTooOld {
                age_secs: age,
                max_secs: self.max_attestation_age_secs,
            });
        }

        if attestation.trust_score < self.min_trust_score {
            return Err(TierError::TrustScoreTooLow {
                have: attestation.trust_score,
                need: self.min_trust_score,
            });
        }

        Ok(())
    }
}

/// Errors related to tier operations
#[derive(Debug, Clone, thiserror::Error)]
pub enum TierError {
    #[error("Provider tier {have} does not meet requirement {need}")]
    TierNotMet { have: CCTier, need: CCTier },

    #[error("Attestation has expired")]
    AttestationExpired,

    #[error("Attestation too old: {age_secs}s > {max_secs}s")]
    AttestationTooOld { age_secs: u64, max_secs: u64 },

    #[error("Trust score {have} below minimum {need}")]
    TrustScoreTooLow { have: u8, need: u8 },

    #[error("Invalid attestation evidence")]
    InvalidEvidence,

    #[error("Hardware not supported for tier: {0}")]
    HardwareNotSupported(String),

    #[error("Insufficient stake for tier")]
    InsufficientStake,
}

#[cfg(test)]
mod tests {
    use super::*;

    // =============================================================================
    // CCTier Basic Tests
    // =============================================================================

    #[test]
    fn test_tier_ordering() {
        assert!(CCTier::Tier1GpuNativeCC.meets_requirement(CCTier::Tier1GpuNativeCC));
        assert!(CCTier::Tier1GpuNativeCC.meets_requirement(CCTier::Tier2ConfidentialVM));
        assert!(CCTier::Tier1GpuNativeCC.meets_requirement(CCTier::Tier4Standard));

        assert!(!CCTier::Tier2ConfidentialVM.meets_requirement(CCTier::Tier1GpuNativeCC));
        assert!(CCTier::Tier2ConfidentialVM.meets_requirement(CCTier::Tier2ConfidentialVM));

        assert!(!CCTier::Tier4Standard.meets_requirement(CCTier::Tier1GpuNativeCC));
        assert!(CCTier::Tier4Standard.meets_requirement(CCTier::Tier4Standard));

        // Unknown tier never meets any requirement
        assert!(!CCTier::Unknown.meets_requirement(CCTier::Tier4Standard));
        assert!(!CCTier::Unknown.meets_requirement(CCTier::Unknown));
    }

    #[test]
    fn test_tier_names_and_descriptions() {
        assert_eq!(CCTier::Tier1GpuNativeCC.name(), "GPU-Native-CC");
        assert_eq!(CCTier::Tier2ConfidentialVM.name(), "Confidential-VM");
        assert_eq!(CCTier::Tier3DeviceTEE.name(), "Device-TEE");
        assert_eq!(CCTier::Tier4Standard.name(), "Standard");
        assert_eq!(CCTier::Unknown.name(), "Unknown");

        // All descriptions should be non-empty
        assert!(!CCTier::Tier1GpuNativeCC.description().is_empty());
        assert!(!CCTier::Tier2ConfidentialVM.description().is_empty());
        assert!(!CCTier::Tier3DeviceTEE.description().is_empty());
        assert!(!CCTier::Tier4Standard.description().is_empty());
        assert!(!CCTier::Unknown.description().is_empty());
    }

    #[test]
    fn test_tier_base_and_max_trust_scores() {
        // Tier 1: 90-100
        assert_eq!(CCTier::Tier1GpuNativeCC.base_trust_score(), 90);
        assert_eq!(CCTier::Tier1GpuNativeCC.max_trust_score(), 100);

        // Tier 2: 70-89
        assert_eq!(CCTier::Tier2ConfidentialVM.base_trust_score(), 70);
        assert_eq!(CCTier::Tier2ConfidentialVM.max_trust_score(), 89);

        // Tier 3: 50-69
        assert_eq!(CCTier::Tier3DeviceTEE.base_trust_score(), 50);
        assert_eq!(CCTier::Tier3DeviceTEE.max_trust_score(), 69);

        // Tier 4: 10-49
        assert_eq!(CCTier::Tier4Standard.base_trust_score(), 10);
        assert_eq!(CCTier::Tier4Standard.max_trust_score(), 49);

        // Unknown: 0
        assert_eq!(CCTier::Unknown.base_trust_score(), 0);
        assert_eq!(CCTier::Unknown.max_trust_score(), 0);
    }

    #[test]
    fn test_tier_reward_multipliers() {
        assert_eq!(CCTier::Tier1GpuNativeCC.reward_multiplier(), 1.5);
        assert_eq!(CCTier::Tier2ConfidentialVM.reward_multiplier(), 1.0);
        assert_eq!(CCTier::Tier3DeviceTEE.reward_multiplier(), 0.75);
        assert_eq!(CCTier::Tier4Standard.reward_multiplier(), 0.5);
        assert_eq!(CCTier::Unknown.reward_multiplier(), 0.0);
    }

    #[test]
    fn test_tier_min_stake() {
        assert_eq!(CCTier::Tier1GpuNativeCC.min_stake_lux(), 100_000);
        assert_eq!(CCTier::Tier2ConfidentialVM.min_stake_lux(), 50_000);
        assert_eq!(CCTier::Tier3DeviceTEE.min_stake_lux(), 10_000);
        assert_eq!(CCTier::Tier4Standard.min_stake_lux(), 1_000);
        assert_eq!(CCTier::Unknown.min_stake_lux(), 0);
    }

    #[test]
    fn test_tier_attestation_validity() {
        assert_eq!(CCTier::Tier1GpuNativeCC.attestation_validity(), Duration::from_secs(6 * 3600));
        assert_eq!(CCTier::Tier2ConfidentialVM.attestation_validity(), Duration::from_secs(24 * 3600));
        assert_eq!(CCTier::Tier3DeviceTEE.attestation_validity(), Duration::from_secs(7 * 24 * 3600));
        assert_eq!(CCTier::Tier4Standard.attestation_validity(), Duration::from_secs(30 * 24 * 3600));
        assert_eq!(CCTier::Unknown.attestation_validity(), Duration::ZERO);
    }

    #[test]
    fn test_tier_from_u8() {
        assert_eq!(CCTier::from_u8(1), CCTier::Tier1GpuNativeCC);
        assert_eq!(CCTier::from_u8(2), CCTier::Tier2ConfidentialVM);
        assert_eq!(CCTier::from_u8(3), CCTier::Tier3DeviceTEE);
        assert_eq!(CCTier::from_u8(4), CCTier::Tier4Standard);
        assert_eq!(CCTier::from_u8(0), CCTier::Unknown);
        assert_eq!(CCTier::from_u8(5), CCTier::Unknown);
        assert_eq!(CCTier::from_u8(255), CCTier::Unknown);
    }

    #[test]
    fn test_tier_default() {
        assert_eq!(CCTier::default(), CCTier::Unknown);
    }

    #[test]
    fn test_tier_display() {
        assert_eq!(format!("{}", CCTier::Tier1GpuNativeCC), "GPU-Native-CC");
        assert_eq!(format!("{}", CCTier::Unknown), "Unknown");
    }

    #[test]
    fn test_tier_from_capabilities() {
        // Tier 1: Blackwell with NRAS
        let mut caps = VendorCapabilities::default();
        caps.gpu_blackwell_tee_io = true;
        caps.nvidia_nras_ok = true;
        assert_eq!(CCTier::from_capabilities(&caps), CCTier::Tier1GpuNativeCC);

        // Tier 2: SEV-SNP
        let mut caps = VendorCapabilities::default();
        caps.cpu_sev_snp = true;
        assert_eq!(CCTier::from_capabilities(&caps), CCTier::Tier2ConfidentialVM);

        // Tier 3: SIM only
        let mut caps = VendorCapabilities::default();
        caps.sim_available = true;
        assert_eq!(CCTier::from_capabilities(&caps), CCTier::Tier3DeviceTEE);

        // Tier 4: Nothing
        let caps = VendorCapabilities::default();
        assert_eq!(CCTier::from_capabilities(&caps), CCTier::Tier4Standard);
    }

    #[test]
    fn test_trust_score_calculation() {
        let input = TrustScoreInput {
            tier: CCTier::Tier1GpuNativeCC,
            gpu_generation: 10,
            cc_features_enabled: true,
            tee_io_enabled: true,
            rim_verified: true,
            attestation_method: AttestationMethod::NvTrust,
            local_verification: true,
            cert_chain_valid: true,
            tasks_completed: 1000,
            tasks_failed: 10,
            uptime_percentage: 99.9,
            last_seen_secs: 30,
            consecutive_heartbeats: 500,
            reputation_score: 0.9,
            ..Default::default()
        };

        let result = input.calculate();
        assert!(result.total_score >= 90 && result.total_score <= 100);
        assert!(result.meets_minimum);
    }

    #[test]
    fn test_tier_attestation_validity_basic() {
        let attestation = TierAttestation::new(
            CCTier::Tier2ConfidentialVM,
            "provider-1".to_string(),
            "gpu-serial-123".to_string(),
            [0u8; 32],
            85,
            96369,
            12345,
        );

        assert!(attestation.is_valid());
        assert!(attestation.meets_requirement(CCTier::Tier2ConfidentialVM));
        assert!(attestation.meets_requirement(CCTier::Tier3DeviceTEE));
        assert!(!attestation.meets_requirement(CCTier::Tier1GpuNativeCC));
    }

    #[test]
    fn test_tier_requirement_validation() {
        let requirement = TierRequirement::for_tier(CCTier::Tier2ConfidentialVM);

        let valid_attestation = TierAttestation::new(
            CCTier::Tier1GpuNativeCC,
            "provider-1".to_string(),
            "gpu-1".to_string(),
            [0u8; 32],
            95,
            96369,
            100,
        );
        assert!(requirement.is_met(&valid_attestation).is_ok());

        let insufficient_attestation = TierAttestation::new(
            CCTier::Tier3DeviceTEE,
            "provider-2".to_string(),
            "device-1".to_string(),
            [0u8; 32],
            55,
            96369,
            100,
        );
        assert!(requirement.is_met(&insufficient_attestation).is_err());
    }

    #[test]
    fn test_privacy_tier_conversion() {
        // Test round-trip conversion
        let cc_tier = CCTier::Tier1GpuNativeCC;
        let privacy_tier = cc_tier.to_privacy_tier();
        assert_eq!(privacy_tier, PrivacyTier::AccessGpuTeeIoMax);

        let back = CCTier::from_privacy_tier(privacy_tier);
        assert_eq!(back, CCTier::Tier1GpuNativeCC);
    }

    #[test]
    fn test_quick_trust_score() {
        let mut caps = VendorCapabilities::default();
        caps.gpu_h100_cc = true;
        caps.nvidia_nras_ok = true;
        caps.cpu_sev_snp = true;

        let score = quick_trust_score(CCTier::Tier1GpuNativeCC, &caps);
        assert!(score >= 90);
    }

    // =============================================================================
    // Trust Score Calculation Tests
    // =============================================================================

    #[test]
    fn test_trust_score_weights_default() {
        let weights = TrustScoreWeights::default();
        let sum = weights.hardware + weights.attestation + weights.reputation + weights.uptime;
        assert!((sum - 1.0).abs() < 0.001, "Weights should sum to 1.0, got {}", sum);
    }

    #[test]
    fn test_hardware_score_by_tier() {
        // Tier 1: Base 35 + GPU gen bonus
        let input = TrustScoreInput {
            tier: CCTier::Tier1GpuNativeCC,
            gpu_generation: 10,
            cc_features_enabled: true,
            tee_io_enabled: true,
            rim_verified: true,
            ..Default::default()
        };
        let result = input.calculate();
        assert!(result.hardware_score >= 35, "Tier1 hardware >= 35");

        // Tier 4: Base 5 only
        let input4 = TrustScoreInput {
            tier: CCTier::Tier4Standard,
            ..Default::default()
        };
        let result4 = input4.calculate();
        assert_eq!(result4.hardware_score, 5, "Tier4 hardware = 5");

        // Unknown: Base 0
        let input_unk = TrustScoreInput {
            tier: CCTier::Unknown,
            ..Default::default()
        };
        let result_unk = input_unk.calculate();
        assert_eq!(result_unk.hardware_score, 0, "Unknown hardware = 0");
    }

    #[test]
    fn test_attestation_score_methods() {
        // NvTrust gives +10
        let input_nv = TrustScoreInput {
            tier: CCTier::Tier1GpuNativeCC,
            attestation_method: AttestationMethod::NvTrust,
            local_verification: true,
            cert_chain_valid: true,
            ..Default::default()
        };
        let score_nv = input_nv.calculate().attestation_score;

        // CCA gives +6
        let input_cca = TrustScoreInput {
            tier: CCTier::Tier1GpuNativeCC,
            attestation_method: AttestationMethod::Cca,
            local_verification: true,
            cert_chain_valid: true,
            ..Default::default()
        };
        let score_cca = input_cca.calculate().attestation_score;

        // SecureEnclave gives +5
        let input_se = TrustScoreInput {
            tier: CCTier::Tier1GpuNativeCC,
            attestation_method: AttestationMethod::SecureEnclave,
            ..Default::default()
        };
        let score_se = input_se.calculate().attestation_score;

        assert!(score_nv > score_cca, "NvTrust > CCA");
        assert!(score_cca > score_se, "CCA > SecureEnclave");
    }

    #[test]
    fn test_attestation_score_freshness() {
        let validity = CCTier::Tier1GpuNativeCC.attestation_validity().as_secs();

        // Very fresh (<25%)
        let fresh = TrustScoreInput {
            tier: CCTier::Tier1GpuNativeCC,
            attestation_age_secs: validity / 10, // 10%
            ..Default::default()
        };
        let score_fresh = fresh.calculate().attestation_score;

        // Old (>75%)
        let old = TrustScoreInput {
            tier: CCTier::Tier1GpuNativeCC,
            attestation_age_secs: validity * 9 / 10, // 90%
            ..Default::default()
        };
        let score_old = old.calculate().attestation_score;

        assert!(score_fresh > score_old, "Fresh attestation scores higher");
    }

    #[test]
    fn test_reputation_score_calculation() {
        // Perfect reputation
        let perfect = TrustScoreInput {
            tier: CCTier::Tier2ConfidentialVM,
            tasks_completed: 10000,
            tasks_failed: 0,
            slashing_events: 0,
            reputation_score: 1.0,
            ..Default::default()
        };
        let score_perfect = perfect.calculate().reputation_score;

        // Bad reputation with slashing
        let bad = TrustScoreInput {
            tier: CCTier::Tier2ConfidentialVM,
            tasks_completed: 100,
            tasks_failed: 50,
            slashing_events: 5,
            reputation_score: 0.3,
            ..Default::default()
        };
        let score_bad = bad.calculate().reputation_score;

        assert!(score_perfect > score_bad, "Perfect > Bad reputation");
        assert!(score_perfect >= 80, "Perfect reputation >= 80");
    }

    #[test]
    fn test_uptime_score_calculation() {
        // Perfect uptime
        let perfect = TrustScoreInput {
            tier: CCTier::Tier3DeviceTEE,
            uptime_percentage: 100.0,
            last_seen_secs: 30,
            consecutive_heartbeats: 5000,
            ..Default::default()
        };
        let score_perfect = perfect.calculate().uptime_score;

        // Poor uptime
        let poor = TrustScoreInput {
            tier: CCTier::Tier3DeviceTEE,
            uptime_percentage: 50.0,
            last_seen_secs: 7200, // 2 hours
            consecutive_heartbeats: 5,
            ..Default::default()
        };
        let score_poor = poor.calculate().uptime_score;

        assert!(score_perfect > score_poor, "Perfect > Poor uptime");
        assert!(score_perfect >= 90, "Perfect uptime >= 90");
    }

    #[test]
    fn test_score_clamping_to_tier_limits() {
        // High inputs for Tier4 - should be clamped to 49
        let high_t4 = TrustScoreInput {
            tier: CCTier::Tier4Standard,
            gpu_generation: 10,
            cc_features_enabled: true,
            tee_io_enabled: true,
            rim_verified: true,
            attestation_method: AttestationMethod::NvTrust,
            local_verification: true,
            cert_chain_valid: true,
            tasks_completed: 100000,
            reputation_score: 1.0,
            uptime_percentage: 100.0,
            last_seen_secs: 10,
            consecutive_heartbeats: 10000,
            ..Default::default()
        };
        let result = high_t4.calculate();
        assert!(result.total_score <= 49, "Tier4 capped at 49, got {}", result.total_score);

        // Low inputs for Tier1 - should be raised to 90 (clamped)
        let low_t1 = TrustScoreInput {
            tier: CCTier::Tier1GpuNativeCC,
            slashing_events: 10,
            ..Default::default()
        };
        let result1 = low_t1.calculate();
        assert!(result1.total_score >= 90, "Tier1 min is 90, got {}", result1.total_score);
        assert!(!result1.warnings.is_empty(), "Should have clamping warning");
    }

    // =============================================================================
    // TierAttestation Tests
    // =============================================================================

    #[test]
    fn test_attestation_new_valid() {
        let attestation = TierAttestation::new_valid(
            CCTier::Tier2ConfidentialVM,
            Duration::from_secs(3600),
        );

        assert_eq!(attestation.tier, CCTier::Tier2ConfidentialVM);
        assert!(attestation.is_valid());
        assert!(attestation.time_until_expiry().is_some());
    }

    #[test]
    fn test_attestation_expired() {
        // Create an attestation that's already expired
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let expired = TierAttestation {
            tier: CCTier::Tier3DeviceTEE,
            provider_id: "test".to_string(),
            hardware_id: "hw".to_string(),
            evidence_hash: [0u8; 32],
            trust_score: 60,
            issued_at: now - 7200, // 2 hours ago
            expires_at: now - 3600, // 1 hour ago - expired
            chain_id: 1,
            block_height: 100,
        };

        assert!(!expired.is_valid());
        assert!(expired.time_until_expiry().is_none());
        assert!(!expired.meets_requirement(CCTier::Tier3DeviceTEE));
    }

    #[test]
    fn test_attestation_unknown_tier_invalid() {
        let attestation = TierAttestation::new_valid(
            CCTier::Unknown,
            Duration::from_secs(3600),
        );
        assert!(!attestation.is_valid(), "Unknown tier is always invalid");
    }

    // =============================================================================
    // TierRequirement Tests
    // =============================================================================

    #[test]
    fn test_requirement_for_tier() {
        let req = TierRequirement::for_tier(CCTier::Tier2ConfidentialVM);

        assert_eq!(req.min_tier, CCTier::Tier2ConfidentialVM);
        assert!(req.require_valid_attestation);
        assert_eq!(req.min_trust_score, 70); // Tier2 base
    }

    #[test]
    fn test_requirement_errors() {
        let requirement = TierRequirement::for_tier(CCTier::Tier1GpuNativeCC);

        // Wrong tier
        let wrong_tier = TierAttestation::new_valid(
            CCTier::Tier3DeviceTEE,
            Duration::from_secs(3600),
        );
        let err = requirement.is_met(&wrong_tier).unwrap_err();
        assert!(matches!(err, TierError::TierNotMet { .. }));

        // Low trust score
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let low_score = TierAttestation {
            tier: CCTier::Tier1GpuNativeCC,
            provider_id: "test".to_string(),
            hardware_id: "hw".to_string(),
            evidence_hash: [0u8; 32],
            trust_score: 50, // Below Tier1 minimum of 90
            issued_at: now,
            expires_at: now + 3600,
            chain_id: 1,
            block_height: 100,
        };
        let err2 = requirement.is_met(&low_score).unwrap_err();
        assert!(matches!(err2, TierError::TrustScoreTooLow { .. }));
    }

    #[test]
    fn test_requirement_attestation_too_old() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let requirement = TierRequirement {
            min_tier: CCTier::Tier4Standard,
            require_valid_attestation: true,
            max_attestation_age_secs: 3600, // 1 hour max
            min_trust_score: 10,
            require_specific_vendor: None,
            require_min_memory_bytes: None,
        };

        // Old attestation (but still valid)
        let old = TierAttestation {
            tier: CCTier::Tier4Standard,
            provider_id: "test".to_string(),
            hardware_id: "hw".to_string(),
            evidence_hash: [0u8; 32],
            trust_score: 40,
            issued_at: now - 7200, // 2 hours ago
            expires_at: now + 3600, // Still valid
            chain_id: 1,
            block_height: 100,
        };

        let err = requirement.is_met(&old).unwrap_err();
        assert!(matches!(err, TierError::AttestationTooOld { .. }));
    }

    // =============================================================================
    // TierError Tests
    // =============================================================================

    #[test]
    fn test_tier_error_display() {
        let err = TierError::TierNotMet {
            have: CCTier::Tier3DeviceTEE,
            need: CCTier::Tier1GpuNativeCC,
        };
        let msg = format!("{}", err);
        assert!(msg.contains("Device-TEE"));
        assert!(msg.contains("GPU-Native-CC"));

        let err2 = TierError::TrustScoreTooLow { have: 50, need: 90 };
        let msg2 = format!("{}", err2);
        assert!(msg2.contains("50"));
        assert!(msg2.contains("90"));
    }
}
