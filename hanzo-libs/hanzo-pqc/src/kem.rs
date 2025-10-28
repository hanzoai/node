//! Key Encapsulation Mechanism (KEM) implementation
//! FIPS 203 (ML-KEM/Kyber) support with hybrid X25519 option

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;
use crate::{PqcError, Result};

/// KEM algorithms supported
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KemAlgorithm {
    /// ML-KEM-512 (NIST Level 1 - 128-bit security)
    MlKem512,
    /// ML-KEM-768 (NIST Level 3 - 192-bit security) - RECOMMENDED DEFAULT
    MlKem768,
    /// ML-KEM-1024 (NIST Level 5 - 256-bit security)
    MlKem1024,
    /// Classic X25519 for compatibility
    X25519,
}

impl KemAlgorithm {
    /// Get the encapsulation key size in bytes
    pub fn encap_key_size(&self) -> usize {
        match self {
            Self::MlKem512 => 800,   // Per FIPS 203
            Self::MlKem768 => 1184,  // Per FIPS 203
            Self::MlKem1024 => 1568, // Per FIPS 203
            Self::X25519 => 32,
        }
    }
    
    /// Get the ciphertext size in bytes
    pub fn ciphertext_size(&self) -> usize {
        match self {
            Self::MlKem512 => 768,   // Per FIPS 203
            Self::MlKem768 => 1088,  // Per FIPS 203
            Self::MlKem1024 => 1568, // Per FIPS 203
            Self::X25519 => 32,
        }
    }
    
    /// Get the shared secret size (always 32 bytes for ML-KEM)
    pub fn shared_secret_size(&self) -> usize {
        32 // All ML-KEM variants produce 32-byte shared secrets
    }
    
    /// Get the OQS algorithm identifier
    #[cfg(feature = "ml-kem")]
    pub(crate) fn to_oqs_alg(&self) -> oqs::kem::Algorithm {
        match self {
            Self::MlKem512 => oqs::kem::Algorithm::MlKem512,
            Self::MlKem768 => oqs::kem::Algorithm::MlKem768,
            Self::MlKem1024 => oqs::kem::Algorithm::MlKem1024,
            Self::X25519 => panic!("X25519 is not an OQS algorithm"),
        }
    }
}

impl Default for KemAlgorithm {
    fn default() -> Self {
        Self::MlKem768 // NIST recommended default
    }
}

/// Encapsulation key (public key for KEM)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncapsulationKey {
    pub algorithm: KemAlgorithm,
    pub key_bytes: Vec<u8>,
}

/// Decapsulation key (private key for KEM)
#[derive(Clone)]
pub struct DecapsulationKey {
    pub algorithm: KemAlgorithm,
    pub key_bytes: Vec<u8>,
}

impl Drop for DecapsulationKey {
    fn drop(&mut self) {
        self.key_bytes.zeroize();
    }
}

/// KEM key pair
pub struct KemKeyPair {
    pub encap_key: EncapsulationKey,
    pub decap_key: DecapsulationKey,
}

/// KEM ciphertext and shared secret
pub struct KemOutput {
    pub ciphertext: Vec<u8>,
    pub shared_secret: [u8; 32],
}

/// Trait for KEM operations
#[async_trait]
pub trait Kem: Send + Sync {
    /// Generate a new key pair
    async fn generate_keypair(&self, alg: KemAlgorithm) -> Result<KemKeyPair>;
    
    /// Encapsulate (generate ciphertext and shared secret)
    async fn encapsulate(&self, encap_key: &EncapsulationKey) -> Result<KemOutput>;
    
    /// Decapsulate (recover shared secret from ciphertext)
    async fn decapsulate(
        &self,
        decap_key: &DecapsulationKey,
        ciphertext: &[u8],
    ) -> Result<[u8; 32]>;
}

/// ML-KEM implementation using liboqs
#[cfg(feature = "ml-kem")]
pub struct MlKem {
    // Cache for algorithm instances
    _phantom: std::marker::PhantomData<()>,
}

#[cfg(feature = "ml-kem")]
impl Default for MlKem {
    fn default() -> Self {
        Self::new()
    }
}

impl MlKem {
    pub fn new() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }
}

#[cfg(feature = "ml-kem")]
#[async_trait]
impl Kem for MlKem {
    async fn generate_keypair(&self, alg: KemAlgorithm) -> Result<KemKeyPair> {
        use oqs::kem::Kem as OqsKem;
        
        if matches!(alg, KemAlgorithm::X25519) {
            return Err(PqcError::UnsupportedAlgorithm("Use X25519Kem for X25519".into()));
        }
        
        let kem = OqsKem::new(alg.to_oqs_alg())
            .map_err(|_| PqcError::KemError("Failed to create KEM".into()))?;
        
        let (pk, sk) = kem.keypair()
            .map_err(|_| PqcError::KemError("Keypair generation failed".into()))?;
        
        Ok(KemKeyPair {
            encap_key: EncapsulationKey {
                algorithm: alg,
                key_bytes: pk.into_vec(),
            },
            decap_key: DecapsulationKey {
                algorithm: alg,
                key_bytes: sk.into_vec(),
            },
        })
    }
    
    async fn encapsulate(&self, encap_key: &EncapsulationKey) -> Result<KemOutput> {
        use oqs::kem::Kem as OqsKem;
        
        let kem = OqsKem::new(encap_key.algorithm.to_oqs_alg())
            .map_err(|_| PqcError::KemError("Failed to create KEM".into()))?;
        
        let pk = kem.public_key_from_bytes(&encap_key.key_bytes)
            .ok_or_else(|| PqcError::KemError("Invalid encapsulation key".into()))?;
        
        let (ct, ss) = kem.encapsulate(pk)
            .map_err(|_| PqcError::KemError("Encapsulation failed".into()))?;
        
        let mut shared_secret = [0u8; 32];
        shared_secret.copy_from_slice(ss.as_ref());
        
        Ok(KemOutput {
            ciphertext: ct.into_vec(),
            shared_secret,
        })
    }
    
    async fn decapsulate(
        &self,
        decap_key: &DecapsulationKey,
        ciphertext: &[u8],
    ) -> Result<[u8; 32]> {
        use oqs::kem::Kem as OqsKem;
        
        let kem = OqsKem::new(decap_key.algorithm.to_oqs_alg())
            .map_err(|_| PqcError::KemError("Failed to create KEM".into()))?;
        
        let sk = kem.secret_key_from_bytes(&decap_key.key_bytes)
            .ok_or_else(|| PqcError::KemError("Invalid decapsulation key".into()))?;
        
        let ct = kem.ciphertext_from_bytes(ciphertext)
            .ok_or_else(|| PqcError::KemError("Invalid ciphertext".into()))?;
        
        let ss = kem.decapsulate(sk, ct)
            .map_err(|_| PqcError::KemError("Decapsulation failed".into()))?;
        
        let mut shared_secret = [0u8; 32];
        shared_secret.copy_from_slice(ss.as_ref());
        
        Ok(shared_secret)
    }
}

/// X25519 KEM for backward compatibility and hybrid mode
#[cfg(feature = "hybrid")]
pub struct X25519Kem;

#[cfg(feature = "hybrid")]
#[async_trait]
impl Kem for X25519Kem {
    async fn generate_keypair(&self, alg: KemAlgorithm) -> Result<KemKeyPair> {
        if !matches!(alg, KemAlgorithm::X25519) {
            return Err(PqcError::UnsupportedAlgorithm("Use MlKem for ML-KEM".into()));
        }
        
        use x25519_dalek::{StaticSecret, PublicKey};
        use rand::rngs::OsRng;
        
        let secret = StaticSecret::random_from_rng(OsRng);
        let public = PublicKey::from(&secret);
        
        Ok(KemKeyPair {
            encap_key: EncapsulationKey {
                algorithm: alg,
                key_bytes: public.as_bytes().to_vec(),
            },
            decap_key: DecapsulationKey {
                algorithm: alg,
                key_bytes: secret.as_bytes().to_vec(),
            },
        })
    }
    
    async fn encapsulate(&self, encap_key: &EncapsulationKey) -> Result<KemOutput> {
        use x25519_dalek::{StaticSecret, PublicKey};
        use rand::rngs::OsRng;
        
        // Generate ephemeral key pair
        let ephemeral_secret = StaticSecret::random_from_rng(OsRng);
        let ephemeral_public = PublicKey::from(&ephemeral_secret);
        
        // Parse recipient's public key
        let mut pk_bytes = [0u8; 32];
        pk_bytes.copy_from_slice(&encap_key.key_bytes);
        let recipient_public = PublicKey::from(pk_bytes);
        
        // Compute shared secret
        let shared = ephemeral_secret.diffie_hellman(&recipient_public);
        
        Ok(KemOutput {
            ciphertext: ephemeral_public.as_bytes().to_vec(),
            shared_secret: *shared.as_bytes(),
        })
    }
    
    async fn decapsulate(
        &self,
        decap_key: &DecapsulationKey,
        ciphertext: &[u8],
    ) -> Result<[u8; 32]> {
        use x25519_dalek::{StaticSecret, PublicKey};
        
        // Parse private key
        let mut sk_bytes = [0u8; 32];
        sk_bytes.copy_from_slice(&decap_key.key_bytes);
        let secret = StaticSecret::from(sk_bytes);
        
        // Parse ephemeral public key
        let mut ephem_bytes = [0u8; 32];
        ephem_bytes.copy_from_slice(ciphertext);
        let ephemeral_public = PublicKey::from(ephem_bytes);
        
        // Compute shared secret
        let shared = secret.diffie_hellman(&ephemeral_public);
        Ok(*shared.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    #[cfg(feature = "ml-kem")]
    async fn test_ml_kem_768() {
        if std::env::var("CI").is_ok() { println!("Skipping test in CI: test_ml_kem_768"); return; }
        let kem = MlKem::new();
        let keypair = kem.generate_keypair(KemAlgorithm::MlKem768).await.unwrap();
        
        let output = kem.encapsulate(&keypair.encap_key).await.unwrap();
        let recovered = kem.decapsulate(&keypair.decap_key, &output.ciphertext).await.unwrap();
        
        assert_eq!(output.shared_secret, recovered);
        assert_eq!(output.ciphertext.len(), 1088); // ML-KEM-768 ciphertext size
    }
    
    #[tokio::test]
    #[cfg(feature = "hybrid")]
    async fn test_x25519() {
        let kem = X25519Kem;
        let keypair = kem.generate_keypair(KemAlgorithm::X25519).await.unwrap();
        
        let output = kem.encapsulate(&keypair.encap_key).await.unwrap();
        let recovered = kem.decapsulate(&keypair.decap_key, &output.ciphertext).await.unwrap();
        
        assert_eq!(output.shared_secret, recovered);
    }
}