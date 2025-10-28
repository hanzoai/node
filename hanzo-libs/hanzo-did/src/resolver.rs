//! DID Resolution
//! 
//! Based on: https://www.w3.org/TR/did-core/#resolution

use async_trait::async_trait;
use std::collections::HashMap;

use crate::{DID, DIDDocument, DIDError};

/// DID Resolver trait
#[async_trait]
pub trait DIDResolver {
    /// Resolve a DID to its document
    async fn resolve(&self, did: &DID) -> Result<DIDDocument, DIDError>;
    
    /// Resolve with options
    async fn resolve_with_options(
        &self,
        did: &DID,
        options: ResolveOptions,
    ) -> Result<ResolveResult, DIDError>;
}

/// Options for DID resolution
#[derive(Debug, Default)]
pub struct ResolveOptions {
    /// Accept header for content negotiation
    pub accept: Option<String>,
    
    /// Version of the DID document to retrieve
    pub version_id: Option<String>,
    
    /// Timestamp to retrieve document at
    pub version_time: Option<String>,
    
    /// Whether to dereference fragments
    pub no_cache: bool,
}

/// Result of DID resolution
#[derive(Debug)]
pub struct ResolveResult {
    /// The resolved DID document
    pub document: DIDDocument,
    
    /// Resolution metadata
    pub metadata: ResolutionMetadata,
    
    /// Document metadata
    pub document_metadata: DocumentMetadata,
}

/// Metadata about the resolution process
#[derive(Debug, Default)]
pub struct ResolutionMetadata {
    /// Content type of the document
    pub content_type: Option<String>,
    
    /// Error message if resolution failed
    pub error: Option<String>,
    
    /// Additional properties
    pub properties: HashMap<String, serde_json::Value>,
}

/// Metadata about the DID document
#[derive(Debug, Default)]
pub struct DocumentMetadata {
    /// When the document was created
    pub created: Option<String>,
    
    /// When the document was last updated
    pub updated: Option<String>,
    
    /// Whether the DID has been deactivated
    pub deactivated: Option<bool>,
    
    /// Version ID of this document
    pub version_id: Option<String>,
    
    /// Next version ID
    pub next_version_id: Option<String>,
    
    /// Canonical ID
    pub canonical_id: Option<String>,
    
    /// Equivalent IDs
    pub equivalent_id: Option<Vec<String>>,
}

/// Universal DID Resolver supporting multiple methods
pub struct UniversalResolver {
    resolvers: HashMap<String, Box<dyn DIDResolver + Send + Sync>>,
}

impl Default for UniversalResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl UniversalResolver {
    /// Create a new universal resolver
    pub fn new() -> Self {
        Self {
            resolvers: HashMap::new(),
        }
    }
    
    /// Register a resolver for a DID method
    pub fn register_resolver(
        &mut self,
        method: String,
        resolver: Box<dyn DIDResolver + Send + Sync>,
    ) {
        self.resolvers.insert(method, resolver);
    }
}

#[async_trait]
impl DIDResolver for UniversalResolver {
    async fn resolve(&self, did: &DID) -> Result<DIDDocument, DIDError> {
        let resolver = self.resolvers
            .get(&did.method)
            .ok_or_else(|| DIDError::ResolutionFailed(
                format!("No resolver for method: {}", did.method)
            ))?;
        
        resolver.resolve(did).await
    }
    
    async fn resolve_with_options(
        &self,
        did: &DID,
        options: ResolveOptions,
    ) -> Result<ResolveResult, DIDError> {
        let resolver = self.resolvers
            .get(&did.method)
            .ok_or_else(|| DIDError::ResolutionFailed(
                format!("No resolver for method: {}", did.method)
            ))?;
        
        resolver.resolve_with_options(did, options).await
    }
}