//! Vault implementations for different privacy tiers

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::error::{Result, SecurityError};
use crate::types::{PrivacyTier, KeyId, SessionKey, AttestationType};
use crate::kms::KeyManagementService;
use crate::kbs::KeyBrokerService;

/// Trait for secure key storage and usage
#[async_trait]
pub trait KeyVault: Send + Sync {
    /// Get the privacy tier this vault supports
    fn tier(&self) -> PrivacyTier;
    
    /// Store a key in the vault
    async fn store_key(&self, key_id: &KeyId, key_data: &[u8]) -> Result<()>;
    
    /// Use a key for an operation (key never leaves vault in plaintext)
    async fn use_key<F, R>(&self, key_id: &KeyId, operation: F) -> Result<R>
    where
        F: FnOnce(&[u8]) -> R + Send,
        R: Send;
    
    /// Delete a key from the vault
    async fn delete_key(&self, key_id: &KeyId) -> Result<()>;
    
    /// Check if vault is properly initialized
    async fn is_initialized(&self) -> Result<bool>;
}

/// Configuration for different vault types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultConfig {
    pub vault_type: VaultType,
    pub kms_url: Option<String>,
    pub kbs_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum VaultType {
    File { path: String },
    Sim { eid: String },
    GpuCc { device_id: u32 },
    GpuTeeIo { device_id: u32, mig_instance: Option<u32> },
}

/// Tier 1: File-based vault with at-rest encryption
pub struct FileVault<K: KeyManagementService> {
    kms: K,
    base_path: std::path::PathBuf,
    tenant_id: String,
}

impl<K: KeyManagementService> FileVault<K> {
    pub fn new(kms: K, base_path: impl AsRef<Path>, tenant_id: String) -> Self {
        Self {
            kms,
            base_path: base_path.as_ref().to_path_buf(),
            tenant_id,
        }
    }
}

#[async_trait]
impl<K: KeyManagementService> KeyVault for FileVault<K> {
    fn tier(&self) -> PrivacyTier {
        PrivacyTier::AtRest
    }
    
    async fn store_key(&self, key_id: &KeyId, key_data: &[u8]) -> Result<()> {
        // Wrap key with tenant KEK before storing
        let wrapped = self.kms.wrap_key(key_data, key_id).await?;
        
        let key_path = self.base_path.join(format!("{}.key", key_id.0));
        tokio::fs::create_dir_all(&self.base_path).await?;
        tokio::fs::write(&key_path, &wrapped).await?;
        
        Ok(())
    }
    
    async fn use_key<F, R>(&self, key_id: &KeyId, operation: F) -> Result<R>
    where
        F: FnOnce(&[u8]) -> R + Send,
        R: Send,
    {
        let key_path = self.base_path.join(format!("{}.key", key_id.0));
        let wrapped = tokio::fs::read(&key_path).await?;
        
        // Unwrap key for use
        let key_data = self.kms.unwrap_key(&wrapped, key_id).await?;
        
        // Use key and zeroize after
        let result = operation(&key_data);
        
        // Zeroize key data
        drop(key_data); // In production, use zeroize crate
        
        Ok(result)
    }
    
    async fn delete_key(&self, key_id: &KeyId) -> Result<()> {
        let key_path = self.base_path.join(format!("{}.key", key_id.0));
        tokio::fs::remove_file(&key_path).await?;
        Ok(())
    }
    
    async fn is_initialized(&self) -> Result<bool> {
        Ok(self.base_path.exists())
    }
}

/// Tier 1+: SIM-based vault with hardware-bound keys
pub struct SimVault<K: KeyManagementService> {
    kms: K,
    sim_eid: String,
    file_vault: FileVault<K>, // Fallback storage
}

impl<K: KeyManagementService + Clone> SimVault<K> {
    pub fn new(kms: K, base_path: impl AsRef<Path>, tenant_id: String, sim_eid: String) -> Self {
        Self {
            kms: kms.clone(),
            sim_eid,
            file_vault: FileVault::new(kms, base_path, tenant_id),
        }
    }
    
    async fn bind_to_sim(&self, key_data: &[u8]) -> Result<Vec<u8>> {
        // TODO: Implement SIM binding using EID
        // For now, just add EID to key derivation
        let mut hasher = blake3::Hasher::new();
        hasher.update(key_data);
        hasher.update(self.sim_eid.as_bytes());
        Ok(hasher.finalize().as_bytes().to_vec())
    }
}

#[async_trait]
impl<K: KeyManagementService + Clone> KeyVault for SimVault<K> {
    fn tier(&self) -> PrivacyTier {
        PrivacyTier::AtRest // Still Tier 1, but with SIM binding
    }
    
    async fn store_key(&self, key_id: &KeyId, key_data: &[u8]) -> Result<()> {
        let sim_bound = self.bind_to_sim(key_data).await?;
        self.file_vault.store_key(key_id, &sim_bound).await
    }
    
    async fn use_key<F, R>(&self, key_id: &KeyId, operation: F) -> Result<R>
    where
        F: FnOnce(&[u8]) -> R + Send,
        R: Send,
    {
        self.file_vault.use_key(key_id, operation).await
    }
    
    async fn delete_key(&self, key_id: &KeyId) -> Result<()> {
        self.file_vault.delete_key(key_id).await
    }
    
    async fn is_initialized(&self) -> Result<bool> {
        // TODO: Check SIM availability
        self.file_vault.is_initialized().await
    }
}

/// Tier 3: GPU Confidential Computing vault
pub struct GpuCcVault<K: KeyBrokerService> {
    kbs: K,
    device_id: u32,
    session: Option<SessionKey>,
}

impl<K: KeyBrokerService> GpuCcVault<K> {
    pub fn new(kbs: K, device_id: u32) -> Self {
        Self {
            kbs,
            device_id,
            session: None,
        }
    }
    
    async fn ensure_session(&mut self, agent_id: &str) -> Result<()> {
        if let Some(ref session) = self.session {
            if session.expires_at > chrono::Utc::now() {
                return Ok(());
            }
        }
        
        // Need new session - create attestation
        let attestation = self.create_gpu_cc_attestation().await?;
        
        // Request authorization from KBS
        let auth_request = crate::types::KeyAuthorizationRequest {
            attestation,
            capability_token: self.create_capability_token(agent_id)?,
            session_public_key: self.generate_session_key()?,
            requested_keys: vec![
                crate::types::KeyRequest {
                    key_type: crate::types::KeyRequestType::SessionKey { duration_secs: 3600 },
                    agent_id: agent_id.to_string(),
                    tenant_id: None,
                },
            ],
            nonce: self.generate_nonce(),
        };
        
        let response = self.kbs.authorize(auth_request).await?;
        
        // Store session
        if let Some(authorized_key) = response.authorized_keys.first() {
            self.session = Some(SessionKey {
                id: authorized_key.key_id.clone(),
                agent_id: agent_id.to_string(),
                hpke_wrapped_key: authorized_key.hpke_wrapped_key.clone(),
                enclave_public_key: vec![], // TODO: Get from GPU
                created_at: chrono::Utc::now(),
                expires_at: response.expires_at,
                tier: PrivacyTier::GpuCc,
            });
            Ok(())
        } else {
            Err(SecurityError::KbsError("No session key received".to_string()))
        }
    }
    
    async fn create_gpu_cc_attestation(&self) -> Result<AttestationType> {
        // TODO: Get actual GPU attestation
        Ok(AttestationType::H100Cc {
            gpu_attestation: vec![0x01; 64], // Mock
            cpu_attestation: Box::new(AttestationType::SevSnp {
                report: vec![0x02; 128],
                vcek_cert: vec![],
                platform_cert_chain: vec![],
            }),
        })
    }
    
    fn create_capability_token(&self, agent_id: &str) -> Result<crate::types::CapabilityToken> {
        // TODO: Get from chain
        Ok(crate::types::CapabilityToken {
            id: uuid::Uuid::new_v4().to_string(),
            subject: agent_id.to_string(),
            tier: PrivacyTier::GpuCc,
            permissions: vec!["gpu_compute".to_string()],
            issued_at: chrono::Utc::now(),
            expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
            chain_signature: vec![],
        })
    }
    
    fn generate_session_key(&self) -> Result<Vec<u8>> {
        // TODO: Generate HPKE key pair
        Ok(vec![0x03; 32])
    }
    
    fn generate_nonce(&self) -> Vec<u8> {
        use rand::RngCore;
        let mut nonce = vec![0u8; 32];
        rand::thread_rng().fill_bytes(&mut nonce);
        nonce
    }
}

#[async_trait]
impl<K: KeyBrokerService> KeyVault for GpuCcVault<K> {
    fn tier(&self) -> PrivacyTier {
        PrivacyTier::GpuCc
    }
    
    async fn store_key(&self, _key_id: &KeyId, _key_data: &[u8]) -> Result<()> {
        // Keys are never stored locally in GPU CC mode
        Err(SecurityError::PolicyViolation(
            "GPU CC vault does not support local key storage".to_string()
        ))
    }
    
    async fn use_key<F, R>(&self, key_id: &KeyId, operation: F) -> Result<R>
    where
        F: FnOnce(&[u8]) -> R + Send,
        R: Send,
    {
        // TODO: Use HPKE-wrapped session key in GPU enclave
        // For now, return mock
        let mock_key = vec![0xFF; 32];
        Ok(operation(&mock_key))
    }
    
    async fn delete_key(&self, _key_id: &KeyId) -> Result<()> {
        // No local storage
        Ok(())
    }
    
    async fn is_initialized(&self) -> Result<bool> {
        // TODO: Check GPU availability and CC mode
        Ok(true)
    }
}

/// Tier 4: GPU TEE-I/O vault (Blackwell)
pub struct GpuTeeIoVault<K: KeyBrokerService> {
    kbs: K,
    device_id: u32,
    mig_instance: Option<u32>,
    session: Option<SessionKey>,
}

impl<K: KeyBrokerService> GpuTeeIoVault<K> {
    pub fn new(kbs: K, device_id: u32, mig_instance: Option<u32>) -> Self {
        Self {
            kbs,
            device_id,
            mig_instance,
            session: None,
        }
    }
}

// Implementation similar to GpuCcVault but with Blackwell-specific attestation
#[async_trait]
impl<K: KeyBrokerService> KeyVault for GpuTeeIoVault<K> {
    fn tier(&self) -> PrivacyTier {
        PrivacyTier::GpuTeeIo
    }
    
    // Similar implementation to GpuCcVault...
    async fn store_key(&self, _key_id: &KeyId, _key_data: &[u8]) -> Result<()> {
        Err(SecurityError::PolicyViolation(
            "GPU TEE-I/O vault does not support local key storage".to_string()
        ))
    }
    
    async fn use_key<F, R>(&self, _key_id: &KeyId, operation: F) -> Result<R>
    where
        F: FnOnce(&[u8]) -> R + Send,
        R: Send,
    {
        // TODO: Implement Blackwell-specific key usage
        let mock_key = vec![0xEE; 32];
        Ok(operation(&mock_key))
    }
    
    async fn delete_key(&self, _key_id: &KeyId) -> Result<()> {
        Ok(())
    }
    
    async fn is_initialized(&self) -> Result<bool> {
        Ok(true)
    }
}