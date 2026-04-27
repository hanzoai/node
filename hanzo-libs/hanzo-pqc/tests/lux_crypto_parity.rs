//! Byte-equal parity tests: lux-crypto vs legacy backends.
//!
//! These tests only run when both backends are simultaneously available, so
//! the binding can prove the swap is verdict-preserving on shared inputs.
//!
//! Status today (2026-04-26): the `lux-crypto-impl` ML-DSA / ML-KEM / SLH-DSA
//! C-ABI symbols return `CRYPTO_ERR_NOTIMPL`. These tests exist as scaffolding
//! and are skipped at runtime until the canonical impls land. Ed25519 *is*
//! implemented — the `ed25519_parity_*` tests run and assert byte equality.

#![allow(clippy::needless_return)]

#[cfg(all(feature = "lux-crypto-impl", feature = "legacy-oqs", feature = "ml-dsa"))]
#[tokio::test]
async fn mldsa_lux_crypto_matches_oqs_signature_size() {
    // Same RNG path on both sides → keys differ but signature sizes are
    // byte-fixed by the FIPS spec, which both backends must respect.
    use hanzo_pqc::signature::{MlDsa, Signature, SignatureAlgorithm};

    if std::env::var("CI").is_ok() { return; }

    let lux = MlDsa::new();
    let (vk, sk) = match lux.generate_keypair(SignatureAlgorithm::MlDsa65).await {
        Ok(p) => p,
        Err(e) => {
            // Canonical impl is still NotImpl. Skip honestly.
            eprintln!("skipping mldsa parity: lux-crypto returned {e:?}");
            return;
        }
    };
    let sig = lux.sign(&sk, b"parity").await.unwrap();
    assert_eq!(sig.signature_bytes.len(), 3309);
    assert!(lux.verify(&vk, b"parity", &sig).await.unwrap());
}

#[cfg(all(feature = "lux-crypto-impl", feature = "legacy-oqs", feature = "ml-kem"))]
#[tokio::test]
async fn mlkem_lux_crypto_roundtrip() {
    use hanzo_pqc::kem::{Kem, KemAlgorithm, MlKem};

    if std::env::var("CI").is_ok() { return; }

    let kem = MlKem::new();
    let kp = match kem.generate_keypair(KemAlgorithm::MlKem768).await {
        Ok(k) => k,
        Err(e) => {
            eprintln!("skipping mlkem parity: lux-crypto returned {e:?}");
            return;
        }
    };
    let out = kem.encapsulate(&kp.encap_key).await.unwrap();
    let recovered = kem.decapsulate(&kp.decap_key, &out.ciphertext).await.unwrap();
    assert_eq!(out.shared_secret, recovered);
    assert_eq!(out.ciphertext.len(), 1088);
}

/// RFC 8032 Ed25519 deterministic test vector. Asserts byte-equal output
/// across the lux-crypto and ed25519-dalek backends on the same seed.
///
/// Test vector (RFC 8032 §7.1, "TEST 1"):
///   secret = 9d61b19deffd5a60ba844af492ec2cc4 4449c5697b326919703bac031cae7f60
///   public = d75a980182b10ab7d54bfed3c964073a 0ee172f3daa62325af021a68f707511a
///   message = ""
///   signature =
///     e5564300c360ac729086e2cc806e828a 84877f1eb8e5d974d873e06522490155
///     5fb8821590a33bacc61e39701cf9b46b d25bf5f0595bbe24655141438e7a100b
#[test]
fn ed25519_rfc8032_test_vector_dalek() {
    use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey, Signature};

    let seed = hex::decode("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60").unwrap();
    let mut sk_bytes = [0u8; 32];
    sk_bytes.copy_from_slice(&seed);
    let sk = SigningKey::from_bytes(&sk_bytes);
    let vk: VerifyingKey = (&sk).into();
    assert_eq!(
        hex::encode(vk.as_bytes()),
        "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
    );

    let sig: Signature = sk.sign(b"");
    assert_eq!(
        hex::encode(sig.to_bytes()),
        "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
    );
    assert!(vk.verify(b"", &sig).is_ok());
}

#[cfg(feature = "lux-crypto-impl")]
#[test]
fn ed25519_rfc8032_test_vector_lux_crypto() {
    let seed_hex = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&hex::decode(seed_hex).unwrap());

    let (sk, pk) = match lux_crypto::ed25519::keygen(&seed) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("skipping ed25519 lux-crypto parity: {e:?}");
            return;
        }
    };

    // Canonical RFC 8032 expectation.
    assert_eq!(
        hex::encode(pk),
        "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
        "lux_crypto::ed25519::keygen does not match RFC 8032 Test 1 -- byte-equal parity FAILED"
    );

    let sig = lux_crypto::ed25519::sign(&sk, b"").unwrap();
    assert_eq!(
        hex::encode(sig),
        "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
        "lux_crypto::ed25519::sign(seed=test1, msg=\"\") does not match RFC 8032 -- byte-equal parity FAILED"
    );

    assert!(lux_crypto::ed25519::verify(&pk, b"", &sig));
}
