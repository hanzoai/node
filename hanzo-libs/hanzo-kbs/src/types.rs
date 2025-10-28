//! Core types for Hanzo Security

use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Privacy tiers for agent execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum PrivacyTier {
    /// Tier 0: Open - No privacy guarantees, suitable for public data
    Open = 0,
    
    /// Tier 1: At-rest encryption with SIM/FileVault
    AtRest = 1,
    
    /// Tier 2: CPU TEE (SEV-SNP, TDX)
    CpuTee = 2,
    
    /// Tier 3: CPU TEE + GPU Confidential Computing (H100 CC)
    GpuCc = 3,
    
    /// Tier 4: GPU TEE-I/O (Blackwell) - Maximum privacy
    GpuTeeIo = 4,
}

impl PrivacyTier {
    pub fn requires_attestation(&self) -> bool {
        *self >= PrivacyTier::CpuTee
    }
    
    pub fn requires_kbs(&self) -> bool {
        *self >= PrivacyTier::CpuTee
    }
}

/// Supported attestation types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AttestationType {
    /// AMD SEV-SNP attestation
    SevSnp {
        report: Vec<u8>,
        vcek_cert: Vec<u8>,
        platform_cert_chain: Vec<u8>,
    },
    
    /// Intel TDX attestation
    Tdx {
        quote: Vec<u8>,
        collateral: Vec<u8>,
    },
    
    /// NVIDIA H100 Confidential Computing
    H100Cc {
        gpu_attestation: Vec<u8>,
        cpu_attestation: Box<AttestationType>,
    },
    
    /// NVIDIA Blackwell TEE-I/O
    BlackwellTeeIo {
        tee_io_report: Vec<u8>,
        mig_config: Option<MigConfiguration>,
    },
    
    /// SIM card attestation
    SimEid {
        eid: String,
        signature: Vec<u8>,
    },
}

/// MIG (Multi-Instance GPU) configuration for Blackwell
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigConfiguration {
    pub instance_id: u32,
    pub memory_size_mb: u64,
    pub compute_units: u32,
}

/// Node security mode configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NodeSecurityMode {
    /// Software only - no hardware security
    SoftwareOnly,
    
    /// SIM card security only
    SimOnly,
    
    /// SIM + TEE security
    SimTee,
}

/// Key types in the hierarchy
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyId(pub Uuid);

impl KeyId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

/// Root Key - Top of the hierarchy
#[derive(Debug, Clone)]
pub struct RootKey {
    pub id: KeyId,
    pub created_at: DateTime<Utc>,
    pub hsm_handle: Option<String>,
}

/// Key Encryption Key (per tenant)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantKek {
    pub id: KeyId,
    pub tenant_id: String,
    pub wrapped_key: Vec<u8>, // Wrapped under root key
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// Data Encryption Key (per agent)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDek {
    pub id: KeyId,
    pub agent_id: String,
    pub tenant_id: String,
    pub wrapped_key: Vec<u8>, // Wrapped under tenant KEK
    pub created_at: DateTime<Utc>,
    pub rotation_due: Option<DateTime<Utc>>,
}

/// Session key for enclave use
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionKey {
    pub id: KeyId,
    pub agent_id: String,
    pub hpke_wrapped_key: Vec<u8>,
    pub enclave_public_key: Vec<u8>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub tier: PrivacyTier,
}

/// Key authorization request from KBS to KMS
#[derive(Debug, Serialize, Deserialize)]
pub struct KeyAuthorizationRequest {
    pub attestation: AttestationType,
    pub capability_token: CapabilityToken,
    pub session_public_key: Vec<u8>, // HPKE public key
    pub requested_keys: Vec<KeyRequest>,
    pub nonce: Vec<u8>,
}

/// Individual key request
#[derive(Debug, Serialize, Deserialize)]
pub struct KeyRequest {
    pub key_type: KeyRequestType,
    pub agent_id: String,
    pub tenant_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum KeyRequestType {
    TenantKek,
    AgentDek,
    SessionKey { duration_secs: u64 },
}

/// On-chain capability token for policy enforcement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityToken {
    pub id: String,
    pub subject: String, // Agent or tenant ID
    pub tier: PrivacyTier,
    pub permissions: Vec<String>,
    pub issued_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub chain_signature: Vec<u8>,
}

/// KBS authorization response
#[derive(Debug, Serialize, Deserialize)]
pub struct KeyAuthorizationResponse {
    pub session_id: Uuid,
    pub authorized_keys: Vec<AuthorizedKey>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AuthorizedKey {
    pub key_id: KeyId,
    pub hpke_wrapped_key: Vec<u8>,
    pub metadata: KeyMetadata,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct KeyMetadata {
    pub key_type: String,
    pub tier: PrivacyTier,
    pub restrictions: Vec<String>,
}

/// Audit log entry for key operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyAuditEntry {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub operation: KeyOperation,
    pub actor: String,
    pub key_id: Option<KeyId>,
    pub success: bool,
    pub details: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum KeyOperation {
    Create,
    Wrap,
    Unwrap,
    Rotate,
    Destroy,
    Authorize,
    Revoke,
}