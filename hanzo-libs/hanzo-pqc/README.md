# Hanzo Post-Quantum Cryptography (PQC)

[![FIPS 203](https://img.shields.io/badge/FIPS%20203-Compliant-green)](https://csrc.nist.gov/pubs/fips/203/final)
[![FIPS 204](https://img.shields.io/badge/FIPS%20204-Compliant-green)](https://csrc.nist.gov/pubs/fips/204/final)
[![SP 800-56C](https://img.shields.io/badge/SP%20800--56C-Compliant-green)](https://csrc.nist.gov/pubs/sp/800/56/c/r2/final)

Production-ready implementation of NIST Post-Quantum Cryptography standards for the Hanzo Node ecosystem, providing quantum-resistant security for key establishment and digital signatures.

## Features

- 🔐 **FIPS 203 ML-KEM**: Quantum-safe key encapsulation (Kyber)
- ✍️ **FIPS 204 ML-DSA**: Quantum-safe digital signatures (Dilithium)
- 🔄 **Hybrid Mode**: Combines PQC with classical cryptography for defense-in-depth
- 🛡️ **Privacy Tiers**: Automatic security level selection based on deployment environment
- ⚡ **High Performance**: Optimized for both security and speed
- 🏭 **Production Ready**: Comprehensive testing, benchmarks, and documentation

## Quick Start

Add to your `Cargo.toml`:

```toml
[dependencies]
hanzo_pqc = { version = "1.1", features = ["ml-kem", "ml-dsa", "hybrid"] }
```

Basic usage:

```rust
use hanzo_pqc::{
    kem::{Kem, KemAlgorithm, MlKem},
    signature::{Signature, SignatureAlgorithm, MlDsa},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Key Encapsulation
    let kem = MlKem::new();
    let keypair = kem.generate_keypair(KemAlgorithm::MlKem768).await?;
    let output = kem.encapsulate(&keypair.encap_key).await?;
    let shared_secret = kem.decapsulate(&keypair.decap_key, &output.ciphertext).await?;
    
    // Digital Signatures
    let dsa = MlDsa::new();
    let (verifying_key, signing_key) = dsa.generate_keypair(SignatureAlgorithm::MlDsa65).await?;
    let message = b"Quantum-safe message";
    let signature = dsa.sign(&signing_key, message).await?;
    let valid = dsa.verify(&verifying_key, message, &signature).await?;
    
    Ok(())
}
```

## Algorithm Support

### ML-KEM (FIPS 203)

| Parameter | Security Level | Use Case |
|-----------|---------------|----------|
| ML-KEM-512 | NIST Level 1 (128-bit) | Lightweight/IoT |
| **ML-KEM-768** | **NIST Level 3 (192-bit)** | **Default/Recommended** |
| ML-KEM-1024 | NIST Level 5 (256-bit) | Maximum Security |

### ML-DSA (FIPS 204)

| Parameter | Security Level | Use Case |
|-----------|---------------|----------|
| ML-DSA-44 | NIST Level 2 (128-bit) | Performance-critical |
| **ML-DSA-65** | **NIST Level 3 (192-bit)** | **Default/Recommended** |
| ML-DSA-87 | NIST Level 5 (256-bit) | Maximum Security |

## Privacy Tiers

Automatic algorithm selection based on deployment environment:

```rust
use hanzo_pqc::{privacy_tiers::PrivacyTier, config::PqcConfig};

// Automatically selects appropriate algorithms
let config = PqcConfig::for_privacy_tier(PrivacyTier::AccessCpuTee);
```

| Tier | Environment | ML-KEM | ML-DSA | Features |
|------|-------------|---------|---------|----------|
| 0 | Open Data | 768 | 65 | Basic quantum resistance |
| 1 | At-Rest Encryption | 768 | 65 | + SIM key protection |
| 2 | CPU TEE | 768 | 65 | + FIPS mode, attestation |
| 3 | GPU CC (H100) | 1024 | 87 | + Encrypted DMA |
| 4 | GPU TEE-I/O (Blackwell) | 1024 | 87 | + NVLink protection |

## Hybrid Mode

Combines ML-KEM with X25519 for defense against both classical and quantum attacks:

```rust
use hanzo_pqc::hybrid::{HybridMode, HybridKem};

let hybrid = HybridKem::new(HybridMode::MlKem768X25519);
let (encap_key, decap_key) = hybrid.generate_keypair(HybridMode::MlKem768X25519).await?;
```

## Examples

See the [`examples/`](examples/) directory for complete examples:

- [`basic_usage.rs`](examples/basic_usage.rs) - Getting started with PQC
- Run with: `cargo run --example basic_usage --features "ml-kem ml-dsa hybrid"`

## Benchmarks

Run performance benchmarks:

```bash
cargo bench --package hanzo_pqc
```

Typical performance on modern hardware:

| Operation | ML-KEM-768 | ML-DSA-65 |
|-----------|------------|-----------|
| Key Generation | ~50 μs | ~100 μs |
| Encapsulate/Sign | ~60 μs | ~250 μs |
| Decapsulate/Verify | ~70 μs | ~120 μs |

## Testing

```bash
# Run all tests
cargo test --package hanzo_pqc --all-features

# Run with specific features
cargo test --package hanzo_pqc --features "ml-kem ml-dsa"

# Run integration tests
cargo test --package hanzo_pqc --test integration_tests
```

## Features

- `ml-kem` - ML-KEM key encapsulation (default)
- `ml-dsa` - ML-DSA digital signatures (default)
- `slh-dsa` - SLH-DSA hash-based signatures (optional)
- `hybrid` - Hybrid PQC+Classical mode (default)
- `fips-mode` - FIPS 140-3 compliance mode
- `gpu-cc` - GPU Confidential Computing support
- `tee-io` - GPU TEE-I/O (Blackwell) support

## Security Considerations

1. **Quantum Resistance**: All algorithms are designed to resist attacks from both classical and quantum computers
2. **Side-Channel Protection**: Implementation uses constant-time operations where possible
3. **Key Zeroization**: Sensitive key material is automatically zeroed on drop
4. **Hybrid Mode**: Provides defense-in-depth by combining PQC with classical crypto
5. **Algorithm Agility**: Easy to upgrade to stronger parameters as needed

## FIPS Compliance

This implementation adheres to:
- FIPS 203 (ML-KEM)
- FIPS 204 (ML-DSA)
- SP 800-56C Rev. 2 (KDF)
- SP 800-90A Rev. 1 (RNG)

See [FIPS_COMPLIANCE.md](FIPS_COMPLIANCE.md) for detailed compliance information.

## Dependencies

- [`oqs`](https://github.com/open-quantum-safe/liboqs-rust) v0.11 - NIST reference implementations
- [`x25519-dalek`](https://github.com/dalek-cryptography/x25519-dalek) - Classical ECDH for hybrid mode
- [`hkdf`](https://github.com/RustCrypto/KDFs) - SP 800-56C compliant KDF
- [`chacha20poly1305`](https://github.com/RustCrypto/AEADs) - AEAD for key wrapping

## License

Apache 2.0 / MIT dual license

## Contributing

Contributions welcome! Please ensure:
1. All tests pass
2. Code follows Rust conventions
3. Security considerations are documented
4. Benchmarks show no performance regression

## Support

For issues and questions:
- GitHub Issues: [hanzo-node/issues](https://github.com/hanzoai/hanzo-node/issues)
- Documentation: [docs.hanzo.ai](https://docs.hanzo.ai)

---

Built with 🔒 by Hanzo AI for quantum-safe future