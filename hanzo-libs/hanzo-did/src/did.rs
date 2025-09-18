//! W3C DID (Decentralized Identifier) implementation
//! 
//! Based on W3C DID Core specification: https://www.w3.org/TR/did-core/

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::error::DIDError;

/// A W3C compliant Decentralized Identifier
/// 
/// Format: did:method:method-specific-id
/// Example: did:hanzo:eth:0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DID {
    /// The DID method (e.g., "hanzo", "ethr", "key", "web")
    pub method: String,
    
    /// Method-specific identifier
    pub id: String,
    
    /// Optional path component
    pub path: Option<String>,
    
    /// Optional query parameters
    pub query: Option<String>,
    
    /// Optional fragment (for referencing parts of DID document)
    pub fragment: Option<String>,
}

impl DID {
    /// Create a new DID
    pub fn new(method: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            method: method.into(),
            id: id.into(),
            path: None,
            query: None,
            fragment: None,
        }
    }

    /// Create a Hanzo network DID (mainnet shorthand)
    pub fn hanzo(username: impl Into<String>) -> Self {
        Self::new("hanzo", username.into())
    }

    /// Create a Lux network DID (mainnet shorthand)
    pub fn lux(username: impl Into<String>) -> Self {
        Self::new("lux", username.into())
    }

    /// Create a local development DID for Hanzo
    pub fn hanzo_local(identifier: impl Into<String>) -> Self {
        Self::new("hanzo", format!("local:{}", identifier.into()))
    }

    /// Create a local development DID for Lux
    pub fn lux_local(identifier: impl Into<String>) -> Self {
        Self::new("lux", format!("local:{}", identifier.into()))
    }

    /// Create a Hanzo DID for Ethereum (explicit chain)
    pub fn hanzo_eth(address: impl Into<String>) -> Self {
        Self::new("hanzo", format!("eth:{}", address.into()))
    }

    /// Create a Hanzo DID for Sepolia testnet
    pub fn hanzo_sepolia(address: impl Into<String>) -> Self {
        Self::new("hanzo", format!("sepolia:{}", address.into()))
    }

    /// Create a Hanzo DID for Base L2
    pub fn hanzo_base(address: impl Into<String>) -> Self {
        Self::new("hanzo", format!("base:{}", address.into()))
    }

    /// Create a Lux DID for specific chain
    pub fn lux_chain(chain: &str, address: impl Into<String>) -> Self {
        Self::new("lux", format!("{}:{}", chain, address.into()))
    }

    // Native chain DIDs (direct chain methods)

    /// Create an Ethereum mainnet DID
    pub fn eth(identifier: impl Into<String>) -> Self {
        Self::new("eth", identifier.into())
    }

    /// Create a Sepolia testnet DID
    pub fn sepolia(identifier: impl Into<String>) -> Self {
        Self::new("sepolia", identifier.into())
    }

    /// Create a Base L2 DID
    pub fn base(identifier: impl Into<String>) -> Self {
        Self::new("base", identifier.into())
    }

    /// Create a Base Sepolia DID
    pub fn base_sepolia(identifier: impl Into<String>) -> Self {
        Self::new("base-sepolia", identifier.into())
    }

    /// Create a Polygon DID
    pub fn polygon(identifier: impl Into<String>) -> Self {
        Self::new("polygon", identifier.into())
    }

    /// Create an Arbitrum DID
    pub fn arbitrum(identifier: impl Into<String>) -> Self {
        Self::new("arbitrum", identifier.into())
    }

    /// Create an Optimism DID
    pub fn optimism(identifier: impl Into<String>) -> Self {
        Self::new("optimism", identifier.into())
    }

    /// Parse a DID string into a DID struct
    pub fn parse(did_string: &str) -> Result<Self, DIDError> {
        // DID format: did:method:method-specific-id[/path][?query][#fragment]
        let regex = Regex::new(r"^did:([a-z0-9]+):([a-zA-Z0-9:._-]+)(/[^?#]*)?(\?[^#]*)?(#.*)?$")
            .map_err(|e| DIDError::InvalidFormat(format!("Regex error: {e}")))?;

        let captures = regex.captures(did_string)
            .ok_or_else(|| DIDError::InvalidFormat(format!("Invalid DID format: {did_string}")))?;

        let method = captures.get(1)
            .ok_or_else(|| DIDError::InvalidFormat("Missing method".to_string()))?
            .as_str().to_string();

        let id = captures.get(2)
            .ok_or_else(|| DIDError::InvalidFormat("Missing method-specific-id".to_string()))?
            .as_str().to_string();

        let path = captures.get(3).map(|m| m.as_str().to_string());
        let query = captures.get(4).map(|m| m.as_str().strip_prefix('?').unwrap_or(m.as_str()).to_string());
        let fragment = captures.get(5).map(|m| m.as_str().strip_prefix('#').unwrap_or(m.as_str()).to_string());

        Ok(Self {
            method,
            id,
            path,
            query,
            fragment,
        })
    }

    /// Set the fragment component
    pub fn with_fragment(mut self, fragment: impl Into<String>) -> Self {
        self.fragment = Some(fragment.into());
        self
    }

    /// Set the path component
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Set the query component
    pub fn with_query(mut self, query: impl Into<String>) -> Self {
        self.query = Some(query.into());
        self
    }

    /// Get the full DID string representation
    pub fn to_string_full(&self) -> String {
        let mut result = format!("did:{}:{}", self.method, self.id);
        
        if let Some(path) = &self.path {
            result.push_str(path);
        }
        
        if let Some(query) = &self.query {
            result.push('?');
            result.push_str(query);
        }
        
        if let Some(fragment) = &self.fragment {
            result.push('#');
            result.push_str(fragment);
        }
        
        result
    }

    /// Parse network from DID
    pub fn get_network(&self) -> Option<Network> {
        // For complex DIDs like did:hanzo:eth:address - check for embedded chain first
        // This takes precedence over method-based network detection
        if self.id.contains(':') {
            let parts: Vec<&str> = self.id.split(':').collect();
            if !parts.is_empty() {
                if let Ok(network) = Network::from_str(parts[0]) {
                    return Some(network);
                }
            }
        }

        // For native chain DIDs like did:eth:zeekay
        if let Ok(network) = Network::from_str(&self.method) {
            return Some(network);
        }

        // For simple DIDs like did:hanzo:username or did:lux:username
        match self.method.as_str() {
            "hanzo" => Some(Network::Hanzo),
            "lux" => Some(Network::Lux),
            _ => None,
        }
    }

    /// Get the base identifier (username/address) from any DID
    pub fn get_identifier(&self) -> String {
        // For simple DIDs like did:hanzo:zeekay or did:lux:zeekay
        if self.id.find(':').is_none() {
            return self.id.clone();
        }

        // For complex DIDs like did:hanzo:eth:0x123, return the last part
        let parts: Vec<&str> = self.id.split(':').collect();
        if parts.len() >= 2 {
            parts[1..].join(":")
        } else {
            self.id.clone()
        }
    }

    /// Check if this DID represents the same entity as another
    /// (useful for omnichain identity verification)
    pub fn is_same_entity(&self, other: &DID) -> bool {
        // Exact match
        if self == other {
            return true;
        }

        // Cross-chain identity: same base identifier = same entity
        self.get_identifier() == other.get_identifier()
    }

    /// Get all possible DID variants for this entity across networks
    pub fn get_omnichain_variants(&self) -> Vec<DID> {
        let identifier = self.get_identifier();
        vec![
            // Primary networks
            DID::hanzo(identifier.clone()),           // did:hanzo:zeekay
            DID::lux(identifier.clone()),             // did:lux:zeekay

            // Native chain DIDs
            DID::eth(identifier.clone()),             // did:eth:zeekay
            DID::base(identifier.clone()),            // did:base:zeekay
            DID::polygon(identifier.clone()),         // did:polygon:zeekay
            DID::arbitrum(identifier.clone()),        // did:arbitrum:zeekay
            DID::optimism(identifier.clone()),        // did:optimism:zeekay

            // Local development
            DID::hanzo_local(identifier.clone()),     // did:hanzo:local:zeekay
            DID::lux_local(identifier.clone()),       // did:lux:local:zeekay

            // Testnets
            DID::sepolia(identifier.clone()),         // did:sepolia:zeekay
            DID::base_sepolia(identifier.clone()),    // did:base-sepolia:zeekay

            // Explicit Hanzo chain mappings
            DID::hanzo_eth(identifier.clone()),       // did:hanzo:eth:zeekay
            DID::hanzo_sepolia(identifier.clone()),   // did:hanzo:sepolia:zeekay
            DID::hanzo_base(identifier.clone()),      // did:hanzo:base:zeekay
            DID::lux_chain("fuji", identifier),       // did:lux:fuji:zeekay
        ]
    }

    /// Create context-aware DID from @username format
    pub fn from_username(username: &str, context: &str) -> DID {
        // Remove @ prefix if present
        let clean_username = username.strip_prefix('@').unwrap_or(username);

        match context.to_lowercase().as_str() {
            "hanzo" => DID::hanzo(clean_username),
            "lux" => DID::lux(clean_username),
            _ => DID::hanzo(clean_username), // Default to hanzo
        }
    }
}

impl fmt::Display for DID {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_string_full())
    }
}

impl FromStr for DID {
    type Err = DIDError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // W3C DID regex pattern
        let did_regex = Regex::new(
            r"^did:([a-z0-9]+):([^?#]+)(/[^?#]*)?(\?[^#]*)?(#.*)?$"
        ).unwrap();

        let captures = did_regex.captures(s)
            .ok_or_else(|| DIDError::InvalidFormat(s.to_string()))?;

        let method = captures.get(1).unwrap().as_str().to_string();
        let id = captures.get(2).unwrap().as_str().to_string();
        let path = captures.get(3).map(|m| m.as_str().to_string());
        let query = captures.get(4).map(|m| m.as_str()[1..].to_string()); // Remove '?'
        let fragment = captures.get(5).map(|m| m.as_str()[1..].to_string()); // Remove '#'

        Ok(DID {
            method,
            id,
            path,
            query,
            fragment,
        })
    }
}

/// Supported networks for DIDs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Network {
    /// Hanzo mainnet (shorthand: did:hanzo:username)
    Hanzo,
    /// Lux mainnet (shorthand: did:lux:username)
    Lux,
    /// Local development networks
    Local,
    /// Ethereum mainnet (native: did:eth:username)
    Ethereum,
    /// Sepolia testnet (native: did:sepolia:username)
    Sepolia,
    /// Base L2 (native: did:base:username)
    Base,
    /// Base Sepolia testnet (native: did:base-sepolia:username)
    BaseSepolia,
    /// Polygon (native: did:polygon:username)
    Polygon,
    /// Arbitrum (native: did:arbitrum:username)
    Arbitrum,
    /// Optimism (native: did:optimism:username)
    Optimism,
    /// Lux Fuji testnet
    LuxFuji,
    /// IPFS
    IPFS,
}

impl Network {
    /// Get the chain ID for EVM-compatible networks
    pub fn chain_id(&self) -> Option<u64> {
        match self {
            Network::Ethereum => Some(1),
            Network::Sepolia => Some(11155111),
            Network::Base => Some(8453),
            Network::BaseSepolia => Some(84532),
            Network::Polygon => Some(137),
            Network::Arbitrum => Some(42161),
            Network::Optimism => Some(10),
            Network::LuxFuji => Some(43113), // Fuji testnet
            _ => None,
        }
    }

    /// Check if this is a testnet
    pub fn is_testnet(&self) -> bool {
        matches!(self,
            Network::Sepolia |
            Network::BaseSepolia |
            Network::LuxFuji |
            Network::Local
        )
    }

    /// Get RPC endpoint for the network
    pub fn rpc_endpoint(&self) -> Option<&'static str> {
        match self {
            Network::Hanzo => Some("https://rpc.hanzo.network"),
            Network::Lux => Some("https://api.lux.network/ext/bc/C/rpc"),
            Network::Ethereum => Some("https://eth.llamarpc.com"),
            Network::Sepolia => Some("https://rpc.sepolia.org"),
            Network::Base => Some("https://mainnet.base.org"),
            Network::BaseSepolia => Some("https://sepolia.base.org"),
            Network::Polygon => Some("https://polygon-rpc.com"),
            Network::Arbitrum => Some("https://arb1.arbitrum.io/rpc"),
            Network::Optimism => Some("https://mainnet.optimism.io"),
            Network::LuxFuji => Some("https://api.lux-test.network/ext/bc/C/rpc"),
            _ => None,
        }
    }
}

impl fmt::Display for Network {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Network::Hanzo => write!(f, "hanzo"),
            Network::Lux => write!(f, "lux"),
            Network::Local => write!(f, "local"),
            Network::Ethereum => write!(f, "eth"),
            Network::Sepolia => write!(f, "sepolia"),
            Network::Base => write!(f, "base"),
            Network::BaseSepolia => write!(f, "base-sepolia"),
            Network::Polygon => write!(f, "polygon"),
            Network::Arbitrum => write!(f, "arbitrum"),
            Network::Optimism => write!(f, "optimism"),
            Network::LuxFuji => write!(f, "lux-fuji"),
            Network::IPFS => write!(f, "ipfs"),
        }
    }
}

impl FromStr for Network {
    type Err = DIDError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "hanzo" => Ok(Network::Hanzo),
            "lux" => Ok(Network::Lux),
            "local" | "localhost" => Ok(Network::Local),
            "eth" | "ethereum" => Ok(Network::Ethereum),
            "sepolia" => Ok(Network::Sepolia),
            "base" => Ok(Network::Base),
            "base-sepolia" => Ok(Network::BaseSepolia),
            "polygon" | "matic" => Ok(Network::Polygon),
            "arbitrum" | "arb" => Ok(Network::Arbitrum),
            "optimism" | "op" => Ok(Network::Optimism),
            "lux-fuji" | "fuji" => Ok(Network::LuxFuji),
            "ipfs" => Ok(Network::IPFS),
            _ => Err(DIDError::UnknownChain(s.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_did_parsing() {
        // Test complex DID
        let did_str = "did:hanzo:eth:0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7";
        let did = DID::from_str(did_str).unwrap();

        assert_eq!(did.method, "hanzo");
        assert_eq!(did.id, "eth:0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7");
        assert_eq!(did.get_network(), Some(Network::Ethereum));
        assert_eq!(did.get_identifier(), "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7");

        // Test simple DID
        let simple_did = DID::from_str("did:hanzo:zeekay").unwrap();
        assert_eq!(simple_did.method, "hanzo");
        assert_eq!(simple_did.id, "zeekay");
        assert_eq!(simple_did.get_network(), Some(Network::Hanzo));
        assert_eq!(simple_did.get_identifier(), "zeekay");
    }

    #[test]
    fn test_did_with_fragment() {
        let did_str = "did:hanzo:eth:0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7#key-1";
        let did = DID::from_str(did_str).unwrap();

        assert_eq!(did.fragment, Some("key-1".to_string()));
    }

    #[test]
    fn test_omnichain_identity() {
        // Test cross-chain identity verification
        let hanzo_did = DID::hanzo("zeekay");
        let lux_did = DID::lux("zeekay");
        let hanzo_eth_did = DID::hanzo_eth("zeekay");

        assert!(hanzo_did.is_same_entity(&lux_did));
        assert!(hanzo_did.is_same_entity(&hanzo_eth_did));
        assert!(lux_did.is_same_entity(&hanzo_eth_did));

        // Test different identities
        let other_did = DID::hanzo("alice");
        assert!(!hanzo_did.is_same_entity(&other_did));
    }

    #[test]
    fn test_context_aware_resolution() {
        // Test @username resolution in different contexts
        let hanzo_context = DID::from_username("@zeekay", "hanzo");
        let lux_context = DID::from_username("zeekay", "lux");

        assert_eq!(hanzo_context, DID::hanzo("zeekay"));
        assert_eq!(lux_context, DID::lux("zeekay"));

        // Test omnichain variants
        let variants = hanzo_context.get_omnichain_variants();
        assert!(variants.contains(&DID::hanzo("zeekay")));
        assert!(variants.contains(&DID::lux("zeekay")));
        assert!(variants.contains(&DID::hanzo_local("zeekay")));
    }

    #[test]
    fn test_native_chain_dids() {
        // Test native chain DIDs
        let eth_did = DID::eth("zeekay");
        let base_did = DID::base("zeekay");
        let polygon_did = DID::polygon("zeekay");

        assert_eq!(eth_did.to_string(), "did:eth:zeekay");
        assert_eq!(base_did.to_string(), "did:base:zeekay");
        assert_eq!(polygon_did.to_string(), "did:polygon:zeekay");

        // Test network detection
        assert_eq!(eth_did.get_network(), Some(Network::Ethereum));
        assert_eq!(base_did.get_network(), Some(Network::Base));
        assert_eq!(polygon_did.get_network(), Some(Network::Polygon));

        // Test omnichain identity
        assert!(eth_did.is_same_entity(&base_did));
        assert!(eth_did.is_same_entity(&polygon_did));
    }

    #[test]
    fn test_network_properties() {
        assert_eq!(Network::Ethereum.chain_id(), Some(1));
        assert_eq!(Network::Base.chain_id(), Some(8453));
        assert_eq!(Network::Polygon.chain_id(), Some(137));
        assert!(!Network::Ethereum.is_testnet());
        assert!(Network::Sepolia.is_testnet());
        assert!(Network::Hanzo.rpc_endpoint().is_some());
        assert!(Network::Lux.rpc_endpoint().is_some());
        assert!(Network::Ethereum.rpc_endpoint().is_some());
    }
}