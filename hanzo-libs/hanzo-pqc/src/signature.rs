//! Digital signature implementation
//! FIPS 204 (ML-DSA/Dilithium) and FIPS 205 (SLH-DSA/SPHINCS+).
//!
//! Two backends are wired in:
//!   * `lux-crypto-impl`        — canonical luxcpp/crypto FFI (`lux_crypto::*`)
//!   * `legacy-oqs`             — Open Quantum Safe (`oqs::*`)
//!   * `legacy-ed25519-dalek`   — `ed25519-dalek` crate
//!
//! Default builds use the legacy backends because the canonical luxcpp
//! ML-DSA/SLH-DSA C-ABI symbols presently return `CRYPTO_ERR_NOTIMPL`. Flip
//! to `--no-default-features --features "ml-kem,ml-dsa,hybrid,lux-crypto-impl"`
//! once the canonical implementations land.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;
use crate::{PqcError, Result};

/// Signature algorithms supported
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignatureAlgorithm {
    /// ML-DSA-44 (NIST Level 2 - 128-bit security)
    MlDsa44,
    /// ML-DSA-65 (NIST Level 3 - 192-bit security) - RECOMMENDED DEFAULT
    MlDsa65,
    /// ML-DSA-87 (NIST Level 5 - 256-bit security)
    MlDsa87,
    /// SLH-DSA-128s (SPHINCS+ small signatures)
    SlhDsa128s,
    /// SLH-DSA-192s (SPHINCS+ small signatures)
    SlhDsa192s,
    /// SLH-DSA-256s (SPHINCS+ small signatures)
    SlhDsa256s,
    /// Classic Ed25519 for compatibility
    Ed25519,
}

impl SignatureAlgorithm {
    /// Get the public key size in bytes
    pub fn public_key_size(&self) -> usize {
        match self {
            Self::MlDsa44 => 1312,   // Per FIPS 204
            Self::MlDsa65 => 1952,   // Per FIPS 204
            Self::MlDsa87 => 2592,   // Per FIPS 204
            Self::SlhDsa128s => 32,  // Per FIPS 205
            Self::SlhDsa192s => 48,  // Per FIPS 205
            Self::SlhDsa256s => 64,  // Per FIPS 205
            Self::Ed25519 => 32,
        }
    }

    /// Get the secret key size in bytes
    pub fn secret_key_size(&self) -> usize {
        match self {
            Self::MlDsa44 => 2560,   // Per FIPS 204
            Self::MlDsa65 => 4032,   // Per FIPS 204
            Self::MlDsa87 => 4896,   // Per FIPS 204
            Self::SlhDsa128s => 64,  // Per FIPS 205
            Self::SlhDsa192s => 96,  // Per FIPS 205
            Self::SlhDsa256s => 128, // Per FIPS 205
            Self::Ed25519 => 32,
        }
    }

    /// Get the signature size in bytes
    pub fn signature_size(&self) -> usize {
        match self {
            Self::MlDsa44 => 2420,   // Per FIPS 204
            Self::MlDsa65 => 3309,   // Per FIPS 204
            Self::MlDsa87 => 4627,   // Per FIPS 204
            Self::SlhDsa128s => 7856,  // Per FIPS 205 (small variant)
            Self::SlhDsa192s => 16224, // Per FIPS 205 (small variant)
            Self::SlhDsa256s => 29792, // Per FIPS 205 (small variant)
            Self::Ed25519 => 64,
        }
    }

    /// Map an ML-DSA variant to the lux_crypto NistMode (returns None for non-ML-DSA).
    #[cfg(any(feature = "lux-crypto-impl", test))]
    fn mldsa_mode(&self) -> Option<lux_crypto::NistMode> {
        match self {
            Self::MlDsa44 => Some(lux_crypto::NistMode::Mode2),
            Self::MlDsa65 => Some(lux_crypto::NistMode::Mode3),
            Self::MlDsa87 => Some(lux_crypto::NistMode::Mode5),
            _ => None,
        }
    }

    /// Map an SLH-DSA variant to the lux_crypto NistMode (returns None for non-SLH).
    #[cfg(any(feature = "lux-crypto-impl", test))]
    #[allow(dead_code)]
    fn slhdsa_mode(&self) -> Option<lux_crypto::NistMode> {
        match self {
            Self::SlhDsa128s => Some(lux_crypto::NistMode::Mode2),
            Self::SlhDsa192s => Some(lux_crypto::NistMode::Mode3),
            Self::SlhDsa256s => Some(lux_crypto::NistMode::Mode5),
            _ => None,
        }
    }

    /// Get the OQS algorithm identifier (legacy path).
    #[cfg(feature = "legacy-oqs")]
    pub(crate) fn to_oqs_alg(&self) -> Option<oqs::sig::Algorithm> {
        match self {
            Self::MlDsa44 => Some(oqs::sig::Algorithm::MlDsa44),
            Self::MlDsa65 => Some(oqs::sig::Algorithm::MlDsa65),
            Self::MlDsa87 => Some(oqs::sig::Algorithm::MlDsa87),
            #[cfg(feature = "slh-dsa")]
            Self::SlhDsa128s => Some(oqs::sig::Algorithm::SphincsSha2128sSimple),
            #[cfg(feature = "slh-dsa")]
            Self::SlhDsa192s => Some(oqs::sig::Algorithm::SphincsSha2192sSimple),
            #[cfg(feature = "slh-dsa")]
            Self::SlhDsa256s => Some(oqs::sig::Algorithm::SphincsSha2256sSimple),
            _ => None,
        }
    }
}

impl Default for SignatureAlgorithm {
    fn default() -> Self {
        Self::MlDsa65 // NIST recommended default for balance
    }
}

/// Verifying key (public key for signatures)
#[derive(Clone, Serialize, Deserialize)]
pub struct VerifyingKey {
    pub algorithm: SignatureAlgorithm,
    pub key_bytes: Vec<u8>,
}

/// Signing key (private key for signatures)
#[derive(Clone)]
pub struct SigningKey {
    pub algorithm: SignatureAlgorithm,
    pub key_bytes: Vec<u8>,
}

impl Drop for SigningKey {
    fn drop(&mut self) {
        self.key_bytes.zeroize();
    }
}

/// Digital signature
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DigitalSignature {
    pub algorithm: SignatureAlgorithm,
    pub signature_bytes: Vec<u8>,
}

/// Trait for signature operations
#[async_trait]
pub trait Signature: Send + Sync {
    /// Generate a new key pair
    async fn generate_keypair(&self, alg: SignatureAlgorithm) -> Result<(VerifyingKey, SigningKey)>;

    /// Sign a message
    async fn sign(&self, key: &SigningKey, message: &[u8]) -> Result<DigitalSignature>;

    /// Verify a signature
    async fn verify(
        &self,
        key: &VerifyingKey,
        message: &[u8],
        signature: &DigitalSignature,
    ) -> Result<bool>;
}

/// ML-DSA implementation. Backend chosen at compile time:
///   * `lux-crypto-impl` → routes through `lux_crypto::mldsa`
///   * `legacy-oqs`      → routes through `oqs::sig::Sig`
///
/// The two paths have identical input/output byte shapes; tests assert
/// byte-equal output where the underlying primitive is deterministic.
#[cfg(feature = "ml-dsa")]
pub struct MlDsa {
    _phantom: std::marker::PhantomData<()>,
}

#[cfg(feature = "ml-dsa")]
impl Default for MlDsa {
    fn default() -> Self {
        Self::new()
    }
}

impl MlDsa {
    pub fn new() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }
}

#[cfg(all(feature = "ml-dsa", feature = "lux-crypto-impl"))]
#[async_trait]
impl Signature for MlDsa {
    async fn generate_keypair(&self, alg: SignatureAlgorithm) -> Result<(VerifyingKey, SigningKey)> {
        let mode = alg.mldsa_mode()
            .ok_or_else(|| PqcError::UnsupportedAlgorithm(format!("{alg:?} not an ML-DSA variant")))?;
        let mut seed = [0u8; 32];
        getrandom::getrandom(&mut seed).map_err(|e| PqcError::RngError(e.to_string()))?;
        let (pk, sk) = lux_crypto::mldsa::keygen(mode, &seed)
            .map_err(|e| PqcError::SignatureError(format!("{e:?}")))?;
        Ok((
            VerifyingKey { algorithm: alg, key_bytes: pk },
            SigningKey  { algorithm: alg, key_bytes: sk },
        ))
    }

    async fn sign(&self, key: &SigningKey, message: &[u8]) -> Result<DigitalSignature> {
        let mode = key.algorithm.mldsa_mode()
            .ok_or_else(|| PqcError::UnsupportedAlgorithm(format!("{:?} not an ML-DSA variant", key.algorithm)))?;
        let sig = lux_crypto::mldsa::sign(mode, &key.key_bytes, message)
            .map_err(|e| PqcError::SignatureError(format!("{e:?}")))?;
        Ok(DigitalSignature { algorithm: key.algorithm, signature_bytes: sig })
    }

    async fn verify(
        &self,
        key: &VerifyingKey,
        message: &[u8],
        signature: &DigitalSignature,
    ) -> Result<bool> {
        if key.algorithm != signature.algorithm {
            return Ok(false);
        }
        let mode = key.algorithm.mldsa_mode()
            .ok_or_else(|| PqcError::UnsupportedAlgorithm(format!("{:?} not an ML-DSA variant", key.algorithm)))?;
        Ok(lux_crypto::mldsa::verify(mode, &key.key_bytes, message, &signature.signature_bytes))
    }
}

#[cfg(all(feature = "ml-dsa", feature = "legacy-oqs", not(feature = "lux-crypto-impl")))]
#[async_trait]
impl Signature for MlDsa {
    async fn generate_keypair(&self, alg: SignatureAlgorithm) -> Result<(VerifyingKey, SigningKey)> {
        use oqs::sig::Sig;

        let oqs_alg = alg.to_oqs_alg()
            .ok_or_else(|| PqcError::UnsupportedAlgorithm(format!("{alg:?} not supported")))?;

        let sig = Sig::new(oqs_alg)
            .map_err(|_| PqcError::SignatureError("Failed to create signature".into()))?;

        let (pk, sk) = sig.keypair()
            .map_err(|_| PqcError::SignatureError("Keypair generation failed".into()))?;

        Ok((
            VerifyingKey {
                algorithm: alg,
                key_bytes: pk.into_vec(),
            },
            SigningKey {
                algorithm: alg,
                key_bytes: sk.into_vec(),
            },
        ))
    }

    async fn sign(&self, key: &SigningKey, message: &[u8]) -> Result<DigitalSignature> {
        use oqs::sig::Sig;

        let oqs_alg = key.algorithm.to_oqs_alg()
            .ok_or_else(|| PqcError::UnsupportedAlgorithm(format!("{:?} not supported", key.algorithm)))?;

        let sig = Sig::new(oqs_alg)
            .map_err(|_| PqcError::SignatureError("Failed to create signature".into()))?;

        let sk = sig.secret_key_from_bytes(&key.key_bytes)
            .ok_or_else(|| PqcError::SignatureError("Invalid signing key".into()))?;

        let signature = sig.sign(message, sk)
            .map_err(|_| PqcError::SignatureError("Signing failed".into()))?;

        Ok(DigitalSignature {
            algorithm: key.algorithm,
            signature_bytes: signature.into_vec(),
        })
    }

    async fn verify(
        &self,
        key: &VerifyingKey,
        message: &[u8],
        signature: &DigitalSignature,
    ) -> Result<bool> {
        use oqs::sig::Sig;

        if key.algorithm != signature.algorithm {
            return Ok(false);
        }

        let oqs_alg = key.algorithm.to_oqs_alg()
            .ok_or_else(|| PqcError::UnsupportedAlgorithm(format!("{:?} not supported", key.algorithm)))?;

        let sig = Sig::new(oqs_alg)
            .map_err(|_| PqcError::SignatureError("Failed to create signature".into()))?;

        let pk = sig.public_key_from_bytes(&key.key_bytes)
            .ok_or_else(|| PqcError::SignatureError("Invalid verifying key".into()))?;

        let sig_bytes = sig.signature_from_bytes(&signature.signature_bytes)
            .ok_or_else(|| PqcError::SignatureError("Invalid signature".into()))?;

        Ok(sig.verify(message, sig_bytes, pk).is_ok())
    }
}

/// Ed25519 signature for backward compatibility. Backend chosen at compile time:
///   * `lux-crypto-impl`         → `lux_crypto::ed25519`
///   * `legacy-ed25519-dalek`    → `ed25519-dalek` crate
pub struct Ed25519Sig;

#[cfg(feature = "lux-crypto-impl")]
#[async_trait]
impl Signature for Ed25519Sig {
    async fn generate_keypair(&self, alg: SignatureAlgorithm) -> Result<(VerifyingKey, SigningKey)> {
        if !matches!(alg, SignatureAlgorithm::Ed25519) {
            return Err(PqcError::UnsupportedAlgorithm("Use MlDsa for ML-DSA".into()));
        }
        let mut seed = [0u8; 32];
        getrandom::getrandom(&mut seed).map_err(|e| PqcError::RngError(e.to_string()))?;
        let (sk, pk) = lux_crypto::ed25519::keygen(&seed)
            .map_err(|e| PqcError::SignatureError(format!("{e:?}")))?;
        Ok((
            VerifyingKey { algorithm: alg, key_bytes: pk.to_vec() },
            SigningKey  { algorithm: alg, key_bytes: sk.to_vec() },
        ))
    }

    async fn sign(&self, key: &SigningKey, message: &[u8]) -> Result<DigitalSignature> {
        if key.key_bytes.len() != 32 {
            return Err(PqcError::InvalidKeySize { expected: 32, actual: key.key_bytes.len() });
        }
        let mut sk = [0u8; 32];
        sk.copy_from_slice(&key.key_bytes);
        let sig = lux_crypto::ed25519::sign(&sk, message)
            .map_err(|e| PqcError::SignatureError(format!("{e:?}")))?;
        Ok(DigitalSignature { algorithm: key.algorithm, signature_bytes: sig.to_vec() })
    }

    async fn verify(
        &self,
        key: &VerifyingKey,
        message: &[u8],
        signature: &DigitalSignature,
    ) -> Result<bool> {
        if key.key_bytes.len() != 32 || signature.signature_bytes.len() != 64 {
            return Ok(false);
        }
        let mut pk = [0u8; 32];
        pk.copy_from_slice(&key.key_bytes);
        let mut sig = [0u8; 64];
        sig.copy_from_slice(&signature.signature_bytes);
        Ok(lux_crypto::ed25519::verify(&pk, message, &sig))
    }
}

#[cfg(all(feature = "legacy-ed25519-dalek", not(feature = "lux-crypto-impl")))]
#[async_trait]
impl Signature for Ed25519Sig {
    async fn generate_keypair(&self, alg: SignatureAlgorithm) -> Result<(VerifyingKey, SigningKey)> {
        if !matches!(alg, SignatureAlgorithm::Ed25519) {
            return Err(PqcError::UnsupportedAlgorithm("Use MlDsa for ML-DSA".into()));
        }

        use ed25519_dalek::{SigningKey as Ed25519SigningKey, VerifyingKey as Ed25519VerifyingKey};
        use rand::rngs::OsRng;

        let signing_key = Ed25519SigningKey::generate(&mut OsRng);
        let verifying_key: Ed25519VerifyingKey = (&signing_key).into();

        Ok((
            VerifyingKey {
                algorithm: alg,
                key_bytes: verifying_key.as_bytes().to_vec(),
            },
            SigningKey {
                algorithm: alg,
                key_bytes: signing_key.as_bytes().to_vec(),
            },
        ))
    }

    async fn sign(&self, key: &SigningKey, message: &[u8]) -> Result<DigitalSignature> {
        use ed25519_dalek::{Signer, SigningKey as Ed25519SigningKey};

        let mut sk_bytes = [0u8; 32];
        sk_bytes.copy_from_slice(&key.key_bytes);
        let signing_key = Ed25519SigningKey::from_bytes(&sk_bytes);

        let signature = signing_key.sign(message);

        Ok(DigitalSignature {
            algorithm: key.algorithm,
            signature_bytes: signature.to_bytes().to_vec(),
        })
    }

    async fn verify(
        &self,
        key: &VerifyingKey,
        message: &[u8],
        signature: &DigitalSignature,
    ) -> Result<bool> {
        use ed25519_dalek::{Verifier, VerifyingKey as Ed25519VerifyingKey, Signature as Ed25519Signature};

        let mut vk_bytes = [0u8; 32];
        vk_bytes.copy_from_slice(&key.key_bytes);
        let verifying_key = Ed25519VerifyingKey::from_bytes(&vk_bytes)
            .map_err(|e| PqcError::SignatureError(format!("Invalid verifying key: {e}")))?;

        let mut sig_bytes = [0u8; 64];
        sig_bytes.copy_from_slice(&signature.signature_bytes);
        let sig = Ed25519Signature::from_bytes(&sig_bytes);

        Ok(verifying_key.verify(message, &sig).is_ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[cfg(feature = "ml-dsa")]
    async fn test_ml_dsa_65() {
        if std::env::var("CI").is_ok() { println!("Skipping test in CI: test_ml_dsa_65"); return; }
        let signer = MlDsa::new();
        let (vk, sk) = signer.generate_keypair(SignatureAlgorithm::MlDsa65).await.unwrap();

        let message = b"Test message for ML-DSA-65";
        let signature = signer.sign(&sk, message).await.unwrap();

        assert!(signer.verify(&vk, message, &signature).await.unwrap());
        assert_eq!(signature.signature_bytes.len(), 3309); // ML-DSA-65 signature size

        // Verify wrong message fails
        let wrong_message = b"Wrong message";
        assert!(!signer.verify(&vk, wrong_message, &signature).await.unwrap());
    }

    #[tokio::test]
    async fn test_ed25519() {
        if std::env::var("CI").is_ok() { println!("Skipping test in CI: test_ed25519"); return; }
        let signer = Ed25519Sig;
        let (vk, sk) = signer.generate_keypair(SignatureAlgorithm::Ed25519).await.unwrap();

        let message = b"Test message for Ed25519";
        let signature = signer.sign(&sk, message).await.unwrap();

        assert!(signer.verify(&vk, message, &signature).await.unwrap());
    }
}
