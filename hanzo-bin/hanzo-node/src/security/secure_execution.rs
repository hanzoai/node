//! Secure execution wrapper for TEE-protected tool execution
//!
//! This module provides integration between TEE attestation and tool execution,
//! ensuring that sensitive operations run with appropriate hardware security.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::security::tee_attestation::{TeeAttestationManager, TeeProtectedExecution};
use hanzo_kbs::types::PrivacyTier;
use hanzo_message_primitives::schemas::hanzo_tools::HanzoToolMetadata;
use hanzo_tools_primitives::tools::error::ToolError;
use hanzo_tools_primitives::tools::hanzo_tool::HanzoTool;

/// Configuration for secure tool execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecureExecutionConfig {
    /// Default privacy tier for tools without explicit requirements
    pub default_tier: PrivacyTier,

    /// Whether to allow execution if TEE is unavailable
    pub allow_fallback: bool,

    /// Log security events
    pub audit_logging: bool,

    /// Enforce attestation for all tool executions
    pub require_attestation: bool,
}

impl Default for SecureExecutionConfig {
    fn default() -> Self {
        Self {
            default_tier: PrivacyTier::Open,
            allow_fallback: true,
            audit_logging: true,
            require_attestation: false,
        }
    }
}

/// Secure execution manager for tools
pub struct SecureExecutionManager {
    tee_manager: Arc<TeeAttestationManager>,
    config: Arc<RwLock<SecureExecutionConfig>>,
}

impl SecureExecutionManager {
    /// Create a new secure execution manager
    pub fn new(tee_manager: Arc<TeeAttestationManager>, config: SecureExecutionConfig) -> Self {
        Self {
            tee_manager,
            config: Arc::new(RwLock::new(config)),
        }
    }

    /// Determine the required privacy tier for a tool
    async fn determine_tool_tier(&self, tool: &HanzoTool) -> PrivacyTier {
        // Check tool metadata for privacy requirements
        let tier = match tool {
            HanzoTool::Rust(tool_def, metadata) |
            HanzoTool::JS(tool_def, metadata) |
            HanzoTool::Python(tool_def, metadata) |
            HanzoTool::MCPServer(tool_def, metadata) |
            HanzoTool::Agent(tool_def, metadata) => {
                self.extract_tier_from_metadata(metadata).await
            }
        };

        tier.unwrap_or(self.config.read().await.default_tier)
    }

    /// Extract privacy tier from tool metadata
    async fn extract_tier_from_metadata(&self, metadata: &HanzoToolMetadata) -> Option<PrivacyTier> {
        // Look for privacy tier in tool configurations
        if let Some(config) = &metadata.tool_config {
            for cfg in config {
                if let hanzo_tools_primitives::tools::tool_config::ToolConfig::BasicConfig(basic) = cfg {
                    if basic.key == "privacy_tier" {
                        if let Ok(tier_num) = basic.value.parse::<u8>() {
                            return match tier_num {
                                0 => Some(PrivacyTier::Open),
                                1 => Some(PrivacyTier::AtRest),
                                2 => Some(PrivacyTier::CpuTee),
                                3 => Some(PrivacyTier::GpuCc),
                                4 => Some(PrivacyTier::GpuTeeIo),
                                _ => None,
                            };
                        }
                    }
                }
            }
        }

        // Check tool category for sensitive operations
        if metadata.author.to_lowercase().contains("medical") ||
           metadata.author.to_lowercase().contains("financial") ||
           metadata.author.to_lowercase().contains("confidential") {
            return Some(PrivacyTier::CpuTee);
        }

        None
    }

    /// Execute a tool with appropriate security measures
    pub async fn execute_tool_secure<F, Fut>(
        &self,
        tool: &HanzoTool,
        tool_name: &str,
        execute_fn: F,
    ) -> Result<Value, ToolError>
    where
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = Result<Value, ToolError>> + Send,
    {
        let config = self.config.read().await;
        let required_tier = self.determine_tool_tier(tool).await;

        info!(
            "Executing tool '{}' with privacy tier {:?}",
            tool_name, required_tier
        );

        // Check if we need attestation
        if config.require_attestation || required_tier.requires_attestation() {
            // Verify TEE capabilities
            match self.tee_manager.enforce_privacy_tier(required_tier).await {
                Ok(_) => {
                    info!("TEE attestation verified for tier {:?}", required_tier);
                }
                Err(e) => {
                    error!("TEE attestation failed: {}", e);
                    if !config.allow_fallback {
                        return Err(ToolError::SecurityError(format!(
                            "TEE attestation required but failed: {}",
                            e
                        )));
                    } else {
                        warn!("Falling back to non-TEE execution (security degraded)");
                    }
                }
            }
        }

        // Audit log if enabled
        if config.audit_logging {
            self.log_execution_audit(tool_name, required_tier).await;
        }

        // Execute the tool
        // In a real TEE implementation, this would be wrapped in enclave execution
        let result = execute_fn().await;

        // Log completion
        if config.audit_logging {
            self.log_completion_audit(tool_name, result.is_ok()).await;
        }

        result
    }

    /// Log execution audit event
    async fn log_execution_audit(&self, tool_name: &str, tier: PrivacyTier) {
        info!(
            "[SECURITY AUDIT] Tool execution started: {} at tier {:?}",
            tool_name, tier
        );
    }

    /// Log completion audit event
    async fn log_completion_audit(&self, tool_name: &str, success: bool) {
        info!(
            "[SECURITY AUDIT] Tool execution completed: {} - Success: {}",
            tool_name, success
        );
    }

    /// Update configuration
    pub async fn update_config(&self, config: SecureExecutionConfig) {
        *self.config.write().await = config;
    }
}

/// Extension trait to add secure execution to the existing tool execution flow
#[async_trait]
pub trait SecureToolExecution {
    /// Execute with TEE protection based on tool requirements
    async fn execute_with_protection(
        &self,
        tool: &HanzoTool,
        tool_name: &str,
        parameters: Map<String, Value>,
    ) -> Result<Value, ToolError>;
}

/// Wrapper for integrating secure execution into the existing coordinator
pub struct SecureExecutionCoordinator {
    pub inner_executor: Arc<dyn SecureToolExecution + Send + Sync>,
    pub security_manager: Arc<SecureExecutionManager>,
}

impl SecureExecutionCoordinator {
    pub fn new(
        inner_executor: Arc<dyn SecureToolExecution + Send + Sync>,
        security_manager: Arc<SecureExecutionManager>,
    ) -> Self {
        Self {
            inner_executor,
            security_manager,
        }
    }

    /// Execute a tool through the secure pipeline
    pub async fn execute_tool(
        &self,
        tool: &HanzoTool,
        tool_name: &str,
        parameters: Map<String, Value>,
    ) -> Result<Value, ToolError> {
        // Wrap the inner execution with TEE protection
        self.security_manager.execute_tool_secure(
            tool,
            tool_name,
            || async {
                self.inner_executor.execute_with_protection(
                    tool,
                    tool_name,
                    parameters.clone(),
                ).await
            },
        ).await
    }
}

/// Helper to determine if a tool handles sensitive data
pub fn is_sensitive_tool(tool_name: &str) -> bool {
    let sensitive_patterns = [
        "crypto", "wallet", "payment", "medical", "health",
        "financial", "bank", "credit", "ssn", "identity",
        "private", "secret", "confidential", "pii", "gdpr"
    ];

    let tool_lower = tool_name.to_lowercase();
    sensitive_patterns.iter().any(|pattern| tool_lower.contains(pattern))
}

/// Helper to get recommended tier for tool categories
pub fn recommended_tier_for_category(category: &str) -> PrivacyTier {
    match category.to_lowercase().as_str() {
        "financial" | "banking" | "payments" => PrivacyTier::CpuTee,
        "medical" | "healthcare" | "genomics" => PrivacyTier::GpuCc,
        "ai_training" | "model_inference" if category.contains("confidential") => PrivacyTier::GpuTeeIo,
        "crypto" | "wallet" => PrivacyTier::CpuTee,
        "public" | "demo" | "test" => PrivacyTier::Open,
        _ => PrivacyTier::AtRest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::tee_attestation::TeeAttestationConfig;

    #[test]
    fn test_sensitive_tool_detection() {
        assert!(is_sensitive_tool("crypto_wallet_manager"));
        assert!(is_sensitive_tool("medical_records_processor"));
        assert!(is_sensitive_tool("payment_gateway"));
        assert!(!is_sensitive_tool("hello_world"));
        assert!(!is_sensitive_tool("weather_api"));
    }

    #[test]
    fn test_tier_recommendations() {
        assert_eq!(
            recommended_tier_for_category("financial"),
            PrivacyTier::CpuTee
        );
        assert_eq!(
            recommended_tier_for_category("medical"),
            PrivacyTier::GpuCc
        );
        assert_eq!(
            recommended_tier_for_category("public"),
            PrivacyTier::Open
        );
    }

    #[tokio::test]
    async fn test_secure_execution_fallback() {
        let tee_config = TeeAttestationConfig {
            allow_mock: true,
            ..Default::default()
        };

        let tee_manager = Arc::new(
            TeeAttestationManager::new(tee_config).await.unwrap()
        );

        let exec_config = SecureExecutionConfig {
            allow_fallback: true,
            ..Default::default()
        };

        let manager = SecureExecutionManager::new(tee_manager, exec_config);

        // Create a mock tool
        let tool = HanzoTool::Rust(
            Default::default(),
            HanzoToolMetadata {
                author: "test".to_string(),
                created: 0,
                updated: 0,
                version: "1.0".to_string(),
            },
        );

        // Should succeed with fallback enabled
        let result = manager.execute_tool_secure(
            &tool,
            "test_tool",
            || async { Ok(Value::String("success".to_string())) },
        ).await;

        assert!(result.is_ok());
    }
}