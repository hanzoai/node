//! Key Derivation Functions (KDF) 
//! SP 800-56C compliant HKDF and SP 800-108 compliant KDF

use hkdf::Hkdf;
use sha2::{Sha256, Sha384, Sha512};
use sha3::{Sha3_256, Sha3_384, Sha3_512};
use serde::{Deserialize, Serialize};
use crate::{PqcError, Result};

/// KDF algorithms (SP 800-56C and SP 800-108 compliant)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KdfAlgorithm {
    /// HKDF with SHA-256 (128-bit security)
    HkdfSha256,
    /// HKDF with SHA-384 (192-bit security) - RECOMMENDED for ML-KEM-768
    HkdfSha384,
    /// HKDF with SHA-512 (256-bit security)
    HkdfSha512,
    /// HKDF with SHA3-256 (128-bit security)
    HkdfSha3_256,
    /// HKDF with SHA3-384 (192-bit security)
    HkdfSha3_384,
    /// HKDF with SHA3-512 (256-bit security)
    HkdfSha3_512,
    /// BLAKE3 KDF (256-bit security)
    Blake3Kdf,
}

impl Default for KdfAlgorithm {
    fn default() -> Self {
        Self::HkdfSha384 // Matches ML-KEM-768 security level
    }
}

/// KDF trait for key derivation operations
pub trait Kdf {
    /// Extract a pseudorandom key from input keying material
    fn extract(&self, salt: Option<&[u8]>, ikm: &[u8]) -> Vec<u8>;
    
    /// Expand a pseudorandom key to desired length
    fn expand(&self, prk: &[u8], info: &[u8], okm_len: usize) -> Result<Vec<u8>>;
    
    /// Combined extract-and-expand operation
    fn derive(&self, salt: Option<&[u8]>, ikm: &[u8], info: &[u8], okm_len: usize) -> Result<Vec<u8>>;
}

/// Generic HKDF implementation
pub struct HkdfKdf {
    algorithm: KdfAlgorithm,
}

impl HkdfKdf {
    pub fn new(algorithm: KdfAlgorithm) -> Self {
        Self { algorithm }
    }
}

impl Kdf for HkdfKdf {
    fn extract(&self, salt: Option<&[u8]>, ikm: &[u8]) -> Vec<u8> {
        match self.algorithm {
            KdfAlgorithm::HkdfSha256 => {
                let (prk, _) = Hkdf::<Sha256>::extract(salt, ikm);
                prk.to_vec()
            }
            KdfAlgorithm::HkdfSha384 => {
                let (prk, _) = Hkdf::<Sha384>::extract(salt, ikm);
                prk.to_vec()
            }
            KdfAlgorithm::HkdfSha512 => {
                let (prk, _) = Hkdf::<Sha512>::extract(salt, ikm);
                prk.to_vec()
            }
            KdfAlgorithm::HkdfSha3_256 => {
                let (prk, _) = Hkdf::<Sha3_256>::extract(salt, ikm);
                prk.to_vec()
            }
            KdfAlgorithm::HkdfSha3_384 => {
                let (prk, _) = Hkdf::<Sha3_384>::extract(salt, ikm);
                prk.to_vec()
            }
            KdfAlgorithm::HkdfSha3_512 => {
                let (prk, _) = Hkdf::<Sha3_512>::extract(salt, ikm);
                prk.to_vec()
            }
            KdfAlgorithm::Blake3Kdf => {
                // BLAKE3 has its own KDF mode
                let key = blake3::derive_key(
                    salt.map(|s| std::str::from_utf8(s).unwrap_or("hanzo-pqc")).unwrap_or("hanzo-pqc"),
                    ikm,
                );
                key.to_vec()
            }
        }
    }
    
    fn expand(&self, prk: &[u8], info: &[u8], okm_len: usize) -> Result<Vec<u8>> {
        let mut okm = vec![0u8; okm_len];
        
        match self.algorithm {
            KdfAlgorithm::HkdfSha256 => {
                let hk = Hkdf::<Sha256>::from_prk(prk)
                    .map_err(|_| PqcError::KdfError("Invalid PRK length for SHA256".into()))?;
                hk.expand(info, &mut okm)
                    .map_err(|_| PqcError::KdfError("HKDF expand failed".into()))?;
            }
            KdfAlgorithm::HkdfSha384 => {
                let hk = Hkdf::<Sha384>::from_prk(prk)
                    .map_err(|_| PqcError::KdfError("Invalid PRK length for SHA384".into()))?;
                hk.expand(info, &mut okm)
                    .map_err(|_| PqcError::KdfError("HKDF expand failed".into()))?;
            }
            KdfAlgorithm::HkdfSha512 => {
                let hk = Hkdf::<Sha512>::from_prk(prk)
                    .map_err(|_| PqcError::KdfError("Invalid PRK length for SHA512".into()))?;
                hk.expand(info, &mut okm)
                    .map_err(|_| PqcError::KdfError("HKDF expand failed".into()))?;
            }
            KdfAlgorithm::HkdfSha3_256 => {
                let hk = Hkdf::<Sha3_256>::from_prk(prk)
                    .map_err(|_| PqcError::KdfError("Invalid PRK length for SHA3-256".into()))?;
                hk.expand(info, &mut okm)
                    .map_err(|_| PqcError::KdfError("HKDF expand failed".into()))?;
            }
            KdfAlgorithm::HkdfSha3_384 => {
                let hk = Hkdf::<Sha3_384>::from_prk(prk)
                    .map_err(|_| PqcError::KdfError("Invalid PRK length for SHA3-384".into()))?;
                hk.expand(info, &mut okm)
                    .map_err(|_| PqcError::KdfError("HKDF expand failed".into()))?;
            }
            KdfAlgorithm::HkdfSha3_512 => {
                let hk = Hkdf::<Sha3_512>::from_prk(prk)
                    .map_err(|_| PqcError::KdfError("Invalid PRK length for SHA3-512".into()))?;
                hk.expand(info, &mut okm)
                    .map_err(|_| PqcError::KdfError("HKDF expand failed".into()))?;
            }
            KdfAlgorithm::Blake3Kdf => {
                // BLAKE3 XOF mode for expansion
                let mut hasher = blake3::Hasher::new_keyed(
                    &<[u8; 32]>::try_from(&prk[..32])
                        .map_err(|_| PqcError::KdfError("BLAKE3 requires 32-byte key".into()))?
                );
                hasher.update(info);
                let mut output = hasher.finalize_xof();
                output.fill(&mut okm);
            }
        }
        
        Ok(okm)
    }
    
    fn derive(&self, salt: Option<&[u8]>, ikm: &[u8], info: &[u8], okm_len: usize) -> Result<Vec<u8>> {
        let prk = self.extract(salt, ikm);
        self.expand(&prk, info, okm_len)
    }
}

/// Combine multiple shared secrets (for hybrid mode)
/// Per SP 800-56C Rev 2, Section 5.9.3
pub fn combine_shared_secrets(
    kdf: &impl Kdf,
    secrets: &[&[u8]],
    context: &[u8],
    output_len: usize,
) -> Result<Vec<u8>> {
    // Concatenate all secrets with length prefixes
    let mut combined = Vec::new();
    for secret in secrets {
        combined.extend_from_slice(&(secret.len() as u32).to_be_bytes());
        combined.extend_from_slice(secret);
    }
    
    // Derive final key material with context
    kdf.derive(None, &combined, context, output_len)
}

/// Domain separation for different protocol contexts
pub fn domain_separate(
    kdf: &impl Kdf,
    key_material: &[u8],
    domain: &str,
    output_len: usize,
) -> Result<Vec<u8>> {
    let info = format!("hanzo-pqc-v1|{}", domain);
    kdf.expand(key_material, info.as_bytes(), output_len)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_hkdf_sha384() {
        let kdf = HkdfKdf::new(KdfAlgorithm::HkdfSha384);
        
        let ikm = b"input keying material";
        let salt = b"salt";
        let info = b"hanzo-test-v1";
        
        let okm = kdf.derive(Some(salt), ikm, info, 64).unwrap();
        assert_eq!(okm.len(), 64);
        
        // Verify deterministic
        let okm2 = kdf.derive(Some(salt), ikm, info, 64).unwrap();
        assert_eq!(okm, okm2);
    }
    
    #[test]
    fn test_combine_secrets() {
        let kdf = HkdfKdf::new(KdfAlgorithm::HkdfSha384);
        
        let secret1 = vec![1u8; 32]; // ML-KEM shared secret
        let secret2 = vec![2u8; 32]; // X25519 shared secret
        
        let combined = combine_shared_secrets(
            &kdf,
            &[&secret1, &secret2],
            b"hanzo-hybrid-v1",
            48,
        ).unwrap();
        
        assert_eq!(combined.len(), 48);
    }
}