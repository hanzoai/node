//! Privacy tiers and capability matrix for attestation-based key release
//! Implements tiered privacy from open data to GPU TEE-I/O

use serde::{Deserialize, Serialize};

/// Privacy tier definitions
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PrivacyTier {
    /// Tier 0: Open data (public research)
    AccessOpen = 0,
    /// Tier 1: At-rest confidentiality (SIM/FileVault)
    AccessAtRest = 1,
    /// Tier 2: CPU-TEE (SEV-SNP/TDX)
    AccessCpuTee = 2,
    /// Tier 3: CPU-TEE + GPU-CC (H100 Confidential Computing)
    AccessCpuTeePlusGpuCc = 3,
    /// Tier 4: CPU-TEE + GPU TEE-I/O (Blackwell)
    AccessGpuTeeIoMax = 4,
}

impl PrivacyTier {
    /// Check if this tier meets or exceeds requirements
    pub fn meets_requirement(&self, required: PrivacyTier) -> bool {
        *self >= required
    }
    
    /// Get human-readable description
    pub fn description(&self) -> &'static str {
        match self {
            Self::AccessOpen => "Open data - no confidentiality guarantees",
            Self::AccessAtRest => "At-rest encryption with SIM-protected keys",
            Self::AccessCpuTee => "CPU TEE protection (SEV-SNP/TDX)",
            Self::AccessCpuTeePlusGpuCc => "CPU TEE + GPU Confidential Computing (H100)",
            Self::AccessGpuTeeIoMax => "Maximum protection with GPU TEE-I/O (Blackwell)",
        }
    }
}

/// Vendor-specific capabilities
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VendorCapabilities {
    /// AMD SEV-SNP support
    pub cpu_sev_snp: bool,
    /// Intel TDX support
    pub cpu_tdx: bool,
    /// Intel SGX support
    pub cpu_sgx: bool,
    /// ARM CCA support
    pub cpu_arm_cca: bool,
    /// NVIDIA H100 Confidential Computing
    pub gpu_h100_cc: bool,
    /// NVIDIA Blackwell TEE-I/O
    pub gpu_blackwell_tee_io: bool,
    /// NVIDIA Remote Attestation Service verified
    pub nvidia_nras_ok: bool,
    /// MIG-slice isolation attested
    pub mig_isolated: bool,
    /// SIM/eSIM available
    pub sim_available: bool,
    /// Hardware security module
    pub hsm_available: bool,
}

impl VendorCapabilities {
    /// Determine the maximum privacy tier supported
    pub fn max_tier(&self) -> PrivacyTier {
        if self.gpu_blackwell_tee_io && (self.cpu_sev_snp || self.cpu_tdx) {
            PrivacyTier::AccessGpuTeeIoMax
        } else if self.gpu_h100_cc && (self.cpu_sev_snp || self.cpu_tdx) {
            PrivacyTier::AccessCpuTeePlusGpuCc
        } else if self.cpu_sev_snp || self.cpu_tdx || self.cpu_sgx || self.cpu_arm_cca {
            PrivacyTier::AccessCpuTee
        } else if self.sim_available || self.hsm_available {
            PrivacyTier::AccessAtRest
        } else {
            PrivacyTier::AccessOpen
        }
    }
}

/// Runtime requirements for a specific tier
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeRequirements {
    pub min_tier: PrivacyTier,
    pub require_sev_snp: bool,
    pub require_tdx: bool,
    pub require_sgx: bool,
    pub require_h100_cc: bool,
    pub require_blackwell_tee_io: bool,
    pub require_mig_isolation: bool,
    pub require_nras_attestation: bool,
}

impl RuntimeRequirements {
    /// Create requirements for a specific tier
    pub fn for_tier(tier: PrivacyTier) -> Self {
        match tier {
            PrivacyTier::AccessOpen => Self {
                min_tier: tier,
                require_sev_snp: false,
                require_tdx: false,
                require_sgx: false,
                require_h100_cc: false,
                require_blackwell_tee_io: false,
                require_mig_isolation: false,
                require_nras_attestation: false,
            },
            PrivacyTier::AccessAtRest => Self {
                min_tier: tier,
                require_sev_snp: false,
                require_tdx: false,
                require_sgx: false,
                require_h100_cc: false,
                require_blackwell_tee_io: false,
                require_mig_isolation: false,
                require_nras_attestation: false,
            },
            PrivacyTier::AccessCpuTee => Self {
                min_tier: tier,
                require_sev_snp: false, // Either SEV-SNP or TDX acceptable
                require_tdx: false,
                require_sgx: false,
                require_h100_cc: false,
                require_blackwell_tee_io: false,
                require_mig_isolation: false,
                require_nras_attestation: false,
            },
            PrivacyTier::AccessCpuTeePlusGpuCc => Self {
                min_tier: tier,
                require_sev_snp: false,
                require_tdx: false,
                require_sgx: false,
                require_h100_cc: true,
                require_blackwell_tee_io: false,
                require_mig_isolation: false,
                require_nras_attestation: true,
            },
            PrivacyTier::AccessGpuTeeIoMax => Self {
                min_tier: tier,
                require_sev_snp: false,
                require_tdx: false,
                require_sgx: false,
                require_h100_cc: false,
                require_blackwell_tee_io: true,
                require_mig_isolation: true,
                require_nras_attestation: true,
            },
        }
    }
    
    /// Check if capabilities meet requirements
    pub fn validate(&self, caps: &VendorCapabilities) -> bool {
        // Check minimum tier
        if caps.max_tier() < self.min_tier {
            return false;
        }
        
        // Check specific requirements
        if self.require_sev_snp && !caps.cpu_sev_snp {
            // If specifically requiring SEV-SNP
            return false;
        }
        
        if self.require_tdx && !caps.cpu_tdx {
            // If specifically requiring TDX
            return false;
        }
        
        if self.require_sgx && !caps.cpu_sgx {
            return false;
        }
        
        if self.require_h100_cc && !caps.gpu_h100_cc {
            return false;
        }
        
        if self.require_blackwell_tee_io && !caps.gpu_blackwell_tee_io {
            return false;
        }
        
        if self.require_mig_isolation && !caps.mig_isolated {
            return false;
        }
        
        if self.require_nras_attestation && !caps.nvidia_nras_ok {
            return false;
        }
        
        // For CPU-TEE tier, require at least one CPU TEE technology
        if self.min_tier >= PrivacyTier::AccessCpuTee
            && !(caps.cpu_sev_snp || caps.cpu_tdx || caps.cpu_sgx || caps.cpu_arm_cca) {
                return false;
            }
        
        true
    }
}

/// Capability matrix for tracking node capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityMatrix {
    pub node_id: String,
    pub capabilities: VendorCapabilities,
    pub evidence_hash: Option<[u8; 32]>,
    pub attestation_timestamp: Option<u64>,
    pub policy_hash: Option<[u8; 32]>,
}

impl CapabilityMatrix {
    /// Create a new capability matrix for a node
    pub fn new(node_id: String) -> Self {
        Self {
            node_id,
            capabilities: VendorCapabilities::default(),
            evidence_hash: None,
            attestation_timestamp: None,
            policy_hash: None,
        }
    }
    
    /// Update with attestation evidence
    pub fn update_with_evidence(
        &mut self,
        caps: VendorCapabilities,
        evidence_hash: [u8; 32],
        policy_hash: [u8; 32],
    ) {
        self.capabilities = caps;
        self.evidence_hash = Some(evidence_hash);
        self.policy_hash = Some(policy_hash);
        self.attestation_timestamp = Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        );
    }
    
    /// Check if attestation is still valid (default 24 hours)
    pub fn is_valid(&self, max_age_secs: u64) -> bool {
        if let Some(timestamp) = self.attestation_timestamp {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            (now - timestamp) <= max_age_secs
        } else {
            false
        }
    }
}

/// Key release policy based on privacy tiers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyReleasePolicy {
    pub dataset_id: String,
    pub requirements: RuntimeRequirements,
    pub owner: String,
    pub created_at: u64,
    pub expires_at: Option<u64>,
}

impl KeyReleasePolicy {
    /// Check if a node can access this dataset
    pub fn can_access(&self, matrix: &CapabilityMatrix) -> bool {
        // Check attestation validity
        if !matrix.is_valid(86400) {
            // 24 hour attestation validity
            return false;
        }
        
        // Check requirements
        self.requirements.validate(&matrix.capabilities)
    }
}

/// Capability token for authorized key release
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityToken {
    pub node_id: String,
    pub dataset_id: String,
    pub session_pubkey: Vec<u8>,
    pub evidence_hash: [u8; 32],
    pub issued_at: u64,
    pub expires_at: u64,
    pub tier: PrivacyTier,
}

impl CapabilityToken {
    /// Check if token is still valid
    pub fn is_valid(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        now <= self.expires_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_tier_ordering() {
        assert!(PrivacyTier::AccessGpuTeeIoMax > PrivacyTier::AccessCpuTee);
        assert!(PrivacyTier::AccessCpuTee.meets_requirement(PrivacyTier::AccessAtRest));
        assert!(!PrivacyTier::AccessAtRest.meets_requirement(PrivacyTier::AccessCpuTee));
    }
    
    #[test]
    fn test_capability_validation() {
        let mut caps = VendorCapabilities::default();
        caps.cpu_sev_snp = true;
        caps.gpu_h100_cc = true;
        caps.nvidia_nras_ok = true;
        
        let req = RuntimeRequirements::for_tier(PrivacyTier::AccessCpuTeePlusGpuCc);
        assert!(req.validate(&caps));
        
        // Should fail for Blackwell tier
        let req_max = RuntimeRequirements::for_tier(PrivacyTier::AccessGpuTeeIoMax);
        assert!(!req_max.validate(&caps));
    }
}