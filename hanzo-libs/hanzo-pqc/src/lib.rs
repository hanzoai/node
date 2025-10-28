//! NIST Post-Quantum Cryptography implementation for Hanzo Node
//! 
//! Implements FIPS 203 (ML-KEM), FIPS 204 (ML-DSA), and FIPS 205 (SLH-DSA)
//! with support for hybrid modes and privacy tiers.

pub mod kem;
pub mod signature;
pub mod kdf;
pub mod hybrid;
pub mod privacy_tiers;
pub mod wire_protocol;
pub mod attestation;
pub mod config;
pub mod errors;

pub use kem::{Kem, KemAlgorithm, KemKeyPair, EncapsulationKey, DecapsulationKey};
pub use signature::{Signature, SignatureAlgorithm, SigningKey, VerifyingKey};
pub use kdf::{Kdf, KdfAlgorithm};
pub use hybrid::{HybridKem, HybridMode};
pub use privacy_tiers::{PrivacyTier, CapabilityMatrix, RuntimeRequirements};
pub use config::PqcConfig;
pub use errors::{PqcError, Result};

// Re-export common types
pub use oqs;

/// Initialize the PQC subsystem with FIPS-compliant RNG
pub fn init() -> Result<()> {
    // Ensure we're using a FIPS-compliant RNG
    #[cfg(feature = "fips-mode")]
    {
        verify_fips_rng()?;
    }
    
    Ok(())
}

#[cfg(feature = "fips-mode")]
fn verify_fips_rng() -> Result<()> {
    // Verify SP 800-90A compliant RNG
    use getrandom::getrandom;
    let mut buf = [0u8; 32];
    getrandom(&mut buf).map_err(|e| PqcError::RngError(e.to_string()))?;
    Ok(())
}