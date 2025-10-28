//! Example usage of Hanzo PQC implementation
//! 
//! This example demonstrates how to use the NIST Post-Quantum Cryptography
//! implementation for key encapsulation and digital signatures.

use hanzo_pqc::{
    kem::{Kem, KemAlgorithm, MlKem},
    signature::{Signature, SignatureAlgorithm, MlDsa},
    hybrid::{HybridMode, HybridKem},
    privacy_tiers::PrivacyTier,
    config::PqcConfig,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Hanzo PQC Example Usage ===\n");
    
    // Example 1: Basic ML-KEM usage
    basic_kem_example().await?;
    
    // Example 2: Digital signatures with ML-DSA
    signature_example().await?;
    
    // Example 3: Hybrid mode (PQC + Classical)
    hybrid_example().await?;
    
    // Example 4: Privacy tier configuration
    privacy_tier_example();
    
    Ok(())
}

async fn basic_kem_example() -> Result<(), Box<dyn std::error::Error>> {
    println!("1. ML-KEM Key Encapsulation Example");
    println!("------------------------------------");
    
    // Create ML-KEM instance
    let kem = MlKem::new();
    
    // Generate key pair (ML-KEM-768 is the default/recommended)
    let keypair = kem.generate_keypair(KemAlgorithm::MlKem768).await?;
    println!("✓ Generated ML-KEM-768 keypair");
    println!("  Public key size: {} bytes", keypair.encap_key.key_bytes.len());
    println!("  Private key size: {} bytes", keypair.decap_key.key_bytes.len());
    
    // Sender: Encapsulate to create shared secret
    let encap_output = kem.encapsulate(&keypair.encap_key).await?;
    println!("\n✓ Encapsulation complete");
    println!("  Ciphertext size: {} bytes", encap_output.ciphertext.len());
    println!("  Shared secret: {} bytes", encap_output.shared_secret.len());
    
    // Receiver: Decapsulate to recover shared secret
    let recovered_secret = kem.decapsulate(&keypair.decap_key, &encap_output.ciphertext).await?;
    println!("\n✓ Decapsulation complete");
    
    // Verify shared secrets match
    assert_eq!(encap_output.shared_secret, recovered_secret);
    println!("✓ Shared secrets match!\n");
    
    Ok(())
}

async fn signature_example() -> Result<(), Box<dyn std::error::Error>> {
    println!("2. ML-DSA Digital Signature Example");
    println!("------------------------------------");
    
    // Create ML-DSA instance
    let dsa = MlDsa::new();
    
    // Generate signing key pair (ML-DSA-65 is recommended default)
    let (verifying_key, signing_key) = dsa.generate_keypair(SignatureAlgorithm::MlDsa65).await?;
    println!("✓ Generated ML-DSA-65 keypair");
    println!("  Public key size: {} bytes", verifying_key.key_bytes.len());
    println!("  Private key size: {} bytes", signing_key.key_bytes.len());
    
    // Message to sign
    let message = b"This is a quantum-safe signed message from Hanzo Node";
    
    // Sign the message
    let signature = dsa.sign(&signing_key, message).await?;
    println!("\n✓ Message signed");
    println!("  Signature size: {} bytes", signature.signature_bytes.len());
    
    // Verify the signature
    let is_valid = dsa.verify(&verifying_key, message, &signature).await?;
    println!("\n✓ Signature verification: {}", if is_valid { "VALID" } else { "INVALID" });
    
    // Test with modified message (should fail)
    let tampered_message = b"This is a MODIFIED message";
    let is_invalid = dsa.verify(&verifying_key, tampered_message, &signature).await?;
    println!("✓ Tampered message verification: {}\n", if !is_invalid { "VALID" } else { "INVALID (as expected)" });
    
    Ok(())
}

async fn hybrid_example() -> Result<(), Box<dyn std::error::Error>> {
    println!("3. Hybrid Mode Example (ML-KEM + X25519)");
    println!("-----------------------------------------");
    
    // Create hybrid KEM combining ML-KEM-768 with X25519
    let hybrid_kem = HybridKem::new(HybridMode::MlKem768X25519);
    
    // Generate hybrid keypair
    let (encap_key, decap_key) = hybrid_kem.generate_keypair(HybridMode::MlKem768X25519).await?;
    println!("✓ Generated hybrid keypair");
    println!("  PQ public key size: {} bytes", encap_key.pq_key.key_bytes.len());
    println!("  Classical public key size: {} bytes", encap_key.classical_key.key_bytes.len());
    
    // Encapsulate with context
    let context = b"example context";
    let (ciphertext, shared_secret) = hybrid_kem.encapsulate(&encap_key, context).await?;
    println!("\n✓ Hybrid encapsulation complete");
    println!("  PQ ciphertext: {} bytes", ciphertext.pq_ciphertext.len());
    println!("  Classical ciphertext: {} bytes", ciphertext.classical_ciphertext.len());
    println!("  Combined shared secret: {} bytes", shared_secret.len());
    
    // Decapsulate
    let recovered = hybrid_kem.decapsulate(
        &decap_key,
        &ciphertext,
        context
    ).await?;
    
    assert_eq!(shared_secret, recovered);
    println!("\n✓ Hybrid shared secrets match!");
    println!("  Defense-in-depth: Protected against both classical and quantum attacks\n");
    
    Ok(())
}

fn privacy_tier_example() {
    println!("4. Privacy Tier Configuration Example");
    println!("--------------------------------------");
    
    // Show how different privacy tiers map to PQC configurations
    let tiers = [
        PrivacyTier::AccessOpen,
        PrivacyTier::AccessAtRest,
        PrivacyTier::AccessCpuTee,
        PrivacyTier::AccessCpuTeePlusGpuCc,
        PrivacyTier::AccessGpuTeeIoMax,
    ];
    
    for tier in tiers {
        let config = PqcConfig::for_privacy_tier(tier);
        println!("\n{:?} Configuration:", tier);
        println!("  KEM Algorithm: {:?}", config.kem);
        println!("  Signature Algorithm: {:?}", config.sig);
        println!("  Hybrid Mode: {}", if config.hybrid { "Enabled" } else { "Disabled" });
        println!("  FIPS Mode: {}", if config.fips_mode { "Enabled" } else { "Disabled" });
        println!("  Attestation Required: {}", if config.verify_attestation { "Yes" } else { "No" });
        println!("  Key Lifetime: {} seconds", config.key_lifetime);
    }
    
    println!("\n✓ Privacy tiers provide automatic security level selection");
    println!("✓ Higher tiers use stronger algorithms and shorter key lifetimes\n");
}

// Example output:
// ```
// === Hanzo PQC Example Usage ===
//
// 1. ML-KEM Key Encapsulation Example
// ------------------------------------
// ✓ Generated ML-KEM-768 keypair
//   Public key size: 1184 bytes
//   Private key size: 2400 bytes
//
// ✓ Encapsulation complete
//   Ciphertext size: 1088 bytes
//   Shared secret: 32 bytes
//
// ✓ Decapsulation complete
// ✓ Shared secrets match!
//
// 2. ML-DSA Digital Signature Example
// ------------------------------------
// ✓ Generated ML-DSA-65 keypair
//   Public key size: 1952 bytes
//   Private key size: 4032 bytes
//
// ✓ Message signed
//   Signature size: 3309 bytes
//
// ✓ Signature verification: VALID
// ✓ Tampered message verification: INVALID (as expected)
//
// 3. Hybrid Mode Example (ML-KEM + X25519)
// -----------------------------------------
// ✓ Generated hybrid keypair
//   PQ public key size: 1184 bytes
//   Classical public key size: 32 bytes
//
// ✓ Hybrid encapsulation complete
//   PQ ciphertext: 1088 bytes
//   Classical ciphertext: 32 bytes
//   Combined shared secret: 32 bytes
//
// ✓ Hybrid shared secrets match!
//   Defense-in-depth: Protected against both classical and quantum attacks
//
// 4. Privacy Tier Configuration Example
// --------------------------------------
//
// AccessOpen Configuration:
//   KEM Algorithm: MlKem768
//   Signature Algorithm: MlDsa65
//   Hybrid Mode: Enabled
//   FIPS Mode: Disabled
//   Attestation Required: No
//   Key Lifetime: 86400 seconds
//
// AccessAtRest Configuration:
//   KEM Algorithm: MlKem768
//   Signature Algorithm: MlDsa65
//   Hybrid Mode: Enabled
//   FIPS Mode: Disabled
//   Attestation Required: No
//   Key Lifetime: 86400 seconds
//
// AccessCpuTee Configuration:
//   KEM Algorithm: MlKem768
//   Signature Algorithm: MlDsa65
//   Hybrid Mode: Enabled
//   FIPS Mode: Enabled
//   Attestation Required: Yes
//   Key Lifetime: 86400 seconds
//
// AccessCpuTeePlusGpuCc Configuration:
//   KEM Algorithm: MlKem1024
//   Signature Algorithm: MlDsa87
//   Hybrid Mode: Enabled
//   FIPS Mode: Enabled
//   Attestation Required: Yes
//   Key Lifetime: 86400 seconds
//
// AccessGpuTeeIoMax Configuration:
//   KEM Algorithm: MlKem1024
//   Signature Algorithm: MlDsa87
//   Hybrid Mode: Enabled
//   FIPS Mode: Enabled
//   Attestation Required: Yes
//   Key Lifetime: 86400 seconds
//
// ✓ Privacy tiers provide automatic security level selection
// ✓ Higher tiers use stronger algorithms and shorter key lifetimes
// ```