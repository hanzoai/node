// PRIVACY ENFORCEMENT - THE RULES OF THE MATRIX
// Enforce privacy tiers across all operations

use super::{PrivacyTier, SecurityContext, ToolSecurityRequirements};
use hanzo_kbs::error::{Result, SecurityError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Privacy policy enforcement engine
pub struct PrivacyEnforcer {
    policies: Arc<RwLock<HashMap<String, TierPolicy>>>,
    audit_log: Arc<RwLock<Vec<AuditEntry>>>,
    strict_mode: bool,
}

/// Policy for a specific privacy tier
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierPolicy {
    pub tier: PrivacyTier,
    pub allowed_operations: Vec<String>,
    pub denied_operations: Vec<String>,
    pub data_classification: DataClassification,
    pub retention_period_hours: u64,
    pub require_encryption_at_rest: bool,
    pub require_encryption_in_transit: bool,
    pub audit_requirements: AuditRequirements,
}

/// Data classification levels
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum DataClassification {
    Public,
    Internal,
    Confidential,
    Secret,
    TopSecret,
}

/// Audit requirements for operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditRequirements {
    pub log_access: bool,
    pub log_modifications: bool,
    pub log_failures: bool,
    pub retain_logs_days: u32,
    pub alert_on_violation: bool,
}

/// Audit log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub operation: String,
    pub tier: PrivacyTier,
    pub user_id: Option<String>,
    pub tool_id: Option<String>,
    pub success: bool,
    pub violation: Option<String>,
    pub metadata: serde_json::Value,
}

impl PrivacyEnforcer {
    pub fn new(strict_mode: bool) -> Self {
        let mut policies = HashMap::new();
        
        // Initialize default policies for each tier
        policies.insert("open".to_string(), TierPolicy {
            tier: PrivacyTier::Open,
            allowed_operations: vec!["*".to_string()],
            denied_operations: vec![],
            data_classification: DataClassification::Public,
            retention_period_hours: 24 * 30, // 30 days
            require_encryption_at_rest: false,
            require_encryption_in_transit: false,
            audit_requirements: AuditRequirements {
                log_access: false,
                log_modifications: true,
                log_failures: true,
                retain_logs_days: 30,
                alert_on_violation: false,
            },
        });
        
        policies.insert("at_rest".to_string(), TierPolicy {
            tier: PrivacyTier::AtRest,
            allowed_operations: vec!["read".to_string(), "write".to_string(), "compute".to_string()],
            denied_operations: vec!["export".to_string(), "share".to_string()],
            data_classification: DataClassification::Internal,
            retention_period_hours: 24 * 90, // 90 days
            require_encryption_at_rest: true,
            require_encryption_in_transit: false,
            audit_requirements: AuditRequirements {
                log_access: true,
                log_modifications: true,
                log_failures: true,
                retain_logs_days: 90,
                alert_on_violation: false,
            },
        });
        
        policies.insert("cpu_tee".to_string(), TierPolicy {
            tier: PrivacyTier::CpuTee,
            allowed_operations: vec!["compute".to_string(), "inference".to_string()],
            denied_operations: vec!["export".to_string(), "debug".to_string(), "trace".to_string()],
            data_classification: DataClassification::Confidential,
            retention_period_hours: 24 * 7, // 7 days
            require_encryption_at_rest: true,
            require_encryption_in_transit: true,
            audit_requirements: AuditRequirements {
                log_access: true,
                log_modifications: true,
                log_failures: true,
                retain_logs_days: 180,
                alert_on_violation: true,
            },
        });
        
        policies.insert("gpu_cc".to_string(), TierPolicy {
            tier: PrivacyTier::GpuCc,
            allowed_operations: vec!["gpu_compute".to_string(), "ml_training".to_string(), "inference".to_string()],
            denied_operations: vec!["export".to_string(), "debug".to_string(), "profile".to_string()],
            data_classification: DataClassification::Secret,
            retention_period_hours: 24, // 1 day
            require_encryption_at_rest: true,
            require_encryption_in_transit: true,
            audit_requirements: AuditRequirements {
                log_access: true,
                log_modifications: true,
                log_failures: true,
                retain_logs_days: 365,
                alert_on_violation: true,
            },
        });
        
        policies.insert("gpu_tee_io".to_string(), TierPolicy {
            tier: PrivacyTier::GpuTeeIo,
            allowed_operations: vec!["secure_inference".to_string(), "confidential_training".to_string()],
            denied_operations: vec!["export".to_string(), "debug".to_string(), "trace".to_string(), "profile".to_string()],
            data_classification: DataClassification::TopSecret,
            retention_period_hours: 1, // 1 hour
            require_encryption_at_rest: true,
            require_encryption_in_transit: true,
            audit_requirements: AuditRequirements {
                log_access: true,
                log_modifications: true,
                log_failures: true,
                retain_logs_days: 365 * 7, // 7 years
                alert_on_violation: true,
            },
        });
        
        Self {
            policies: Arc::new(RwLock::new(policies)),
            audit_log: Arc::new(RwLock::new(Vec::new())),
            strict_mode,
        }
    }
    
    /// Check if an operation is allowed at the given tier
    pub async fn check_operation(
        &self,
        operation: &str,
        tier: PrivacyTier,
        context: &SecurityContext,
    ) -> Result<()> {
        let policies = self.policies.read().await;
        let policy_key = format!("{:?}", tier).to_lowercase();
        
        let policy = policies.get(&policy_key)
            .ok_or_else(|| SecurityError::PolicyViolation(
                format!("No policy defined for tier {:?}", tier)
            ))?;
        
        // Check if we have sufficient tier
        if context.current_tier < tier {
            self.log_violation(
                operation,
                tier,
                "Insufficient privacy tier",
                context,
            ).await;
            
            return Err(SecurityError::TierMismatch {
                requested: tier as u8,
                available: context.current_tier as u8,
            });
        }
        
        // Check if operation is explicitly denied
        if policy.denied_operations.iter().any(|op| op == operation || op == "*") {
            self.log_violation(
                operation,
                tier,
                "Operation explicitly denied",
                context,
            ).await;
            
            return Err(SecurityError::PolicyViolation(
                format!("Operation '{}' is denied at tier {:?}", operation, tier)
            ));
        }
        
        // Check if operation is allowed
        let allowed = policy.allowed_operations.iter().any(|op| 
            op == operation || op == "*" || operation.starts_with(&format!("{}.", op))
        );
        
        if !allowed && self.strict_mode {
            self.log_violation(
                operation,
                tier,
                "Operation not in allowed list",
                context,
            ).await;
            
            return Err(SecurityError::PolicyViolation(
                format!("Operation '{}' is not allowed at tier {:?}", operation, tier)
            ));
        }
        
        // Log successful authorization
        self.log_success(operation, tier, context).await;
        
        Ok(())
    }
    
    /// Check tool requirements against current context
    pub async fn check_tool_requirements(
        &self,
        requirements: &ToolSecurityRequirements,
        context: &SecurityContext,
    ) -> Result<()> {
        // Check minimum tier
        if context.current_tier < requirements.min_tier {
            if !requirements.allow_fallback || self.strict_mode {
                return Err(SecurityError::TierMismatch {
                    requested: requirements.min_tier as u8,
                    available: context.current_tier as u8,
                });
            }
            
            log::warn!(
                "⚠️ Tool requires tier {:?} but running at {:?} (fallback allowed)",
                requirements.min_tier,
                context.current_tier
            );
        }
        
        // Check attestation freshness
        if requirements.require_fresh_attestation {
            if let Some(attestation) = &context.attestation {
                let age = chrono::Utc::now() - (attestation.expires_at - chrono::Duration::hours(1));
                if age > chrono::Duration::minutes(5) {
                    return Err(SecurityError::SessionExpired);
                }
            } else if requirements.min_tier.requires_attestation() {
                return Err(SecurityError::InvalidAttestation(
                    "Fresh attestation required but none available".to_string()
                ));
            }
        }
        
        // Check hardware requirements
        if let Some(hw_reqs) = &requirements.hardware_requirements {
            for req in hw_reqs {
                if !self.check_hardware_requirement(req, &context.hardware_caps) {
                    return Err(SecurityError::Other(anyhow::anyhow!(
                        "Hardware requirement not met: {}", req
                    )));
                }
            }
        }
        
        Ok(())
    }
    
    /// Check specific hardware requirement
    fn check_hardware_requirement(
        &self,
        requirement: &str,
        caps: &super::HardwareCapabilities,
    ) -> bool {
        match requirement.to_lowercase().as_str() {
            "sev-snp" | "sev_snp" => caps.sev_snp_available,
            "tdx" => caps.tdx_available,
            "sgx" => caps.sgx_available,
            "h100" | "h100-cc" => caps.h100_cc_available,
            "blackwell" | "tee-io" => caps.blackwell_tee_io_available,
            "sim" | "eid" => caps.sim_eid_available,
            _ => false,
        }
    }
    
    /// Log a policy violation
    async fn log_violation(
        &self,
        operation: &str,
        tier: PrivacyTier,
        violation: &str,
        context: &SecurityContext,
    ) {
        let entry = AuditEntry {
            timestamp: chrono::Utc::now(),
            operation: operation.to_string(),
            tier,
            user_id: None, // Would be populated from context in production
            tool_id: None,
            success: false,
            violation: Some(violation.to_string()),
            metadata: serde_json::json!({
                "current_tier": format!("{:?}", context.current_tier),
                "has_attestation": context.attestation.is_some(),
            }),
        };
        
        let mut log = self.audit_log.write().await;
        log.push(entry.clone());
        
        // Alert if required
        let policies = self.policies.read().await;
        if let Some(policy) = policies.get(&format!("{:?}", tier).to_lowercase()) {
            if policy.audit_requirements.alert_on_violation {
                log::error!("🚨 SECURITY VIOLATION: {} - {}", operation, violation);
                // In production, send alert to monitoring system
            }
        }
    }
    
    /// Log successful operation
    async fn log_success(
        &self,
        operation: &str,
        tier: PrivacyTier,
        context: &SecurityContext,
    ) {
        let policies = self.policies.read().await;
        let policy_key = format!("{:?}", tier).to_lowercase();
        
        if let Some(policy) = policies.get(&policy_key) {
            if policy.audit_requirements.log_access {
                let entry = AuditEntry {
                    timestamp: chrono::Utc::now(),
                    operation: operation.to_string(),
                    tier,
                    user_id: None,
                    tool_id: None,
                    success: true,
                    violation: None,
                    metadata: serde_json::json!({
                        "current_tier": format!("{:?}", context.current_tier),
                        "has_attestation": context.attestation.is_some(),
                    }),
                };
                
                let mut log = self.audit_log.write().await;
                log.push(entry);
            }
        }
    }
    
    /// Get audit log entries
    pub async fn get_audit_log(&self, limit: Option<usize>) -> Vec<AuditEntry> {
        let log = self.audit_log.read().await;
        let limit = limit.unwrap_or(100);
        
        log.iter()
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }
    
    /// Clean old audit entries
    pub async fn cleanup_audit_log(&self, retain_days: u32) {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(retain_days as i64);
        let mut log = self.audit_log.write().await;
        log.retain(|entry| entry.timestamp > cutoff);
    }
}

/// Data sanitization for different privacy tiers
pub struct DataSanitizer;

impl DataSanitizer {
    /// Sanitize data based on privacy tier
    pub fn sanitize(data: &serde_json::Value, tier: PrivacyTier) -> serde_json::Value {
        match tier {
            PrivacyTier::Open => data.clone(),
            PrivacyTier::AtRest => Self::remove_sensitive_fields(data, &["password", "secret", "key"]),
            PrivacyTier::CpuTee => Self::remove_sensitive_fields(data, &["password", "secret", "key", "token", "credential"]),
            PrivacyTier::GpuCc | PrivacyTier::GpuTeeIo => {
                // Maximum sanitization for highest tiers
                Self::deep_sanitize(data)
            }
        }
    }
    
    fn remove_sensitive_fields(data: &serde_json::Value, fields: &[&str]) -> serde_json::Value {
        match data {
            serde_json::Value::Object(map) => {
                let mut sanitized = serde_json::Map::new();
                for (key, value) in map {
                    if fields.iter().any(|f| key.to_lowercase().contains(f)) {
                        sanitized.insert(key.clone(), serde_json::Value::String("***REDACTED***".to_string()));
                    } else {
                        sanitized.insert(key.clone(), Self::remove_sensitive_fields(value, fields));
                    }
                }
                serde_json::Value::Object(sanitized)
            }
            serde_json::Value::Array(arr) => {
                serde_json::Value::Array(
                    arr.iter().map(|v| Self::remove_sensitive_fields(v, fields)).collect()
                )
            }
            _ => data.clone(),
        }
    }
    
    fn deep_sanitize(data: &serde_json::Value) -> serde_json::Value {
        match data {
            serde_json::Value::Object(map) => {
                let mut sanitized = serde_json::Map::new();
                for (key, _) in map {
                    sanitized.insert(key.clone(), serde_json::Value::String("***CLASSIFIED***".to_string()));
                }
                serde_json::Value::Object(sanitized)
            }
            serde_json::Value::Array(_) => {
                serde_json::Value::String("***ARRAY_CLASSIFIED***".to_string())
            }
            serde_json::Value::String(_) => {
                serde_json::Value::String("***CLASSIFIED***".to_string())
            }
            serde_json::Value::Number(_) => {
                serde_json::Value::Number(serde_json::Number::from(0))
            }
            _ => data.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_privacy_enforcement() {
        let enforcer = PrivacyEnforcer::new(false);
        
        let context = SecurityContext {
            current_tier: PrivacyTier::CpuTee,
            attestation: None,
            hardware_caps: Default::default(),
            enforce_strict: false,
        };
        
        // Should allow compute at CPU TEE tier
        assert!(enforcer.check_operation("compute", PrivacyTier::CpuTee, &context).await.is_ok());
        
        // Should deny export at CPU TEE tier
        assert!(enforcer.check_operation("export", PrivacyTier::CpuTee, &context).await.is_err());
        
        // Should fail for higher tier requirement
        assert!(enforcer.check_operation("secure_inference", PrivacyTier::GpuTeeIo, &context).await.is_err());
    }
    
    #[test]
    fn test_data_sanitization() {
        let data = serde_json::json!({
            "username": "neo",
            "password": "matrix123",
            "api_key": "sk-12345",
            "data": [1, 2, 3]
        });
        
        let sanitized = DataSanitizer::sanitize(&data, PrivacyTier::CpuTee);
        assert_eq!(sanitized["password"], "***REDACTED***");
        assert_eq!(sanitized["username"], "neo");
        
        let classified = DataSanitizer::sanitize(&data, PrivacyTier::GpuTeeIo);
        assert_eq!(classified["username"], "***CLASSIFIED***");
    }
}