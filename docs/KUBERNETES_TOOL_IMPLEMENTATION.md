# Kubernetes Runtime for Distributed Tool Execution

## Overview
Implemented Kubernetes runtime support for distributed tool execution in the Hanzo node, enabling tools to run as Kubernetes Jobs across clusters for better scalability and resource management.

## Implementation Summary

### 1. Dependencies Added
- `kube = "0.95"` - Kubernetes client library
- `k8s-openapi = "0.23"` - Kubernetes API types

### 2. Core Components Created

#### A. KubernetesTool Type (`hanzo-libs/hanzo-tools-primitives/src/tools/kubernetes_tools.rs`)
- New tool type for Kubernetes-based execution
- Resource requirements specification (CPU, memory, GPU)
- Node selector and tolerations for scheduling
- ConfigMap and Secret support for configuration
- Parallel execution support with completions/parallelism

Key Features:
- GPU support with configurable GPU types
- Resource limits and requests
- Node affinity and anti-affinity rules
- Security context configuration
- Persistent volume support
- Environment variable injection

#### B. Kubernetes Executor (`hanzo-bin/hanzo-node/src/tools/tool_execution/execution_kubernetes.rs`)
- `K8sToolExecutor` struct for managing Kubernetes operations
- Job creation with proper pod specifications
- ConfigMap creation for tool code and parameters
- Secret management for sensitive data
- Log streaming from running pods
- Job status monitoring and cleanup
- Timeout enforcement

Key Methods:
- `execute_tool()` - Main execution entry point
- `create_config_map()` - Store tool code and parameters
- `create_secret()` - Manage sensitive configuration
- `create_job()` - Create Kubernetes Job with proper specs
- `wait_and_stream_logs()` - Monitor execution and capture output
- `cleanup_resources()` - Clean up Jobs, ConfigMaps, and Secrets

#### C. Integration with HanzoTool Enum
- Added `Kubernetes(KubernetesTool, IsEnabled)` variant to `HanzoTool` enum
- Implemented all required trait methods for the new variant
- Added routing in `execute_tool_cmd()` for Kubernetes tools

### 3. Configuration Model

```rust
pub struct K8sExecutionConfig {
    pub namespace: String,
    pub service_account: Option<String>,
    pub image_pull_secret: Option<String>,
    pub node_selector: Option<BTreeMap<String, String>>,
    pub tolerations: Option<Vec<Value>>,
    pub resources: K8sResourceRequirements,
    pub timeout_seconds: u64,
    pub cleanup_on_completion: bool,
    pub enable_istio_sidecar: bool,
}
```

### 4. Resource Management

```rust
pub struct K8sResourceRequirements {
    pub cpu_request: Option<String>,
    pub cpu_limit: Option<String>,
    pub memory_request: Option<String>,
    pub memory_limit: Option<String>,
    pub gpu_count: Option<u32>,
    pub gpu_type: Option<String>, // e.g., "nvidia.com/gpu"
}
```

## Usage Examples

### Creating a Kubernetes Tool

```rust
let k8s_tool = KubernetesTool::new(
    "data-processor".to_string(),
    "Process large datasets".to_string(),
    "python code here".to_string(),
    "python".to_string(),
)
.with_gpu(2, Some("nvidia.com/gpu".to_string()))
.with_resources(K8sResourceRequirements {
    cpu_request: Some("4000m".to_string()),
    cpu_limit: Some("8000m".to_string()),
    memory_request: Some("8Gi".to_string()),
    memory_limit: Some("16Gi".to_string()),
    gpu_count: Some(2),
    gpu_type: Some("nvidia.com/gpu".to_string()),
})
.with_node_selector(HashMap::from([
    ("node.kubernetes.io/gpu".to_string(), "true".to_string()),
]));
```

### Executing a Tool on Kubernetes

```rust
let result = execute_kubernetes_tool(
    k8s_tool,
    parameters,
    extra_config,
    node_name,
).await?;
```

## Benefits

1. **Scalability**: Tools can run across multiple nodes in a cluster
2. **Resource Management**: Fine-grained control over CPU, memory, and GPU allocation
3. **Isolation**: Each tool runs in its own pod with security boundaries
4. **Flexibility**: Support for different container images and runtimes
5. **Fault Tolerance**: Kubernetes handles pod failures and restarts
6. **Observability**: Integration with Kubernetes logging and monitoring

## Security Considerations

1. **Secret Management**: Sensitive data stored in Kubernetes Secrets
2. **Network Policies**: Tools can run with restricted network access
3. **Security Context**: Non-root execution, read-only filesystems
4. **RBAC**: Service accounts with minimal required permissions
5. **Image Security**: Support for private registries with pull secrets

## Future Enhancements

1. **Horizontal Pod Autoscaling**: Scale tool instances based on load
2. **Job Queuing**: Integration with Kubernetes job queue systems
3. **Distributed Training**: Support for multi-pod ML training jobs
4. **Spot Instance Support**: Cost optimization with spot/preemptible nodes
5. **Custom Resource Definitions**: CRDs for complex tool workflows
6. **Operator Pattern**: Kubernetes operator for managing tool lifecycle

## Testing

Integration tests have been created in:
- `hanzo-bin/hanzo-node/tests/kubernetes_integration_test.rs`
- `hanzo-bin/hanzo-node/src/tools/tool_execution/execution_kubernetes.rs` (unit tests)

Tests cover:
- Tool creation and configuration
- GPU resource allocation
- HanzoTool enum integration
- Configuration extraction
- Resource requirement building

## Files Modified

1. `/Users/z/work/hanzo/node/hanzo-bin/hanzo-node/Cargo.toml` - Added dependencies
2. `/Users/z/work/hanzo/node/hanzo-bin/hanzo-node/src/tools/tool_execution/mod.rs` - Added module
3. `/Users/z/work/hanzo/node/hanzo-bin/hanzo-node/src/tools/tool_execution/execution_kubernetes.rs` - Main implementation
4. `/Users/z/work/hanzo/node/hanzo-bin/hanzo-node/src/tools/tool_execution/execution_coordinator.rs` - Integration
5. `/Users/z/work/hanzo/node/hanzo-libs/hanzo-tools-primitives/src/tools/mod.rs` - Module registration
6. `/Users/z/work/hanzo/node/hanzo-libs/hanzo-tools-primitives/src/tools/kubernetes_tools.rs` - Tool type
7. `/Users/z/work/hanzo/node/hanzo-libs/hanzo-tools-primitives/src/tools/hanzo_tool.rs` - Enum updates
8. `/Users/z/work/hanzo/node/hanzo-libs/hanzo-tools-primitives/src/tools/tool_types.rs` - RunnerType addition

## Compilation Status

The Kubernetes implementation compiles with only unused import warnings. Other compilation errors in the codebase are unrelated to this implementation (mainly in the hanzo-kbs security module).

## Next Steps

1. **Production Testing**: Deploy to a Kubernetes cluster and test with real workloads
2. **Monitoring Integration**: Add Prometheus metrics for job execution
3. **Performance Optimization**: Implement job caching and resource pooling
4. **Documentation**: Add user-facing documentation and examples
5. **CI/CD Integration**: Add Kubernetes tests to CI pipeline