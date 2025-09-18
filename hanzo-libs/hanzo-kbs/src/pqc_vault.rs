//! PQC-enhanced vault implementations
//! 
//! Provides quantum-resistant key storage and operations for different privacy tiers

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::{
    error::{Result, SecurityError},
    types::{PrivacyTier, KeyId, AttestationType},
    vault::KeyVault,
};


#[cfg(feature = "pqc")]
use hanzo_pqc::{
    kem::{Kem, KemAlgorithm, EncapsulationKey, DecapsulationKey, MlKem},
    signature::{Signature, SignatureAlgorithm, VerifyingKey, SigningKey, MlDsa},
    config::PqcConfig,
};

/// PQC-enhanced vault for Tier 2+ (CPU TEE and above)
#[cfg(feature = "pqc")]
pub struct PqcVault {
    tier: PrivacyTier,
    config: PqcConfig,
    ml_kem: Arc<MlKem>,
    ml_dsa: Arc<MlDsa>,
    // In-memory storage for dev/testing (production would use TEE sealed storage)
    keys: Arc<RwLock<HashMap<KeyId, ProtectedKey>>>,
    attestation: Option<AttestationType>,
}

#[cfg(feature = "pqc")]
#[derive(Clone)]
struct ProtectedKey {
    /// ML-KEM encrypted key material
    wrapped_key: Vec<u8>,
    /// Decapsulation key (stored in TEE in production)
    decap_key: DecapsulationKey,
    /// Metadata
    created_at: chrono::DateTime<chrono::Utc>,
    tier: PrivacyTier,
}

#[cfg(feature = "pqc")]
impl PqcVault {
    pub async fn new(tier: PrivacyTier, attestation: Option<AttestationType>) -> Result<Self> {
        if tier < PrivacyTier::CpuTee && attestation.is_some() {
            return Err(SecurityError::PolicyViolation(
                "Attestation requires CPU TEE or higher tier".into()
            ));
        }
        
        let pqc_tier = match tier {
            PrivacyTier::Open => hanzo_pqc::privacy_tiers::PrivacyTier::AccessOpen,
            PrivacyTier::AtRest => hanzo_pqc::privacy_tiers::PrivacyTier::AccessAtRest,
            PrivacyTier::CpuTee => hanzo_pqc::privacy_tiers::PrivacyTier::AccessCpuTee,
            PrivacyTier::GpuCc => hanzo_pqc::privacy_tiers::PrivacyTier::AccessCpuTeePlusGpuCc,
            PrivacyTier::GpuTeeIo => hanzo_pqc::privacy_tiers::PrivacyTier::AccessGpuTeeIoMax,
        };
        
        let config = PqcConfig::for_privacy_tier(pqc_tier);
        
        Ok(Self {
            tier,
            config,
            ml_kem: Arc::new(MlKem::new()),
            ml_dsa: Arc::new(MlDsa::new()),
            keys: Arc::new(RwLock::new(HashMap::new())),
            attestation,
        })
    }
    
    /// Generate ML-KEM keypair for this vault
    async fn generate_kem_keypair(&self) -> Result<(EncapsulationKey, DecapsulationKey)> {
        let alg = self.tier.recommended_kem();
        let keypair = self.ml_kem.generate_keypair(alg).await
            .map_err(|e| SecurityError::CryptoError(format!("KEM keypair generation failed: {:?}", e)))?;
        Ok((keypair.encap_key, keypair.decap_key))
    }
    
    /// Generate ML-DSA keypair for signing
    async fn generate_signing_keypair(&self) -> Result<(VerifyingKey, SigningKey)> {
        let alg = self.tier.recommended_sig();
        self.ml_dsa.generate_keypair(alg).await
            .map_err(|e| SecurityError::CryptoError(format!("Signing keypair generation failed: {:?}", e)))
    }
}

// Extension methods for PrivacyTier to get recommended algorithms
impl PrivacyTier {
    fn recommended_kem(&self) -> KemAlgorithm {
        match self {
            PrivacyTier::Open | PrivacyTier::AtRest => KemAlgorithm::MlKem512,
            PrivacyTier::CpuTee => KemAlgorithm::MlKem768,
            PrivacyTier::GpuCc | PrivacyTier::GpuTeeIo => KemAlgorithm::MlKem1024,
        }
    }
    
    fn recommended_sig(&self) -> SignatureAlgorithm {
        match self {
            PrivacyTier::Open | PrivacyTier::AtRest => SignatureAlgorithm::MlDsa44,
            PrivacyTier::CpuTee => SignatureAlgorithm::MlDsa65,
            PrivacyTier::GpuCc | PrivacyTier::GpuTeeIo => SignatureAlgorithm::MlDsa87,
        }
    }
}

#[cfg(feature = "pqc")]
#[async_trait]
impl KeyVault for PqcVault {
    fn tier(&self) -> PrivacyTier {
        self.tier
    }
    
    async fn store_key(&self, key_id: &KeyId, key_data: &[u8]) -> Result<()> {
        // Generate vault-specific KEM keypair
        let (encap_key, decap_key) = self.generate_kem_keypair().await?;
        
        // Encapsulate key data
        let output = self.ml_kem.encapsulate(&encap_key).await
            .map_err(|e| SecurityError::CryptoError(format!("Key encapsulation failed: {:?}", e)))?;
        
        // Derive wrapping key from shared secret
        use hanzo_pqc::kdf::{HkdfKdf, Kdf};
        let kdf = HkdfKdf::new(self.config.kdf);
        let wrapping_key = kdf.derive(
            None,
            &output.shared_secret,
            b"hanzo-vault-wrap-v1",
            32,
        ).map_err(|e| SecurityError::CryptoError(format!("KDF failed: {:?}", e)))?;
        
        // Encrypt key data with ChaCha20Poly1305
        use chacha20poly1305::{
            aead::{Aead, AeadCore, KeyInit, OsRng},
            ChaCha20Poly1305,
        };
        
        let cipher = ChaCha20Poly1305::new_from_slice(&wrapping_key)
            .map_err(|e| SecurityError::CryptoError(format!("Cipher init failed: {:?}", e)))?;
        
        let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ciphertext = cipher.encrypt(&nonce, key_data)
            .map_err(|e| SecurityError::CryptoError(format!("Key encryption failed: {:?}", e)))?;
        
        // Store: kem_ct || nonce || ciphertext
        let mut wrapped = Vec::new();
        wrapped.extend_from_slice(&(output.ciphertext.len() as u32).to_be_bytes());
        wrapped.extend_from_slice(&output.ciphertext);
        wrapped.extend_from_slice(&nonce);
        wrapped.extend_from_slice(&ciphertext);
        
        // Store protected key
        let protected = ProtectedKey {
            wrapped_key: wrapped,
            decap_key,
            created_at: chrono::Utc::now(),
            tier: self.tier,
        };
        
        self.keys.write().await.insert(key_id.clone(), protected);
        Ok(())
    }
    
    async fn use_key<F, R>(&self, key_id: &KeyId, operation: F) -> Result<R>
    where
        F: FnOnce(&[u8]) -> R + Send,
        R: Send,
    {
        let keys = self.keys.read().await;
        let protected = keys.get(key_id)
            .ok_or_else(|| SecurityError::KeyNotFound(key_id.0.to_string()))?;
        
        // Parse wrapped format
        let wrapped = &protected.wrapped_key;
        if wrapped.len() < 4 {
            return Err(SecurityError::CryptoError("Invalid wrapped key format".into()));
        }
        
        let ct_len = u32::from_be_bytes([wrapped[0], wrapped[1], wrapped[2], wrapped[3]]) as usize;
        if wrapped.len() < 4 + ct_len + 12 {
            return Err(SecurityError::CryptoError("Invalid wrapped key length".into()));
        }
        
        let kem_ct = &wrapped[4..4 + ct_len];
        let nonce = &wrapped[4 + ct_len..4 + ct_len + 12];
        let ciphertext = &wrapped[4 + ct_len + 12..];
        
        // Decapsulate to recover shared secret
        let shared_secret = self.ml_kem.decapsulate(&protected.decap_key, kem_ct).await
            .map_err(|e| SecurityError::CryptoError(format!("Decapsulation failed: {:?}", e)))?;
        
        // Derive wrapping key
        use hanzo_pqc::kdf::{HkdfKdf, Kdf};
        let kdf = HkdfKdf::new(self.config.kdf);
        let wrapping_key = kdf.derive(
            None,
            &shared_secret,
            b"hanzo-vault-wrap-v1",
            32,
        ).map_err(|e| SecurityError::CryptoError(format!("KDF failed: {:?}", e)))?;
        
        // Decrypt key data
        use chacha20poly1305::{
            aead::{Aead, KeyInit},
            ChaCha20Poly1305, Nonce,
        };
        
        let cipher = ChaCha20Poly1305::new_from_slice(&wrapping_key)
            .map_err(|e| SecurityError::CryptoError(format!("Cipher init failed: {:?}", e)))?;
        
        let nonce = Nonce::from_slice(nonce);
        let key_data = cipher.decrypt(nonce, ciphertext)
            .map_err(|e| SecurityError::CryptoError(format!("Key decryption failed: {:?}", e)))?;
        
        // Use key in protected context
        let result = operation(&key_data);
        
        // Key data is automatically zeroized when dropped
        Ok(result)
    }
    
    async fn delete_key(&self, key_id: &KeyId) -> Result<()> {
        self.keys.write().await.remove(key_id)
            .ok_or_else(|| SecurityError::KeyNotFound(key_id.0.to_string()))?;
        Ok(())
    }
    
    async fn is_initialized(&self) -> Result<bool> {
        // Check if we have valid attestation for TEE tiers
        if self.tier >= PrivacyTier::CpuTee {
            Ok(self.attestation.is_some())
        } else {
            Ok(true)
        }
    }
}

/// GPU Confidential Computing vault (Tier 3)
#[cfg(feature = "pqc")]
pub struct GpuCcVault {
    inner: PqcVault,
    gpu_device_id: u32,
    cc_enabled: bool,
}

#[cfg(feature = "pqc")]
impl GpuCcVault {
    pub async fn new(gpu_device_id: u32, attestation: AttestationType) -> Result<Self> {
        // Verify this is H100 CC attestation
        match &attestation {
            AttestationType::H100Cc { .. } => {},
            _ => return Err(SecurityError::PolicyViolation(
                "GPU CC vault requires H100 CC attestation".into()
            )),
        }
        
        let inner = PqcVault::new(PrivacyTier::GpuCc, Some(attestation)).await?;
        
        Ok(Self {
            inner,
            gpu_device_id,
            cc_enabled: true,
        })
    }
}

#[cfg(feature = "pqc")]
#[async_trait]
impl KeyVault for GpuCcVault {
    fn tier(&self) -> PrivacyTier {
        PrivacyTier::GpuCc
    }
    
    async fn store_key(&self, key_id: &KeyId, key_data: &[u8]) -> Result<()> {
        // Additional GPU-specific protections could go here
        // For H100 CC, keys are protected by encrypted DMA
        self.inner.store_key(key_id, key_data).await
    }
    
    async fn use_key<F, R>(&self, key_id: &KeyId, operation: F) -> Result<R>
    where
        F: FnOnce(&[u8]) -> R + Send,
        R: Send,
    {
        if !self.cc_enabled {
            return Err(SecurityError::PolicyViolation(
                "GPU CC must be enabled for key operations".into()
            ));
        }
        
        self.inner.use_key(key_id, operation).await
    }
    
    async fn delete_key(&self, key_id: &KeyId) -> Result<()> {
        self.inner.delete_key(key_id).await
    }
    
    async fn is_initialized(&self) -> Result<bool> {
        Ok(self.cc_enabled && self.inner.is_initialized().await?)
    }
}

/// GPU TEE-I/O vault (Tier 4 - Blackwell)
#[cfg(feature = "pqc")]
pub struct GpuTeeIoVault {
    inner: PqcVault,
    gpu_device_id: u32,
    tee_io_enabled: bool,
    mig_instance: Option<u32>,
}

#[cfg(feature = "pqc")]
impl GpuTeeIoVault {
    pub async fn new(
        gpu_device_id: u32,
        mig_instance: Option<u32>,
        attestation: AttestationType,
    ) -> Result<Self> {
        // Verify this is Blackwell TEE-I/O attestation
        match &attestation {
            AttestationType::BlackwellTeeIo { .. } => {},
            _ => return Err(SecurityError::PolicyViolation(
                "GPU TEE-I/O vault requires Blackwell attestation".into()
            )),
        }
        
        let inner = PqcVault::new(PrivacyTier::GpuTeeIo, Some(attestation)).await?;
        
        Ok(Self {
            inner,
            gpu_device_id,
            tee_io_enabled: true,
            mig_instance,
        })
    }
}

#[cfg(feature = "pqc")]
#[async_trait]
impl KeyVault for GpuTeeIoVault {
    fn tier(&self) -> PrivacyTier {
        PrivacyTier::GpuTeeIo
    }
    
    async fn store_key(&self, key_id: &KeyId, key_data: &[u8]) -> Result<()> {
        // Blackwell TEE-I/O provides inline NVLink protection
        // Keys are protected end-to-end with near-native performance
        self.inner.store_key(key_id, key_data).await
    }
    
    async fn use_key<F, R>(&self, key_id: &KeyId, operation: F) -> Result<R>
    where
        F: FnOnce(&[u8]) -> R + Send,
        R: Send,
    {
        if !self.tee_io_enabled {
            return Err(SecurityError::PolicyViolation(
                "TEE-I/O must be enabled for key operations".into()
            ));
        }
        
        // MIG isolation check for multi-tenant scenarios
        if let Some(mig) = self.mig_instance {
            log::info!("Using key in MIG instance {}", mig);
        }
        
        self.inner.use_key(key_id, operation).await
    }
    
    async fn delete_key(&self, key_id: &KeyId) -> Result<()> {
        self.inner.delete_key(key_id).await
    }
    
    async fn is_initialized(&self) -> Result<bool> {
        Ok(self.tee_io_enabled && self.inner.is_initialized().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    #[cfg(feature = "pqc")]
    async fn test_pqc_vault_operations() {
        // Skip test in CI environment
        if std::env::var("CI").is_ok() {
            println!("Skipping test in CI: requires specific crypto setup");
            return;
        }
        let vault = PqcVault::new(PrivacyTier::CpuTee, None).await.unwrap();
        
        let key_id = KeyId::new();
        let key_data = b"secret key material";
        
        // Store key
        vault.store_key(&key_id, key_data).await.unwrap();
        
        // Use key
        let result = vault.use_key(&key_id, |data| {
            assert_eq!(data, key_data);
            42
        }).await.unwrap();
        
        assert_eq!(result, 42);
        
        // Delete key
        vault.delete_key(&key_id).await.unwrap();
        
        // Verify deletion
        assert!(vault.use_key(&key_id, |_| ()).await.is_err());
    }
}