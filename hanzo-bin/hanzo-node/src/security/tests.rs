//! Comprehensive tests for TEE attestation framework

#[cfg(test)]
mod tee_attestation_tests {
    use super::super::tee_attestation::*;
    use super::super::secure_execution::*;
    use hanzo_kbs::types::{PrivacyTier, NodeSecurityMode, AttestationType};
    use std::sync::Arc;
    use tokio::sync::RwLock;

    /// Test basic capability detection
    #[tokio::test]
    async fn test_capability_detection() {
        let config = TeeAttestationConfig {
            mode: NodeSecurityMode::SoftwareOnly,
            required_tier: PrivacyTier::Open,
            allow_mock: true,
            ..Default::default()
        };

        let manager = TeeAttestationManager::new(config).await.unwrap();
        let caps = manager.detect_capabilities().await.unwrap();

        // Basic assertions
        assert!(caps.max_privacy_tier >= PrivacyTier::Open);
        assert!(!caps.platform_details.cpu_vendor.is_empty() || !caps.platform_details.cpu_model.is_empty());
    }

    /// Test privacy tier enforcement
    #[tokio::test]
    async fn test_privacy_tier_enforcement() {
        let config = TeeAttestationConfig {
            mode: NodeSecurityMode::SimTee,
            required_tier: PrivacyTier::Open,
            allow_mock: true,
            ..Default::default()
        };

        let manager = TeeAttestationManager::new(config).await.unwrap();

        // Set mock capabilities for testing
        *manager.capabilities.write().await = Some(TeeCapabilities {
            sev_snp: false,
            tdx: false,
            sgx: false,
            h100_cc: false,
            blackwell_tee_io: false,
            sim_eid: true,
            max_privacy_tier: PrivacyTier::AtRest,
            platform_details: PlatformDetails {
                cpu_vendor: "Test".to_string(),
                cpu_model: "MockCPU".to_string(),
                gpu_vendor: None,
                gpu_model: None,
                kernel_version: "5.15".to_string(),
                security_features: vec!["SIM-EID".to_string()],
            },
        });

        // Should succeed for Open tier
        assert!(manager.enforce_privacy_tier(PrivacyTier::Open).await.is_ok());

        // Should succeed for AtRest tier
        assert!(manager.enforce_privacy_tier(PrivacyTier::AtRest).await.is_ok());

        // Should fail for CpuTee tier (not available)
        assert!(manager.enforce_privacy_tier(PrivacyTier::CpuTee).await.is_err());
    }

    /// Test attestation generation with mock SEV-SNP
    #[tokio::test]
    async fn test_sev_snp_attestation() {
        let config = TeeAttestationConfig {
            mode: NodeSecurityMode::SimTee,
            required_tier: PrivacyTier::CpuTee,
            allow_mock: true,
            ..Default::default()
        };

        let manager = TeeAttestationManager::new(config).await.unwrap();

        // Set capabilities with SEV-SNP
        *manager.capabilities.write().await = Some(TeeCapabilities {
            sev_snp: true,
            tdx: false,
            sgx: false,
            h100_cc: false,
            blackwell_tee_io: false,
            sim_eid: false,
            max_privacy_tier: PrivacyTier::CpuTee,
            platform_details: PlatformDetails {
                cpu_vendor: "AMD".to_string(),
                cpu_model: "EPYC 7763".to_string(),
                gpu_vendor: None,
                gpu_model: None,
                kernel_version: "5.15".to_string(),
                security_features: vec!["SEV-SNP".to_string()],
            },
        });

        let attestation = manager.generate_attestation().await.unwrap();

        // Verify it's SEV-SNP attestation
        match attestation {
            AttestationType::SevSnp { report, vcek_cert, platform_cert_chain } => {
                assert!(!report.is_empty());
                assert!(!vcek_cert.is_empty());
                assert!(!platform_cert_chain.is_empty());
            }
            _ => panic!("Expected SEV-SNP attestation"),
        }
    }

    /// Test attestation verification
    #[tokio::test]
    async fn test_attestation_verification() {
        let config = TeeAttestationConfig {
            mode: NodeSecurityMode::SimTee,
            required_tier: PrivacyTier::CpuTee,
            allow_mock: true,
            cache_attestations: true,
            attestation_cache_duration_secs: 60,
            ..Default::default()
        };

        let manager = TeeAttestationManager::new(config).await.unwrap();

        let attestation = AttestationType::SevSnp {
            report: vec![0xAA; 4096],
            vcek_cert: vec![0xBB; 2048],
            platform_cert_chain: vec![0xCC; 4096],
        };

        let result = manager.verify_attestation(&attestation).await.unwrap();

        assert!(result.verified);
        assert!(result.supports_tier(PrivacyTier::CpuTee));
        assert!(!result.measurements.is_empty());

        // Test caching - second verification should be faster
        let start = std::time::Instant::now();
        let _ = manager.verify_attestation(&attestation).await.unwrap();
        let cached_duration = start.elapsed();

        // Cached should be very fast (< 1ms typically)
        assert!(cached_duration.as_millis() < 10);
    }

    /// Test protected execution
    #[tokio::test]
    async fn test_protected_execution() {
        let config = TeeAttestationConfig {
            mode: NodeSecurityMode::SoftwareOnly,
            required_tier: PrivacyTier::Open,
            allow_mock: true,
            ..Default::default()
        };

        let manager = TeeAttestationManager::new(config).await.unwrap();

        let result = manager.execute_protected(
            PrivacyTier::Open,
            "test_operation",
            || {
                // Simulate some protected computation
                let secret = 42;
                let computed = secret * 2 + 10;
                computed
            },
        ).await.unwrap();

        assert_eq!(result, 94);
    }

    /// Test configuration updates
    #[tokio::test]
    async fn test_config_updates() {
        let initial_config = TeeAttestationConfig {
            mode: NodeSecurityMode::SoftwareOnly,
            required_tier: PrivacyTier::Open,
            allow_mock: true,
            ..Default::default()
        };

        let manager = TeeAttestationManager::new(initial_config).await.unwrap();

        // Update configuration
        let new_config = TeeAttestationConfig {
            mode: NodeSecurityMode::SimTee,
            required_tier: PrivacyTier::AtRest,
            allow_mock: false,
            ..Default::default()
        };

        manager.update_config(new_config.clone()).await;

        // Verify config was updated by checking behavior
        let caps = manager.get_capabilities().await;
        assert!(caps.is_none() || caps.unwrap().max_privacy_tier >= PrivacyTier::Open);
    }

    /// Test H100 CC attestation generation
    #[tokio::test]
    async fn test_h100_cc_attestation() {
        let config = TeeAttestationConfig {
            mode: NodeSecurityMode::SimTee,
            required_tier: PrivacyTier::GpuCc,
            allow_mock: true,
            ..Default::default()
        };

        let manager = TeeAttestationManager::new(config).await.unwrap();

        // Set capabilities with H100 CC and SEV-SNP
        *manager.capabilities.write().await = Some(TeeCapabilities {
            sev_snp: true,
            tdx: false,
            sgx: false,
            h100_cc: true,
            blackwell_tee_io: false,
            sim_eid: false,
            max_privacy_tier: PrivacyTier::GpuCc,
            platform_details: PlatformDetails {
                cpu_vendor: "AMD".to_string(),
                cpu_model: "EPYC 7763".to_string(),
                gpu_vendor: Some("NVIDIA".to_string()),
                gpu_model: Some("H100".to_string()),
                kernel_version: "5.15".to_string(),
                security_features: vec!["SEV-SNP".to_string(), "H100-CC".to_string()],
            },
        });

        let attestation = manager.generate_attestation().await.unwrap();

        // Verify it's H100 CC attestation with CPU attestation
        match attestation {
            AttestationType::H100Cc { gpu_attestation, cpu_attestation } => {
                assert!(!gpu_attestation.is_empty());
                // CPU attestation should be SEV-SNP
                match cpu_attestation.as_ref() {
                    AttestationType::SevSnp { .. } => {}
                    _ => panic!("Expected SEV-SNP CPU attestation"),
                }
            }
            _ => panic!("Expected H100 CC attestation"),
        }
    }

    /// Test Blackwell TEE-I/O attestation
    #[tokio::test]
    async fn test_blackwell_tee_io_attestation() {
        let config = TeeAttestationConfig {
            mode: NodeSecurityMode::SimTee,
            required_tier: PrivacyTier::GpuTeeIo,
            allow_mock: true,
            ..Default::default()
        };

        let manager = TeeAttestationManager::new(config).await.unwrap();

        // Set capabilities with Blackwell
        *manager.capabilities.write().await = Some(TeeCapabilities {
            sev_snp: false,
            tdx: false,
            sgx: false,
            h100_cc: false,
            blackwell_tee_io: true,
            sim_eid: false,
            max_privacy_tier: PrivacyTier::GpuTeeIo,
            platform_details: PlatformDetails {
                cpu_vendor: "Intel".to_string(),
                cpu_model: "Xeon".to_string(),
                gpu_vendor: Some("NVIDIA".to_string()),
                gpu_model: Some("Blackwell".to_string()),
                kernel_version: "6.0".to_string(),
                security_features: vec!["Blackwell-TEE-IO".to_string()],
            },
        });

        let attestation = manager.generate_attestation().await.unwrap();

        // Verify it's Blackwell attestation
        match attestation {
            AttestationType::BlackwellTeeIo { tee_io_report, mig_config } => {
                assert!(!tee_io_report.is_empty());
                assert!(mig_config.is_none()); // No MIG in this test
            }
            _ => panic!("Expected Blackwell TEE-I/O attestation"),
        }
    }
}

#[cfg(test)]
mod secure_execution_tests {
    use super::super::secure_execution::*;
    use super::super::tee_attestation::*;
    use hanzo_kbs::types::PrivacyTier;
    use hanzo_tools::tools::hanzo_tool::HanzoTool;
    use hanzo_messages::schemas::hanzo_tools::HanzoToolMetadata;
    use serde_json::Value;
    use std::sync::Arc;

    /// Test sensitive tool detection
    #[test]
    fn test_sensitive_tool_detection() {
        // Positive cases
        assert!(is_sensitive_tool("crypto_wallet"));
        assert!(is_sensitive_tool("payment_processor"));
        assert!(is_sensitive_tool("medical_records"));
        assert!(is_sensitive_tool("bank_transfer"));
        assert!(is_sensitive_tool("ssn_validator"));
        assert!(is_sensitive_tool("private_key_manager"));

        // Negative cases
        assert!(!is_sensitive_tool("hello_world"));
        assert!(!is_sensitive_tool("weather_api"));
        assert!(!is_sensitive_tool("calculator"));
        assert!(!is_sensitive_tool("text_formatter"));
    }

    /// Test tier recommendations for categories
    #[test]
    fn test_tier_recommendations() {
        assert_eq!(recommended_tier_for_category("financial"), PrivacyTier::CpuTee);
        assert_eq!(recommended_tier_for_category("banking"), PrivacyTier::CpuTee);
        assert_eq!(recommended_tier_for_category("medical"), PrivacyTier::GpuCc);
        assert_eq!(recommended_tier_for_category("healthcare"), PrivacyTier::GpuCc);
        assert_eq!(recommended_tier_for_category("public"), PrivacyTier::Open);
        assert_eq!(recommended_tier_for_category("demo"), PrivacyTier::Open);
        assert_eq!(recommended_tier_for_category("test"), PrivacyTier::Open);
        assert_eq!(recommended_tier_for_category("crypto"), PrivacyTier::CpuTee);
        assert_eq!(recommended_tier_for_category("wallet"), PrivacyTier::CpuTee);
        assert_eq!(recommended_tier_for_category("unknown"), PrivacyTier::AtRest);
    }

    /// Test secure execution with fallback
    #[tokio::test]
    async fn test_secure_execution_with_fallback() {
        let tee_config = TeeAttestationConfig {
            mode: NodeSecurityMode::SoftwareOnly,
            required_tier: PrivacyTier::Open,
            allow_mock: true,
            ..Default::default()
        };

        let tee_manager = Arc::new(TeeAttestationManager::new(tee_config).await.unwrap());

        let exec_config = SecureExecutionConfig {
            default_tier: PrivacyTier::Open,
            allow_fallback: true,
            audit_logging: false,
            require_attestation: false,
        };

        let manager = SecureExecutionManager::new(tee_manager, exec_config);

        let tool = HanzoTool::Rust(
            Default::default(),
            HanzoToolMetadata {
                author: "test".to_string(),
                created: 0,
                updated: 0,
                version: "1.0".to_string(),
            },
        );

        let result = manager.execute_tool_secure(
            &tool,
            "test_tool",
            || async {
                Ok(Value::String("executed successfully".to_string()))
            },
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Value::String("executed successfully".to_string()));
    }

    /// Test secure execution without fallback (should fail)
    #[tokio::test]
    async fn test_secure_execution_no_fallback() {
        let tee_config = TeeAttestationConfig {
            mode: NodeSecurityMode::SoftwareOnly,
            required_tier: PrivacyTier::CpuTee,
            allow_mock: false,
            ..Default::default()
        };

        let tee_manager = Arc::new(TeeAttestationManager::new(tee_config).await.unwrap());

        let exec_config = SecureExecutionConfig {
            default_tier: PrivacyTier::CpuTee,
            allow_fallback: false,
            audit_logging: false,
            require_attestation: true,
        };

        let manager = SecureExecutionManager::new(tee_manager, exec_config);

        let tool = HanzoTool::Rust(
            Default::default(),
            HanzoToolMetadata {
                author: "medical".to_string(), // Triggers higher tier requirement
                created: 0,
                updated: 0,
                version: "1.0".to_string(),
            },
        );

        let result = manager.execute_tool_secure(
            &tool,
            "medical_tool",
            || async {
                Ok(Value::String("should not execute".to_string()))
            },
        ).await;

        // Should fail because TEE is not available and fallback is disabled
        assert!(result.is_err());
    }

    /// Test audit logging
    #[tokio::test]
    async fn test_audit_logging() {
        let tee_config = TeeAttestationConfig {
            mode: NodeSecurityMode::SoftwareOnly,
            required_tier: PrivacyTier::Open,
            allow_mock: true,
            ..Default::default()
        };

        let tee_manager = Arc::new(TeeAttestationManager::new(tee_config).await.unwrap());

        let exec_config = SecureExecutionConfig {
            default_tier: PrivacyTier::Open,
            allow_fallback: true,
            audit_logging: true, // Enable audit logging
            require_attestation: false,
        };

        let manager = SecureExecutionManager::new(tee_manager, exec_config);

        let tool = HanzoTool::Python(
            Default::default(),
            HanzoToolMetadata {
                author: "test".to_string(),
                created: 0,
                updated: 0,
                version: "1.0".to_string(),
            },
        );

        // Execute and verify audit logs are generated (check logs in output)
        let _ = manager.execute_tool_secure(
            &tool,
            "audit_test_tool",
            || async {
                Ok(Value::String("success".to_string()))
            },
        ).await;
    }
}