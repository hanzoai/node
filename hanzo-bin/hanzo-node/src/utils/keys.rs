use ed25519_dalek::{SigningKey, VerifyingKey};
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use hanzo_messages::hanzo_utils::{
    encryption::{
        clone_static_secret_key, encryption_secret_key_to_string, ephemeral_encryption_keys,
        string_to_encryption_static_key,
    },
    hanzo_logging::{hanzo_log, HanzoLogLevel, HanzoLogOption},
    signatures::{
        clone_signature_secret_key, ephemeral_signature_keypair, signature_secret_key_to_string,
        string_to_signature_secret_key,
    },
};

use std::{collections::HashMap, env, fs};
use x25519_dalek::{PublicKey as EncryptionPublicKey, StaticSecret as EncryptionStaticKey};

use k256::ecdsa::SigningKey as Secp256k1SigningKey;
use k256::elliptic_curve::sec1::ToEncodedPoint;
use tiny_keccak::{Hasher, Keccak};

/// keccak256 of an arbitrary byte slice.
fn keccak256(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak::v256();
    let mut out = [0u8; 32];
    hasher.update(bytes);
    hasher.finalize(&mut out);
    out
}

/// Deterministically derive a real, signable secp256k1 (Ethereum/EVM) private key
/// from the node's ed25519 SECRET seed.
///
/// The 32-byte ed25519 seed (`SigningKey::to_bytes()`) is hashed with keccak256 to
/// produce 32 bytes used as the secp256k1 scalar. `k256`'s `SigningKey::from_slice`
/// rejects scalars that are zero or ≥ the curve order; in the astronomically
/// unlikely event of such a candidate we re-hash until we obtain a valid scalar.
/// The result is a real secp256k1 key that can sign EVM transactions.
pub fn derive_secp256k1_signing_key(identity_secret_key: &SigningKey) -> Secp256k1SigningKey {
    let seed = identity_secret_key.to_bytes();
    let mut candidate = keccak256(&seed);
    loop {
        match Secp256k1SigningKey::from_slice(&candidate) {
            Ok(key) => return key,
            // Candidate was 0 or ≥ curve order — re-hash and retry (≈ 2^-128 odds each).
            Err(_) => candidate = keccak256(&candidate),
        }
    }
}

/// Derive the lowercase `0x`-prefixed 40-hex-char EVM address for a secp256k1 key:
/// `keccak256(uncompressed_pubkey[1..65])[12..32]`.
pub fn evm_address_from_secp256k1(signing_key: &Secp256k1SigningKey) -> String {
    let verifying_key = signing_key.verifying_key();
    let encoded = verifying_key.to_encoded_point(false); // uncompressed: 0x04 || X || Y
    let pubkey_bytes = encoded.as_bytes(); // 65 bytes
    let hash = keccak256(&pubkey_bytes[1..65]);
    format!("0x{}", hex::encode(&hash[12..32]))
}

/// Derive the node's real EVM wallet address (`0x` + 40 lowercase hex) from its
/// ed25519 secret seed via the canonical secp256k1 derivation above.
pub fn derive_eth_address_from_identity(identity_secret_key: &SigningKey) -> String {
    let secp = derive_secp256k1_signing_key(identity_secret_key);
    evm_address_from_secp256k1(&secp)
}

/// Derive a node's identity DID from its ed25519 SECRET key when the configured
/// `GLOBAL_IDENTITY_NAME` is a chain-only DID prefix.
///
/// The node's wallet is unified with its identity: a real, signable secp256k1
/// keypair is deterministically derived from the ed25519 secret seed (see
/// [`derive_secp256k1_signing_key`]), and the resulting EVM address becomes the
/// node's DID id. Thus identity == wallet == mining-payout address.
///
/// Behavior:
/// - Input like `did:hanzo:`, `did:zoo:`, `did:lux:`, or `did:<chain>:auto`
///   (with optional trailing slash/whitespace) → `did:<chain>:0x<evm_address>`.
/// - Any other value (a full identity such as `did:hanzo:mainnet` or a legacy
///   `@@name.hanzo`) is returned verbatim.
pub fn resolve_identity_name(raw_name: &str, identity_secret_key: &SigningKey) -> String {
    let trimmed = raw_name.trim();

    // Only the supported chains may be auto-derived.
    let chain = ["hanzo", "zoo", "lux"].into_iter().find_map(|chain| {
        let prefix = format!("did:{chain}:");
        let rest = trimmed.strip_prefix(&prefix)?;
        // Chain-only prefix: nothing after `did:<chain>:`, or the explicit `auto`
        // sentinel. A trailing slash is tolerated.
        let rest = rest.trim_end_matches('/');
        if rest.is_empty() || rest == "auto" {
            Some(chain)
        } else {
            None
        }
    });

    match chain {
        Some(chain) => {
            let address = derive_eth_address_from_identity(identity_secret_key);
            // hanzo-did builds `did:<method>:<id>` via Display; id == `0x<addr>`.
            hanzo_did::DID::new(chain, address).to_string()
        }
        // Full identity (or legacy format) — keep verbatim.
        None => trimmed.to_string(),
    }
}

pub struct NodeKeys {
    pub identity_secret_key: SigningKey,
    pub identity_public_key: VerifyingKey,
    pub encryption_secret_key: EncryptionStaticKey,
    pub encryption_public_key: EncryptionPublicKey,
    pub private_https_certificate: Option<String>,
    pub public_https_certificate: Option<String>,
}

pub fn generate_or_load_keys(secrets_file_path: &str) -> NodeKeys {
    // First check for .secret file
    if let Ok(contents) = fs::read_to_string(secrets_file_path) {
        // Parse the contents of the file
        let mut map = HashMap::new();

        for line in contents.lines() {
            if let Some((key, value)) = line.split_once('=') {
                map.insert(key.trim().to_string(), value.trim().to_string());
            }
        }

        // Use the values from the file if they exist
        if let (Some(identity_secret_key_string), Some(encryption_secret_key_string)) =
            (map.get("IDENTITY_SECRET_KEY"), map.get("ENCRYPTION_SECRET_KEY"))
        {
            // Convert the strings back to secret keys
            let identity_secret_key = string_to_signature_secret_key(identity_secret_key_string).unwrap();
            let encryption_secret_key = string_to_encryption_static_key(encryption_secret_key_string).unwrap();

            // Generate public keys from secret keys
            let identity_public_key = identity_secret_key.verifying_key();
            let encryption_public_key = x25519_dalek::PublicKey::from(&encryption_secret_key);

            // Read the HTTPS certificates if they exist
            let private_https_certificate = match map.get("PRIVATE_HTTPS_CERTIFICATE").cloned() {
                Some(certificate) if certificate.trim().len() > 0 => Some(certificate.replace("\\n", "\n")),
                _ => None,
            };
            let public_https_certificate = match map.get("PUBLIC_HTTPS_CERTIFICATE").cloned() {
                Some(certificate) if certificate.trim().len() > 0 => Some(certificate.replace("\\n", "\n")),
                _ => None,
            };

            return NodeKeys {
                identity_secret_key,
                identity_public_key,
                encryption_secret_key,
                encryption_public_key,
                private_https_certificate,
                public_https_certificate,
            };
        }
    }

    // If keys are not found in Stronghold, fall back to ENV or generate new keys
    let (identity_secret_key, identity_public_key) = match env::var("IDENTITY_SECRET_KEY") {
        Ok(secret_key_str) => {
            let secret_key = string_to_signature_secret_key(&secret_key_str.clone()).unwrap();
            let public_key = secret_key.verifying_key();

            // Keys Validation (it case of scalar clamp)
            {
                let computed_sk = signature_secret_key_to_string(clone_signature_secret_key(&secret_key));
                if secret_key_str != computed_sk {
                    panic!("Identity secret key is invalid. Original: {} Modified: {}. Recommended to start the node with the modified one from now on.", secret_key_str, computed_sk);
                }
            }

            (secret_key, public_key)
        }
        _ => {
            hanzo_log(
                HanzoLogOption::Node,
                HanzoLogLevel::Error,
                "No identity secret key found or invalid. Generating ephemeral keys",
            );
            ephemeral_signature_keypair()
        }
    };

    let (encryption_secret_key, encryption_public_key) = match env::var("ENCRYPTION_SECRET_KEY") {
        Ok(secret_key_str) => {
            let secret_key = string_to_encryption_static_key(&secret_key_str).unwrap();
            let public_key = x25519_dalek::PublicKey::from(&secret_key);

            // Keys Validation (it case of scalar clamp)
            {
                let computed_sk = encryption_secret_key_to_string(clone_static_secret_key(&secret_key));
                if secret_key_str != computed_sk {
                    panic!("Encryption secret key is invalid. Original: {} Modified: {}. Recommended to start the node with the modified one from now on.", secret_key_str, computed_sk);
                }
            }

            (secret_key, public_key)
        }
        _ => {
            hanzo_log(
                HanzoLogOption::Node,
                HanzoLogLevel::Error,
                "No encryption secret key found or invalid. Generating ephemeral keys",
            );
            ephemeral_encryption_keys()
        }
    };

    let (private_https_certificate, public_https_certificate) = match env::var("HTTPS_CERTIFICATE_PEM") {
        Ok(private_key_pem) => {
            // Parse the private key from the PEM string
            let key_pair = KeyPair::from_pem(&private_key_pem).expect("Failed to parse private key");

            // Generate the public certificate from the private key
            let mut params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
            let mut dn = DistinguishedName::new();
            dn.push(DnType::CommonName, "localhost");
            params.distinguished_name = dn;
            let cert = params.self_signed(&key_pair).unwrap();
            let public_cert = cert.pem();

            (private_key_pem.trim().to_string(), public_cert)
        }
        _ => {
            // Generate a new self-signed certificate
            let mut params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
            let mut dn = DistinguishedName::new();
            dn.push(DnType::CommonName, "localhost");
            params.distinguished_name = dn;
            let key_pair = KeyPair::generate().unwrap();
            let cert = params.self_signed(&key_pair).unwrap();

            // Serialize the private key using KeyPair
            let private_key = key_pair.serialize_pem().trim().to_string();

            // Serialize the certificate
            let public_cert = cert.pem();

            (private_key, public_cert)
        }
    };

    NodeKeys {
        identity_secret_key,
        identity_public_key,
        encryption_secret_key,
        encryption_public_key,
        private_https_certificate: Some(private_https_certificate),
        public_https_certificate: Some(public_https_certificate),
    }
}
