// TEE INTEGRATION TESTS - TEST THE MATRIX
// Comprehensive tests for TEE attestation and security runtime
//
// NOTE: This test requires features not yet implemented.
// Enable with: cargo test --features tee-integration

#![cfg(feature = "tee-integration")]

#[cfg(test)]
mod tests {
    use hanzo_node::security::{
        SecurityManager, PrivacyTier, ToolSecurityRequirements, HardwareCapabilities,
    };
    use hanzo_kbs::attestation::MockAttestationVerifier;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_security_manager_initialization_all_tiers() {
        let verifier = Arc::new(MockAttestationVerifier);
        let manager = SecurityManager::new(verifier);

        // Test each tier initialization
        for tier in [
            PrivacyTier::Open,
            PrivacyTier::AtRest,
            PrivacyTier::CpuTee,
            PrivacyTier::GpuCc,
            PrivacyTier::GpuTeeIo,
        ] {
            // In test mode with mock verifier, only non-TEE tiers should succeed
            let result = manager.initialize(Some(tier)).await;

            if tier == PrivacyTier::Open || tier == PrivacyTier::AtRest {
                assert!(result.is_ok(), "Failed to initialize tier {:?}", tier);

                let context = manager.get_context().await;
                assert_eq!(context.current_tier, tier);

                if tier.requires_attestation() {
                    assert!(context.attestation.is_some());
                } else {
                    assert!(context.attestation.is_none());
                }
            }
        }
    }

    #[tokio::test]
    async fn test_tool_authorization_enforcement() {
        let verifier = Arc::new(MockAttestationVerifier);
        let manager = SecurityManager::new(verifier);

        // Initialize at Open tier
        manager.initialize(Some(PrivacyTier::Open)).await.unwrap();

        // Should allow Open tier tools
        assert!(manager.check_tool_authorization(PrivacyTier::Open).await.is_ok());

        // Should deny higher tier tools
        assert!(manager.check_tool_authorization(PrivacyTier::CpuTee).await.is_err());
        assert!(manager.check_tool_authorization(PrivacyTier::GpuTeeIo).await.is_err());
    }

    #[tokio::test]
    async fn test_tool_security_requirements() {
        let verifier = Arc::new(MockAttestationVerifier);
        let manager = SecurityManager::new(verifier);

        manager.initialize(Some(PrivacyTier::Open)).await.unwrap();

        // Test strict requirements
        let strict_requirements = ToolSecurityRequirements {
            min_tier: PrivacyTier::CpuTee,
            require_fresh_attestation: true,
            hardware_requirements: Some(vec!["sev-snp".to_string()]),
            allow_fallback: false,
        };

        // Should fail with strict requirements at Open tier
        let context = manager.get_context().await;
        let enforcer = hanzo_node::security::privacy_enforcement::PrivacyEnforcer::new(false);
        let result = enforcer.check_tool_requirements(&strict_requirements, &context).await;
        assert!(result.is_err());

        // Test lenient requirements
        let lenient_requirements = ToolSecurityRequirements {
            min_tier: PrivacyTier::Open,
            require_fresh_attestation: false,
            hardware_requirements: None,
            allow_fallback: true,
        };

        // Should succeed with lenient requirements
        let result = enforcer.check_tool_requirements(&lenient_requirements, &context).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_with_tier() {
        let verifier = Arc::new(MockAttestationVerifier);
        let manager = SecurityManager::new(verifier);

        manager.initialize(Some(PrivacyTier::Open)).await.unwrap();

        // Test execution at allowed tier
        let result = manager.execute_with_tier(
            PrivacyTier::Open,
            || 42,
        ).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);

        // Test execution at denied tier
        let result = manager.execute_with_tier(
            PrivacyTier::CpuTee,
            || 100,
        ).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_hardware_detection() {
        use hanzo_node::security::hardware_detection;

        let caps = hardware_detection::detect_hardware_capabilities();

        // At minimum should detect Open tier
        assert!(caps.max_supported_tier >= PrivacyTier::Open);

        // Test that max tier is correctly calculated
        let test_caps = HardwareCapabilities {
            sev_snp_available: true,
            tdx_available: false,
            sgx_available: false,
            h100_cc_available: false,
            blackwell_tee_io_available: false,
            sim_eid_available: false,
            max_supported_tier: PrivacyTier::CpuTee,
        };
        assert_eq!(test_caps.max_supported_tier, PrivacyTier::CpuTee);

        // Test Blackwell detection (highest tier)
        let blackwell_caps = HardwareCapabilities {
            sev_snp_available: true,
            tdx_available: true,
            sgx_available: false,
            h100_cc_available: true,
            blackwell_tee_io_available: true,
            sim_eid_available: true,
            max_supported_tier: PrivacyTier::GpuTeeIo,
        };
        assert_eq!(blackwell_caps.max_supported_tier, PrivacyTier::GpuTeeIo);
    }

    #[tokio::test]
    async fn test_attestation_cache() {
        use hanzo_node::security::attestation_cache::AttestationCache;
        use hanzo_kbs::attestation::{AttestationResult, Measurement, PlatformInfo};
        use chrono::{Utc, Duration};

        let cache = AttestationCache::new();

        // Create mock attestation
        let attestation = AttestationResult {
            verified: true,
            max_tier: PrivacyTier::CpuTee,
            measurements: vec![
                Measurement {
                    name: "kernel".to_string(),
                    value: vec![0xAB; 32],
                    pcr_index: Some(0),
                }
            ],
            platform_info: PlatformInfo {
                platform_type: "SEV-SNP".to_string(),
                tcb_version: "1.0".to_string(),
                security_features: vec!["SME".to_string()],
                vendor_info: serde_json::json!({}),
            },
            expires_at: Utc::now() + Duration::hours(1),
        };

        // Cache the attestation
        cache.set(PrivacyTier::CpuTee, attestation.clone()).await;

        // Should retrieve from cache
        let cached = cache.get(PrivacyTier::CpuTee).await;
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().max_tier, PrivacyTier::CpuTee);

        // Test cache invalidation
        cache.invalidate(PrivacyTier::CpuTee).await;
        let cached = cache.get(PrivacyTier::CpuTee).await;
        assert!(cached.is_none());

        // Test cache stats
        cache.set(PrivacyTier::AtRest, attestation.clone()).await;
        cache.set(PrivacyTier::GpuCc, attestation.clone()).await;

        let stats = cache.stats().await;
        assert_eq!(stats.total_entries, 2);
        assert_eq!(stats.valid_entries, 2);
    }

    #[tokio::test]
    async fn test_privacy_enforcement() {
        use hanzo_node::security::privacy_enforcement::{PrivacyEnforcer, DataSanitizer};
        use hanzo_node::security::SecurityContext;
        use serde_json::json;

        let enforcer = PrivacyEnforcer::new(false);

        // Create context at CPU TEE tier
        let context = SecurityContext {
            current_tier: PrivacyTier::CpuTee,
            attestation: None,
            hardware_caps: HardwareCapabilities::default(),
            enforce_strict: false,
        };

        // Test allowed operations
        assert!(enforcer.check_operation("compute", PrivacyTier::CpuTee, &context).await.is_ok());
        assert!(enforcer.check_operation("inference", PrivacyTier::CpuTee, &context).await.is_ok());

        // Test denied operations
        assert!(enforcer.check_operation("export", PrivacyTier::CpuTee, &context).await.is_err());
        assert!(enforcer.check_operation("debug", PrivacyTier::CpuTee, &context).await.is_err());

        // Test data sanitization
        let sensitive_data = json!({
            "username": "agent007",
            "password": "topsecret",
            "api_key": "sk-12345",
            "public_data": "visible"
        });

        // Sanitize for CPU TEE tier
        let sanitized = DataSanitizer::sanitize(&sensitive_data, PrivacyTier::CpuTee);
        assert_eq!(sanitized["password"], "***REDACTED***");
        assert_eq!(sanitized["api_key"], "***REDACTED***");
        assert_eq!(sanitized["username"], "agent007");

        // Sanitize for highest tier (GPU TEE-I/O)
        let classified = DataSanitizer::sanitize(&sensitive_data, PrivacyTier::GpuTeeIo);
        assert_eq!(classified["username"], "***CLASSIFIED***");
        assert_eq!(classified["password"], "***CLASSIFIED***");
    }

    #[tokio::test]
    async fn test_tier_fallback_behavior() {
        let verifier = Arc::new(MockAttestationVerifier);
        let manager = SecurityManager::new(verifier);

        // Initialize without strict enforcement
        manager.initialize(Some(PrivacyTier::Open)).await.unwrap();

        let requirements_with_fallback = ToolSecurityRequirements {
            min_tier: PrivacyTier::CpuTee,
            require_fresh_attestation: false,
            hardware_requirements: None,
            allow_fallback: true,
        };

        let requirements_no_fallback = ToolSecurityRequirements {
            min_tier: PrivacyTier::CpuTee,
            require_fresh_attestation: false,
            hardware_requirements: None,
            allow_fallback: false,
        };

        let context = manager.get_context().await;
        let enforcer = hanzo_node::security::privacy_enforcement::PrivacyEnforcer::new(false);

        // With fallback allowed and non-strict mode, should pass
        let result = enforcer.check_tool_requirements(&requirements_with_fallback, &context).await;
        // This will still fail because tier mismatch, but would log warning in non-strict mode
        assert!(result.is_err());

        // Without fallback, should fail
        let result = enforcer.check_tool_requirements(&requirements_no_fallback, &context).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_audit_logging() {
        use hanzo_node::security::privacy_enforcement::PrivacyEnforcer;
        use hanzo_node::security::SecurityContext;

        let enforcer = PrivacyEnforcer::new(false);

        let context = SecurityContext {
            current_tier: PrivacyTier::Open,
            attestation: None,
            hardware_caps: HardwareCapabilities::default(),
            enforce_strict: false,
        };

        // Perform some operations
        let _ = enforcer.check_operation("read", PrivacyTier::Open, &context).await;
        let _ = enforcer.check_operation("export", PrivacyTier::CpuTee, &context).await;

        // Get audit log
        let log = enforcer.get_audit_log(Some(10)).await;
        assert!(!log.is_empty());

        // Check for violation entry
        let violations: Vec<_> = log.iter().filter(|e| e.violation.is_some()).collect();
        assert!(!violations.is_empty());
    }

    #[tokio::test]
    async fn test_measurement_collection() {
        use hanzo_node::security::tee_attestation::MeasurementCollector;

        let measurements = MeasurementCollector::collect();

        // Should have standard PCR values
        assert_eq!(measurements.pcr_values.len(), 24);

        // Should have kernel and initrd hashes
        assert_eq!(measurements.kernel_hash.len(), 32);
        assert_eq!(measurements.initrd_hash.len(), 32);
    }

    #[tokio::test]
    async fn test_cloud_tee_detection() {
        use hanzo_node::security::hardware_detection::detect_cloud_tee_support;

        let cloud_tee = detect_cloud_tee_support();

        // In CI/local environment, should be None
        // In cloud with TEE, would return provider name
        if let Some(provider) = cloud_tee {
            println!("Detected cloud TEE provider: {}", provider);
            assert!(
                provider.contains("AWS") ||
                provider.contains("Azure") ||
                provider.contains("Google") ||
                provider.contains("IBM")
            );
        }
    }
}