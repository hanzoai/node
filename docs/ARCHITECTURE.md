# Hanzo Node Architecture

## System Design Overview

Hanzo Node implements a modular, event-driven architecture designed for scalability, extensibility, and fault tolerance. The system follows microkernel patterns with pluggable components for LLM providers, tool runtimes, and storage backends.

## Core Design Principles

### 1. Separation of Concerns
- **API Layer**: Handles HTTP/WebSocket communication
- **Manager Layer**: Business logic and orchestration
- **Provider Layer**: External service integrations
- **Storage Layer**: Persistent data management
- **Security Layer**: Authentication, encryption, attestation

### 2. Async-First Architecture
- Built on Tokio runtime for non-blocking I/O
- Actor model for component communication
- Message-passing between subsystems
- Zero-copy where possible

### 3. Plugin Architecture
- Trait-based abstractions for providers
- Dynamic tool loading
- Runtime-agnostic execution
- Hot-swappable components

## Component Architecture

### Node Manager (`managers/node_manager.rs`)

The central orchestrator that coordinates all subsystems:

```rust
pub struct NodeManager {
    identity_manager: Arc<RwLock<IdentityManager>>,
    job_queue_manager: Arc<Mutex<JobQueueManager>>,
    db: Arc<db::HanzoDB>,
    llm_provider_manager: Arc<LLMProviderManager>,
    tool_router: Arc<RwLock<ToolRouter>>,
    model_capabilities: Arc<ModelCapabilitiesManager>,
    agent_manager: Arc<RwLock<AgentManager>>,
}
```

**Responsibilities:**
- Lifecycle management
- Service coordination
- Resource allocation
- Health monitoring

### Job Queue Manager (`managers/job_queue_manager.rs`)

Implements a sophisticated job scheduling system with:

**Features:**
- Priority queuing with heap-based scheduling
- Concurrency control with semaphores
- Tree-based job dependencies
- Retry logic with exponential backoff
- Fork/branch workflow support

**Job State Machine:**
```
   ┌─────────┐
   │ PENDING │
   └────┬────┘
        │
   ┌────▼────┐
   │PROCESSING│
   └────┬────┘
        │
   ┌────┴────┐────┐────┐
   │         │    │    │
┌──▼──┐ ┌───▼──┐ ▼    ▼
│DONE │ │FAILED│ FORK BRANCH
└─────┘ └──────┘
```

### LLM Provider Manager (`llm_provider/`)

Unified interface for 100+ LLM providers:

**Provider Categories:**
1. **Major Providers**: OpenAI, Anthropic, Google, Meta
2. **Open Source**: Llama, Mistral, Qwen, Yi
3. **Specialized**: Code models, vision models, audio models
4. **Local**: Ollama, llama.cpp, GGUF models

**Provider Interface:**
```rust
#[async_trait]
pub trait LLMProvider {
    async fn create_inference(&self, request: InferenceRequest) 
        -> Result<InferenceResponse>;
    async fn stream_inference(&self, request: InferenceRequest) 
        -> Result<BoxStream<'static, Result<StreamChunk>>>;
    fn get_capabilities(&self) -> ModelCapabilities;
}
```

**Smart Routing:**
- Cost-based routing
- Capability matching
- Automatic failover
- Load balancing
- Rate limit management

### Tool Execution Framework (`tools/`)

Multi-runtime tool execution system:

**Execution Coordinator:**
```rust
pub struct ExecutionCoordinator {
    native_runtime: NativeRuntime,
    deno_runtime: Option<DenoRuntime>,
    python_runtime: Option<PythonRuntime>,
    docker_runtime: Option<DockerRuntime>,
    k8s_runtime: Option<KubernetesRuntime>,
    wasm_runtime: Option<WasmRuntime>,
    mcp_client: Option<McpClient>,
}
```

**Runtime Selection Logic:**
1. Check tool definition for preferred runtime
2. Validate runtime availability
3. Prepare execution environment
4. Execute with timeout and resource limits
5. Handle results and cleanup

**Tool Definition Schema:**
```rust
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: JsonSchema,
    pub returns: JsonSchema,
    pub runtime: ToolRuntime,
    pub timeout: Duration,
    pub memory_limit: Option<usize>,
    pub network_access: bool,
}
```

### Storage Architecture

#### SQLite Database (`hanzo-sqlite/`)

**Connection Pooling:**
- R2D2 connection pool
- Configurable pool size (default: 10)
- Automatic connection recycling
- WAL mode for concurrent reads

**Schema Design:**
```sql
-- Core tables
CREATE TABLE users (
    id TEXT PRIMARY KEY,
    identity_public_key TEXT UNIQUE,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE jobs (
    id TEXT PRIMARY KEY,
    user_id TEXT REFERENCES users(id),
    status TEXT CHECK(status IN ('pending','processing','done','failed')),
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE tools (
    id TEXT PRIMARY KEY,
    name TEXT UNIQUE,
    definition JSON,
    runtime TEXT,
    enabled BOOLEAN DEFAULT TRUE
);

CREATE TABLE agents (
    id TEXT PRIMARY KEY,
    name TEXT UNIQUE,
    config JSON,
    tools JSON,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
```

#### LanceDB Vector Store

**Architecture:**
- Column-oriented storage for vectors
- Approximate nearest neighbor (ANN) indexing
- Hybrid search (vector + metadata)
- Automatic compaction

**Integration:**
```rust
pub struct VectorStore {
    db: Arc<lancedb::Database>,
    embedding_model: Arc<EmbeddingModel>,
}

impl VectorStore {
    pub async fn insert(&self, text: &str, metadata: Value) -> Result<String>;
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>>;
    pub async fn hybrid_search(&self, query: &str, filter: Filter) -> Result<Vec<SearchResult>>;
}
```

### Network Architecture

#### HTTP/REST API (`network/http_server.rs`)

**Middleware Stack:**
1. CORS handling
2. Authentication
3. Rate limiting
4. Request logging
5. Error handling
6. Response compression

**Route Organization:**
```
/v2/
├── health/
├── autonomous_node/
├── jobs/
│   ├── {id}/
│   ├── {id}/status/
│   └── {id}/results/
├── tools/
│   ├── list/
│   ├── {name}/
│   └── execute/
├── agents/
│   ├── list/
│   ├── create/
│   └── {id}/
└── embeddings/
    ├── generate/
    └── search/
```

#### WebSocket Server (`network/websocket_server.rs`)

**Message Protocol:**
```rust
pub enum WsMessage {
    // Client -> Server
    JobCreate(JobCreateRequest),
    JobStatus(String),
    ToolExecute(ToolExecuteRequest),
    Subscribe(SubscribeRequest),
    
    // Server -> Client
    JobUpdate(JobUpdate),
    ToolResult(ToolResult),
    StreamChunk(StreamChunk),
    Error(ErrorMessage),
}
```

**Connection Management:**
- Connection pooling
- Heartbeat/keepalive
- Automatic reconnection
- Message buffering

#### P2P Networking (`hanzo-libp2p-relayer/`)

**libp2p Integration:**
- Kademlia DHT for peer discovery
- Gossipsub for message propagation
- Circuit relay for NAT traversal
- Noise protocol for encryption

## Security Model

### Authentication & Authorization

**Identity Management:**
- Ed25519 key pairs for identity
- X25519 for key exchange
- Blake3 for hashing
- Signature-based authentication

**Authorization Levels:**
1. **Public**: No authentication required
2. **User**: Valid signature required
3. **Agent**: Agent-specific permissions
4. **Admin**: Full system access

### Trusted Execution Environment (TEE)

**Hardware Support:**
```rust
pub enum TeeType {
    SevSnp,      // AMD SEV-SNP
    Tdx,         // Intel TDX
    H100Cc,      // NVIDIA H100 Confidential Computing
    BlackwellTeeIo, // NVIDIA Blackwell TEE-I/O
}
```

**Attestation Flow:**
1. Generate attestation report
2. Verify with manufacturer CA
3. Extract measurements
4. Validate against policy
5. Release secrets

### Key Management (KBS/KMS)

**Privacy Tiers:**
- **Tier 0 (Open)**: No protection
- **Tier 1 (Encrypted)**: TLS/encryption
- **Tier 2 (Confidential)**: TEE execution
- **Tier 3 (Verified)**: Attestation required
- **Tier 4 (TEE-I/O)**: Full isolation

**Key Lifecycle:**
```
Generate → Store → Attest → Release → Use → Rotate → Destroy
```

## Data Flow

### Job Execution Flow

```
Client Request
     │
     ▼
API Gateway ──────► Authentication
     │                    │
     ▼                    ▼
Job Queue ◄────────── Authorized
     │
     ▼
Scheduler ──────► Resource Check
     │                    │
     ▼                    ▼
Job Worker ◄────────── Available
     │
     ▼
Tool Router ──────► Runtime Selection
     │                    │
     ▼                    ▼
Tool Execution ◄──── Runtime Ready
     │
     ▼
Result Processing ──► Storage
     │                    │
     ▼                    ▼
Response ◄──────────── Stored
     │
     ▼
Client
```

### LLM Inference Flow

```
User Query
     │
     ▼
Prompt Construction
     │
     ▼
Model Selection ──────► Capability Check
     │                         │
     ▼                         ▼
Provider Router ◄────── Model Available
     │
     ▼
Rate Limiter ──────► Token Check
     │                    │
     ▼                    ▼
API Call ◄──────────── Tokens Available
     │
     ▼
Response Stream ──────► Token Counting
     │                         │
     ▼                         ▼
Result Parser ◄──────── Count Updated
     │
     ▼
Response Cache ──────► Store Result
     │
     ▼
Client Response
```

## Performance Optimizations

### Concurrency Management

**Thread Pool Configuration:**
- Tokio runtime with multi-threaded scheduler
- Separate pools for I/O and CPU-bound tasks
- Work-stealing for load balancing
- Configurable worker threads

### Memory Management

**Strategies:**
- Arena allocators for temporary data
- Object pooling for reusable resources
- Streaming for large data
- Zero-copy where possible

### Caching Strategy

**Multi-Level Cache:**
1. **L1**: In-memory LRU cache
2. **L2**: Redis distributed cache
3. **L3**: SQLite result cache
4. **L4**: CDN for static assets

### Database Optimizations

**SQLite Tuning:**
```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA cache_size = -64000;  -- 64MB
PRAGMA page_size = 4096;
PRAGMA mmap_size = 268435456; -- 256MB
```

**Connection Pool Settings:**
```rust
PoolConfig {
    max_connections: 10,
    min_connections: 2,
    connection_timeout: Duration::from_secs(30),
    idle_timeout: Some(Duration::from_secs(600)),
    max_lifetime: Some(Duration::from_secs(1800)),
}
```

## Scalability Considerations

### Horizontal Scaling

**Stateless Design:**
- Session state in Redis
- Job state in database
- No in-memory dependencies
- Shared-nothing architecture

**Load Balancing:**
- Round-robin for API requests
- Consistent hashing for WebSocket
- Queue-based job distribution
- Provider-specific routing

### Vertical Scaling

**Resource Limits:**
```rust
pub struct ResourceLimits {
    max_memory: usize,        // Default: 8GB
    max_cpu_cores: usize,     // Default: 8
    max_file_handles: usize,  // Default: 65536
    max_connections: usize,   // Default: 10000
}
```

### Federation Support

**Multi-Node Architecture:**
- Peer discovery via libp2p
- Gossip-based state sync
- Distributed job queue
- Cross-node tool sharing

## Monitoring & Observability

### Metrics Collection

**Prometheus Metrics:**
```rust
// Counter metrics
job_total{status="completed|failed"}
tool_executions_total{tool="name",runtime="type"}
llm_requests_total{provider="name",model="name"}

// Histogram metrics
job_duration_seconds{bucket="..."}
tool_execution_duration_seconds{tool="name"}
llm_response_time_seconds{provider="name"}

// Gauge metrics
active_jobs
active_connections
memory_usage_bytes
cpu_usage_percent
```

### Distributed Tracing

**OpenTelemetry Integration:**
- Span creation for requests
- Context propagation
- Baggage for metadata
- Sampling strategies

### Logging Architecture

**Structured Logging:**
```rust
tracing::info!(
    job_id = %job_id,
    tool = %tool_name,
    runtime = %runtime_type,
    duration_ms = %duration.as_millis(),
    "Tool execution completed"
);
```

**Log Levels:**
- TRACE: Detailed execution flow
- DEBUG: Development information
- INFO: Normal operations
- WARN: Potential issues
- ERROR: Errors requiring attention

## Error Handling

### Error Categories

1. **Recoverable Errors**: Retry with backoff
2. **Client Errors**: Return 4xx status
3. **Server Errors**: Log and return 5xx
4. **Fatal Errors**: Graceful shutdown

### Retry Strategy

```rust
pub struct RetryConfig {
    max_attempts: u32,        // Default: 3
    initial_delay: Duration,   // Default: 1s
    max_delay: Duration,       // Default: 60s
    multiplier: f64,          // Default: 2.0
    randomization_factor: f64, // Default: 0.1
}
```

### Circuit Breaker

**States:**
- **Closed**: Normal operation
- **Open**: Failing, reject requests
- **Half-Open**: Testing recovery

## Future Architecture Considerations

### Planned Enhancements

1. **GraphQL API**: Alternative to REST
2. **gRPC Support**: Binary protocol option
3. **Event Sourcing**: Complete audit trail
4. **CQRS Pattern**: Read/write separation
5. **Kubernetes Operator**: Native K8s integration

### Extensibility Points

- Custom LLM providers via trait implementation
- Plugin system for tools
- Middleware hooks for request processing
- Custom storage backends
- Alternative transport protocols

---

*This architecture is designed to evolve with the growing demands of AI infrastructure while maintaining simplicity, performance, and reliability.*