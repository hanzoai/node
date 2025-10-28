# Hanzo Node Runtime Extensions Design

## Overview
Extend hanzo_node to support multiple runtime environments, enabling it to run any computation workload as part of a decentralized cloud platform.

## Current State
The hanzo_node currently supports:
- **Native Rust tools** - Compiled directly into the binary
- **Deno runtime** - JavaScript/TypeScript execution
- **Python runtime** - Via `uv` package manager
- **MCP servers** - External Model Context Protocol tools
- **Agent tools** - AI agent orchestration

## Proposed Runtime Extensions

### 1. WASM Runtime Engine
**Technology**: `wasmtime` or `wasmer` for Rust integration

```rust
// hanzo-libs/hanzo-wasm-runtime/src/lib.rs
pub struct WasmRuntime {
    engine: wasmtime::Engine,
    store: wasmtime::Store<()>,
    modules: HashMap<String, Module>,
}

impl WasmRuntime {
    pub async fn execute_wasm_tool(
        &mut self,
        wasm_bytes: Vec<u8>,
        function_name: &str,
        params: Value,
    ) -> Result<Value, ToolError> {
        // Compile and instantiate WASM module
        // Execute function with sandboxed permissions
        // Return results as JSON
    }
}
```

**Benefits**:
- Language agnostic - compile from C/C++, Rust, Go, AssemblyScript, etc.
- Sandboxed execution with fine-grained permissions
- Near-native performance
- Deterministic execution for consensus
- Small binary size for distribution

### 2. Native Go SDK
**Technology**: Use Go's plugin system or compile to WASM

```go
// hanzo-go-sdk/tool.go
package hanzo

type Tool interface {
    Run(params map[string]interface{}) (map[string]interface{}, error)
    GetSchema() ToolSchema
}

// Compile with: go build -buildmode=plugin -o tool.so tool.go
// Or compile to WASM: GOOS=wasip1 GOARCH=wasm go build -o tool.wasm
```

**Integration**:
```rust
// hanzo-bin/hanzo-node/src/tools/tool_execution/execution_go_native.rs
pub async fn execute_go_tool(
    plugin_path: PathBuf,
    params: Map<String, Value>,
) -> Result<Value, ToolError> {
    // Option 1: Load Go plugin via FFI
    // Option 2: Execute compiled WASM
}
```

### 3. Native Rust SDK
**Technology**: Dynamic library loading or WASM compilation

```rust
// hanzo-rust-sdk/src/lib.rs
#[no_mangle]
pub extern "C" fn execute_tool(
    params_ptr: *const u8,
    params_len: usize,
) -> *mut u8 {
    // Deserialize params
    // Execute tool logic
    // Return serialized result
}

// Compile with: cargo build --target wasm32-wasi
```

### 4. Docker Engine Integration
**Technology**: Docker API via `bollard` crate

```rust
// hanzo-libs/hanzo-docker-runtime/src/lib.rs
use bollard::Docker;
use bollard::container::{Config, CreateContainerOptions};

pub struct DockerRuntime {
    docker: Docker,
    resource_limits: ResourceLimits,
}

impl DockerRuntime {
    pub async fn execute_container_tool(
        &self,
        image: String,
        params: Value,
        mounts: Vec<Mount>,
    ) -> Result<Value, ToolError> {
        // Pull image if needed
        // Create container with resource limits
        // Execute with timeout
        // Stream logs and results
        // Clean up container
    }
}
```

**Container Spec**:
```yaml
# Tool container requirements
FROM alpine:latest
WORKDIR /app
COPY tool /app/
ENTRYPOINT ["/app/tool"]
# Must accept JSON on stdin, return JSON on stdout
```

### 5. Kubernetes Orchestration
**Technology**: `kube-rs` for Kubernetes API

```rust
// hanzo-libs/hanzo-k8s-runtime/src/lib.rs
use kube::{Client, Api};
use k8s_openapi::api::core::v1::{Pod, Service};
use k8s_openapi::api::batch::v1::Job;

pub struct K8sRuntime {
    client: Client,
    namespace: String,
}

impl K8sRuntime {
    pub async fn execute_k8s_job(
        &self,
        job_spec: JobSpec,
        params: Value,
    ) -> Result<Value, ToolError> {
        // Create Job/Pod
        // Mount ConfigMaps/Secrets
        // Execute with resource limits
        // Watch for completion
        // Retrieve logs/results
        // Cleanup resources
    }

    pub async fn deploy_service(
        &self,
        service_spec: ServiceSpec,
    ) -> Result<String, ToolError> {
        // Deploy long-running services
        // Return service endpoint
    }
}
```

## Universal Compute Abstraction

```rust
// hanzo-libs/hanzo-compute/src/lib.rs
pub enum ComputeRuntime {
    Native,           // Compiled Rust
    Wasm(WasmConfig),
    Docker(DockerConfig),
    Kubernetes(K8sConfig),
    Deno,
    Python,
    Go,
}

pub trait ComputeExecutor {
    async fn execute(
        &self,
        code_or_image: CodeSource,
        params: Value,
        resources: ResourceLimits,
    ) -> Result<Value, ComputeError>;

    async fn health_check(&self) -> Result<HealthStatus, ComputeError>;
}

pub struct UniversalCompute {
    runtimes: HashMap<ComputeRuntime, Box<dyn ComputeExecutor>>,
    scheduler: TaskScheduler,
    resource_manager: ResourceManager,
}
```

## Tool Definition Extension

```rust
// Extend existing tool types
pub enum ToolRuntime {
    // Existing
    Native,
    Deno,
    Python,
    MCP,
    Agent,

    // New
    Wasm {
        source: WasmSource,
        memory_limit: u32,
    },
    Docker {
        image: String,
        tag: String,
        registry: Option<String>,
    },
    Kubernetes {
        manifest: String,
        namespace: String,
    },
    Go {
        plugin_path: Option<PathBuf>,
        wasm_path: Option<PathBuf>,
    },
}
```

## Security Considerations

### Resource Limits
```rust
pub struct ResourceLimits {
    cpu_shares: u32,        // CPU shares (relative weight)
    memory_mb: u32,         // Memory limit in MB
    disk_mb: u32,           // Disk space limit
    network_bandwidth: u32,  // Network bandwidth in Mbps
    execution_timeout: Duration,
}
```

### Sandboxing
- WASM: Built-in sandboxing with capability-based security
- Docker: Container isolation with seccomp/AppArmor
- Kubernetes: Pod Security Policies, Network Policies
- Native plugins: Process isolation with systemd-nspawn or firecracker

### Attestation
```rust
pub struct RuntimeAttestation {
    runtime_type: ComputeRuntime,
    code_hash: [u8; 32],        // SHA256 of code/image
    attestation_proof: Vec<u8>,  // TEE attestation if available
    timestamp: i64,
}
```

## Network Integration

### P2P Job Distribution
```rust
pub struct DistributedScheduler {
    local_resources: ResourceInventory,
    peer_resources: HashMap<PeerId, ResourceInventory>,
    job_queue: PriorityQueue<ComputeJob>,
}

impl DistributedScheduler {
    pub async fn schedule_job(&self, job: ComputeJob) -> Result<PeerId, ScheduleError> {
        // Find optimal peer based on resources
        // Consider network latency
        // Handle failover/redundancy
    }
}
```

### Result Verification
```rust
pub enum VerificationStrategy {
    None,                    // Trust the executor
    Redundant { min_peers: u32 }, // Multiple execution
    ZKProof,                 // Zero-knowledge proof
    TEE,                     // Trusted execution environment
}
```

## Implementation Phases

### Phase 1: WASM Runtime (2 weeks)
- Integrate wasmtime
- Create WASM tool type
- Add compilation toolchain
- Test with simple tools

### Phase 2: Docker Integration (2 weeks)
- Add bollard dependency
- Implement Docker runtime
- Create container management
- Add registry support

### Phase 3: Native SDKs (3 weeks)
- Go plugin system
- Rust dynamic libraries
- FFI interfaces
- SDK documentation

### Phase 4: Kubernetes (3 weeks)
- kube-rs integration
- Job/Pod management
- Service deployment
- Ingress configuration

### Phase 5: Universal Compute (2 weeks)
- Abstract runtime interface
- Resource management
- Scheduling system
- Monitoring/metrics

## Configuration

```toml
# hanzo-node.toml
[compute]
enabled_runtimes = ["native", "wasm", "docker", "k8s", "deno", "python"]

[compute.wasm]
engine = "wasmtime"
max_memory_mb = 256
max_execution_time_ms = 30000

[compute.docker]
socket = "/var/run/docker.sock"
registry = "registry.hanzo.network"
pull_policy = "if-not-present"

[compute.kubernetes]
config_path = "~/.kube/config"
namespace = "hanzo-compute"
service_account = "hanzo-node"

[compute.resources]
max_concurrent_jobs = 10
total_memory_gb = 16
total_cpu_cores = 8
```

## Benefits for Decentralized Cloud

1. **Universal Execution**: Run any code, any language, any framework
2. **Resource Monetization**: Nodes can sell compute resources
3. **Fault Tolerance**: Automatic failover and redundancy
4. **Scalability**: Horizontal scaling across the network
5. **Verifiable Compute**: Cryptographic proofs of execution
6. **Edge Computing**: Run workloads close to data/users
7. **Cost Efficiency**: Utilize idle resources globally

## Example Use Cases

### Web3 dApp Backend
```rust
// Deploy a service on the decentralized cloud
let service = hanzo.deploy_service(ServiceSpec {
    runtime: ComputeRuntime::Kubernetes,
    image: "myapp:latest",
    replicas: 3,
    resources: ResourceLimits::default(),
});
```

### ML Model Inference
```rust
// Run ML model as WASM
let result = hanzo.execute_tool(ToolSpec {
    runtime: ComputeRuntime::Wasm,
    code: model_wasm_bytes,
    function: "predict",
    params: input_data,
});
```

### Batch Processing
```rust
// Process data with Docker container
let output = hanzo.execute_tool(ToolSpec {
    runtime: ComputeRuntime::Docker,
    image: "data-processor:v2",
    params: json!({"input": "s3://bucket/data"}),
    resources: ResourceLimits {
        memory_mb: 4096,
        cpu_shares: 2000,
        ..Default::default()
    },
});
```

## Testing Strategy

1. **Unit Tests**: Test each runtime in isolation
2. **Integration Tests**: Test runtime coordination
3. **Load Tests**: Verify resource limits
4. **Security Tests**: Attempt sandbox escapes
5. **Network Tests**: Test distributed execution

## Future Extensions

- **GPU Support**: CUDA/ROCm container support
- **Confidential Computing**: SGX/SEV integration
- **IPFS Integration**: Distributed code storage
- **Smart Contract Integration**: On-chain job coordination
- **Federated Learning**: Distributed ML training
- **Quantum Computing**: Interface with quantum backends

This architecture would make hanzo_node a complete compute platform capable of running any workload in a decentralized, verifiable manner.