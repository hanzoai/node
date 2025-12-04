# Hanzo Node - AI Infrastructure Platform with Post-Quantum Security

## Major Feature Merge (Oct 2025)

### Merged from hanzo-node-next
Integrated comprehensive feature set from hanzo-node-next branch, adding 11 new crates and significantly expanding functionality:

**New Core Libraries:**
- `hanzo-db` - LanceDB vector database integration for high-performance vector search
- `hanzo-llm` - **Local LLM inference support** - Run models locally without external APIs
- `hanzo-pqc` - Post-Quantum Cryptography (ML-KEM-768/1024, ML-DSA-65/87)
- `hanzo-kbs` - Key Broker Service with TEE attestation support
- `hanzo-did` - Decentralized Identity system
- `hanzo-model-discovery` - Automatic model discovery and configuration
- `hanzo-config` - Centralized configuration management
- `hanzo-mining` - Mining and compute marketplace support
- `hanzo-wasm-runtime` - WebAssembly runtime for sandboxed execution
- `hanzo-hmm` - Hidden Markov Model support for sequence analysis
- `hanzo-tools-runner` - Native tool execution framework (migrated from shinkai_tools_runner)

**Key Capabilities Added:**
1. **Local LLM Inference** - Run Qwen3, Llama, and other models locally via hanzo-llm
2. **LanceDB Integration** - High-performance columnar vector storage
3. **Quantum-Resistant Security** - Full NIST-compliant PQC implementation
4. **TEE Support** - Confidential computing with SEV-SNP, TDX, H100 CC
5. **WebAssembly Isolation** - Sandboxed tool execution
6. **Enhanced Configuration** - TOML-based centralized config

**Migration Notes:**
- Removed all `shinkai_tools_runner` dependencies - now using native `hanzo_tools_runner`
- All workspace members updated to hanzo naming conventions
- Maintains backward compatibility with existing hanzo-desktop integration

## Recent Improvements (Nov 2025)

### AI Coin Mining & Teleport Protocol (Nov 30, 2025)

Added EVM integration to `hanzo-mining` crate for AI Coin mining rewards and cross-chain teleportation:

**AI Coin Mining System**:
- Native currency mined on open AI protocol (BitTorrent-style)
- Mining reward types: `DataSharing`, `ComputeProvision`, `ModelHosting`, `ModelRegistration`, `InferenceServing`
- Rewards earned by: sharing training data, providing GPU/CPU compute, keeping models loaded, hosting registered models/embeddings

**Teleport Protocol**:
- Bridges AI coins from protocol to EVM chains
- Destination chains: Lux C-Chain (96369), Zoo EVM (200200), Hanzo EVM (36963)
- `TeleportTransfer` struct tracks teleport state across chains
- Status tracking: Initiated → PendingConfirmation → Processing → Minting → Completed

**Chain Configurations**:
| Chain | Chain ID | Token | Mining Contract |
|-------|----------|-------|-----------------|
| Hanzo Mainnet | 36963 | HAI | 0x369000...aAAI |
| Hanzo Testnet | 36964 | HAI | 0x369100...aAAI |
| Zoo Mainnet | 200200 | ZOO | 0x200200...aAAI |
| Zoo Testnet | 200201 | ZOO | 0x200201...aAAI |
| Lux C-Chain | 96369 | LUX | 0x4C5558...aAAI |
| Lux Testnet | 96368 | LUX | 0x4C5559...aAAI |

**Implementation Files**:
- `hanzo-libs/hanzo-mining/src/evm.rs` - EVM client, rewards manager, teleport protocol
- `hanzo-libs/hanzo-mining/src/lib.rs` - Module exports
- 7 tests passing for EVM integration

**Usage Example**:
```rust
use hanzo_mining::evm::{ChainConfig, RewardsManager, TeleportDestination};

// Create rewards manager for Hanzo mainnet
let manager = RewardsManager::new(NetworkType::HanzoMainnet);

// Get pending rewards across all chains
let rewards = manager.refresh_pending_rewards(&miner_address).await?;

// Teleport AI coins to Zoo EVM
let transfer = TeleportTransfer {
    destination: TeleportDestination::ZooEvm,
    amount: 1_000_000_000_000_000_000, // 1 AI coin
    ..
};
```

### ML-DSA Quantum-Safe Mining Wallets (Nov 30, 2025)

Added quantum-safe ML-DSA (FIPS 204) mining wallets with full Teleport bridge integration:

**Architecture**:
```
┌────────────────────────────────────────────────────────────────┐
│                     Hanzo Networks L1 (Mining)                 │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐     │
│  │   ML-DSA     │───▶│     Lux      │───▶│   Global     │     │
│  │   Wallet     │    │   Consensus  │    │   Ledger     │     │
│  └──────────────┘    └──────────────┘    └──────────────┘     │
│                              │                                 │
│                       Teleport Bridge                          │
└──────────────────────────────┼─────────────────────────────────┘
        ┌──────────────────────┼──────────────────┐
        ▼                      ▼                  ▼
   Hanzo EVM (36963)     Zoo EVM (200200)   Lux C-Chain (96369)
```

**ML-DSA Security Levels**:
| Level | Algorithm | Public Key | Signature | Quantum Security |
|-------|-----------|------------|-----------|-----------------|
| 2 | ML-DSA-44 | 1,312 bytes | 2,420 bytes | 128-bit |
| 3 | ML-DSA-65 | 1,952 bytes | 3,309 bytes | 192-bit (default) |
| 5 | ML-DSA-87 | 2,592 bytes | 4,627 bytes | 256-bit |

**Key Components**:
- `MiningWallet` - Quantum-safe wallet with ML-DSA keypair
- `MiningLedger` - Global reward ledger with Lux consensus
- `MiningBridge` - Connects wallet to Teleport for cross-chain transfers
- `TeleportTransfer` - Tracks transfers from L1 to EVM L2s

**Wallet Operations**:
```rust
use hanzo_mining::{MiningWallet, SecurityLevel, MiningBridge};

// Generate quantum-safe mining wallet
let wallet = MiningWallet::generate(SecurityLevel::Level3).await?;
println!("Address: {}", wallet.address());  // 0x-prefixed

// Sign mining transactions
let signature = wallet.sign(&message).await?;
let valid = wallet.verify(&message, &signature)?;

// Export/import with ChaCha20Poly1305 encryption
let encrypted = wallet.export_to_bytes("passphrase")?;
let restored = MiningWallet::import_from_bytes(&encrypted, "passphrase")?;
```

**Bridge Integration**:
```rust
// Create bridge with new wallet
let bridge = MiningBridge::generate_new(SecurityLevel::Level3).await?;

// Teleport rewards to Zoo EVM
let teleport_id = bridge.teleport_to_evm(
    ChainId::ZooEVM,
    "0x1234...recipient",
    1_000_000_000_000_000_000, // 1 AI token
).await?;

// Check mining stats
let stats = bridge.mining_stats().await;
println!("Total mined: {}", stats.total_mined);
println!("Teleported: {}", stats.total_teleported);
```

**Implementation Files**:
- `hanzo-libs/hanzo-mining/src/wallet.rs` - ML-DSA wallet implementation
- `hanzo-libs/hanzo-mining/src/ledger.rs` - Global reward ledger
- `hanzo-libs/hanzo-mining/src/bridge.rs` - Teleport bridge integration
- `hanzo-libs/hanzo-mining/src/evm.rs` - EVM chain configurations

**Test Coverage**: 23 tests passing
```bash
cargo test -p hanzo-mining  # All 23 tests pass
```

**Cross-Chain Proposals** (Documentation):
- LP-2000: AI Mining Standard (~/work/lux/lps/LPs/lp-2000-ai-mining-standard.md)
- HIP-006: Hanzo AI Mining Protocol (~/work/hanzo/hips/HIP-006-ai-mining-protocol.md)
- ZIP-005: Zoo AI Mining Integration (~/work/zoo/zips/ZIP-005-ai-mining-integration.md)

**Solidity Precompiles** (~/work/lux/standard/src/precompiles/):
- `AIMining.sol` - AI Mining precompile at 0x0300
- `TeleportBridge.sol` - Teleport bridge precompile at 0x0301

**EVM Precompile Interface**:
```solidity
interface IAIMining {
    function miningBalance(address miner) external view returns (uint256);
    function verifyMLDSA(bytes calldata pk, bytes calldata msg, bytes calldata sig) external view returns (bool);
    function claimTeleport(bytes32 teleportId) external returns (uint256);
    function pendingTeleports(address recipient) external view returns (bytes32[] memory);
}
```

### Native AI-Chain L1 with Lux Consensus (Nov 30, 2025)

Added native Lux consensus integration for operating as a sovereign L1 "AI-Chain":

**AI-Chain Architecture**:
```
┌─────────────────────────────────────────────────────────────────┐
│                      AI-Chain (Hanzo L1)                        │
│                                                                 │
│  ┌──────────────┐   ┌──────────────┐   ┌──────────────┐        │
│  │  Consensus   │   │   Ledger     │   │   Teleport   │        │
│  │  Engine      │──▶│   State      │──▶│   Bridge     │        │
│  │ (Lux BFT)    │   │              │   │              │        │
│  └──────────────┘   └──────────────┘   └──────────────┘        │
│         │                                     │                 │
│         │ ML-DSA Signatures                   │                 │
│         ▼                                     ▼                 │
│  ┌──────────────┐                   ┌──────────────────────┐   │
│  │  Block       │                   │   Destination EVMs   │   │
│  │  Proposer    │                   │   - Hanzo (36963)    │   │
│  │              │                   │   - Zoo (200200)     │   │
│  └──────────────┘                   │   - Lux (96369)      │   │
│                                     └──────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

**Consensus Parameters**:
| Parameter | Value | Description |
|-----------|-------|-------------|
| Network ID | 36963 (mainnet), 36964 (testnet) | AI-Chain network identifier |
| Block Time | 500ms | Sub-second finality |
| Quorum | 69% | BFT threshold |
| Finality Rounds | 2 | 2-round finality |
| Sample Size | 20 | Validator sample for voting |
| Quantum-Safe | ✓ | ML-DSA-65 signatures (NIST Level 3) |

**Operating Modes**:
1. **Embedded Mode** (`--features consensus`): Native L1 with embedded lux-consensus
2. **RPC Mode** (default): Client connecting to remote consensus nodes

**Implementation Files**:
- `hanzo-libs/hanzo-mining/src/consensus.rs` - Native consensus engine wrapper
- `hanzo-libs/hanzo-mining/src/ledger.rs` - Global mining ledger (730 lines)
- `hanzo-libs/hanzo-mining/Cargo.toml` - Optional lux-consensus dependency

**Cargo Configuration**:
```toml
[dependencies.lux-consensus]
path = "../../../../lux/consensus/pkg/rust"
optional = true

[features]
default = []
consensus = ["lux-consensus"]  # Enable native L1 mode
```

**Usage Example**:
```rust
use hanzo_mining::consensus::{ConsensusEngine, ConsensusConfig, AIChainBlock, AIChainVote};
use hanzo_mining::ledger::VoteType;

// Create AI-Chain consensus engine
let config = ConsensusConfig::ai_chain_mainnet();
let mut engine = ConsensusEngine::new(config)?;
engine.start().await?;

// Propose a new block with mining transactions
let block = AIChainBlock::new(
    parent_id,
    height,
    proposer_pubkey,
    transactions,
);
engine.submit_block(block).await?;

// Record votes from validators
let vote = AIChainVote::new(block.id, VoteType::Preference, voter_pubkey);
let accepted = engine.record_vote(vote).await?;
if accepted {
    println!("Block finalized!");
}
```

**Key Types**:
- `ConsensusEngine` - Main engine wrapping lux-consensus Chain
- `ConsensusConfig` - AI-Chain consensus configuration
- `AIChainBlock` - Block with mining transactions and ML-DSA signatures
- `AIChainVote` - Validator vote with quantum-safe signature
- `ConsensusMode` - Embedded (native L1) or RPC (client) mode

**Test Coverage**: 31 tests passing
```bash
cargo test -p hanzo-mining  # Base tests (31 pass)
cargo test -p hanzo-mining --features consensus  # With native consensus
```

**Dependencies**:
- `lux-consensus` v1.22.0 from `~/work/lux/consensus/pkg/rust`
- Default mode (no feature) uses RPC to connect to consensus nodes

**Planned Architecture (Pure Rust/MLX)**:
```
┌─────────────────────────────────────────────────────────────────┐
│                    TypeScript SDK                                │
│                (@hanzo/consensus-sdk)                            │
└───────────────────────────┬─────────────────────────────────────┘
                            │ NAPI-RS / wasm-bindgen
                            ▼
┌─────────────────────────────────────────────────────────────────┐
│                   Rust Core (lux-consensus)                      │
│                                                                  │
│    ┌────────────────────┬────────────────────┐                  │
│    │   Pure Rust (CPU)  │   C++ MLX (GPU)    │                  │
│    │   (default)        │   (Apple Silicon)  │                  │
│    │                    │                    │                  │
│    │  - BFT Engine      │  - Vote batch GPU  │                  │
│    │  - BLS Signatures  │  - Sig aggregation │                  │
│    │  - DAG Consensus   │  - DAG finality    │                  │
│    │  - Storage         │  - Hash parallel   │                  │
│    └────────────────────┴────────────────────┘                  │
│                                                                  │
│    Feature Flags:                                                │
│    - default: Pure Rust CPU implementation                       │
│    - mlx: C++ MLX GPU acceleration (Apple Silicon)               │
│    - cuda: CUDA GPU acceleration (NVIDIA)                        │
└─────────────────────────────────────────────────────────────────┘
```

**SDK Binding Layers**:
| Language | Package | Binding Method |
|----------|---------|----------------|
| Rust | `lux-consensus` | Native |
| TypeScript | `@hanzo/consensus-sdk` | NAPI-RS + wasm-bindgen |
| Python | `lux-consensus-py` | PyO3 |
| Go | `lux-consensus` | CGO (existing) |

**GPU Acceleration Targets**:
- BLS signature aggregation (50-100x speedup with batch ops)
- Vote counting & quorum detection (20-50x)
- DAG finality computation (20-50x via parallel BFS)
- SHA256 hash parallelization (30-100x)

**Estimated Port Effort**: 12-16 weeks for comprehensive Rust rewrite with MLX

### TypeScript SDK Implementation (Nov 30, 2025)

Created `@luxfi/consensus` TypeScript SDK with NAPI-RS bindings wrapping the native Rust consensus:

**Location**: `/Users/z/work/lux/consensus/pkg/typescript/`

**Files Created**:
- `Cargo.toml` - NAPI-RS binding configuration
- `src/lib.rs` - Native Node.js bindings (~400 lines)
- `package.json` - npm package configuration
- `index.js` - Native module loader with platform detection
- `index.d.ts` - Full TypeScript type definitions
- `README.md` - Documentation with usage examples
- `test/consensus.test.js` - Test suite

**TypeScript Usage**:
```typescript
import { ConsensusEngine, testnetConfig, VoteType } from '@luxfi/consensus';

const engine = new ConsensusEngine(testnetConfig());
engine.start();

// Add a block
engine.addBlock({
  id: '01'.repeat(32),
  parentId: '00'.repeat(32),
  height: 1,
  payload: Buffer.from('Hello, Lux!').toString('hex'),
  timestamp: Date.now(),
});

// Record votes
for (let i = 0; i < 5; i++) {
  engine.recordVote({
    blockId: '01'.repeat(32),
    voteType: VoteType.Preference,
    voter: i.toString(16).padStart(64, '0'),
  });
}

console.log(engine.isAccepted('01'.repeat(32))); // true
engine.stop();
```

**Platform Support**:
- macOS arm64 (Apple Silicon)
- macOS x64
- Linux arm64/x64 (glibc/musl)
- Windows x64

**GPU Feature Flags** (added to `lux-consensus` Cargo.toml):
```toml
[features]
default = []
simd = ["blake3", "rayon"]      # CPU-optimized with SIMD
mlx = ["mlx-sys", "simd"]       # Apple Silicon GPU
cuda = ["simd"]                  # NVIDIA GPU (future)
gpu = ["mlx", "cuda"]           # Full GPU acceleration
```

**Build Commands**:
```bash
cd /Users/z/work/lux/consensus/pkg/typescript
pnpm build        # Build native module
pnpm test         # Run tests
```

**Status**: ✅ Both SDKs compile, all 5 Rust tests pass

### hanzo-db-sqlite Compilation Fixes
- Fixed unused mutable variable warning in `tool_payment_req_manager.rs:62` - removed unnecessary `mut` from `transaction` variable
- hanzo-db-sqlite now compiles cleanly with zero warnings
- All 138 tests pass successfully
- Verified clean build: `cargo build -p hanzo-db-sqlite` passes without errors
- Test suite verified: `cargo test -p hanzo-db-sqlite` shows all tests passing

### hanzo-tools Compilation Fixes
- Fixed unused variable warning in `mcp_server_tool.rs:118` - prefixed error variable with underscore
- Fixed irrefutable if-let pattern in `mcp_server_tool.rs:153` - replaced pattern match with direct assignment
- All hanzo-tools crate warnings resolved
- Verified clean build: `cargo build -p hanzo-tools` passes without errors

### Prior Improvements (Sep 2025)

### Build System Enhancements
- Added comprehensive Makefile for easier building and development
- Fixed compilation warnings across the codebase
- Improved project structure for better composability and reusability

### Configuration System
- Created `hanzo.toml` configuration file for centralized settings
- Added `config` module for loading configuration from file or environment
- Supports both TOML file and environment variable configuration

### Code Quality Improvements
- Fixed unreachable pattern warnings
- Added missing imports for `EmbeddingModelType` and related types
- Suppressed dead code warnings where appropriate
- Fixed unused variable warnings
- Removed unused imports across multiple modules

### Project Structure
- Added library support (`lib.rs`) alongside binary for reusability
- Created example programs showing library usage
- Improved modularity for external consumption

### Build Instructions
```bash
make build         # Build debug version
make build-release # Build optimized release
make run          # Run the node locally
make dev          # Build and run for development
make test         # Run tests
make clean        # Clean build artifacts
```

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
---

# GRPO and Experience-Based Learning System Analysis (Oct 28, 2025)

## Executive Summary

**Assessment of GRPO Implementation Feasibility in Hanzo Node**

The Hanzo Node codebase has **foundational infrastructure components** that can support GRPO implementation, but **lacks explicit training loops, reward mechanisms, and policy optimization frameworks**. The system is primarily designed for agent execution and routing, not for learning-based adaptation.

**Current State**: Capable of capturing execution traces, but missing core RL/learning components.
**Readiness for GRPO**: **40% - Requires significant new implementation**
**MVP Timeline**: 4 weeks for basic local rollouts
**Production Timeline**: 8 weeks for distributed optimization

## Current Relevant Capabilities

### Experience Storage & Trajectory Tracking

**inbox_messages Table** (`hanzo-sqlite/src/lib.rs`):
- Parent-child message hashing for implicit trajectory tracking
- Full message history storage per job/inbox
- Temporal ordering via time_key
- BLOB storage for complete message state

**Job System** (`hanzo-sqlite/src/job_manager.rs`):
- Step-by-step execution history in `step_history: Vec<HanzoMessage>`
- Job completion status tracking
- Fork/branch support for alternative trajectories
- Config snapshots per job

**Limitation**: Steps are HanzoMessages (general purpose), not structured (state, action, reward) tuples

### Agent Execution & Tool Orchestration

**Agent Framework** (`hanzo-sqlite/src/agent_manager.rs`):
- Agent identity and configuration with tools list
- Tool routing and execution capability
- Knowledge base association
- Per-agent configuration overrides

**Tools Runner** (`hanzo-tools-runner/src/`):
- Multiple execution runtimes (Deno, Python, containers)
- Execution context preservation
- Result capture and serialization
- Error tracking

**Limitation**: No trajectory collection across tool calls or reward signals

### Adaptive Learning System (HLLM)

Located in: `hanzo-libs/hanzo-llm/src/`

**Existing Components**:
- `UserAdapter` with BitDelta compression
- `AdapterManager` for per-user model selection
- Feedback mechanism with gradient-based updates
- Adaptive learning rate scheduling
- Expected Free Energy (EFE) calculations

**Current Scope**: Only for LLM provider routing (model selection), NOT policy optimization

### Job Queue Management

**JobQueueManager** (`hanzo-job-queue-manager/src/`):
- In-memory + persistent queue system
- Pub/sub for job updates
- FIFO with priority support
- Multi-key queue management

**Limitation**: Generic job queuing, not rollout orchestration

## Critical Gaps for GRPO

| Component | Status | Impact |
|-----------|--------|--------|
| **Reward Functions** | ❌ Not implemented | Cannot measure trajectory quality |
| **Policy Representation** | ❌ Not implemented | Cannot store policy states |
| **Experience Buffers** | ⚠️ Partial | Job history only, not RL format |
| **Group Rollouts** | ❌ Not implemented | Cannot run parallel experiments |
| **Relative Ranking** | ❌ Not implemented | Cannot compute relative scores |
| **Policy Loss Functions** | ❌ Not implemented | Cannot optimize |
| **Distributed Rollouts** | ❌ Not implemented | Single-node only |

### Missing Data Structures

Not found in codebase:
```rust
struct Experience {
    state: State,
    action: Action,
    reward: f64,
    next_state: State,
    trajectory_id: String,
    group_id: String,
}

struct PolicyVersion {
    version_id: String,
    parameters: Vec<f32>,
    timestamp: DateTime,
}

struct RolloutGroup {
    trajectories: Vec<Trajectory>,
    relative_rewards: Vec<f64>,
}
```

### Missing API Endpoints

Current endpoints (`api_v2_handlers_jobs.rs`):
- POST /create_job, /job_message, /last_messages, /update_job_config

**Missing for GRPO**:
- POST /v1/rollouts/create - Initialize rollout group
- POST /v1/rollouts/submit - Submit trajectory batch
- POST /v1/rollouts/get_results - Get ranked results
- POST /v1/policy/update - Apply gradient updates
- POST /v1/policy/checkpoint - Save policy version
- POST /v1/experiences/query - Retrieve experience buffer

### Missing Database Tables

```sql
CREATE TABLE rollouts (
    rollout_id TEXT PRIMARY KEY,
    group_count INT,
    trajectory_count INT,
    created_at INTEGER,
    metadata TEXT
);

CREATE TABLE rollout_trajectories (
    trajectory_id TEXT PRIMARY KEY,
    rollout_id TEXT,
    job_id TEXT,
    experiences_blob BLOB,
    total_reward REAL,
    relative_rank REAL,
    group_position INT,
    created_at INTEGER
);

CREATE TABLE policy_checkpoints (
    checkpoint_id TEXT PRIMARY KEY,
    version INT,
    parameters_blob BLOB,
    training_loss REAL,
    training_metadata TEXT,
    created_at INTEGER
);
```

## Recommended Integration Approach

### Phase 1: Foundation (Weeks 1-2)

1. **Create hanzo-grpo-core crate**
   - Experience/Trajectory data structures
   - Reward function trait system
   - State/Action abstractions

2. **Extend SQLite schema** in hanzo-sqlite
   - Add rollout tables
   - Add policy checkpoint tables
   - Add trajectory storage

3. **Basic reward framework**
   - Task success rewards
   - Execution time rewards
   - Composite reward functions

### Phase 2: Execution Layer (Weeks 3-4)

1. **Create RolloutOrchestrator**
   - Local rollout execution
   - Parallel trajectory generation
   - Relative reward computation

2. **Integrate with Job system**
   - Use job_id as trajectory_id
   - Add reward tracking to execution flow
   - New GRPO REST endpoints

3. **Trajectory collection**
   - Capture states from message context
   - Extract actions from tool calls
   - Compute rewards from execution results

### Phase 3: Policy Learning (Weeks 5-6)

1. **Policy gradient computation**
   - GRPO loss calculation
   - Relative ranking integration
   - Entropy regularization

2. **Policy checkpointing**
   - Save/load parameter versions
   - Training metadata tracking
   - Version comparison

### Phase 4: Integration & Testing (Week 7)

1. **End-to-end testing**
2. **Performance optimization**
3. **Production documentation**

## File Locations Reference

### Current Infrastructure
- Job Management: `hanzo-libs/hanzo-sqlite/src/job_manager.rs`
- Message Storage: `hanzo-libs/hanzo-sqlite/src/inbox_manager.rs`
- Agent System: `hanzo-libs/hanzo-sqlite/src/agent_manager.rs`
- Tool Runner: `hanzo-libs/hanzo-tools-runner/src/`
- HTTP API: `hanzo-libs/hanzo-http-api/src/api_v2/`
- Routing/Learning: `hanzo-libs/hanzo-llm/src/`

### Recommended New Locations
- GRPO Core: `hanzo-libs/hanzo-grpo-core/src/`
- GRPO API: `hanzo-libs/hanzo-http-api/src/api_v2/grpo_handlers.rs`
- Config: Extend `hanzo.toml` with `[grpo]` section

## Key Integration Points

**Leverage Existing Components**:
1. Job System → Use job_id as trajectory_id (add reward tracking)
2. Message History → Store trajectory states in inbox (add serialization)
3. Agent System → Agent becomes policy executor (add policy storage)
4. Tool Execution → Reward computation from tool outputs (new reward service)
5. DB Layer → Persistence for trajectories (new tables + migrations)
6. HTTP API → REST endpoints for GRPO (new handlers)

## Conclusion

**Hanzo Node** has solid foundational infrastructure (persistent storage, agent execution, tool orchestration) but needs explicit GRPO components (experience representation, policy storage, reward functions, gradient computation).

**Effort Estimate**:
- MVP (4 weeks): Basic local rollouts + policy storage
- Production (8 weeks): Distributed rollouts + full optimization loop

**Quick Win**: Start with experience buffer storage and simple reward functions while keeping job IDs as trajectory IDs. Reuse existing message history infrastructure for trajectory storage.

