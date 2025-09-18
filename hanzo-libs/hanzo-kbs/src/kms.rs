//! Key Management Service (KMS) trait and implementations

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::types::*;

// Declare submodules
pub mod api;
pub mod memory_kms;

/// Key Management Service trait - handles key lifecycle and storage
#[async_trait]
pub trait KeyManagementService: Send + Sync {
    /// Initialize the KMS with root key
    async fn initialize(&mut self, config: KmsConfig) -> Result<()>;
    
    /// Create a new tenant KEK
    async fn create_tenant_kek(&self, tenant_id: &str) -> Result<TenantKek>;
    
    /// Create a new agent DEK
    async fn create_agent_dek(&self, agent_id: &str, tenant_id: &str) -> Result<AgentDek>;
    
    /// Wrap a key under a parent key
    async fn wrap_key(&self, key_data: &[u8], parent_key_id: &KeyId) -> Result<Vec<u8>>;
    
    /// Unwrap a key (internal use only, never exposed to KBS)
    async fn unwrap_key(&self, wrapped_key: &[u8], parent_key_id: &KeyId) -> Result<Vec<u8>>;
    
    /// Rotate a key
    async fn rotate_key(&self, key_id: &KeyId) -> Result<KeyId>;
    
    /// Destroy a key
    async fn destroy_key(&self, key_id: &KeyId) -> Result<()>;
    
    /// Get key metadata
    async fn get_key_metadata(&self, key_id: &KeyId) -> Result<KeyInfo>;
    
    /// Get audit logs
    async fn get_audit_logs(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        filter: Option<AuditFilter>,
    ) -> Result<Vec<KeyAuditEntry>>;
    
    /// BYOK: Import customer key
    async fn import_customer_key(
        &self,
        tenant_id: &str,
        wrapped_key: &[u8],
        key_metadata: CustomerKeyMetadata,
    ) -> Result<KeyId>;
    
    /// HYOK: Register customer-held key reference
    async fn register_customer_held_key(
        &self,
        tenant_id: &str,
        key_reference: CustomerKeyReference,
    ) -> Result<KeyId>;
}

/// KMS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KmsConfig {
    pub hsm_type: HsmType,
    pub root_key_source: RootKeySource,
    pub audit_retention_days: u32,
    pub key_rotation_days: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HsmType {
    Software,
    AwsCloudHsm { cluster_id: String },
    AzureKeyVault { vault_url: String },
    HashicorpVault { url: String, namespace: Option<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RootKeySource {
    Generate,
    Import { wrapped_key: Vec<u8> },
    Existing { key_id: String },
}

/// Extended key information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyInfo {
    pub key_id: KeyId,
    pub key_type: KeyType,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub rotation_due: Option<DateTime<Utc>>,
    pub state: KeyState,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum KeyType {
    Root,
    TenantKek { tenant_id: String },
    AgentDek { agent_id: String, tenant_id: String },
    CustomerKey { tenant_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum KeyState {
    Active,
    Rotating,
    Expired,
    Destroyed,
}

/// Audit log filter
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AuditFilter {
    pub key_ids: Option<Vec<KeyId>>,
    pub operations: Option<Vec<KeyOperation>>,
    pub actors: Option<Vec<String>>,
    pub success_only: Option<bool>,
}

/// Customer key metadata for BYOK
#[derive(Debug, Serialize, Deserialize)]
pub struct CustomerKeyMetadata {
    pub algorithm: String,
    pub key_size: u32,
    pub usage: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// Customer key reference for HYOK
#[derive(Debug, Serialize, Deserialize)]
pub struct CustomerKeyReference {
    pub provider: String,
    pub key_id: String,
    pub region: Option<String>,
    pub permissions: Vec<String>,
}

/// Key wrapping algorithms supported
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum KeyWrapAlgorithm {
    AesKwp256,
    Rsa4096OaepSha256,
    ChaCha20Poly1305,
}

/// Key hierarchy operations
pub struct KeyHierarchy;

impl KeyHierarchy {
    /// Derive a KEK from root key and tenant salt
    pub fn derive_tenant_kek(root_key: &[u8], tenant_id: &str) -> Vec<u8> {
        let salt = format!("tenant_kek_{}", tenant_id);
        let mut hasher = blake3::Hasher::new_derive_key(&salt);
        hasher.update(root_key);
        let mut output = vec![0u8; 32];
        hasher.finalize_xof().fill(&mut output);
        output
    }
    
    /// Generate a random DEK
    pub fn generate_dek() -> Vec<u8> {
        use rand::RngCore;
        let mut key = vec![0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);
        key
    }
}