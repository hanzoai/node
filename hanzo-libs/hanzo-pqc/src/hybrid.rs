//! Hybrid KEM implementation combining ML-KEM with X25519
//! Per NIST guidance for defense-in-depth

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use crate::{
    kem::{Kem, KemAlgorithm, KemKeyPair, KemOutput, EncapsulationKey, DecapsulationKey},
    kdf::{HkdfKdf, KdfAlgorithm, Kdf, combine_shared_secrets},
    PqcError, Result,
};

/// Hybrid mode configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HybridMode {
    /// ML-KEM-512 + X25519
    MlKem512X25519,
    /// ML-KEM-768 + X25519 (RECOMMENDED)
    MlKem768X25519,
    /// ML-KEM-1024 + X25519
    MlKem1024X25519,
}

impl Default for HybridMode {
    fn default() -> Self {
        Self::MlKem768X25519
    }
}

impl HybridMode {
    pub fn pq_algorithm(&self) -> KemAlgorithm {
        match self {
            Self::MlKem512X25519 => KemAlgorithm::MlKem512,
            Self::MlKem768X25519 => KemAlgorithm::MlKem768,
            Self::MlKem1024X25519 => KemAlgorithm::MlKem1024,
        }
    }
    
    pub fn kdf_algorithm(&self) -> KdfAlgorithm {
        match self {
            Self::MlKem512X25519 => KdfAlgorithm::HkdfSha256,
            Self::MlKem768X25519 => KdfAlgorithm::HkdfSha384,
            Self::MlKem1024X25519 => KdfAlgorithm::HkdfSha512,
        }
    }
}

/// Hybrid encapsulation key (PQ + Classical)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridEncapsulationKey {
    pub mode: HybridMode,
    pub pq_key: EncapsulationKey,
    pub classical_key: EncapsulationKey,
}

/// Hybrid decapsulation key (PQ + Classical)
#[derive(Clone)]
pub struct HybridDecapsulationKey {
    pub mode: HybridMode,
    pub pq_key: DecapsulationKey,
    pub classical_key: DecapsulationKey,
}

/// Hybrid KEM output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridCiphertext {
    pub pq_ciphertext: Vec<u8>,
    pub classical_ciphertext: Vec<u8>,
}

/// Hybrid KEM implementation
pub struct HybridKem {
    pq_kem: Box<dyn Kem>,
    classical_kem: Box<dyn Kem>,
    kdf: HkdfKdf,
}

impl HybridKem {
    #[cfg(all(feature = "ml-kem", feature = "hybrid"))]
    pub fn new(mode: HybridMode) -> Self {
        use crate::kem::{MlKem, X25519Kem};
        
        Self {
            pq_kem: Box::new(MlKem::new()),
            classical_kem: Box::new(X25519Kem),
            kdf: HkdfKdf::new(mode.kdf_algorithm()),
        }
    }
    
    /// Generate hybrid key pair
    pub async fn generate_keypair(&self, mode: HybridMode) -> Result<(HybridEncapsulationKey, HybridDecapsulationKey)> {
        let pq_keypair = self.pq_kem.generate_keypair(mode.pq_algorithm()).await?;
        let classical_keypair = self.classical_kem.generate_keypair(KemAlgorithm::X25519).await?;
        
        Ok((
            HybridEncapsulationKey {
                mode,
                pq_key: pq_keypair.encap_key,
                classical_key: classical_keypair.encap_key,
            },
            HybridDecapsulationKey {
                mode,
                pq_key: pq_keypair.decap_key,
                classical_key: classical_keypair.decap_key,
            },
        ))
    }
    
    /// Hybrid encapsulation
    pub async fn encapsulate(
        &self,
        key: &HybridEncapsulationKey,
        context: &[u8],
    ) -> Result<(HybridCiphertext, [u8; 32])> {
        // Encapsulate with both algorithms
        let pq_output = self.pq_kem.encapsulate(&key.pq_key).await?;
        let classical_output = self.classical_kem.encapsulate(&key.classical_key).await?;
        
        // Combine shared secrets per SP 800-56C
        let combined = combine_shared_secrets(
            &self.kdf,
            &[&pq_output.shared_secret, &classical_output.shared_secret],
            context,
            32,
        )?;
        
        let mut shared_secret = [0u8; 32];
        shared_secret.copy_from_slice(&combined);
        
        Ok((
            HybridCiphertext {
                pq_ciphertext: pq_output.ciphertext,
                classical_ciphertext: classical_output.ciphertext,
            },
            shared_secret,
        ))
    }
    
    /// Hybrid decapsulation
    pub async fn decapsulate(
        &self,
        key: &HybridDecapsulationKey,
        ciphertext: &HybridCiphertext,
        context: &[u8],
    ) -> Result<[u8; 32]> {
        // Decapsulate with both algorithms
        let pq_secret = self.pq_kem.decapsulate(&key.pq_key, &ciphertext.pq_ciphertext).await?;
        let classical_secret = self.classical_kem.decapsulate(&key.classical_key, &ciphertext.classical_ciphertext).await?;
        
        // Combine shared secrets per SP 800-56C
        let combined = combine_shared_secrets(
            &self.kdf,
            &[&pq_secret, &classical_secret],
            context,
            32,
        )?;
        
        let mut shared_secret = [0u8; 32];
        shared_secret.copy_from_slice(&combined);
        
        Ok(shared_secret)
    }
}

/// Wire format for hybrid handshake
#[derive(Serialize, Deserialize)]
pub struct HybridHandshakeMessage {
    pub mode: HybridMode,
    pub sender_id: Vec<u8>,
    pub pq_public_key: Vec<u8>,
    pub classical_public_key: Vec<u8>,
    pub ciphertext: Option<HybridCiphertext>,
    pub nonce: [u8; 32],
    pub supported_algorithms: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    #[cfg(all(feature = "ml-kem", feature = "hybrid"))]
    async fn test_hybrid_kem() {
        let kem = HybridKem::new(HybridMode::MlKem768X25519);
        let (encap_key, decap_key) = kem.generate_keypair(HybridMode::MlKem768X25519).await.unwrap();
        
        let context = b"hanzo-hybrid-test-v1";
        let (ciphertext, shared1) = kem.encapsulate(&encap_key, context).await.unwrap();
        let shared2 = kem.decapsulate(&decap_key, &ciphertext, context).await.unwrap();
        
        assert_eq!(shared1, shared2);
        assert_eq!(ciphertext.pq_ciphertext.len(), 1088); // ML-KEM-768 ciphertext
        assert_eq!(ciphertext.classical_ciphertext.len(), 32); // X25519 ephemeral pubkey
    }
}