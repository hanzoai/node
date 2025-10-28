# Model Pull Capability Design for Hanzo Node

## Overview
Add native model pulling capabilities to `hanzod` and integrate with `@hanzo/cli` for seamless model management.

## Architecture

### 1. Model Pull in hanzod

#### Command Line Interface
```bash
# Pull models directly with hanzod
hanzod pull qwen3-embedding-8b
hanzod pull dengcao/Qwen3-Embedding-8B:Q5_K_M
hanzod pull --provider ollama qwen3-embedding-8b
hanzod pull --provider huggingface Qwen/Qwen3-8B
```

#### Implementation Components

##### A. Model Provider Abstraction
```rust
// hanzo-bin/hanzo-node/src/model_pull/mod.rs
pub trait ModelProvider {
    async fn pull(&self, model_id: &str) -> Result<ModelInfo, Error>;
    async fn list(&self) -> Result<Vec<ModelInfo>, Error>;
    async fn delete(&self, model_id: &str) -> Result<(), Error>;
}

pub struct OllamaProvider;
pub struct HuggingFaceProvider;
pub struct LMStudioProvider;
```

##### B. Integration with mistral.rs
```rust
// Use hanzoai/inference (mistral.rs fork) for native loading
use mistralrs_core::{TokenSource, NormalLoaderType};

pub struct NativeModelLoader {
    token_source: TokenSource,
    cache_path: PathBuf,
}
```

### 2. Ollama Integration (Primary)

Since Ollama is our primary embedding provider, integrate directly:

```rust
// hanzo-bin/hanzo-node/src/model_pull/ollama.rs
impl ModelProvider for OllamaProvider {
    async fn pull(&self, model_id: &str) -> Result<ModelInfo, Error> {
        // Call ollama API to pull model
        let cmd = Command::new("ollama")
            .args(&["pull", model_id])
            .output()
            .await?;
        // Parse output and return ModelInfo
    }
}
```

### 3. CLI Integration (@hanzo/cli)

#### @hanzo/dev Commands
```typescript
// packages/cli/src/commands/node.ts
export const nodeCommands = {
  // Start/stop node
  'node:start': startNode,
  'node:stop': stopNode,
  'node:status': nodeStatus,
  
  // Model management
  'model:pull': pullModel,
  'model:list': listModels,
  'model:delete': deleteModel,
  
  // Embedding specific
  'embedding:pull': pullEmbeddingModel,
  'embedding:test': testEmbedding,
}
```

#### Implementation
```typescript
// packages/cli/src/commands/model.ts
import { spawn } from 'child_process';

export async function pullModel(modelId: string, provider = 'ollama') {
  console.log(`Pulling ${modelId} from ${provider}...`);
  
  if (provider === 'ollama') {
    // Use Ollama directly
    await execCommand('ollama', ['pull', modelId]);
  } else if (provider === 'native') {
    // Use hanzod's native pull
    await execCommand('hanzod', ['pull', modelId]);
  }
}

// Recommended Qwen3 models
export const RECOMMENDED_MODELS = {
  embedding: {
    flagship: 'dengcao/Qwen3-Embedding-8B:Q5_K_M',
    balanced: 'dengcao/Qwen3-Embedding-4B:Q5_K_M',
    lightweight: 'dengcao/Qwen3-Embedding-0.6B:Q5_K_M',
  },
  reranker: {
    primary: 'dengcao/Qwen3-Reranker-4B:F16',
    lightweight: 'dengcao/Qwen3-Reranker-0.6B:F16',
  }
};
```

### 4. Node Management in CLI

```typescript
// packages/cli/src/commands/node.ts
export async function startNode(options: NodeOptions) {
  const env = {
    NODE_IP: options.ip || '0.0.0.0',
    NODE_PORT: options.port || '9452',
    EMBEDDING_MODEL_TYPE: options.model || 'qwen3-embedding-8b',
    EMBEDDINGS_SERVER_URL: options.embedUrl || 'http://localhost:11434',
  };
  
  // Start hanzod with configuration
  const node = spawn('hanzod', [], { env });
  
  // Monitor and log
  node.stdout.on('data', (data) => {
    console.log(`[Node]: ${data}`);
  });
}
```

## Usage Examples

### Via @hanzo/cli
```bash
# Install CLI globally
npm install -g @hanzo/cli

# Pull the best embedding model
hanzo model:pull qwen3-embedding-8b

# Start node with Qwen3
hanzo node:start --model qwen3-embedding-8b

# List available models
hanzo model:list

# Test embedding
hanzo embedding:test "Hello world"
```

### Via hanzod directly
```bash
# Pull model
hanzod pull dengcao/Qwen3-Embedding-8B:Q5_K_M

# Start with model
hanzod --embedding-model qwen3-embedding-8b

# List models
hanzod models list
```

## Implementation Plan

### Phase 1: Ollama Integration (Quick Win)
1. Add `ollama pull` wrapper in hanzod
2. Auto-detect and pull missing models on startup
3. Basic model management commands

### Phase 2: Native Pull via mistral.rs
1. Integrate hanzoai/inference for native model loading
2. Support HuggingFace direct downloads
3. GGUF format support

### Phase 3: CLI Integration
1. Create @hanzo/cli node management commands
2. Add model pull/list/delete commands
3. Integration tests

### Phase 4: Advanced Features
1. Model caching and versioning
2. Automatic model updates
3. Model performance benchmarking
4. Multi-model management

## Benefits

1. **Simplified Setup**: Users can pull models with one command
2. **Unified Interface**: Same commands work across Ollama, HuggingFace, LM Studio
3. **Better UX**: No need to switch between different tools
4. **Production Ready**: Models managed by the node itself
5. **CLI Integration**: Seamless workflow with @hanzo/cli

## Next Steps

1. Implement basic Ollama wrapper in hanzod
2. Add model pull command to CLI arguments
3. Create @hanzo/cli node management commands
4. Test with Qwen3 models
5. Document in README