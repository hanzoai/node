//! W3C Verification Methods
//! 
//! Based on: https://www.w3.org/TR/did-core/#verification-methods

use serde::{Deserialize, Serialize};
use std::fmt;

/// Verification Method for DIDs
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct VerificationMethod {
    /// The verification method ID
    pub id: String,

    /// The verification method type
    #[serde(rename = "type")]
    pub type_: VerificationMethodType,

    /// The controller of this verification method
    pub controller: String,

    /// Public key in Multibase format (preferred)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_key_multibase: Option<String>,

    /// Public key in JWK format
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_key_jwk: Option<serde_json::Value>,

    /// Public key in PEM format
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_key_pem: Option<String>,

    /// Blockchain account identifier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blockchain_account_id: Option<String>,

    /// Ethereum address (for EcdsaSecp256k1RecoveryMethod2020)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ethereum_address: Option<String>,
}

/// Verification Method Types as per W3C DID specification
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum VerificationMethodType {
    /// Ed25519 signature verification
    #[default]
    #[serde(rename = "Ed25519VerificationKey2020")]
    Ed25519VerificationKey2020,

    /// Ed25519 signature verification (2018 version)
    #[serde(rename = "Ed25519VerificationKey2018")]
    Ed25519VerificationKey2018,

    /// X25519 key agreement
    #[serde(rename = "X25519KeyAgreementKey2020")]
    X25519KeyAgreementKey2020,

    /// X25519 key agreement (2019 version)
    #[serde(rename = "X25519KeyAgreementKey2019")]
    X25519KeyAgreementKey2019,

    /// ECDSA with secp256k1 curve
    #[serde(rename = "EcdsaSecp256k1VerificationKey2019")]
    EcdsaSecp256k1VerificationKey2019,

    /// ECDSA with secp256k1 recovery (Ethereum-style)
    #[serde(rename = "EcdsaSecp256k1RecoveryMethod2020")]
    EcdsaSecp256k1RecoveryMethod2020,

    /// JSON Web Key 2020
    #[serde(rename = "JsonWebKey2020")]
    JsonWebKey2020,

    /// BLS12-381 signature
    #[serde(rename = "Bls12381G2Key2020")]
    Bls12381G2Key2020,

    /// GPG verification key
    #[serde(rename = "GpgVerificationKey2020")]
    GpgVerificationKey2020,

    /// RSA verification key
    #[serde(rename = "RsaVerificationKey2018")]
    RsaVerificationKey2018,

    /// Custom type
    #[serde(untagged)]
    Custom(String),
}

impl fmt::Display for VerificationMethodType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ed25519VerificationKey2020 => write!(f, "Ed25519VerificationKey2020"),
            Self::Ed25519VerificationKey2018 => write!(f, "Ed25519VerificationKey2018"),
            Self::X25519KeyAgreementKey2020 => write!(f, "X25519KeyAgreementKey2020"),
            Self::X25519KeyAgreementKey2019 => write!(f, "X25519KeyAgreementKey2019"),
            Self::EcdsaSecp256k1VerificationKey2019 => write!(f, "EcdsaSecp256k1VerificationKey2019"),
            Self::EcdsaSecp256k1RecoveryMethod2020 => write!(f, "EcdsaSecp256k1RecoveryMethod2020"),
            Self::JsonWebKey2020 => write!(f, "JsonWebKey2020"),
            Self::Bls12381G2Key2020 => write!(f, "Bls12381G2Key2020"),
            Self::GpgVerificationKey2020 => write!(f, "GpgVerificationKey2020"),
            Self::RsaVerificationKey2018 => write!(f, "RsaVerificationKey2018"),
            Self::Custom(s) => write!(f, "{s}"),
        }
    }
}

impl VerificationMethod {
    /// Create a new Ed25519 verification method
    pub fn new_ed25519(
        id: String,
        controller: String,
        public_key: &[u8; 32],
    ) -> Self {
        use multibase::Base;
        let public_key_multibase = multibase::encode(Base::Base58Btc, public_key);
        
        Self {
            id,
            type_: VerificationMethodType::Ed25519VerificationKey2020,
            controller,
            public_key_multibase: Some(public_key_multibase),
            ..Default::default()
        }
    }

    /// Create a new X25519 key agreement method
    pub fn new_x25519(
        id: String,
        controller: String,
        public_key: &[u8; 32],
    ) -> Self {
        use multibase::Base;
        let public_key_multibase = multibase::encode(Base::Base58Btc, public_key);
        
        Self {
            id,
            type_: VerificationMethodType::X25519KeyAgreementKey2020,
            controller,
            public_key_multibase: Some(public_key_multibase),
            ..Default::default()
        }
    }

    /// Create an Ethereum-style verification method
    pub fn new_ethereum(
        id: String,
        controller: String,
        ethereum_address: String,
    ) -> Self {
        Self {
            id,
            type_: VerificationMethodType::EcdsaSecp256k1RecoveryMethod2020,
            controller,
            ethereum_address: Some(ethereum_address),
            ..Default::default()
        }
    }

    /// Check if this is a signing method
    pub fn is_signing_method(&self) -> bool {
        matches!(self.type_,
            VerificationMethodType::Ed25519VerificationKey2020 |
            VerificationMethodType::Ed25519VerificationKey2018 |
            VerificationMethodType::EcdsaSecp256k1VerificationKey2019 |
            VerificationMethodType::EcdsaSecp256k1RecoveryMethod2020 |
            VerificationMethodType::Bls12381G2Key2020 |
            VerificationMethodType::RsaVerificationKey2018
        )
    }

    /// Check if this is a key agreement method
    pub fn is_key_agreement_method(&self) -> bool {
        matches!(self.type_,
            VerificationMethodType::X25519KeyAgreementKey2020 |
            VerificationMethodType::X25519KeyAgreementKey2019
        )
    }
}