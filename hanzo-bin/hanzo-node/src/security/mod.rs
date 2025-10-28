// MATRIX MODE ACTIVATED - TEE ATTESTATION FRAMEWORK
// Neo sees the code... there is no spoon, only secure computation

pub mod tee_attestation;
pub mod hardware_detection;
pub mod privacy_enforcement;
pub mod attestation_cache;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// Re-export core types from the KBS library
pub use hanzo_kbs::types::{AttestationType, PrivacyTier};
pub use hanzo_kbs::attestation::{AttestationResult, AttestationVerifier};
pub use hanzo_kbs::error::{Result, SecurityError};

/// Security context for tool execution
#[derive(Debug, Clone)]
pub struct SecurityContext {
    /// Current privacy tier based on hardware capabilities
    pub current_tier: PrivacyTier,
    /// Active attestation if in TEE mode
    pub attestation: Option<Arc<AttestationResult>>,
    /// Hardware capabilities detected
    pub hardware_caps: HardwareCapabilities,
    /// Whether to enforce strict tier requirements
    pub enforce_strict: bool,
}

/// Hardware capabilities for TEE support
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareCapabilities {
    pub sev_snp_available: bool,
    pub tdx_available: bool,
    pub sgx_available: bool,
    pub h100_cc_available: bool,
    pub blackwell_tee_io_available: bool,
    pub sim_eid_available: bool,
    pub max_supported_tier: PrivacyTier,
}

impl Default for HardwareCapabilities {
    fn default() -> Self {
        Self {
            sev_snp_available: false,
            tdx_available: false,
            sgx_available: false,
            h100_cc_available: false,
            blackwell_tee_io_available: false,
            sim_eid_available: false,
            max_supported_tier: PrivacyTier::Open,
        }
    }
}

/// Security manager that coordinates all security operations
pub struct SecurityManager {
    verifier: Arc<dyn AttestationVerifier>,
    context: Arc<tokio::sync::RwLock<SecurityContext>>,
    cache: Arc<attestation_cache::AttestationCache>,
}

impl SecurityManager {
    pub fn new(verifier: Arc<dyn AttestationVerifier>) -> Self {
        let hardware_caps = hardware_detection::detect_hardware_capabilities();
        let context = SecurityContext {
            current_tier: PrivacyTier::Open,
            attestation: None,
            hardware_caps: hardware_caps.clone(),
            enforce_strict: false,
        };
        
        Self {
            verifier,
            context: Arc::new(tokio::sync::RwLock::new(context)),
            cache: Arc::new(attestation_cache::AttestationCache::new()),
        }
    }

    /// Initialize security based on configuration
    pub async fn initialize(&self, required_tier: Option<PrivacyTier>) -> Result<()> {
        let mut context = self.context.write().await;
        
        // Detect hardware capabilities
        context.hardware_caps = hardware_detection::detect_hardware_capabilities();
        
        // Set the target tier
        let target_tier = required_tier.unwrap_or(context.hardware_caps.max_supported_tier);
        
        // Generate attestation if needed
        if target_tier.requires_attestation() {
            match self.generate_attestation(target_tier).await {
                Ok(attestation) => {
                    context.current_tier = target_tier;
                    context.attestation = Some(Arc::new(attestation));
                    log::info!("🔐 TEE initialized at tier {:?}", target_tier);
                }
                Err(e) if !context.enforce_strict => {
                    log::warn!("⚠️ Failed to initialize TEE: {}, falling back to Open tier", e);
                    context.current_tier = PrivacyTier::Open;
                    context.attestation = None;
                }
                Err(e) => return Err(e),
            }
        } else {
            context.current_tier = target_tier;
            log::info!("📂 Running in Open mode (no TEE required)");
        }
        
        Ok(())
    }

    /// Generate attestation for the requested tier
    async fn generate_attestation(&self, tier: PrivacyTier) -> Result<AttestationResult> {
        // Check cache first
        if let Some(cached) = self.cache.get(tier).await {
            if cached.expires_at > chrono::Utc::now() {
                log::debug!("✅ Using cached attestation for tier {:?}", tier);
                return Ok(cached);
            }
        }

        // Generate new attestation based on tier
        let attestation_type = tee_attestation::generate_attestation_for_tier(tier).await?;
        
        // Verify the attestation
        let result = self.verifier.verify_attestation(&attestation_type).await?;
        
        // Validate tier support
        if !result.supports_tier(tier) {
            return Err(SecurityError::TierMismatch {
                requested: tier as u8,
                available: result.max_tier as u8,
            });
        }
        
        // Cache the result
        self.cache.set(tier, result.clone()).await;
        
        Ok(result)
    }

    /// Get the current security context
    pub async fn get_context(&self) -> SecurityContext {
        self.context.read().await.clone()
    }

    /// Check if a tool can be executed at the current tier
    pub async fn check_tool_authorization(&self, required_tier: PrivacyTier) -> Result<()> {
        let context = self.context.read().await;
        
        if context.current_tier < required_tier {
            return Err(SecurityError::TierMismatch {
                requested: required_tier as u8,
                available: context.current_tier as u8,
            });
        }
        
        // Verify attestation is still valid if required
        if required_tier.requires_attestation() {
            if let Some(attestation) = &context.attestation {
                if attestation.expires_at < chrono::Utc::now() {
                    return Err(SecurityError::SessionExpired);
                }
                
                if !attestation.verified {
                    return Err(SecurityError::InvalidAttestation(
                        "Attestation verification failed".to_string()
                    ));
                }
            } else {
                return Err(SecurityError::InvalidAttestation(
                    "No attestation available for secure tier".to_string()
                ));
            }
        }
        
        Ok(())
    }

    /// Execute a function with tier enforcement
    pub async fn execute_with_tier<F, T>(
        &self,
        required_tier: PrivacyTier,
        f: F,
    ) -> Result<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        // Check authorization
        self.check_tool_authorization(required_tier).await?;
        
        // Log the execution
        log::info!("🔒 Executing at tier {:?} with attestation", required_tier);
        
        // Execute the function
        let result = tokio::task::spawn_blocking(f).await
            .map_err(|e| SecurityError::Other(anyhow::anyhow!("Execution error: {}", e)))?;
        
        Ok(result)
    }

    /// Refresh attestation if needed
    pub async fn refresh_attestation(&self) -> Result<()> {
        let mut context = self.context.write().await;
        
        if let Some(attestation) = &context.attestation {
            let remaining = attestation.expires_at - chrono::Utc::now();
            if remaining < chrono::Duration::minutes(5) {
                log::info!("🔄 Refreshing attestation (expires in {}s)", remaining.num_seconds());
                
                let new_attestation = self.generate_attestation(context.current_tier).await?;
                context.attestation = Some(Arc::new(new_attestation));
            }
        }
        
        Ok(())
    }
}

/// Tool security requirements
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSecurityRequirements {
    /// Minimum privacy tier required
    pub min_tier: PrivacyTier,
    /// Whether attestation must be fresh (< 5 min old)
    pub require_fresh_attestation: bool,
    /// Specific hardware requirements
    pub hardware_requirements: Option<Vec<String>>,
    /// Whether to allow fallback to lower tier
    pub allow_fallback: bool,
}

impl Default for ToolSecurityRequirements {
    fn default() -> Self {
        Self {
            min_tier: PrivacyTier::Open,
            require_fresh_attestation: false,
            hardware_requirements: None,
            allow_fallback: true,
        }
    }
}

/// Integration trait for tool execution
#[async_trait]
pub trait SecureToolExecution: Send + Sync {
    /// Get security requirements for this tool
    fn security_requirements(&self) -> ToolSecurityRequirements;
    
    /// Execute with security context
    async fn execute_secure(
        &self,
        params: serde_json::Value,
        security: &SecurityManager,
    ) -> Result<serde_json::Value>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use hanzo_kbs::attestation::MockAttestationVerifier;

    #[tokio::test]
    async fn test_security_manager_initialization() {
        let verifier = Arc::new(MockAttestationVerifier);
        let manager = SecurityManager::new(verifier);
        
        // Initialize in open mode
        manager.initialize(Some(PrivacyTier::Open)).await.unwrap();
        
        let context = manager.get_context().await;
        assert_eq!(context.current_tier, PrivacyTier::Open);
        assert!(context.attestation.is_none());
    }

    #[tokio::test]
    async fn test_tier_enforcement() {
        let verifier = Arc::new(MockAttestationVerifier);
        let manager = SecurityManager::new(verifier);
        
        // Initialize in open mode
        manager.initialize(Some(PrivacyTier::Open)).await.unwrap();
        
        // Should succeed for Open tier
        manager.check_tool_authorization(PrivacyTier::Open).await.unwrap();
        
        // Should fail for higher tier
        let result = manager.check_tool_authorization(PrivacyTier::CpuTee).await;
        assert!(result.is_err());
    }
}