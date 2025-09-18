//! In-memory KMS implementation for development and testing

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::error::{Result, SecurityError};
use crate::kms::{
    AuditFilter, CustomerKeyMetadata, CustomerKeyReference, HsmType, KeyInfo, KeyManagementService,
    KeyState, KeyType, KmsConfig, RootKeySource,
};
use crate::types::{
    AgentDek, KeyAuditEntry, KeyId, KeyOperation, RootKey, TenantKek,
};

/// In-memory KMS implementation
pub struct MemoryKms {
    root_key: Arc<RootKey>,
    root_key_material: Vec<u8>,
    tenant_keks: DashMap<String, (TenantKek, Vec<u8>)>, // tenant_id -> (metadata, key_material)
    agent_deks: DashMap<String, (AgentDek, Vec<u8>)>, // agent_id -> (metadata, key_material)
    key_info: DashMap<KeyId, KeyInfo>,
    audit_log: DashMap<Uuid, KeyAuditEntry>,
    config: KmsConfig,
}

impl MemoryKms {
    pub fn new() -> Self {
        let root_key_id = KeyId::new();
        let root_key = RootKey {
            id: root_key_id.clone(),
            created_at: Utc::now(),
            hsm_handle: None,
        };
        
        // Generate root key material
        let root_key_material = crate::kms::KeyHierarchy::generate_dek();
        
        let key_info = DashMap::new();
        key_info.insert(
            root_key_id.clone(),
            KeyInfo {
                key_id: root_key_id,
                key_type: KeyType::Root,
                created_at: Utc::now(),
                updated_at: Utc::now(),
                expires_at: None,
                rotation_due: None,
                state: KeyState::Active,
                metadata: serde_json::json!({}),
            },
        );
        
        Self {
            root_key: Arc::new(root_key),
            root_key_material,
            tenant_keks: DashMap::new(),
            agent_deks: DashMap::new(),
            key_info,
            audit_log: DashMap::new(),
            config: KmsConfig {
                hsm_type: HsmType::Software,
                root_key_source: RootKeySource::Generate,
                audit_retention_days: 30,
                key_rotation_days: Some(90),
            },
        }
    }
    
    fn log_audit(&self, operation: KeyOperation, key_id: Option<KeyId>, success: bool, details: serde_json::Value) {
        let entry = KeyAuditEntry {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            operation,
            actor: "system".to_string(),
            key_id,
            success,
            details,
        };
        
        self.audit_log.insert(entry.id, entry);
    }
    
    fn wrap_with_aes_kwp(&self, key_data: &[u8], kek: &[u8]) -> Result<Vec<u8>> {
        // Simplified wrapping - in production use proper AES-KWP
        use chacha20poly1305::{
            aead::{Aead, KeyInit},
            ChaCha20Poly1305, Nonce,
        };
        
        let cipher = ChaCha20Poly1305::new_from_slice(kek)
            .map_err(|e| SecurityError::CryptoError(e.to_string()))?;
        
        let nonce = Nonce::from_slice(b"unique nonce"); // In production, use random nonce
        
        cipher
            .encrypt(nonce, key_data)
            .map_err(|e| SecurityError::CryptoError(e.to_string()))
    }
    
    fn unwrap_with_aes_kwp(&self, wrapped: &[u8], kek: &[u8]) -> Result<Vec<u8>> {
        // Simplified unwrapping - in production use proper AES-KWP
        use chacha20poly1305::{
            aead::{Aead, KeyInit},
            ChaCha20Poly1305, Nonce,
        };
        
        let cipher = ChaCha20Poly1305::new_from_slice(kek)
            .map_err(|e| SecurityError::CryptoError(e.to_string()))?;
        
        let nonce = Nonce::from_slice(b"unique nonce");
        
        cipher
            .decrypt(nonce, wrapped)
            .map_err(|e| SecurityError::CryptoError(e.to_string()))
    }
}

#[async_trait]
impl KeyManagementService for MemoryKms {
    async fn initialize(&mut self, config: KmsConfig) -> Result<()> {
        self.config = config;
        self.log_audit(
            KeyOperation::Create,
            Some(self.root_key.id.clone()),
            true,
            serde_json::json!({ "type": "initialization" }),
        );
        Ok(())
    }
    
    async fn create_tenant_kek(&self, tenant_id: &str) -> Result<TenantKek> {
        // Derive KEK from root key
        let kek_material = crate::kms::KeyHierarchy::derive_tenant_kek(&self.root_key_material, tenant_id);
        
        // Wrap KEK under root key
        let wrapped_kek = self.wrap_with_aes_kwp(&kek_material, &self.root_key_material)?;
        
        let kek_id = KeyId::new();
        let kek = TenantKek {
            id: kek_id.clone(),
            tenant_id: tenant_id.to_string(),
            wrapped_key: wrapped_kek,
            created_at: Utc::now(),
            expires_at: None,
        };
        
        // Store metadata
        self.tenant_keks.insert(tenant_id.to_string(), (kek.clone(), kek_material));
        
        self.key_info.insert(
            kek_id.clone(),
            KeyInfo {
                key_id: kek_id.clone(),
                key_type: KeyType::TenantKek { tenant_id: tenant_id.to_string() },
                created_at: kek.created_at,
                updated_at: kek.created_at,
                expires_at: kek.expires_at,
                rotation_due: self.config.key_rotation_days
                    .map(|days| kek.created_at + chrono::Duration::days(days as i64)),
                state: KeyState::Active,
                metadata: serde_json::json!({ "tenant_id": tenant_id }),
            },
        );
        
        self.log_audit(
            KeyOperation::Create,
            Some(kek_id),
            true,
            serde_json::json!({ "type": "tenant_kek", "tenant_id": tenant_id }),
        );
        
        Ok(kek)
    }
    
    async fn create_agent_dek(&self, agent_id: &str, tenant_id: &str) -> Result<AgentDek> {
        // Get tenant KEK
        let (_, tenant_kek_material) = self.tenant_keks.get(tenant_id)
            .ok_or_else(|| SecurityError::KeyNotFound(format!("Tenant KEK not found: {}", tenant_id)))?
            .clone();
        
        // Generate new DEK
        let dek_material = crate::kms::KeyHierarchy::generate_dek();
        
        // Wrap DEK under tenant KEK
        let wrapped_dek = self.wrap_with_aes_kwp(&dek_material, &tenant_kek_material)?;
        
        let dek_id = KeyId::new();
        let dek = AgentDek {
            id: dek_id.clone(),
            agent_id: agent_id.to_string(),
            tenant_id: tenant_id.to_string(),
            wrapped_key: wrapped_dek,
            created_at: Utc::now(),
            rotation_due: self.config.key_rotation_days
                .map(|days| Utc::now() + chrono::Duration::days(days as i64)),
        };
        
        // Store metadata
        self.agent_deks.insert(agent_id.to_string(), (dek.clone(), dek_material));
        
        self.key_info.insert(
            dek_id.clone(),
            KeyInfo {
                key_id: dek_id.clone(),
                key_type: KeyType::AgentDek { 
                    agent_id: agent_id.to_string(),
                    tenant_id: tenant_id.to_string(),
                },
                created_at: dek.created_at,
                updated_at: dek.created_at,
                expires_at: None,
                rotation_due: dek.rotation_due,
                state: KeyState::Active,
                metadata: serde_json::json!({ 
                    "agent_id": agent_id,
                    "tenant_id": tenant_id,
                }),
            },
        );
        
        self.log_audit(
            KeyOperation::Create,
            Some(dek_id),
            true,
            serde_json::json!({ 
                "type": "agent_dek",
                "agent_id": agent_id,
                "tenant_id": tenant_id,
            }),
        );
        
        Ok(dek)
    }
    
    async fn wrap_key(&self, key_data: &[u8], parent_key_id: &KeyId) -> Result<Vec<u8>> {
        // Find parent key material
        let parent_key_material = if *parent_key_id == self.root_key.id {
            self.root_key_material.clone()
        } else {
            return Err(SecurityError::KeyNotFound(format!("Parent key not found: {:?}", parent_key_id)));
        };
        
        let wrapped = self.wrap_with_aes_kwp(key_data, &parent_key_material)?;
        
        self.log_audit(
            KeyOperation::Wrap,
            Some(parent_key_id.clone()),
            true,
            serde_json::json!({ "wrapped_size": wrapped.len() }),
        );
        
        Ok(wrapped)
    }
    
    async fn unwrap_key(&self, wrapped_key: &[u8], parent_key_id: &KeyId) -> Result<Vec<u8>> {
        // Find parent key material
        let parent_key_material = if *parent_key_id == self.root_key.id {
            self.root_key_material.clone()
        } else {
            return Err(SecurityError::KeyNotFound(format!("Parent key not found: {:?}", parent_key_id)));
        };
        
        let unwrapped = self.unwrap_with_aes_kwp(wrapped_key, &parent_key_material)?;
        
        self.log_audit(
            KeyOperation::Unwrap,
            Some(parent_key_id.clone()),
            true,
            serde_json::json!({ "unwrapped_size": unwrapped.len() }),
        );
        
        Ok(unwrapped)
    }
    
    async fn rotate_key(&self, key_id: &KeyId) -> Result<KeyId> {
        // TODO: Implement key rotation
        self.log_audit(
            KeyOperation::Rotate,
            Some(key_id.clone()),
            false,
            serde_json::json!({ "error": "not implemented" }),
        );
        
        Err(SecurityError::Other(anyhow::anyhow!("Key rotation not implemented")))
    }
    
    async fn destroy_key(&self, key_id: &KeyId) -> Result<()> {
        if let Some(mut info) = self.key_info.get_mut(key_id) {
            info.state = KeyState::Destroyed;
            info.updated_at = Utc::now();
        }
        
        self.log_audit(
            KeyOperation::Destroy,
            Some(key_id.clone()),
            true,
            serde_json::json!({}),
        );
        
        Ok(())
    }
    
    async fn get_key_metadata(&self, key_id: &KeyId) -> Result<KeyInfo> {
        self.key_info.get(key_id)
            .map(|info| info.value().clone())
            .ok_or_else(|| SecurityError::KeyNotFound(format!("Key not found: {:?}", key_id)))
    }
    
    async fn get_audit_logs(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        filter: Option<AuditFilter>,
    ) -> Result<Vec<KeyAuditEntry>> {
        let mut logs: Vec<KeyAuditEntry> = self.audit_log.iter()
            .filter(|entry| entry.timestamp >= start && entry.timestamp <= end)
            .map(|entry| entry.value().clone())
            .collect();
        
        if let Some(f) = filter {
            logs.retain(|log| {
                let mut keep = true;
                
                if let Some(ref key_ids) = f.key_ids {
                    keep &= log.key_id.as_ref().map(|id| key_ids.contains(id)).unwrap_or(false);
                }
                
                if let Some(ref ops) = f.operations {
                    keep &= ops.iter().any(|op| std::mem::discriminant(op) == std::mem::discriminant(&log.operation));
                }
                
                if let Some(ref actors) = f.actors {
                    keep &= actors.contains(&log.actor);
                }
                
                if let Some(success_only) = f.success_only {
                    keep &= log.success == success_only;
                }
                
                keep
            });
        }
        
        logs.sort_by_key(|log| log.timestamp);
        Ok(logs)
    }
    
    async fn import_customer_key(
        &self,
        tenant_id: &str,
        wrapped_key: &[u8],
        key_metadata: CustomerKeyMetadata,
    ) -> Result<KeyId> {
        // TODO: Implement customer key import
        let key_id = KeyId::new();
        
        self.log_audit(
            KeyOperation::Create,
            Some(key_id.clone()),
            false,
            serde_json::json!({ 
                "type": "customer_key_import",
                "tenant_id": tenant_id,
                "error": "not implemented",
            }),
        );
        
        Err(SecurityError::Other(anyhow::anyhow!("Customer key import not implemented")))
    }
    
    async fn register_customer_held_key(
        &self,
        tenant_id: &str,
        key_reference: CustomerKeyReference,
    ) -> Result<KeyId> {
        // TODO: Implement HYOK registration
        let key_id = KeyId::new();
        
        self.log_audit(
            KeyOperation::Create,
            Some(key_id.clone()),
            false,
            serde_json::json!({ 
                "type": "hyok_registration",
                "tenant_id": tenant_id,
                "provider": key_reference.provider,
                "error": "not implemented",
            }),
        );
        
        Err(SecurityError::Other(anyhow::anyhow!("HYOK registration not implemented")))
    }
}