# Hanzo Node - AI Infrastructure Platform with Post-Quantum Security

## Project Overview

Hanzo Node is a comprehensive AI infrastructure platform built by Hanzo Industries Inc. It provides a powerful framework for creating AI agents without coding, managing LLM providers, and orchestrating AI workflows at scale. The platform now includes full NIST Post-Quantum Cryptography support for quantum-resistant security.

**Company**: Hanzo Industries Inc  
**Domain**: hanzo.ai  
**Repository**: /Users/z/work/hanzo/node

## Architecture

### Core Components

#### 1. Hanzo Node Core (`hanzo-bin/hanzo-node/`)
The main application providing:
- **LLM Provider Management**: Support for 20+ providers (OpenAI, Claude, Gemini, Ollama, etc.)
- **Job Execution Engine**: Async job processing with workflow support
- **Tool Orchestration**: Native, Python, TypeScript, and MCP tool execution
- **Network Management**: P2P networking via LibP2P, HTTP/WebSocket APIs
- **Wallet Integration**: Crypto payments and identity management

#### 2. Core Libraries (`hanzo-libs/`)

| Library | Purpose | Key Features |
|---------|---------|--------------|
| `hanzo-pqc` | Post-Quantum Cryptography | NIST FIPS 203/204, ML-KEM, ML-DSA, Hybrid modes |
| `hanzo-kbs` | Key Broker Service | Attestation, TEE support, PQC integration |
| `hanzo-crypto-identities` | Blockchain identity | NFT registry, identity verification |
| `hanzo-message-primitives` | Core messaging | Message schemas, LLM providers, job configs |
| `hanzo-libp2p-relayer` | P2P networking | Relay management, peer discovery |
| `hanzo-http-api` | API layer | REST endpoints, WebSocket, SSE |
| `hanzo-sqlite` | Database | SQLite with R2D2 pooling |
| `hanzo-embedding` | AI embeddings | Vector generation for RAG |
| `hanzo-fs` | File system | Multi-format parsing (PDF, DOCX, CSV) |
| `hanzo-tools-primitives` | Tool framework | Tool definitions and execution |
| `hanzo-mcp` | MCP integration | Model Context Protocol support |

#### 3. Tool Ecosystem
- **Native Tools**: Built-in Rust implementations
- **Python Tools**: Executed via `uv` runtime
- **JavaScript/TypeScript Tools**: Deno runtime execution
- **MCP Servers**: External tool servers via Model Context Protocol
- **Agent Tools**: Autonomous agent execution

### Technology Stack

**Core Technologies**:
- **Language**: Rust (async with Tokio)
- **Database**: SQLite with connection pooling
- **Networking**: LibP2P for P2P, Warp for HTTP
- **Cryptography**: Ed25519/X25519, Blake3 hashing
- **Storage**: S3-compatible, local filesystem

**AI/ML Integration**:
- Multiple LLM provider support
- Embedding generation for vector search
- Tool calling and function execution
- Streaming responses
- Context management

**Supported LLM Providers**:
- OpenAI (GPT-4, GPT-3.5)
- Anthropic (Claude 3)
- Google (Gemini)
- Ollama (local models)
- Groq
- DeepSeek
- Together AI
- OpenRouter
- Exo
- Grok
- Custom Hanzo Backend

## Key Features

### 1. Multi-Agent System
- Agent creation without coding
- Parallel agent execution
- Tool delegation
- Memory management

### 2. Workflow Orchestration
- Job chains and dependencies
- Retry mechanisms
- Error handling
- Progress tracking

### 3. Knowledge Management
- Vector databases (VecFS)
- Document parsing and indexing
- Semantic search
- RAG (Retrieval Augmented Generation)

### 4. Security & Privacy
- **Post-Quantum Cryptography**: NIST-compliant ML-KEM and ML-DSA
- **Privacy Tiers**: 5-level system from Open to GPU TEE-I/O
- **Hybrid Cryptography**: ML-KEM + X25519 for defense-in-depth
- **Key Broker Service**: Attestation-based key release
- **TEE Support**: SEV-SNP, TDX, H100 CC, Blackwell TEE-I/O
- End-to-end encryption
- Identity verification
- Access control
- Secure key management

### 5. Extensibility
- Plugin architecture
- Custom tool development
- Provider abstraction
- Protocol-agnostic design

## Development Patterns

### Code Organization
```
/hanzo-node/
├── hanzo-bin/          # Binary applications
│   └── hanzo-node/     # Main node application
├── hanzo-libs/         # Core libraries
│   ├── hanzo-*/        # Individual library modules
├── hanzo-test-*/       # Testing frameworks
├── docs/               # Documentation
├── cloud-node/         # Cloud deployment configs
└── scripts/            # Utility scripts
```

### Build System
- **Cargo Workspace**: Monorepo with shared dependencies
- **Build Command**: `cargo build --release`
- **Test Command**: `cargo test --workspace`
- **Features**: Conditional compilation for different environments

### API Structure
- **V2 API**: Modern REST/WebSocket at `/v2/*`
- **SSE Endpoints**: Real-time streaming at `/sse/*`
- **WebSocket**: Bidirectional communication at `/ws`
- **Health Check**: `/health` endpoint

## Configuration

### Environment Variables
```bash
# Core Configuration
NODE_API_LISTEN_ADDRESS=0.0.0.0:9550
NODE_WS_LISTEN_ADDRESS=0.0.0.0:9551
NODE_STORAGE_PATH=./storage
RUST_LOG=info

# Network Configuration
RELAY_SERVER=true
LIBP2P_PORT=4001
LIBP2P_RELAY_ADDRESS=/dns4/relay.hanzo.ai/tcp/4001

# Database
DATABASE_URL=sqlite://./storage/db.sqlite
SQLITE_ENABLE_WAL=true

# Optional Services
REDIS_URL=redis://localhost:6379
S3_ENDPOINT=https://s3.amazonaws.com
```

### Key Files
- `Cargo.toml` - Workspace configuration
- `cloud-node/env.conf` - Production environment
- `cloud-node/hanzo-node.service` - Systemd service
- `docker-build/Dockerfile` - Container image

## Common Operations

### Starting the Node
```bash
# Development
cargo run --bin hanzo_node -- --node-api-port 9550

# Production
./target/release/hanzo_node --config /etc/hanzo/config.toml

# Docker
docker run -p 9550:9550 hanzo/node:latest
```

### Managing LLM Providers
```bash
# Add provider via API
curl -X POST http://localhost:9550/v2/add_llm_provider \
  -H "Content-Type: application/json" \
  -d '{"provider_type": "openai", "api_key": "sk-..."}'
```

### Creating Agents
```bash
# Create agent via API
curl -X POST http://localhost:9550/v2/create_agent \
  -H "Content-Type: application/json" \
  -d '{"name": "Assistant", "tools": ["web_search", "calculator"]}'
```

## Architecture Decisions

### Why Rust?
- Memory safety without garbage collection
- High performance for AI workloads
- Excellent async runtime (Tokio)
- Strong type system for reliability

### Why SQLite?
- Embedded database (no separate process)
- Excellent performance for single-node
- Built-in full-text search
- Easy backup and migration

### Why LibP2P?
- Decentralized networking
- NAT traversal
- Protocol multiplexing
- Built-in encryption

### Why Multiple LLM Providers?
- Avoid vendor lock-in
- Cost optimization
- Model diversity
- Fallback options

## Testing Strategy

### Unit Tests
- Per-module tests in `src/` directories
- Mock providers for isolation
- Property-based testing for core logic

### Integration Tests
- End-to-end workflows in `tests/it/`
- Real provider testing (when configured)
- Network simulation tests

### Performance Tests
- Benchmark suite for critical paths
- Load testing for concurrent operations
- Memory profiling for long-running processes

## Deployment

### Cloud Deployment
- Systemd service configuration
- Docker containerization
- Kubernetes manifests available
- Auto-scaling support

### Local Development
- Single binary deployment
- Minimal dependencies
- Cross-platform support (Linux, macOS, Windows)
- Development tools included

## Security Considerations

### Post-Quantum Security
- **ML-KEM (FIPS 203)**: Quantum-resistant key encapsulation
  - ML-KEM-768 default for most operations
  - ML-KEM-1024 for highest security tiers
- **ML-DSA (FIPS 204)**: Quantum-resistant digital signatures
  - ML-DSA-65 default for most operations
  - ML-DSA-87 for highest security tiers
- **Hybrid Mode**: Combines PQC with classical crypto
- **Privacy Tiers**: Automatic security level selection based on environment

### Authentication
- Ed25519 signature verification (with PQC migration path)
- ML-DSA for quantum-resistant signatures
- API key management
- OAuth2 integration (optional)

### Encryption
- TLS for network communication (PQC-ready)
- File encryption at rest with ML-KEM key wrapping
- Key derivation with Blake3 and SP 800-56C KDF
- ChaCha20Poly1305 for AEAD operations

### Access Control
- Role-based permissions
- TEE attestation verification
- Resource isolation
- Rate limiting

## Performance Optimization

### Concurrency
- Async/await throughout
- Connection pooling
- Parallel job execution
- Stream processing

### Caching
- In-memory caches for hot data
- Redis integration (optional)
- Embedding cache for vectors
- LLM response caching

### Resource Management
- Configurable thread pools
- Memory limits
- Disk quota management
- Network bandwidth control

## Monitoring & Observability

### Logging
- Structured logging with `tracing`
- Log levels: ERROR, WARN, INFO, DEBUG, TRACE
- File and console outputs
- Log rotation

### Metrics
- Prometheus metrics export
- Custom metrics for AI operations
- Performance counters
- Resource utilization

### Health Checks
- `/health` endpoint
- Component status
- Dependency checks
- Performance indicators

## Future Roadmap

### Planned Features
- Enhanced MCP support
- Multi-node clustering
- Advanced workflow templates
- Visual workflow builder
- More LLM provider integrations
- CAVP validation for PQC algorithms
- Hardware acceleration for PQC operations

### Research Areas
- Distributed inference
- Federated learning support
- Advanced RAG techniques
- Multi-modal processing improvements
- SLH-DSA (SPHINCS+) for stateless signatures
- Migration tools for classical to PQC transition

## Contributing

### Development Setup
1. Install Rust toolchain
2. Clone repository
3. Run `cargo build`
4. Run tests: `cargo test`

### Code Style
- Follow Rust conventions
- Use `cargo fmt` for formatting
- Run `cargo clippy` for linting
- Write comprehensive tests

### Documentation
- Update LLM.md for architecture changes
- Document new features in `/docs`
- Add inline code comments
- Update API documentation

## License

Copyright © 2024 Hanzo Industries Inc. All rights reserved.

## Post-Quantum Cryptography Details

### PQC Implementation (`hanzo-pqc`)
Full NIST-compliant Post-Quantum Cryptography implementation:

#### Algorithms
- **ML-KEM (FIPS 203)**: Module-Lattice Key Encapsulation
  - ML-KEM-512 (Level 1): 128-bit security
  - ML-KEM-768 (Level 3): 192-bit security [DEFAULT]
  - ML-KEM-1024 (Level 5): 256-bit security
  
- **ML-DSA (FIPS 204)**: Module-Lattice Digital Signatures
  - ML-DSA-44 (Level 2): 128-bit security
  - ML-DSA-65 (Level 3): 192-bit security [DEFAULT]
  - ML-DSA-87 (Level 5): 256-bit security

#### Privacy Tiers
| Tier | Environment | ML-KEM | ML-DSA | Features |
|------|-------------|---------|---------|----------|
| 0 | Open Data | 768 | 65 | Basic quantum resistance |
| 1 | At-Rest | 768 | 65 | + SIM key protection |
| 2 | CPU TEE | 768 | 65 | + FIPS mode, attestation |
| 3 | GPU CC (H100) | 1024 | 87 | + Encrypted DMA |
| 4 | GPU TEE-I/O | 1024 | 87 | + NVLink protection |

#### Usage Example
```rust
use hanzo_pqc::{
    kem::{Kem, KemAlgorithm, MlKem},
    signature::{Signature, SignatureAlgorithm, MlDsa},
    privacy_tiers::PrivacyTier,
    config::PqcConfig,
};

// Configure for privacy tier
let config = PqcConfig::for_privacy_tier(PrivacyTier::AccessCpuTee);

// Quantum-safe key encapsulation
let kem = MlKem::new();
let keypair = kem.generate_keypair(config.kem).await?;

// Quantum-safe signatures
let dsa = MlDsa::new();
let (vk, sk) = dsa.generate_keypair(config.sig).await?;
```

### Key Broker Service (`hanzo-kbs`)
Attestation-based key release with PQC integration:

- **PqcVault**: General quantum-resistant vault
- **GpuCcVault**: H100 Confidential Computing vault
- **GpuTeeIoVault**: Blackwell TEE-I/O vault
- DEK wrapping with ML-KEM
- Attestation signing with ML-DSA

### Performance Metrics
| Operation | Time | Notes |
|-----------|------|-------|
| ML-KEM-768 Keygen | ~50 μs | Default KEM |
| ML-KEM-768 Encapsulate | ~60 μs | 1088-byte ciphertext |
| ML-KEM-768 Decapsulate | ~70 μs | 32-byte shared secret |
| ML-DSA-65 Sign | ~250 μs | 3309-byte signature |
| ML-DSA-65 Verify | ~120 μs | Deterministic |

### Testing PQC
```bash
# Build PQC components
cargo build --package hanzo_pqc --all-features
cargo build --package hanzo_kbs --all-features

# Run tests
cargo test --package hanzo_pqc --all-features
cargo test --package hanzo_kbs --features pqc

# Run benchmarks
cargo bench --package hanzo_pqc

# Run examples
cargo run --example basic_usage --features "ml-kem ml-dsa hybrid"
```

### FIPS Compliance
- FIPS 203 (ML-KEM) ✅
- FIPS 204 (ML-DSA) ✅
- SP 800-56C (KDF) ✅
- SP 800-90A (RNG) ✅
- FIPS mode configurable

---

*Last Updated: December 2024*
*Hanzo Node Version: 1.1.8*
*PQC Status: 100% Complete*
*Maintained for: Hanzo Node Development Team*