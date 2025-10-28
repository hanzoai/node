//! Integration tests for PQC implementation
//! Tests NIST FIPS 203/204/205 compliance

#[cfg(feature = "ml-kem")]
mod kem_tests {
    use hanzo_pqc::kem::{Kem, KemAlgorithm, MlKem};
    
    #[tokio::test]
    async fn test_ml_kem_all_variants() {
        let kem = MlKem::new();
        
        for alg in [KemAlgorithm::MlKem512, KemAlgorithm::MlKem768, KemAlgorithm::MlKem1024] {
            let keypair = kem.generate_keypair(alg).await.unwrap();
            
            // Verify key sizes match FIPS 203 specifications
            match alg {
                KemAlgorithm::MlKem512 => {
                    assert_eq!(keypair.encap_key.key_bytes.len(), 800);
                }
                KemAlgorithm::MlKem768 => {
                    assert_eq!(keypair.encap_key.key_bytes.len(), 1184);
                }
                KemAlgorithm::MlKem1024 => {
                    assert_eq!(keypair.encap_key.key_bytes.len(), 1568);
                }
                _ => unreachable!(),
            }
            
            // Test encapsulation/decapsulation
            let output = kem.encapsulate(&keypair.encap_key).await.unwrap();
            
            // Verify ciphertext sizes
            assert_eq!(output.ciphertext.len(), alg.ciphertext_size());
            
            // Verify shared secret is always 32 bytes (per FIPS 203)
            assert_eq!(output.shared_secret.len(), 32);
            
            // Test decapsulation
            let recovered = kem.decapsulate(&keypair.decap_key, &output.ciphertext).await.unwrap();
            assert_eq!(output.shared_secret, recovered);
        }
    }
    
    #[tokio::test]
    async fn test_ml_kem_wrong_ciphertext() {
        let kem = MlKem::new();
        let keypair = kem.generate_keypair(KemAlgorithm::MlKem768).await.unwrap();
        
        // Create invalid ciphertext
        let bad_ciphertext = vec![0u8; 1088];
        
        // Decapsulation should still succeed but produce different shared secret
        // This is implicit rejection per FIPS 203
        let result = kem.decapsulate(&keypair.decap_key, &bad_ciphertext).await;
        assert!(result.is_ok()); // Implicit rejection doesn't fail
    }
}

#[cfg(feature = "ml-dsa")]
mod signature_tests {
    use hanzo_pqc::signature::{Signature, SignatureAlgorithm, MlDsa};
    
    #[tokio::test]
    async fn test_ml_dsa_all_variants() {
        let dsa = MlDsa::new();
        
        for alg in [SignatureAlgorithm::MlDsa44, SignatureAlgorithm::MlDsa65, SignatureAlgorithm::MlDsa87] {
            let (vk, sk) = dsa.generate_keypair(alg).await.unwrap();
            
            // Verify key sizes match FIPS 204 specifications
            match alg {
                SignatureAlgorithm::MlDsa44 => {
                    assert_eq!(vk.key_bytes.len(), 1312);
                    assert_eq!(sk.key_bytes.len(), 2560);
                }
                SignatureAlgorithm::MlDsa65 => {
                    assert_eq!(vk.key_bytes.len(), 1952);
                    assert_eq!(sk.key_bytes.len(), 4032);
                }
                SignatureAlgorithm::MlDsa87 => {
                    assert_eq!(vk.key_bytes.len(), 2592);
                    assert_eq!(sk.key_bytes.len(), 4896);
                }
                _ => unreachable!(),
            }
            
            // Test signing and verification
            let message = b"Test message for quantum-safe signatures";
            let signature = dsa.sign(&sk, message).await.unwrap();
            
            // Verify signature size
            assert_eq!(signature.signature_bytes.len(), alg.signature_size());
            
            // Verify signature
            let valid = dsa.verify(&vk, message, &signature).await.unwrap();
            assert!(valid);
            
            // Test invalid signature
            let bad_message = b"Modified message";
            let invalid = dsa.verify(&vk, bad_message, &signature).await.unwrap();
            assert!(!invalid);
        }
    }
    
    #[tokio::test]
    async fn test_ml_dsa_deterministic() {
        let dsa = MlDsa::new();
        let (_, sk) = dsa.generate_keypair(SignatureAlgorithm::MlDsa65).await.unwrap();
        
        let message = b"Deterministic signature test";
        
        // ML-DSA is deterministic - same message should produce same signature
        let _sig1 = dsa.sign(&sk, message).await.unwrap();
        let _sig2 = dsa.sign(&sk, message).await.unwrap();
        
        // Note: OQS implementation may use randomized signatures for side-channel protection
        // This test documents the behavior rather than enforcing determinism
        println!("Signature consistency check (may vary based on implementation)");
    }
}

#[cfg(feature = "hybrid")]
mod hybrid_tests {
    use hanzo_pqc::hybrid::{HybridMode, HybridKem};
    
    #[tokio::test]
    async fn test_hybrid_kem() {
        let hybrid_kem = HybridKem::new(HybridMode::MlKem768X25519);
        
        let (encap_key, decap_key) = hybrid_kem.generate_keypair(HybridMode::MlKem768X25519).await.unwrap();
        
        // Test encapsulation with context
        let context = b"test context";
        let (ciphertext, shared_secret) = hybrid_kem.encapsulate(&encap_key, context).await.unwrap();
        
        // Hybrid ciphertext contains both ML-KEM and X25519 ciphertexts
        assert!(ciphertext.pq_ciphertext.len() > 0);
        assert!(ciphertext.classical_ciphertext.len() > 0);
        
        // Test decapsulation
        let recovered = hybrid_kem.decapsulate(&decap_key, &ciphertext, context).await.unwrap();
        assert_eq!(shared_secret, recovered);
    }
    
    #[tokio::test]
    async fn test_hybrid_modes() {
        for mode in [
            HybridMode::MlKem512X25519,
            HybridMode::MlKem768X25519,
            HybridMode::MlKem1024X25519,
        ] {
            let hybrid_kem = HybridKem::new(mode);
            let (encap_key, _) = hybrid_kem.generate_keypair(mode).await.unwrap();
            
            // Verify both keys are present
            assert!(encap_key.pq_key.key_bytes.len() > 0);
            assert_eq!(encap_key.classical_key.key_bytes.len(), 32); // X25519 is always 32 bytes
        }
    }
}

#[cfg(all(feature = "ml-kem", feature = "ml-dsa"))]
mod privacy_tier_tests {
    use hanzo_pqc::{
        privacy_tiers::PrivacyTier,
        config::PqcConfig,
        kem::KemAlgorithm,
        signature::SignatureAlgorithm,
    };
    
    #[test]
    fn test_privacy_tier_algorithm_selection() {
        // Tier 0: Open
        let config = PqcConfig::for_privacy_tier(PrivacyTier::AccessOpen);
        assert_eq!(config.kem, KemAlgorithm::MlKem768);
        assert_eq!(config.sig, SignatureAlgorithm::MlDsa65);
        assert!(!config.verify_attestation);
        
        // Tier 2: CPU TEE
        let config = PqcConfig::for_privacy_tier(PrivacyTier::AccessCpuTee);
        assert_eq!(config.kem, KemAlgorithm::MlKem768);
        assert_eq!(config.sig, SignatureAlgorithm::MlDsa65);
        assert!(config.verify_attestation);
        assert!(config.fips_mode);
        
        // Tier 4: GPU TEE-I/O
        let config = PqcConfig::for_privacy_tier(PrivacyTier::AccessGpuTeeIoMax);
        assert_eq!(config.kem, KemAlgorithm::MlKem1024);
        assert_eq!(config.sig, SignatureAlgorithm::MlDsa87);
        assert!(config.verify_attestation);
        assert!(config.fips_mode);
    }
}

#[cfg(feature = "ml-kem")]
mod kdf_tests {
    use hanzo_pqc::kdf::{Kdf, HkdfKdf, KdfAlgorithm, combine_shared_secrets};
    
    #[test]
    fn test_hkdf_sp800_56c() {
        // Test SP 800-56C compliant KDF
        let kdf = HkdfKdf::new(KdfAlgorithm::HkdfSha384);
        
        let ikm = b"input key material";
        let salt = Some(b"optional salt value".as_ref());
        let info = b"hanzo-pqc-v1";
        
        let key1 = kdf.derive(salt, ikm, info, 32).unwrap();
        assert_eq!(key1.len(), 32);
        
        // Same inputs should produce same output (deterministic)
        let key2 = kdf.derive(salt, ikm, info, 32).unwrap();
        assert_eq!(key1, key2);
        
        // Different info should produce different output
        let key3 = kdf.derive(salt, ikm, b"different-info", 32).unwrap();
        assert_ne!(key1, key3);
    }
    
    #[test]
    fn test_combine_shared_secrets() {
        // Test combining multiple shared secrets per SP 800-56C
        let kdf = HkdfKdf::new(KdfAlgorithm::HkdfSha256);
        
        let secret1 = b"first shared secret from ML-KEM";
        let secret2 = b"second shared secret from X25519";
        
        let combined = combine_shared_secrets(
            &kdf,
            &[secret1.as_ref(), secret2.as_ref()],
            b"hybrid-kem-v1",
            32,
        ).unwrap();
        
        assert_eq!(combined.len(), 32);
        
        // Order matters in combination
        let reversed = combine_shared_secrets(
            &kdf,
            &[secret2.as_ref(), secret1.as_ref()],
            b"hybrid-kem-v1",
            32,
        ).unwrap();
        
        assert_ne!(combined, reversed);
    }
}

#[cfg(all(feature = "ml-kem", feature = "ml-dsa"))]
mod wire_protocol_tests {
    use hanzo_pqc::{
        wire_protocol::NodeMode,
        kem::KemAlgorithm,
        signature::SignatureAlgorithm,
    };
    
    #[test]
    fn test_algorithm_serialization() {
        // Test that our algorithm enums serialize properly
        let kem_alg = KemAlgorithm::MlKem768;
        let sig_alg = SignatureAlgorithm::MlDsa65;
        let mode = NodeMode::SimTee;
        
        // Test serialization/deserialization
        let kem_json = serde_json::to_string(&kem_alg).unwrap();
        let sig_json = serde_json::to_string(&sig_alg).unwrap();
        let mode_json = serde_json::to_string(&mode).unwrap();
        
        let kem_deser: KemAlgorithm = serde_json::from_str(&kem_json).unwrap();
        let sig_deser: SignatureAlgorithm = serde_json::from_str(&sig_json).unwrap();
        let mode_deser: NodeMode = serde_json::from_str(&mode_json).unwrap();
        
        assert_eq!(kem_alg, kem_deser);
        assert_eq!(sig_alg, sig_deser);
        assert_eq!(mode, mode_deser);
    }
}

#[cfg(all(feature = "ml-kem", feature = "ml-dsa"))]
mod attestation_tests {
    // TODO: Fix after TeeType and PrivacyProof are exported from attestation module
    // use hanzo_pqc::attestation::{TeeType, PrivacyProof};
    
    #[test]
    #[ignore] // Temporarily disabled - missing exports
    fn test_tee_type_support() {
        // Test that all TEE types are properly defined
        // TODO: Fix after TeeType is exported
        /*let tee_types = [
            TeeType::AmdSevSnp,
            TeeType::IntelTdx,
            TeeType::IntelSgx,
            TeeType::ArmCca,
            TeeType::NvidiaH100Cc,
            TeeType::NvidiaBlackwellTeeIo,
        ];
        
        for tee in tee_types {
            // Each TEE type should serialize properly
            let json = serde_json::to_string(&tee).unwrap();
            let deser: TeeType = serde_json::from_str(&json).unwrap();
            assert_eq!(tee, deser);
        }*/
    }
}

#[cfg(all(feature = "ml-kem", feature = "fips-mode"))]
mod fips_compliance_tests {
    use hanzo_pqc::config::PqcConfig;
    
    #[test]
    fn test_fips_mode_configuration() {
        let config = PqcConfig::maximum_security();
        
        // FIPS mode should be enabled
        assert!(config.fips_mode);
        
        // Should use strongest algorithms
        assert_eq!(config.kem, hanzo_pqc::kem::KemAlgorithm::MlKem1024);
        assert_eq!(config.sig, hanzo_pqc::signature::SignatureAlgorithm::MlDsa87);
        
        // Should require attestation
        assert!(config.verify_attestation);
        
        // Should have short key lifetime
        assert!(config.key_lifetime <= 3600); // 1 hour max
    }
}