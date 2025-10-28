# Hanzo Node Identity: W3C DID Implementation

## Overview

Hanzo uses W3C compliant Decentralized Identifiers (DIDs) for cross-chain identity verification, improved interoperability, and standards compliance. Legacy identity formats are not supported.

## W3C DID Implementation

### DID Format
- **Simple Format (Recommended)**: `did:{network}:{username}`
- **Complex Format**: `did:{network}:{chain}:{identifier}`
- **Examples**:
  - `did:hanzo:zeekay` (Hanzo mainnet)
  - `did:lux:zeekay` (Lux mainnet)
  - `did:hanzo:local:zeekay` (Hanzo local development)
  - `did:lux:local:zeekay` (Lux local development)
  - `did:hanzo:eth:0x742d35Cc...` (explicit Ethereum chain)
  - `did:hanzo:sepolia:0x742d35Cc...` (explicit Sepolia testnet)

### Supported Networks
- **Primary Networks**: `hanzo`, `lux` (mainnets)
- **Development**: `local` (any network)
- **Explicit Chains**: `eth`, `sepolia`, `base`, `base-sepolia`, `lux-fuji`, `ipfs`

## Omnichain Identity

### Context-Aware Resolution
Applications automatically resolve `@` prefixed identities to their network's mainnet:

```rust
// Context-aware resolution
"@zeekay" in Hanzo app  -> "did:hanzo:zeekay"  // Hanzo mainnet
"@zeekay" in Lux app    -> "did:lux:zeekay"    // Lux mainnet
"@zeekay" in other app  -> "did:hanzo:zeekay"  // Default to Hanzo

// Explicit resolution
DID::from_username("@zeekay", "hanzo") -> "did:hanzo:zeekay"
DID::from_username("@zeekay", "lux")   -> "did:lux:zeekay"
```

### Cross-Chain Identity Verification
The same user identity is verifiable across all networks:

```rust
let hanzo_did = DID::hanzo("zeekay");
let lux_did = DID::lux("zeekay");
let eth_did = DID::hanzo_eth("zeekay");

// All represent the same entity
assert!(hanzo_did.is_same_entity(&lux_did));
assert!(hanzo_did.is_same_entity(&eth_did));

// Get all network variants for omnichain verification
let variants = hanzo_did.get_omnichain_variants();
// Returns: did:hanzo:zeekay, did:lux:zeekay, did:hanzo:local:zeekay, etc.
```

### DID Resolution
```rust
use hanzo_did::{DID, DIDDocument};

// Create DIDs
let eth_did = DID::hanzo_eth("0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7");
let local_did = DID::new("hanzo", "local:localhost");

// Parse DID strings
let did = DID::parse("did:hanzo:sepolia:0x123...")?;

// Create DID Documents with verification methods and services
let did_doc = DIDDocument::new(&did);
```

## DID Method Specification

**Method Name**: `hanzo`
**Method-Specific Identifier**: `{chain}:{identifier}`

```
did-hanzo = "did:hanzo:" method-specific-id
method-specific-id = chain ":" identifier
chain = "eth" / "sepolia" / "base" / "base-sepolia" / "lux" / "lux-fuji" / "ipfs" / "local"
identifier = address / node-id / content-id
```

## DID Document Structure

Full W3C compliant DID Documents with:
- **Verification Methods**: Ed25519, X25519, ECDSA support
- **Service Endpoints**: Hanzo node services, messaging, etc.
- **Proof Mechanisms**: Cryptographic document integrity
- **JSON-LD Context**: Standard W3C contexts

```json
{
  "@context": [
    "https://www.w3.org/ns/did/v1",
    "https://w3id.org/security/suites/ed25519-2020/v1"
  ],
  "id": "did:hanzo:sepolia:0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7",
  "verificationMethod": [{
    "id": "did:hanzo:sepolia:0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7#keys-1",
    "type": "Ed25519VerificationKey2020",
    "controller": "did:hanzo:sepolia:0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7",
    "publicKeyMultibase": "z2E4dL4WenojaJ5fYtZs7hVbvcQPnTMDPytRG4XgNbtHd"
  }],
  "authentication": ["...#keys-1"],
  "service": [{
    "id": "...#hanzo-node",
    "type": "HanzoNode",
    "serviceEndpoint": "https://api.hanzo.network"
  }]
}
```

## Benefits

### Standards Compliance
- Full W3C DID Core specification compliance
- Interoperability with existing DID infrastructure
- Standard tooling compatibility

### Enhanced Security
- Cryptographic verification methods
- Multi-key support (Ed25519, X25519, ECDSA)
- Document integrity proofs

### Cross-Chain Identity
- Single identity format across all supported chains
- Chain-specific resolution
- Unified identity verification

### Developer Experience
- Clean, standards-based API
- Comprehensive Rust implementation
- No legacy compatibility complexity

## Usage Examples

```rust
use hanzo_did::{DID, DIDDocument, VerificationMethod, Service};

// Simple mainnet DIDs (recommended)
let hanzo_did = DID::hanzo("zeekay");        // did:hanzo:zeekay
let lux_did = DID::lux("zeekay");            // did:lux:zeekay

// Local development
let hanzo_local = DID::hanzo_local("zeekay"); // did:hanzo:local:zeekay
let lux_local = DID::lux_local("zeekay");     // did:lux:local:zeekay

// Context-aware resolution
let hanzo_user = DID::from_username("@zeekay", "hanzo"); // did:hanzo:zeekay
let lux_user = DID::from_username("@zeekay", "lux");     // did:lux:zeekay

// Omnichain identity verification
assert!(hanzo_did.is_same_entity(&lux_did)); // true - same user
let variants = hanzo_did.get_omnichain_variants(); // All network variants

// Explicit chain DIDs (when needed)
let eth_did = DID::hanzo_eth("0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7");
let sepolia_did = DID::hanzo_sepolia("0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7");

// Parse from string
let parsed = DID::parse("did:hanzo:zeekay")?;

// Create complete DID document
let mut doc = DIDDocument::new(&hanzo_did);
doc.add_verification_method(/* ... */);
doc.add_service(/* ... */);
```

## Native Chain DIDs

The implementation supports native chain DIDs for direct blockchain identity:

```rust
// Native Ethereum DID
let eth_did = DID::eth("0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7");
// Results in: did:eth:0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7

// Native Base L2 DID
let base_did = DID::base("zeekay");
// Results in: did:base:zeekay

// Native Polygon DID
let polygon_did = DID::polygon("zeekay.eth");
// Results in: did:polygon:zeekay.eth

// Native Arbitrum DID
let arb_did = DID::arbitrum("0x123...");
// Results in: did:arbitrum:0x123...

// Complex DID with embedded chain (Hanzo network managing Ethereum identity)
let hanzo_eth = DID::hanzo_eth("0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7");
// Results in: did:hanzo:eth:0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7
```

### Network Detection

The `get_network()` method intelligently detects the network:

```rust
// Native chain DID returns the chain network
let eth_did = DID::eth("address");
assert_eq!(eth_did.get_network(), Some(Network::Ethereum));

// Complex DID returns the embedded chain network
let hanzo_eth = DID::from_str("did:hanzo:eth:address").unwrap();
assert_eq!(hanzo_eth.get_network(), Some(Network::Ethereum));

// Simple DID returns the method network
let hanzo_did = DID::hanzo("zeekay");
assert_eq!(hanzo_did.get_network(), Some(Network::Hanzo));
```

### Supported Networks

The implementation includes support for multiple blockchain networks:

- **Hanzo**: Hanzo mainnet (`did:hanzo:username`)
- **Lux**: Lux mainnet (`did:lux:username`)
- **Ethereum**: Ethereum mainnet (`did:eth:address`)
- **Base**: Base L2 (`did:base:identifier`)
- **Polygon**: Polygon network (`did:polygon:identifier`)
- **Arbitrum**: Arbitrum L2 (`did:arbitrum:identifier`)
- **Optimism**: Optimism L2 (`did:optimism:identifier`)
- **Sepolia**: Sepolia testnet (`did:sepolia:address`)
- **Base Sepolia**: Base Sepolia testnet (`did:base-sepolia:address`)
- **Local**: Local development networks (`did:hanzo:local:identifier`)

Each network includes:
- Chain ID for EVM-compatible networks
- RPC endpoints for network interaction
- Testnet/mainnet classification

## Implementation

The W3C DID implementation is available in the `hanzo-did` crate with:
- Core DID types and parsing
- DID Document creation and validation
- Verification method support
- Service endpoint management
- Proof generation and verification
- DID resolution infrastructure
- Native chain DID support
- Omnichain identity verification
- Context-aware @ prefix resolution

No legacy migration support is provided - applications should implement their own `@` to DID mapping as needed for their specific use cases.