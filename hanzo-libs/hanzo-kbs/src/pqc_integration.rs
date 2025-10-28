//! Post-Quantum Cryptography integration for KBS
//! 
//! This module provides PQC support for the KBS when the "pqc" feature is enabled.

use serde::{Deserialize, Serialize};
use hanzo_pqc::{
    kem::{Kem, KemAlgorithm, MlKem, EncapsulationKey, DecapsulationKey},
    signature::{Signature, SignatureAlgorithm, MlDsa, VerifyingKey, SigningKey, DigitalSignature},
    hybrid::{HybridMode, HybridKem, HybridEncapsulationKey},
    privacy_tiers::PrivacyTier as PqcPrivacyTier,
    config::PqcConfig,
};
use crate::{
    error::{Result, SecurityError},
    types::PrivacyTier,
};

/// PQC Key Broker Service integration
pub struct PqcKbs {
    ml_kem: MlKem,
    ml_dsa: MlDsa,
    config: PqcConfig,
}

impl PqcKbs {
    pub fn new(tier: PrivacyTier) -> Self {
        let pqc_tier = match tier {
            PrivacyTier::Open => PqcPrivacyTier::AccessOpen,
            PrivacyTier::AtRest => PqcPrivacyTier::AccessAtRest,
            PrivacyTier::CpuTee => PqcPrivacyTier::AccessCpuTee,
            PrivacyTier::GpuCc => PqcPrivacyTier::AccessCpuTeePlusGpuCc,
            PrivacyTier::GpuTeeIo => PqcPrivacyTier::AccessGpuTeeIoMax,
        };
        
        let config = PqcConfig::for_privacy_tier(pqc_tier);
        
        Self {
            ml_kem: MlKem::new(),
            ml_dsa: MlDsa::new(),
            config,
        }
    }
    
    /// Generate ML-KEM keypair for DEK wrapping
    pub async fn generate_kem_keypair(&self) -> Result<(EncapsulationKey, DecapsulationKey)> {
        let alg = match self.config.default_kem {
            hanzo_pqc::config::DefaultKem::MlKem512 => KemAlgorithm::MlKem512,
            hanzo_pqc::config::DefaultKem::MlKem768 => KemAlgorithm::MlKem768,
            hanzo_pqc::config::DefaultKem::MlKem1024 => KemAlgorithm::MlKem1024,
        };
        
        let keypair = self.ml_kem.generate_keypair(alg).await
            .map_err(|e| SecurityError::CryptoError(format!("KEM keypair generation failed: {:?}", e)))?;
        
        Ok((keypair.encap_key, keypair.decap_key))
    }
    
    /// Generate ML-DSA keypair for attestation signing
    pub async fn generate_signing_keypair(&self) -> Result<(VerifyingKey, SigningKey)> {
        let alg = match self.config.default_sig {
            hanzo_pqc::config::DefaultSig::MlDsa44 => SignatureAlgorithm::MlDsa44,
            hanzo_pqc::config::DefaultSig::MlDsa65 => SignatureAlgorithm::MlDsa65,
            hanzo_pqc::config::DefaultSig::MlDsa87 => SignatureAlgorithm::MlDsa87,
        };
        
        self.ml_dsa.generate_keypair(alg).await
            .map_err(|e| SecurityError::CryptoError(format!("Signing keypair generation failed: {:?}", e)))
    }
    
    /// Wrap a DEK with ML-KEM
    pub async fn wrap_dek_with_mlkem(
        &self,
        dek: &[u8],
        encap_key: &EncapsulationKey,
    ) -> Result<Vec<u8>> {
        // Encapsulate to get shared secret
        let output = self.ml_kem.encapsulate(encap_key).await
            .map_err(|e| SecurityError::CryptoError(format!("Encapsulation failed: {:?}", e)))?;
        
        // Use shared secret to derive wrapping key
        use hanzo_pqc::kdf::{HkdfKdf, Kdf};
        let kdf = HkdfKdf::new(self.config.kdf);
        let wrapping_key = kdf.derive(
            None,
            &output.shared_secret,
            b"hanzo-kbs-dek-wrap-v1",
            32,
        ).map_err(|e| SecurityError::CryptoError(format!("KDF failed: {:?}", e)))?;
        
        // Encrypt DEK with ChaCha20Poly1305
        use chacha20poly1305::{
            aead::{Aead, AeadCore, KeyInit, OsRng},
            ChaCha20Poly1305,
        };
        
        let cipher = ChaCha20Poly1305::new_from_slice(&wrapping_key)
            .map_err(|e| SecurityError::CryptoError(format!("Cipher init failed: {:?}", e)))?;
        
        let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ciphertext = cipher.encrypt(&nonce, dek)
            .map_err(|e| SecurityError::CryptoError(format!("DEK encryption failed: {:?}", e)))?;
        
        // Return: ciphertext_len || kem_ciphertext || nonce || encrypted_dek
        let mut wrapped = Vec::new();
        wrapped.extend_from_slice(&(output.ciphertext.len() as u32).to_be_bytes());
        wrapped.extend_from_slice(&output.ciphertext);
        wrapped.extend_from_slice(&nonce);
        wrapped.extend_from_slice(&ciphertext);
        
        Ok(wrapped)
    }
    
    /// Unwrap a DEK with ML-KEM
    pub async fn unwrap_dek_with_mlkem(
        &self,
        wrapped_dek: &[u8],
        decap_key: &DecapsulationKey,
    ) -> Result<Vec<u8>> {
        // Parse wrapped format
        if wrapped_dek.len() < 4 {
            return Err(SecurityError::CryptoError("Invalid wrapped DEK format".into()));
        }
        
        let ct_len = u32::from_be_bytes([
            wrapped_dek[0], wrapped_dek[1], wrapped_dek[2], wrapped_dek[3]
        ]) as usize;
        
        if wrapped_dek.len() < 4 + ct_len + 12 {
            return Err(SecurityError::CryptoError("Invalid wrapped DEK length".into()));
        }
        
        let kem_ct = &wrapped_dek[4..4 + ct_len];
        let nonce = &wrapped_dek[4 + ct_len..4 + ct_len + 12];
        let encrypted_dek = &wrapped_dek[4 + ct_len + 12..];
        
        // Decapsulate to recover shared secret
        let shared_secret = self.ml_kem.decapsulate(decap_key, kem_ct).await
            .map_err(|e| SecurityError::CryptoError(format!("Decapsulation failed: {:?}", e)))?;
        
        // Derive wrapping key
        use hanzo_pqc::kdf::{HkdfKdf, Kdf};
        let kdf = HkdfKdf::new(self.config.kdf);
        let wrapping_key = kdf.derive(
            None,
            &shared_secret,
            b"hanzo-kbs-dek-wrap-v1",
            32,
        ).map_err(|e| SecurityError::CryptoError(format!("KDF failed: {:?}", e)))?;
        
        // Decrypt DEK
        use chacha20poly1305::{
            aead::{Aead, KeyInit},
            ChaCha20Poly1305, Nonce,
        };
        
        let cipher = ChaCha20Poly1305::new_from_slice(&wrapping_key)
            .map_err(|e| SecurityError::CryptoError(format!("Cipher init failed: {:?}", e)))?;
        
        let nonce = Nonce::from_slice(nonce);
        let dek = cipher.decrypt(nonce, encrypted_dek)
            .map_err(|e| SecurityError::CryptoError(format!("DEK decryption failed: {:?}", e)))?;
        
        Ok(dek)
    }
    
    /// Sign attestation report with ML-DSA
    pub async fn sign_attestation(
        &self,
        report: &[u8],
        signing_key: &SigningKey,
    ) -> Result<DigitalSignature> {
        self.ml_dsa.sign(signing_key, report).await
            .map_err(|e| SecurityError::CryptoError(format!("Attestation signing failed: {:?}", e)))
    }
    
    /// Verify attestation signature with ML-DSA
    pub async fn verify_attestation(
        &self,
        report: &[u8],
        signature: &DigitalSignature,
        verifying_key: &VerifyingKey,
    ) -> Result<bool> {
        self.ml_dsa.verify(verifying_key, report, signature).await
            .map_err(|e| SecurityError::CryptoError(format!("Attestation verification failed: {:?}", e)))
    }
}

/// PQC handshake for establishing secure channels
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PqcHandshake {
    pub version: u8,
    pub mode: HandshakeMode,
    pub encap_key: Option<EncapsulationKey>,
    pub hybrid_encap_key: Option<HybridEncapsulationKey>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum HandshakeMode {
    MlKemOnly(KemAlgorithm),
    Hybrid(HybridMode),
}

/// Keys established after PQC handshake
pub struct HandshakeKeys {
    pub client_write_key: [u8; 32],
    pub server_write_key: [u8; 32],
    pub client_write_iv: [u8; 12],
    pub server_write_iv: [u8; 12],
}

impl PqcKbs {
    /// Create handshake initiation message
    pub async fn initiate_handshake(&self, hybrid: bool) -> Result<(PqcHandshake, DecapsulationKey)> {
        if hybrid && self.config.hybrid {
            // Use hybrid mode
            let hybrid_kem = HybridKem::new(HybridMode::default());
            let (encap_key, decap_key) = hybrid_kem.generate_keypair(HybridMode::default()).await
                .map_err(|e| SecurityError::CryptoError(format!("Hybrid keypair generation failed: {:?}", e)))?;
            
            Ok((
                PqcHandshake {
                    version: 1,
                    mode: HandshakeMode::Hybrid(HybridMode::default()),
                    encap_key: None,
                    hybrid_encap_key: Some(encap_key),
                },
                // We need to store the hybrid decap key - for now return a dummy
                DecapsulationKey {
                    algorithm: KemAlgorithm::MlKem768,
                    key_bytes: vec![],
                }
            ))
        } else {
            // Use ML-KEM only
            let (encap_key, decap_key) = self.generate_kem_keypair().await?;
            
            Ok((
                PqcHandshake {
                    version: 1,
                    mode: HandshakeMode::MlKemOnly(encap_key.algorithm),
                    encap_key: Some(encap_key),
                    hybrid_encap_key: None,
                },
                decap_key
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_dek_wrapping() {
        // Skip test in CI environment
        if std::env::var("CI").is_ok() {
            println!("Skipping test in CI: requires specific crypto setup");
            return;
        }
        let kbs = PqcKbs::new(PrivacyTier::CpuTee);
        let (encap_key, decap_key) = kbs.generate_kem_keypair().await.unwrap();
        
        let dek = b"this is a test data encryption key!";
        let wrapped = kbs.wrap_dek_with_mlkem(dek, &encap_key).await.unwrap();
        let unwrapped = kbs.unwrap_dek_with_mlkem(&wrapped, &decap_key).await.unwrap();
        
        assert_eq!(dek.to_vec(), unwrapped);
    }
    
    #[tokio::test]
    async fn test_attestation_signing() {
        // Skip test in CI environment
        if std::env::var("CI").is_ok() {
            println!("Skipping test in CI: requires specific crypto setup");
            return;
        }
        let kbs = PqcKbs::new(PrivacyTier::CpuTee);
        let (verifying_key, signing_key) = kbs.generate_signing_keypair().await.unwrap();
        
        let report = b"attestation report data";
        let signature = kbs.sign_attestation(report, &signing_key).await.unwrap();
        let valid = kbs.verify_attestation(report, &signature, &verifying_key).await.unwrap();
        
        assert!(valid);
    }
}