# FIPS Compliance Documentation for Hanzo PQC

## Overview

This document details the FIPS (Federal Information Processing Standards) compliance of the Hanzo Post-Quantum Cryptography implementation, specifically adherence to:

- **FIPS 203**: Module-Lattice-Based Key-Encapsulation Mechanism (ML-KEM)
- **FIPS 204**: Module-Lattice-Based Digital Signature Algorithm (ML-DSA)
- **FIPS 205**: Stateless Hash-Based Digital Signature Algorithm (SLH-DSA)
- **SP 800-56C Rev. 2**: Key Derivation through Extraction-then-Expansion
- **SP 800-90A Rev. 1**: Random Number Generation

## FIPS 203 Compliance (ML-KEM)

### Implemented Parameter Sets

| Parameter Set | Security Level | Public Key Size | Ciphertext Size | Shared Secret |
|--------------|----------------|-----------------|-----------------|---------------|
| ML-KEM-512   | NIST Level 1   | 800 bytes      | 768 bytes      | 32 bytes     |
| ML-KEM-768   | NIST Level 3   | 1184 bytes     | 1088 bytes     | 32 bytes     |
| ML-KEM-1024  | NIST Level 5   | 1568 bytes     | 1568 bytes     | 32 bytes     |

### Implementation Details

```rust
// Location: hanzo-libs/hanzo-pqc/src/kem.rs
pub enum KemAlgorithm {
    MlKem512,   // FIPS 203 compliant
    MlKem768,   // FIPS 203 compliant (DEFAULT)
    MlKem1024,  // FIPS 203 compliant
}
```

### Key Features
- ✅ All three ML-KEM parameter sets implemented
- ✅ Correct key and ciphertext sizes per FIPS 203
- ✅ 32-byte shared secret output for all variants
- ✅ IND-CCA2 secure through implicit rejection
- ✅ Uses liboqs v0.11 for NIST-approved implementation

## FIPS 204 Compliance (ML-DSA)

### Implemented Parameter Sets

| Parameter Set | Security Level | Public Key Size | Private Key Size | Signature Size |
|--------------|----------------|-----------------|------------------|----------------|
| ML-DSA-44    | NIST Level 2   | 1312 bytes     | 2560 bytes      | 2420 bytes    |
| ML-DSA-65    | NIST Level 3   | 1952 bytes     | 4032 bytes      | 3309 bytes    |
| ML-DSA-87    | NIST Level 5   | 2592 bytes     | 4896 bytes      | 4627 bytes    |

### Implementation Details

```rust
// Location: hanzo-libs/hanzo-pqc/src/signature.rs
pub enum SignatureAlgorithm {
    MlDsa44,   // FIPS 204 compliant
    MlDsa65,   // FIPS 204 compliant (DEFAULT)
    MlDsa87,   // FIPS 204 compliant
}
```

### Key Features
- ✅ All three ML-DSA parameter sets implemented
- ✅ Correct key and signature sizes per FIPS 204
- ✅ Deterministic signatures for reproducibility
- ✅ Strong EUF-CMA security
- ✅ Uses liboqs v0.11 for NIST-approved implementation

## SP 800-56C Rev. 2 Compliance (KDF)

### Key Derivation Function Implementation

```rust
// Location: hanzo-libs/hanzo-pqc/src/kdf.rs
pub enum KdfAlgorithm {
    HkdfSha256,  // SP 800-56C compliant
    HkdfSha384,  // SP 800-56C compliant (DEFAULT for hybrid)
    HkdfSha512,  // SP 800-56C compliant
}
```

### Hybrid Mode Secret Combination

The implementation follows SP 800-56C Rev. 2 Section 5.8.2 for combining shared secrets:

```rust
// Combines ML-KEM and X25519 shared secrets
pub fn combine_shared_secrets(
    kdf: &impl Kdf,
    secrets: &[&[u8]],  // [ml_kem_secret, x25519_secret]
    context: &[u8],
    output_len: usize,
) -> Result<Vec<u8>>
```

### Key Features
- ✅ HKDF-Extract-Expand pattern per SP 800-56C
- ✅ Proper salt and info parameter handling
- ✅ Concatenation of multiple shared secrets
- ✅ Domain separation through context strings

## SP 800-90A Compliance (RNG)

### Random Number Generation

```rust
// Location: hanzo-libs/hanzo-pqc/src/config.rs
pub enum RngSource {
    Os,        // OS-provided RNG (getrandom)
    Hardware,  // Hardware RNG (RDRAND/RDSEED)
    FipsDrbg,  // FIPS 140-3 approved DRBG
}
```

### Key Features
- ✅ Uses OS entropy sources via `getrandom` crate
- ✅ Supports hardware RNG when available
- ✅ Option for FIPS-approved DRBG in FIPS mode

## FIPS Mode Configuration

### Enabling FIPS Mode

```rust
// Maximum security configuration with FIPS mode
let config = PqcConfig::maximum_security();
assert!(config.fips_mode);
assert_eq!(config.kem, KemAlgorithm::MlKem1024);
assert_eq!(config.sig, SignatureAlgorithm::MlDsa87);
assert_eq!(config.rng, RngSource::FipsDrbg);
```

### FIPS Mode Enforcement

When `fips_mode` is enabled:
1. Only NIST-approved algorithms are used
2. Strongest parameter sets selected (Level 5)
3. Attestation verification required
4. Shorter key lifetimes enforced
5. FIPS-approved RNG required

## Privacy Tier Mapping

| Privacy Tier | ML-KEM | ML-DSA | FIPS Mode | Attestation |
|-------------|---------|---------|-----------|-------------|
| Tier 0: Open | ML-KEM-768 | ML-DSA-65 | No | No |
| Tier 1: At-Rest | ML-KEM-768 | ML-DSA-65 | No | No |
| Tier 2: CPU TEE | ML-KEM-768 | ML-DSA-65 | Yes | Yes |
| Tier 3: GPU CC | ML-KEM-1024 | ML-DSA-87 | Yes | Yes |
| Tier 4: GPU TEE-I/O | ML-KEM-1024 | ML-DSA-87 | Yes | Yes |

## Compliance Testing

### Test Coverage

```bash
# Run FIPS compliance tests
cargo test --package hanzo_pqc --features "ml-kem ml-dsa fips-mode"

# Run benchmarks to verify performance
cargo bench --package hanzo_pqc
```

### Key Test Areas
- ✅ Parameter set correctness
- ✅ Key size validation
- ✅ Ciphertext/signature size validation
- ✅ Shared secret size (always 32 bytes)
- ✅ KDF output validation
- ✅ Hybrid mode secret combination
- ✅ Deterministic operation verification

## Security Considerations

### Post-Quantum Security Levels

| NIST Level | Classical Security | Quantum Security | Hanzo Default |
|------------|-------------------|------------------|---------------|
| Level 1 | 128-bit | 64-bit | ML-KEM-512, ML-DSA-44 |
| Level 3 | 192-bit | 96-bit | **ML-KEM-768, ML-DSA-65** ✓ |
| Level 5 | 256-bit | 128-bit | ML-KEM-1024, ML-DSA-87 |

### Defense in Depth

The implementation provides multiple layers of security:

1. **Hybrid Mode**: Combines ML-KEM with X25519 ECDH
2. **Algorithm Agility**: Easy migration between parameter sets
3. **Side-Channel Protection**: Constant-time operations via liboqs
4. **Key Zeroization**: Automatic memory clearing for sensitive data
5. **Attestation**: TEE-based key release policies

## Certification Status

### Current Status
- Implementation based on NIST final standards (August 2024)
- Uses liboqs v0.11 (NIST reference implementation)
- Ready for FIPS 140-3 validation process

### Validation Checklist
- [x] FIPS 203 ML-KEM implementation
- [x] FIPS 204 ML-DSA implementation
- [x] SP 800-56C KDF implementation
- [x] SP 800-90A RNG compliance
- [x] Key size compliance
- [x] Algorithm parameter compliance
- [x] Test vector validation (via liboqs)
- [ ] FIPS 140-3 module validation (pending)
- [ ] CAVP algorithm validation (pending)

## Usage in FIPS Mode

```rust
use hanzo_pqc::{
    config::PqcConfig,
    kem::{Kem, MlKem},
    signature::{Signature, MlDsa},
};

// Create FIPS-compliant configuration
let config = PqcConfig {
    fips_mode: true,
    kem: KemAlgorithm::MlKem1024,
    sig: SignatureAlgorithm::MlDsa87,
    rng: RngSource::FipsDrbg,
    verify_attestation: true,
    ..Default::default()
};

// Use only FIPS-approved operations
let kem = MlKem::new();
let dsa = MlDsa::new();

// All operations now FIPS-compliant
let keypair = kem.generate_keypair(config.kem).await?;
let (vk, sk) = dsa.generate_keypair(config.sig).await?;
```

## References

1. [FIPS 203](https://csrc.nist.gov/pubs/fips/203/final): Module-Lattice-Based Key-Encapsulation Mechanism Standard
2. [FIPS 204](https://csrc.nist.gov/pubs/fips/204/final): Module-Lattice-Based Digital Signature Standard
3. [FIPS 205](https://csrc.nist.gov/pubs/fips/205/final): Stateless Hash-Based Digital Signature Standard
4. [SP 800-56C Rev. 2](https://csrc.nist.gov/pubs/sp/800/56/c/r2/final): Recommendation for Key-Derivation Methods in Key-Establishment Schemes
5. [SP 800-90A Rev. 1](https://csrc.nist.gov/pubs/sp/800/90/a/r1/final): Recommendation for Random Number Generation Using Deterministic Random Bit Generators

---

*Last Updated: December 2024*
*Hanzo PQC Version: 1.1.8*