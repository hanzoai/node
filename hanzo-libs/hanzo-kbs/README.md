# Hanzo KBS (Key Broker Service)

A comprehensive Key Broker Service for Hanzo Node that manages cryptographic key lifecycle with TEE (Trusted Execution Environment) attestation and privacy tiers.

## Architecture Overview

The Hanzo auth stack consists of three integrated components:

### 1. **KMS (Key Management System)** - Infisical
- External service for centralized key management
- Provides enterprise-grade secret storage
- Handles encryption, rotation, and access control
- Located at: `/Users/z/work/hanzo/kms` (Infisical deployment)

### 2. **KBS (Key Broker Service)** - This Module
- Attestation-based key release
- Privacy tier enforcement (0-4)
- TEE integration (SEV-SNP, TDX, NVIDIA H100/Blackwell)
- Web2/Web3 authentication support

### 3. **IAM (Identity & Access Management)**
- Integrated with both Web2 (OAuth, SAML) and Web3 (DIDs, wallets)
- Role-based access control (RBAC)
- Capability-based security tokens
- Cross-chain identity verification

## Privacy Tiers

The KBS implements a 5-tier privacy model with graceful degradation:

| Tier | Name | Description | TEE Support | Use Cases |
|------|------|-------------|-------------|-----------|
| **0** | Open | No encryption, public data | None | Public APIs, open datasets |
| **1** | Encrypted | Basic encryption at rest/transit | None | Standard web services |
| **2** | Secure | Hardware security module support | HSM/TPM | Enterprise applications |
| **3** | GPU CC | GPU Confidential Computing | NVIDIA H100 | AI model training/inference |
| **4** | GPU TEE-I/O | Full I/O isolation | NVIDIA Blackwell | Maximum security workloads |

### Graceful Degradation

The system automatically degrades to lower tiers when higher tier hardware is unavailable:

```rust
// Tier 4 (Blackwell) → Tier 3 (H100) → Tier 2 (HSM) → Tier 1 (Software)
let vault = VaultFactory::create_with_fallback(
    PrivacyTier::Tier4GpuTeeIo,
    &kbs,
)?;
```

## Web2/Web3 Compatibility

### Web2 Authentication
- OAuth 2.0 / OIDC providers
- SAML integration
- API keys with HMAC
- Session-based auth

### Web3 Authentication
- Ethereum wallet signatures
- DIDs (Decentralized Identifiers)
- Chain-based attestation
- Zero-knowledge proofs

## Integration with Infisical KMS

The KBS integrates with Infisical for enterprise key management:

```rust
// Configure Infisical connection
let kms_config = KmsConfig {
    endpoint: "https://kms.hanzo.ai",
    api_key: std::env::var("INFISICAL_API_KEY")?,
    project_id: std::env::var("INFISICAL_PROJECT_ID")?,
};

// Create KBS with Infisical backend
let kbs = KeyBrokerService::new(
    kms_config,
    attestation_config,
)?;
```

## TEE Support

### NVIDIA GPU TEE Integration

#### H100 Confidential Computing (Tier 3)
```rust
// Automatic detection of H100 CC mode
let vault = GpuCcVault::new(kbs, device_id);
if vault.is_cc_enabled()? {
    // Use hardware-protected keys
    vault.use_key(key_id, |key| {
        // Key operations in GPU enclave
    })?;
}
```

#### Blackwell TEE-I/O (Tier 4)
```rust
// Maximum security with I/O isolation
let vault = GpuTeeIoVault::new(kbs, device_id, mig_instance);
// All I/O is encrypted and authenticated
vault.use_key_with_io_protection(key_id, |key| {
    // Operations with full isolation
})?;
```

### CPU TEE Support

- **AMD SEV-SNP**: Secure Encrypted Virtualization
- **Intel TDX**: Trust Domain Extensions
- **ARM CCA**: Confidential Compute Architecture

## API Usage

### Basic Key Operations

```rust
use hanzo_kbs::{KeyBrokerService, PrivacyTier, KeyId};

// Initialize KBS
let kbs = KeyBrokerService::new(config).await?;

// Request key with attestation
let attestation = generate_attestation(TeeType::SevSnp).await?;
let key = kbs.request_key(
    &KeyId::new("api-key"),
    attestation,
    PrivacyTier::Tier2Secure,
).await?;

// Use key with automatic cleanup
kbs.use_key(&key_id, |key_material| {
    // Perform cryptographic operations
    encrypt_data(key_material, data)
})?;
```

### Capability Tokens

```rust
// Create capability token for agent
let token = kbs.create_capability_token(
    agent_id,
    vec!["gpu_compute", "tee_access"],
    Duration::hours(1),
)?;

// Verify and use token
let authorized = kbs.verify_capability(token)?;
```

## Deployment

### Docker Compose

```yaml
version: '3.8'
services:
  infisical-kms:
    image: infisical/infisical:latest
    environment:
      - ENCRYPTION_KEY=${ENCRYPTION_KEY}
      - AUTH_SECRET=${AUTH_SECRET}
    ports:
      - "8080:8080"
  
  hanzo-node:
    image: hanzo/node:latest
    environment:
      - KMS_ENDPOINT=http://infisical-kms:8080
      - ENABLE_TEE=true
      - PRIVACY_TIER=3
    depends_on:
      - infisical-kms
```

### Kubernetes

```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: hanzo-kbs-config
data:
  kms_endpoint: "https://kms.hanzo.ai"
  default_privacy_tier: "2"
  enable_tee_attestation: "true"
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: hanzo-node
spec:
  template:
    spec:
      containers:
      - name: hanzo-node
        image: hanzo/node:latest
        envFrom:
        - configMapRef:
            name: hanzo-kbs-config
```

## Security Considerations

### Key Protection Hierarchy

1. **Master Keys**: Never leave TEE, hardware-bound
2. **KEKs (Key Encryption Keys)**: Wrapped by master keys
3. **DEKs (Data Encryption Keys)**: Wrapped by KEKs
4. **Session Keys**: Ephemeral, time-bound

### Attestation Chain

```
Hardware Root of Trust
    ↓
Platform Firmware (PSP/ME)
    ↓
TEE Attestation (SEV/TDX/GPU)
    ↓
KBS Verification
    ↓
Key Release
```

### Zero Trust Architecture

- No implicit trust between components
- Every request requires attestation
- Continuous verification
- Least privilege access

## Performance Optimization

### Caching Strategy
- Session key caching (5 min default)
- Attestation result caching (1 min)
- Capability token caching

### Connection Pooling
- KMS connection pool (10 connections)
- Database connection pool (R2D2)
- HTTP/2 multiplexing for API calls

## Monitoring & Observability

### Metrics (Prometheus)
- Key request latency
- Attestation success rate
- TEE availability
- Vault operations/sec

### Logging
- Audit logs for all key operations
- Attestation verification logs
- Security events
- Performance metrics

## Testing

```bash
# Run unit tests
cargo test --lib

# Run integration tests (requires TEE simulator)
cargo test --test integration

# Run with TEE hardware
TEE_MODE=hardware cargo test

# Benchmark performance
cargo bench
```

## Migration from Other Systems

### From HashiCorp Vault
```rust
// Compatible API for easy migration
let vault_compat = VaultCompatLayer::new(kbs);
vault_compat.read("secret/data/api-key")?;
```

### From AWS KMS
```rust
// AWS KMS compatibility layer
let aws_compat = AwsKmsCompat::new(kbs);
aws_compat.decrypt(ciphertext_blob)?;
```

## Contributing

See [CONTRIBUTING.md](../../CONTRIBUTING.md) for development guidelines.

## License

Apache 2.0 - See [LICENSE](../../LICENSE) for details.

## Support

- Documentation: https://docs.hanzo.ai/kbs
- Issues: https://github.com/hanzoai/hanzo-node/issues
- Discord: https://discord.gg/hanzo