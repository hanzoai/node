//! Key Broker Service (KBS) trait and implementations

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{Result, SecurityError};
use crate::types::*;
use crate::kms::KeyManagementService;
use crate::attestation::AttestationVerifier;

/// Key Broker Service trait - handles attestation and policy-based key release
#[async_trait]
pub trait KeyBrokerService: Send + Sync {
    /// Authorize key release based on attestation and policy
    async fn authorize(
        &self,
        request: KeyAuthorizationRequest,
    ) -> Result<KeyAuthorizationResponse>;
    
    /// Renew an existing session with fresh attestation
    async fn renew(
        &self,
        session_id: Uuid,
        attestation: AttestationType,
    ) -> Result<RenewResponse>;
    
    /// Revoke a session (admin or chain-triggered)
    async fn revoke(&self, session_id: Uuid, reason: RevocationReason) -> Result<()>;
    
    /// Get session status
    async fn get_session_status(&self, session_id: Uuid) -> Result<SessionStatus>;
    
    /// Get policy for a given tier
    async fn get_tier_policy(&self, tier: PrivacyTier) -> Result<TierPolicy>;
}

/// KBS implementation that connects to KMS
pub struct HanzoKbs<K: KeyManagementService, V: AttestationVerifier> {
    kms: K,
    verifier: V,
    config: KbsConfig,
    sessions: dashmap::DashMap<Uuid, SessionInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KbsConfig {
    pub max_session_duration: Duration,
    pub attestation_cache_ttl: Duration,
    pub rate_limit_per_minute: u32,
    pub require_chain_verification: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RenewResponse {
    pub session_id: Uuid,
    pub new_expires_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum RevocationReason {
    AdminAction { admin_id: String, reason: String },
    ChainTriggered { transaction_hash: String },
    PolicyViolation { details: String },
    Expired,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionStatus {
    pub session_id: Uuid,
    pub agent_id: String,
    pub tier: PrivacyTier,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub renewals: u32,
    pub active: bool,
}

#[derive(Debug, Clone)]
struct SessionInfo {
    pub agent_id: String,
    pub tier: PrivacyTier,
    pub enclave_public_key: Vec<u8>,
    pub authorized_keys: Vec<KeyId>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub renewals: u32,
    pub active: bool,
}

/// Policy for each privacy tier
#[derive(Debug, Serialize, Deserialize)]
pub struct TierPolicy {
    pub tier: PrivacyTier,
    pub required_attestations: Vec<AttestationRequirement>,
    pub max_session_duration: Duration,
    pub allowed_operations: Vec<String>,
    pub key_restrictions: Vec<KeyRestriction>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AttestationRequirement {
    pub attestation_type: String,
    pub min_tcb_version: Option<String>,
    pub allowed_measurements: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct KeyRestriction {
    pub key_type: String,
    pub max_usage_count: Option<u64>,
    pub require_audit: bool,
}

#[async_trait]
impl<K: KeyManagementService, V: AttestationVerifier> KeyBrokerService for HanzoKbs<K, V> {
    async fn authorize(
        &self,
        request: KeyAuthorizationRequest,
    ) -> Result<KeyAuthorizationResponse> {
        // Rate limiting check
        if !self.check_rate_limit(&request.capability_token.subject).await? {
            return Err(SecurityError::RateLimitExceeded);
        }
        
        // Verify attestation
        let attestation_result = self.verifier
            .verify_attestation(&request.attestation)
            .await?;
        
        // Check tier compatibility
        let requested_tier = request.capability_token.tier;
        if !attestation_result.supports_tier(requested_tier) {
            return Err(SecurityError::TierMismatch {
                requested: requested_tier as u8,
                available: attestation_result.max_tier as u8,
            });
        }
        
        // Verify capability token (on-chain if configured)
        if self.config.require_chain_verification {
            self.verify_capability_token(&request.capability_token).await?;
        }
        
        // Get tier policy
        let policy = self.get_tier_policy(requested_tier).await?;
        
        // Authorize requested keys
        let mut authorized_keys = Vec::new();
        for key_request in &request.requested_keys {
            let authorized_key = self.authorize_key(
                &key_request,
                &request.session_public_key,
                requested_tier,
                &policy,
            ).await?;
            authorized_keys.push(authorized_key);
        }
        
        // Create session
        let session_id = Uuid::new_v4();
        let expires_at = Utc::now() + self.config.max_session_duration;
        
        self.sessions.insert(session_id, SessionInfo {
            agent_id: request.capability_token.subject.clone(),
            tier: requested_tier,
            enclave_public_key: request.session_public_key.clone(),
            authorized_keys: authorized_keys.iter().map(|k| k.key_id.clone()).collect(),
            created_at: Utc::now(),
            expires_at,
            renewals: 0,
            active: true,
        });
        
        Ok(KeyAuthorizationResponse {
            session_id,
            authorized_keys,
            expires_at,
        })
    }
    
    async fn renew(
        &self,
        session_id: Uuid,
        attestation: AttestationType,
    ) -> Result<RenewResponse> {
        let mut session = self.sessions.get_mut(&session_id)
            .ok_or_else(|| SecurityError::SessionExpired)?;
        
        if !session.active || session.expires_at < Utc::now() {
            return Err(SecurityError::SessionExpired);
        }
        
        // Verify fresh attestation
        let attestation_result = self.verifier.verify_attestation(&attestation).await?;
        if !attestation_result.supports_tier(session.tier) {
            return Err(SecurityError::InvalidAttestation(
                "Attestation no longer supports required tier".to_string()
            ));
        }
        
        // Update session
        session.expires_at = Utc::now() + self.config.max_session_duration;
        session.renewals += 1;
        
        Ok(RenewResponse {
            session_id,
            new_expires_at: session.expires_at,
        })
    }
    
    async fn revoke(&self, session_id: Uuid, reason: RevocationReason) -> Result<()> {
        if let Some(mut session) = self.sessions.get_mut(&session_id) {
            session.active = false;
            
            // Log revocation
            log::info!(
                "Session {} revoked for agent {}: {:?}",
                session_id, session.agent_id, reason
            );
        }
        
        Ok(())
    }
    
    async fn get_session_status(&self, session_id: Uuid) -> Result<SessionStatus> {
        let session = self.sessions.get(&session_id)
            .ok_or_else(|| SecurityError::KeyNotFound("Session not found".to_string()))?;
        
        Ok(SessionStatus {
            session_id,
            agent_id: session.agent_id.clone(),
            tier: session.tier,
            created_at: session.created_at,
            expires_at: session.expires_at,
            renewals: session.renewals,
            active: session.active && session.expires_at > Utc::now(),
        })
    }
    
    async fn get_tier_policy(&self, tier: PrivacyTier) -> Result<TierPolicy> {
        // This would typically load from configuration or database
        Ok(match tier {
            PrivacyTier::Open => TierPolicy {
                tier,
                required_attestations: vec![],
                max_session_duration: Duration::hours(24),
                allowed_operations: vec!["*".to_string()],
                key_restrictions: vec![],
            },
            PrivacyTier::AtRest => TierPolicy {
                tier,
                required_attestations: vec![],
                max_session_duration: Duration::hours(12),
                allowed_operations: vec!["read".to_string(), "write".to_string()],
                key_restrictions: vec![],
            },
            PrivacyTier::CpuTee => TierPolicy {
                tier,
                required_attestations: vec![
                    AttestationRequirement {
                        attestation_type: "SevSnp".to_string(),
                        min_tcb_version: Some("1.0".to_string()),
                        allowed_measurements: None,
                    },
                ],
                max_session_duration: Duration::hours(4),
                allowed_operations: vec!["compute".to_string()],
                key_restrictions: vec![
                    KeyRestriction {
                        key_type: "AgentDek".to_string(),
                        max_usage_count: Some(1000),
                        require_audit: true,
                    },
                ],
            },
            PrivacyTier::GpuCc => TierPolicy {
                tier,
                required_attestations: vec![
                    AttestationRequirement {
                        attestation_type: "H100Cc".to_string(),
                        min_tcb_version: Some("2.0".to_string()),
                        allowed_measurements: None,
                    },
                ],
                max_session_duration: Duration::hours(2),
                allowed_operations: vec!["gpu_compute".to_string()],
                key_restrictions: vec![
                    KeyRestriction {
                        key_type: "AgentDek".to_string(),
                        max_usage_count: Some(100),
                        require_audit: true,
                    },
                ],
            },
            PrivacyTier::GpuTeeIo => TierPolicy {
                tier,
                required_attestations: vec![
                    AttestationRequirement {
                        attestation_type: "BlackwellTeeIo".to_string(),
                        min_tcb_version: Some("1.0".to_string()),
                        allowed_measurements: None,
                    },
                ],
                max_session_duration: Duration::hours(1),
                allowed_operations: vec!["secure_inference".to_string()],
                key_restrictions: vec![
                    KeyRestriction {
                        key_type: "AgentDek".to_string(),
                        max_usage_count: Some(10),
                        require_audit: true,
                    },
                ],
            },
        })
    }
}

impl<K: KeyManagementService, V: AttestationVerifier> HanzoKbs<K, V> {
    pub fn new(kms: K, verifier: V, config: KbsConfig) -> Self {
        Self {
            kms,
            verifier,
            config,
            sessions: dashmap::DashMap::new(),
        }
    }
    
    async fn check_rate_limit(&self, subject: &str) -> Result<bool> {
        // TODO: Implement proper rate limiting with Redis or in-memory cache
        Ok(true)
    }
    
    async fn verify_capability_token(&self, token: &CapabilityToken) -> Result<()> {
        // TODO: Verify on-chain signature
        if token.expires_at.map(|exp| exp < Utc::now()).unwrap_or(false) {
            return Err(SecurityError::PolicyViolation("Token expired".to_string()));
        }
        Ok(())
    }
    
    async fn authorize_key(
        &self,
        request: &KeyRequest,
        enclave_public_key: &[u8],
        tier: PrivacyTier,
        policy: &TierPolicy,
    ) -> Result<AuthorizedKey> {
        // TODO: Implement HPKE wrapping
        // For now, return a mock authorized key
        Ok(AuthorizedKey {
            key_id: KeyId::new(),
            hpke_wrapped_key: vec![0; 32], // Mock wrapped key
            metadata: KeyMetadata {
                key_type: format!("{:?}", request.key_type),
                tier,
                restrictions: policy.key_restrictions.iter()
                    .map(|r| format!("{}: max_usage={:?}", r.key_type, r.max_usage_count))
                    .collect(),
            },
        })
    }
}