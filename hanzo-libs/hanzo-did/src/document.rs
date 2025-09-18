//! W3C DID Document implementation
//! 
//! Based on: https://www.w3.org/TR/did-core/#did-documents

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

use crate::did::DID;
use crate::service::Service;
use crate::verification_method::VerificationMethod;
use crate::proof::Proof;

/// W3C DID Document
/// 
/// A DID document is the resource that is associated with a DID
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DIDDocument {
    /// JSON-LD context
    #[serde(rename = "@context")]
    pub context: Vec<String>,

    /// The DID this document is about
    pub id: String,

    /// Alternative identifiers for this DID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub also_known_as: Option<Vec<String>>,

    /// DID controller(s)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub controller: Option<StringOrVec>,

    /// Verification methods
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_method: Option<Vec<VerificationMethod>>,

    /// Authentication verification relationships
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authentication: Option<Vec<VerificationRelationship>>,

    /// Assertion method verification relationships
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assertion_method: Option<Vec<VerificationRelationship>>,

    /// Key agreement verification relationships
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_agreement: Option<Vec<VerificationRelationship>>,

    /// Capability invocation verification relationships
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_invocation: Option<Vec<VerificationRelationship>>,

    /// Capability delegation verification relationships
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_delegation: Option<Vec<VerificationRelationship>>,

    /// Service endpoints
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<Vec<Service>>,

    /// Document metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<DateTime<Utc>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated: Option<DateTime<Utc>>,

    /// Cryptographic proof
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proof: Option<OneOrMany<Proof>>,

    /// Additional properties
    #[serde(flatten)]
    pub additional_properties: HashMap<String, Value>,
}

impl DIDDocument {
    /// Create a new DID Document with default W3C context
    pub fn new(did: &DID) -> Self {
        Self {
            context: vec![
                "https://www.w3.org/ns/did/v1".to_string(),
                "https://w3id.org/security/suites/ed25519-2020/v1".to_string(),
                "https://w3id.org/security/suites/x25519-2020/v1".to_string(),
            ],
            id: did.to_string(),
            also_known_as: None,
            controller: None,
            verification_method: None,
            authentication: None,
            assertion_method: None,
            key_agreement: None,
            capability_invocation: None,
            capability_delegation: None,
            service: None,
            created: Some(Utc::now()),
            updated: None,
            proof: None,
            additional_properties: HashMap::new(),
        }
    }

    /// Add a verification method
    pub fn add_verification_method(&mut self, method: VerificationMethod) {
        match &mut self.verification_method {
            Some(methods) => methods.push(method),
            None => self.verification_method = Some(vec![method]),
        }
    }

    /// Add an authentication method
    pub fn add_authentication(&mut self, auth: VerificationRelationship) {
        match &mut self.authentication {
            Some(methods) => methods.push(auth),
            None => self.authentication = Some(vec![auth]),
        }
    }

    /// Add a key agreement method
    pub fn add_key_agreement(&mut self, agreement: VerificationRelationship) {
        match &mut self.key_agreement {
            Some(methods) => methods.push(agreement),
            None => self.key_agreement = Some(vec![agreement]),
        }
    }

    /// Add a service endpoint
    pub fn add_service(&mut self, service: Service) {
        match &mut self.service {
            Some(services) => services.push(service),
            None => self.service = Some(vec![service]),
        }
    }

    /// Set controller(s)
    pub fn set_controller(&mut self, controller: impl Into<StringOrVec>) {
        self.controller = Some(controller.into());
    }

    /// Add an alternative identifier
    pub fn add_also_known_as(&mut self, identifier: String) {
        match &mut self.also_known_as {
            Some(ids) => ids.push(identifier),
            None => self.also_known_as = Some(vec![identifier]),
        }
    }

    /// Find a verification method by ID
    pub fn find_verification_method(&self, id: &str) -> Option<&VerificationMethod> {
        self.verification_method.as_ref()?.iter()
            .find(|m| m.id == id || m.id.ends_with(&format!("#{id}")))
    }

    /// Get all verification methods for authentication
    pub fn get_authentication_methods(&self) -> Vec<&VerificationMethod> {
        let mut methods = Vec::new();
        
        if let Some(auth_refs) = &self.authentication {
            for auth_ref in auth_refs {
                match auth_ref {
                    VerificationRelationship::Reference(id) => {
                        if let Some(method) = self.find_verification_method(id) {
                            methods.push(method);
                        }
                    }
                    VerificationRelationship::Embedded(method) => {
                        methods.push(method);
                    }
                }
            }
        }
        
        methods
    }

    /// Validate the document structure
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        // Check required fields
        if self.id.is_empty() {
            errors.push("DID Document must have an id".to_string());
        }

        if self.context.is_empty() {
            errors.push("DID Document must have at least one context".to_string());
        }

        // Validate DID format
        if DID::from_str(&self.id).is_err() {
            errors.push(format!("Invalid DID format: {}", self.id));
        }

        // Validate verification method references
        if let Some(auth) = &self.authentication {
            for auth_ref in auth {
                if let VerificationRelationship::Reference(id) = auth_ref {
                    if self.find_verification_method(id).is_none() {
                        errors.push(format!("Authentication references non-existent verification method: {id}"));
                    }
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

/// Either a string or a vector of strings
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StringOrVec {
    String(String),
    Vec(Vec<String>),
}

impl From<String> for StringOrVec {
    fn from(s: String) -> Self {
        StringOrVec::String(s)
    }
}

impl From<Vec<String>> for StringOrVec {
    fn from(v: Vec<String>) -> Self {
        StringOrVec::Vec(v)
    }
}

/// Either one or many of something
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

/// Verification relationship - either a reference or embedded method
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum VerificationRelationship {
    /// Reference to a verification method by ID
    Reference(String),
    /// Embedded verification method
    Embedded(VerificationMethod),
}

impl From<String> for VerificationRelationship {
    fn from(s: String) -> Self {
        // If it starts with # it's a reference to a fragment in the same document
        if s.starts_with('#') {
            VerificationRelationship::Reference(s)
        } else {
            VerificationRelationship::Reference(s)
        }
    }
}

impl From<&str> for VerificationRelationship {
    fn from(s: &str) -> Self {
        VerificationRelationship::Reference(s.to_string())
    }
}

impl From<VerificationMethod> for VerificationRelationship {
    fn from(method: VerificationMethod) -> Self {
        VerificationRelationship::Embedded(method)
    }
}

use std::str::FromStr;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verification_method::VerificationMethodType;

    #[test]
    fn test_did_document_creation() {
        let did = DID::hanzo_eth("0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7");
        let mut doc = DIDDocument::new(&did);

        assert_eq!(doc.id, did.to_string());
        assert!(!doc.context.is_empty());

        // Add verification method
        let vm = VerificationMethod {
            id: format!("{}#key-1", did),
            type_: VerificationMethodType::Ed25519VerificationKey2020,
            controller: did.to_string(),
            public_key_multibase: Some("z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK".to_string()),
            ..Default::default()
        };

        doc.add_verification_method(vm.clone());
        doc.add_authentication(format!("{}#key-1", did).into());

        assert!(doc.verification_method.is_some());
        assert!(doc.authentication.is_some());
        assert!(doc.validate().is_ok());
    }

    #[test]
    fn test_find_verification_method() {
        let did = DID::hanzo_eth("0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7");
        let mut doc = DIDDocument::new(&did);

        let vm_id = format!("{}#key-1", did);
        let vm = VerificationMethod {
            id: vm_id.clone(),
            type_: VerificationMethodType::Ed25519VerificationKey2020,
            controller: did.to_string(),
            public_key_multibase: Some("z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK".to_string()),
            ..Default::default()
        };

        doc.add_verification_method(vm);

        // Find by full ID
        assert!(doc.find_verification_method(&vm_id).is_some());
        
        // Find by fragment
        assert!(doc.find_verification_method("key-1").is_some());
    }
}