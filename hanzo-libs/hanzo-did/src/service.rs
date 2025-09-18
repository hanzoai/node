//! W3C DID Service Endpoints

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Service endpoint in a DID document
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    /// Service ID
    pub id: String,

    /// Service type(s)
    #[serde(rename = "type")]
    pub type_: ServiceType,

    /// Service endpoint(s)
    pub service_endpoint: ServiceEndpoint,

    /// Additional properties
    #[serde(flatten)]
    pub properties: HashMap<String, Value>,
}

/// Service types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ServiceType {
    Single(String),
    Multiple(Vec<String>),
}

/// Service endpoints
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ServiceEndpoint {
    Uri(String),
    Multiple(Vec<String>),
    Map(HashMap<String, Value>),
}

impl Service {
    /// Create a new service
    pub fn new(id: String, type_: String, endpoint: String) -> Self {
        Self {
            id,
            type_: ServiceType::Single(type_),
            service_endpoint: ServiceEndpoint::Uri(endpoint),
            properties: HashMap::new(),
        }
    }

    /// Create a Hanzo node service
    pub fn hanzo_node(did: &str, endpoint: String) -> Self {
        Self::new(
            format!("{did}#hanzo-node"),
            "HanzoNode".to_string(),
            endpoint,
        )
    }

    /// Create an LLM provider service
    pub fn llm_provider(did: &str, endpoint: String) -> Self {
        Self::new(
            format!("{did}#llm-provider"),
            "LLMProvider".to_string(),
            endpoint,
        )
    }

    /// Create a messaging service
    pub fn messaging(did: &str, endpoint: String) -> Self {
        Self::new(
            format!("{did}#messaging"),
            "MessagingService".to_string(),
            endpoint,
        )
    }
}