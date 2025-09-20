# Hanzo Node

## Overview

Hanzo Node is a high-performance AI infrastructure platform that enables creation and orchestration of AI agents without coding. It provides a unified interface to 100+ LLM providers, distributed job orchestration, and a comprehensive tool execution framework with support for multiple runtimes.

## Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                         Hanzo Node Architecture                      │
├─────────────────────────────────────────────────────────────────────┤
│                                                                       │
│  ┌─────────────────────────────────────────────────────────────┐    │
│  │                        API Layer                             │    │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐   │    │
│  │  │ REST API │  │WebSocket │  │   SSE    │  │ Swagger  │   │    │
│  │  │  (V2)    │  │   API    │  │ Streams  │  │    UI    │   │    │
│  │  └──────────┘  └──────────┘  └──────────┘  └──────────┘   │    │
│  └─────────────────────────────────────────────────────────────┘    │
│                                                                       │
│  ┌─────────────────────────────────────────────────────────────┐    │
│  │                     Core Managers                            │    │
│  │  ┌────────────┐  ┌────────────┐  ┌────────────┐            │    │
│  │  │   Node     │  │  Job Queue │  │   Agent    │            │    │
│  │  │  Manager   │  │  Manager   │  │  Manager   │            │    │
│  │  └────────────┘  └────────────┘  └────────────┘            │    │
│  │  ┌────────────┐  ┌────────────┐  ┌────────────┐            │    │
│  │  │  Identity  │  │   Tool     │  │   Model    │            │    │
│  │  │  Manager   │  │   Router   │  │Capabilities│            │    │
│  │  └────────────┘  └────────────┘  └────────────┘            │    │
│  └─────────────────────────────────────────────────────────────┘    │
│                                                                       │
│  ┌─────────────────────────────────────────────────────────────┐    │
│  │                    LLM Provider Layer                        │    │
│  │  ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐    │    │
│  │  │OpenAI│ │Claude│ │Gemini│ │Llama │ │Mistral│ │100+  │    │    │
│  │  └──────┘ └──────┘ └──────┘ └──────┘ └──────┘ └──────┘    │    │
│  └─────────────────────────────────────────────────────────────┘    │
│                                                                       │
│  ┌─────────────────────────────────────────────────────────────┐    │
│  │                  Tool Execution Runtimes                     │    │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐   │    │
│  │  │  Native  │  │   Deno   │  │  Python  │  │   MCP    │   │    │
│  │  │  (Rust)  │  │ (JS/TS)  │  │   (uv)   │  │  Server  │   │    │
│  │  └──────────┘  └──────────┘  └──────────┘  └──────────┘   │    │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐   │    │
│  │  │  Docker  │  │Kubernetes│  │   WASM   │  │  Custom  │   │    │
│  │  └──────────┘  └──────────┘  └──────────┘  └──────────┘   │    │
│  └─────────────────────────────────────────────────────────────┘    │
│                                                                       │
│  ┌─────────────────────────────────────────────────────────────┐    │
│  │                    Storage & Security                        │    │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐   │    │
│  │  │  SQLite  │  │ LanceDB  │  │   KBS    │  │   TEE    │   │    │
│  │  │   Pool   │  │  Vector  │  │Attestation│ │ Support  │   │    │
│  │  └──────────┘  └──────────┘  └──────────┘  └──────────┘   │    │
│  └─────────────────────────────────────────────────────────────┘    │
│                                                                       │
│  ┌─────────────────────────────────────────────────────────────┐    │
│  │                     Network Layer                            │    │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────┐                  │    │
│  │  │  libp2p  │  │   HTTP   │  │WebSocket │                  │    │
│  │  │  Relay   │  │  Server  │  │  Server  │                  │    │
│  │  └──────────┘  └──────────┘  └──────────┘                  │    │
│  └─────────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────────┘
```

## Key Features

### 🤖 Multi-LLM Provider Support
- **100+ Providers**: OpenAI, Anthropic, Google, Meta, Mistral, and more
- **Unified Interface**: Single API for all providers
- **Automatic Failover**: Seamless provider switching on failures
- **Cost Optimization**: Smart routing based on cost and performance

### 🛠️ Tool Execution Framework
- **Multi-Runtime Support**: Native Rust, Deno (JS/TS), Python (uv), MCP servers
- **Container Orchestration**: Docker and Kubernetes integration
- **WebAssembly Runtime**: Edge deployment capabilities
- **260+ Built-in Tools**: File operations, web scraping, data processing

### 🔄 Job Orchestration
- **Distributed Processing**: Concurrent job execution with configurable limits
- **Tree-based Workflows**: Complex branching and forking logic
- **Retry Mechanisms**: Automatic retry with exponential backoff
- **Cron Jobs**: Scheduled task execution

### 🔐 Security & Privacy
- **TEE Support**: Hardware attestation for SEV-SNP, TDX, H100 CC
- **Post-Quantum Crypto**: Kyber, Dilithium, Falcon, SPHINCS+
- **Key Management**: KBS/KMS with privacy tiers 0-4
- **Ed25519 Identity**: Cryptographic identity management

### 📊 Vector Database
- **LanceDB Integration**: High-performance vector storage
- **Semantic Search**: RAG and similarity search
- **Embedding Generation**: Native Qwen3 models via Ollama
- **Automatic Indexing**: Real-time vector updates

### 🌐 Networking
- **libp2p Integration**: P2P networking capabilities
- **WebSocket Support**: Real-time bidirectional communication
- **SSE Streaming**: Server-sent events for LLM responses
- **REST API**: Comprehensive V2 API with OpenAPI spec

## Quick Start

### Prerequisites
- Rust 1.75+ (for building from source)
- Docker (optional, for containerized deployment)
- Ollama (optional, for local embeddings)

### Installation

#### From Source
```bash
# Clone the repository
git clone https://github.com/hanzoai/hanzo-node
cd hanzo-node

# Build the project
cargo build --release --bin hanzod

# Run with default settings
./scripts/run_node_localhost.sh
```

#### Using Docker
```bash
# Pull the image
docker pull hanzoai/hanzo-node:latest

# Run the container
docker run -p 3690:3690 -p 3691:3691 -p 3692:3692 hanzoai/hanzo-node
```

### Basic Usage

#### 1. Start the Node
```bash
# Run with all default settings
sh scripts/run_node_localhost.sh

# Or run full stack with agent provider
sh scripts/run_all_localhost.sh
```

#### 2. Access the Swagger UI
Open your browser and navigate to:
```
http://localhost:3690/v2/swagger-ui/
```

#### 3. Create Your First Job
```bash
curl -X POST http://localhost:3690/v2/autonomous_node \
  -H "Content-Type: application/json" \
  -d '{
    "objective": "Write a hello world program in Python",
    "tool_names": ["generate_python_code"],
    "identity": "user123"
  }'
```

## Configuration

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `NODE_IP` | Node IP address | `0.0.0.0` |
| `NODE_PORT` | P2P port | `3691` |
| `NODE_API_IP` | API server IP | `0.0.0.0` |
| `NODE_API_PORT` | API server port | `3690` |
| `NODE_WS_PORT` | WebSocket port | `3692` |
| `RUST_LOG` | Log level | `debug,error,info` |
| `EMBEDDINGS_SERVER_URL` | Ollama URL | `http://localhost:11434` |
| `USE_NATIVE_EMBEDDINGS` | Use Qwen3 models | `true` |
| `DATABASE_URL` | SQLite database path | `./storage/db.sqlite` |
| `LANCEDB_PATH` | LanceDB storage path | `./storage/lancedb` |

### LLM Provider Configuration

Add your API keys to enable specific providers:

```bash
# OpenAI
export OPENAI_API_KEY="sk-..."

# Anthropic
export ANTHROPIC_API_KEY="sk-ant-..."

# Google
export GOOGLE_API_KEY="..."

# Together AI
export TOGETHER_API_KEY="..."

# And many more...
```

## API Documentation

### REST API Endpoints

The node exposes a comprehensive REST API at `http://localhost:3690/v2/`:

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/v2/health` | GET | Health check |
| `/v2/autonomous_node` | POST | Create autonomous job |
| `/v2/job/{id}` | GET | Get job status |
| `/v2/tools` | GET | List available tools |
| `/v2/providers` | GET | List LLM providers |
| `/v2/agents` | GET | List configured agents |
| `/v2/embeddings` | POST | Generate embeddings |
| `/v2/search` | POST | Vector similarity search |

For complete API documentation, visit the Swagger UI at `/v2/swagger-ui/`.

### WebSocket API

Connect to `ws://localhost:3692` for real-time communication:

```javascript
const ws = new WebSocket('ws://localhost:3692');

ws.on('message', (data) => {
  const message = JSON.parse(data);
  console.log('Received:', message);
});

ws.send(JSON.stringify({
  type: 'job_create',
  payload: {
    objective: 'Your task here',
    tool_names: ['tool1', 'tool2']
  }
}));
```

## Development

### Building from Source

```bash
# Standard build
cargo build --bin hanzod

# Release build with optimizations
cargo build --release --bin hanzod

# Build with Swagger UI support
cargo build --features hanzo_node/swagger-ui

# Generate OpenAPI documentation
cargo run --example generate_openapi_docs
```

### Running Tests

```bash
# Run all tests (single-threaded required)
IS_TESTING=1 cargo test -- --test-threads=1

# Run specific test suite
IS_TESTING=1 cargo test job_manager -- --nocapture

# Run integration tests
IS_TESTING=1 cargo test --test '*' -- --test-threads=1

# Run benchmarks
cargo bench
```

### Project Structure

```
hanzo-node/
├── hanzo-bin/
│   └── hanzo-node/       # Main binary
│       ├── src/
│       │   ├── main.rs               # Entry point
│       │   ├── llm_provider/         # LLM integrations
│       │   ├── managers/             # Core managers
│       │   ├── network/              # Networking
│       │   ├── tools/                # Tool framework
│       │   ├── wallet/               # Crypto wallets
│       │   └── security/             # TEE/attestation
│       └── tests/                    # Integration tests
├── hanzo-libs/           # Shared libraries
│   ├── hanzo-message-primitives/    # Message schemas
│   ├── hanzo-sqlite/                # Database layer
│   ├── hanzo-embedding/             # Vector embeddings
│   ├── hanzo-kbs/                   # Key management
│   ├── hanzo-pqc/                   # Post-quantum crypto
│   ├── hanzo-mcp/                   # MCP integration
│   └── hanzo-tools-primitives/      # Tool definitions
└── scripts/              # Deployment scripts
```

## Advanced Features

### Tool Development

Create custom tools in multiple languages:

#### Rust Native Tool
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MyTool;

impl HanzoTool for MyTool {
    async fn execute(&self, input: Value) -> Result<Value> {
        // Your implementation
        Ok(json!({"result": "success"}))
    }
}
```

#### JavaScript/TypeScript Tool
```typescript
export async function myTool(input: any): Promise<any> {
    // Your implementation
    return { result: "success" };
}
```

#### Python Tool
```python
def my_tool(input: dict) -> dict:
    # Your implementation
    return {"result": "success"}
```

### Agent Configuration

Define custom agents with specific capabilities:

```json
{
  "name": "DataAnalyst",
  "model": "claude-3-opus",
  "tools": ["read_file", "python_execute", "create_chart"],
  "system_prompt": "You are a data analysis expert...",
  "max_iterations": 10
}
```

### Vector Search Integration

```rust
// Generate embeddings
let embeddings = node.generate_embeddings("Your text here").await?;

// Store in LanceDB
node.store_vector("doc_id", embeddings, metadata).await?;

// Similarity search
let results = node.vector_search("query text", top_k: 10).await?;
```

## Monitoring & Observability

### Metrics
- Prometheus metrics exposed at `/metrics`
- Job execution statistics
- LLM provider usage tracking
- Tool execution performance

### Logging
- Structured logging with `tracing`
- Log levels: TRACE, DEBUG, INFO, WARN, ERROR
- File and console output options

### Health Checks
- `/v2/health` - Basic health status
- `/v2/health/detailed` - Component health
- `/v2/metrics` - Prometheus metrics

## Security Considerations

### Authentication
- Ed25519 signature-based authentication
- API key management for LLM providers
- JWT support for session management

### Encryption
- TLS support for all network communication
- End-to-end encryption for sensitive data
- Hardware-backed key storage in TEEs

### Privacy
- Local embedding generation option
- Data residency controls
- GDPR-compliant data handling

## Troubleshooting

### Common Issues

**Port Already in Use**
```bash
# Find and kill process
lsof -i :3690
kill -9 <PID>
```

**Database Lock Error**
```bash
# Remove lock file
rm ./storage/db.sqlite-shm
rm ./storage/db.sqlite-wal
```

**LLM Provider Errors**
- Verify API keys are set correctly
- Check rate limits and quotas
- Enable debug logging: `RUST_LOG=trace`

### Debug Mode
```bash
# Enable verbose logging
export RUST_LOG=trace
export LOG_ALL=1

# Run with debugging
cargo run --bin hanzod
```

## Performance Tuning

### Database Optimization
- SQLite connection pooling (default: 10 connections)
- WAL mode enabled for concurrent reads
- Automatic VACUUM scheduling

### Concurrency Settings
```bash
# Job processing
export MAX_CONCURRENT_JOBS=10
export JOB_QUEUE_SIZE=100

# Tool execution
export TOOL_TIMEOUT_SECONDS=300
export MAX_TOOL_RETRIES=3
```

### Memory Management
- Streaming for large responses
- Chunked file processing
- Automatic garbage collection

## Contributing

We welcome contributions! Please see our [Contributing Guide](CONTRIBUTING.md) for details.

### Development Setup
1. Fork the repository
2. Create a feature branch
3. Write tests for new functionality
4. Ensure all tests pass
5. Submit a pull request

### Code Style
- Follow Rust conventions
- Use `cargo fmt` for formatting
- Run `cargo clippy` for linting
- Add documentation for public APIs

## License

This project is licensed under the Apache License 2.0 - see the [LICENSE](LICENSE) file for details.

## Support

- **Documentation**: [https://docs.hanzo.ai](https://docs.hanzo.ai)
- **Discord**: [Join our community](https://discord.gg/hanzoai)
- **GitHub Issues**: [Report bugs](https://github.com/hanzoai/hanzo-node/issues)
- **Email**: support@hanzo.ai

## Acknowledgments

Built with:
- Rust and Tokio for high-performance async
- LanceDB for vector storage
- OpenAPI/Swagger for API documentation
- libp2p for peer-to-peer networking

---

*Hanzo Node - Building the future of AI infrastructure*