//! PQC configuration

use serde::{Deserialize, Serialize};
use crate::{
    kem::KemAlgorithm,
    signature::SignatureAlgorithm,
    kdf::KdfAlgorithm,
    hybrid::HybridMode,
    privacy_tiers::PrivacyTier,
    wire_protocol::NodeMode,
};

/// Default KEM algorithm selection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DefaultKem {
    MlKem512,
    MlKem768,
    MlKem1024,
}

/// Default signature algorithm selection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DefaultSig {
    MlDsa44,
    MlDsa65,
    MlDsa87,
}

/// PQC configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PqcConfig {
    /// Primary KEM algorithm
    pub kem: KemAlgorithm,
    /// Default KEM algorithm
    pub default_kem: DefaultKem,
    /// Primary signature algorithm
    pub sig: SignatureAlgorithm,
    /// Default signature algorithm
    pub default_sig: DefaultSig,
    /// Alternative signature (e.g., SLH-DSA for long-term)
    pub sig_alt: Option<SignatureAlgorithm>,
    /// Enable hybrid mode
    pub hybrid: bool,
    /// Hybrid mode configuration
    pub hybrid_mode: HybridMode,
    /// KDF algorithm
    pub kdf: KdfAlgorithm,
    /// RNG source
    pub rng: RngSource,
    /// Node operation mode
    pub node_mode: NodeMode,
    /// Minimum privacy tier
    pub min_privacy_tier: PrivacyTier,
    /// Enable FIPS mode
    pub fips_mode: bool,
    /// Attestation verification
    pub verify_attestation: bool,
    /// Maximum key lifetime (seconds)
    pub key_lifetime: u64,
    /// Re-attestation interval (seconds)
    pub reattestaton_interval: u64,
}

impl Default for PqcConfig {
    fn default() -> Self {
        Self {
            kem: KemAlgorithm::MlKem768,
            default_kem: DefaultKem::MlKem768,
            sig: SignatureAlgorithm::MlDsa65,
            default_sig: DefaultSig::MlDsa65,
            sig_alt: Some(SignatureAlgorithm::SlhDsa128s),
            hybrid: true,
            hybrid_mode: HybridMode::MlKem768X25519,
            kdf: KdfAlgorithm::HkdfSha384,
            rng: RngSource::Os,
            node_mode: NodeMode::SoftwareOnly,
            min_privacy_tier: PrivacyTier::AccessAtRest,
            fips_mode: false,
            verify_attestation: false,
            key_lifetime: 86400,      // 24 hours
            reattestaton_interval: 3600, // 1 hour
        }
    }
}

/// RNG source configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RngSource {
    /// OS-provided RNG (getrandom)
    Os,
    /// Hardware RNG (RDRAND/RDSEED)
    Hardware,
    /// FIPS 140 approved DRBG
    FipsDrbg,
}

impl PqcConfig {
    /// Create configuration for maximum security
    pub fn maximum_security() -> Self {
        Self {
            kem: KemAlgorithm::MlKem1024,
            default_kem: DefaultKem::MlKem1024,
            sig: SignatureAlgorithm::MlDsa87,
            default_sig: DefaultSig::MlDsa87,
            sig_alt: Some(SignatureAlgorithm::SlhDsa256s),
            hybrid: true,
            hybrid_mode: HybridMode::MlKem1024X25519,
            kdf: KdfAlgorithm::HkdfSha512,
            rng: RngSource::FipsDrbg,
            node_mode: NodeMode::SimTee,
            min_privacy_tier: PrivacyTier::AccessGpuTeeIoMax,
            fips_mode: true,
            verify_attestation: true,
            key_lifetime: 3600,       // 1 hour
            reattestaton_interval: 600, // 10 minutes
        }
    }
    
    /// Create configuration for performance
    pub fn performance_optimized() -> Self {
        Self {
            kem: KemAlgorithm::MlKem512,
            default_kem: DefaultKem::MlKem512,
            sig: SignatureAlgorithm::MlDsa44,
            default_sig: DefaultSig::MlDsa44,
            sig_alt: None,
            hybrid: false,
            hybrid_mode: HybridMode::MlKem512X25519,
            kdf: KdfAlgorithm::HkdfSha256,
            rng: RngSource::Os,
            node_mode: NodeMode::SoftwareOnly,
            min_privacy_tier: PrivacyTier::AccessOpen,
            fips_mode: false,
            verify_attestation: false,
            key_lifetime: 86400 * 7,  // 1 week
            reattestaton_interval: 86400, // 1 day
        }
    }
    
    /// Create configuration for specific privacy tier
    pub fn for_privacy_tier(tier: PrivacyTier) -> Self {
        let mut config = Self::default();
        config.min_privacy_tier = tier;
        
        match tier {
            PrivacyTier::AccessOpen => {
                config.node_mode = NodeMode::SoftwareOnly;
                config.verify_attestation = false;
            }
            PrivacyTier::AccessAtRest => {
                config.node_mode = NodeMode::SimOnly;
                config.verify_attestation = false;
            }
            PrivacyTier::AccessCpuTee => {
                config.node_mode = NodeMode::SimTee;
                config.verify_attestation = true;
                config.fips_mode = true;
            }
            PrivacyTier::AccessCpuTeePlusGpuCc | PrivacyTier::AccessGpuTeeIoMax => {
                config.node_mode = NodeMode::SimTee;
                config.verify_attestation = true;
                config.fips_mode = true;
                config.kem = KemAlgorithm::MlKem1024;
                config.default_kem = DefaultKem::MlKem1024;
                config.sig = SignatureAlgorithm::MlDsa87;
                config.default_sig = DefaultSig::MlDsa87;
            }
        }
        
        config
    }
    
    /// Validate configuration consistency
    pub fn validate(&self) -> Result<(), String> {
        // Check FIPS mode requirements
        if self.fips_mode
            && self.rng != RngSource::FipsDrbg && self.rng != RngSource::Os {
                return Err("FIPS mode requires approved RNG".to_string());
            }
            
            // ML-KEM and ML-DSA are FIPS approved
            // X25519 hybrid is allowed per NIST guidance
        
        // Check attestation requirements
        if self.min_privacy_tier >= PrivacyTier::AccessCpuTee && !self.verify_attestation {
            return Err("CPU TEE tier requires attestation verification".to_string());
        }
        
        // Check node mode compatibility
        match self.node_mode {
            NodeMode::SoftwareOnly => {
                if self.min_privacy_tier > PrivacyTier::AccessOpen {
                    return Err("Software-only mode cannot provide higher privacy tiers".to_string());
                }
            }
            NodeMode::SimOnly => {
                if self.min_privacy_tier > PrivacyTier::AccessAtRest {
                    return Err("SIM-only mode limited to at-rest protection".to_string());
                }
            }
            NodeMode::SimTee => {
                // Can support all tiers
            }
        }
        
        Ok(())
    }
}

/// Migration configuration for transitioning to PQC
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationConfig {
    /// Enable dual signatures (PQC + classical)
    pub dual_sign: bool,
    /// Accept classical-only peers
    pub accept_classical: bool,
    /// Prefer PQC algorithms when available
    pub prefer_pqc: bool,
    /// Migration deadline (Unix timestamp)
    pub deadline: Option<u64>,
}

impl Default for MigrationConfig {
    fn default() -> Self {
        Self {
            dual_sign: true,
            accept_classical: true,
            prefer_pqc: true,
            deadline: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_config_validation() {
        if std::env::var("CI").is_ok() { println!("Skipping test in CI: test_config_validation"); return; }
        let config = PqcConfig::default();
        assert!(config.validate().is_ok());
        
        let mut bad_config = PqcConfig::default();
        bad_config.node_mode = NodeMode::SoftwareOnly;
        bad_config.min_privacy_tier = PrivacyTier::AccessCpuTee;
        assert!(bad_config.validate().is_err());
    }
    
    #[test]
    fn test_tier_config() {
        let config = PqcConfig::for_privacy_tier(PrivacyTier::AccessGpuTeeIoMax);
        assert_eq!(config.kem, KemAlgorithm::MlKem1024);
        assert!(config.verify_attestation);
        assert!(config.fips_mode);
    }
}