//! W3C DID Document Proofs

use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Cryptographic proof for DID documents
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Proof {
    /// Proof type
    #[serde(rename = "type")]
    pub type_: String,

    /// When the proof was created
    pub created: DateTime<Utc>,

    /// Verification method used
    pub verification_method: String,

    /// Proof purpose
    pub proof_purpose: ProofPurpose,

    /// The actual proof value
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proof_value: Option<String>,

    /// JWS signature
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jws: Option<String>,

    /// Domain for domain-bound proofs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,

    /// Challenge for challenge-response proofs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub challenge: Option<String>,

    /// Additional properties
    #[serde(flatten)]
    pub properties: HashMap<String, Value>,
}

/// Purpose of the proof
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProofPurpose {
    AssertionMethod,
    Authentication,
    KeyAgreement,
    CapabilityInvocation,
    CapabilityDelegation,
}

impl Proof {
    /// Create a new Ed25519 signature proof
    pub fn new_ed25519_signature(
        verification_method: String,
        signature: Vec<u8>,
    ) -> Self {
        Self {
            type_: "Ed25519Signature2020".to_string(),
            created: Utc::now(),
            verification_method,
            proof_purpose: ProofPurpose::AssertionMethod,
            proof_value: Some(STANDARD.encode(signature)),
            jws: None,
            domain: None,
            challenge: None,
            properties: HashMap::new(),
        }
    }
}