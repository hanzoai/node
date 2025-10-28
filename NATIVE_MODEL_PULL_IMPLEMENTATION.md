# Native Model Pull Implementation for Hanzo Node (via hanzoai/engine)

## Key Findings

### What Ollama Does (for compatibility reference)
1. **Registry System**: Uses `registry.ollama.ai/v2/` with manifests
2. **Layer-based downloading**: Downloads model layers with SHA256 verification
3. **Blob management**: Manages binary large objects with caching
4. **Progress reporting**: Provides status updates during download
5. **Resume support**: Can resume interrupted downloads
6. **Port 11434**: Ollama port (supported for compatibility)
7. **Port 3690**: Default Hanzo Engine port (primary)

### What hanzoai/engine (our mistral.rs fork) Already Has
✅ **HuggingFace Hub Support**: Uses `hf_hub` crate for downloading models
✅ **Token support**: Handles HF API tokens
✅ **Cache management**: Uses GLOBAL_HF_CACHE
✅ **Progress reporting**: Built-in progress updates
✅ **GGUF support**: Can load GGUF format models

## MLX Support for Apple Silicon (NEW)

### Qwen3 MLX Models Available
- **4-bit, 6-bit, 8-bit, BF16 quantizations**
- **44K+ tokens/second** on Apple Silicon
- **All Qwen3 models**: 0.6B, 4B, 8B variants
- **Optimized for M1/M2/M3/M4 chips**

### MLX Model Sources
1. **Official Qwen MLX Models**: Available on HuggingFace
2. **Community MLX Server**: github.com/jakedahn/qwen3-embeddings-mlx
3. **mlx-lm**: Official MLX model runner (supports Qwen3 since v0.24.0)

## Implementation Plan for hanzod (using hanzoai/engine)

Since mistral.rs (hanzoai/inference) already has HuggingFace support, we should:

### 1. Create Native Model Pull Module (in hanzoai/engine)

```rust
// hanzo-bin/hanzo-node/src/model_pull/mod.rs
use hf_hub::{api::sync::ApiBuilder, Repo, RepoType, Cache};
use std::path::PathBuf;

pub struct ModelPuller {
    cache: Cache,
    token: Option<String>,
}

impl ModelPuller {
    pub async fn pull_from_huggingface(&self, model_id: &str) -> Result<PathBuf> {
        let api = ApiBuilder::from_cache(self.cache.clone())
            .with_progress(true)
            .with_token(self.token.clone())
            .build()?;
        
        let repo = api.repo(Repo::model(model_id));
        
        // Download model files
        let files = repo.get_file("model.safetensors")?;
        Ok(files)
    }
    
    pub async fn pull_gguf(&self, url: &str) -> Result<PathBuf> {
        // Download GGUF models from URLs
    }
}
```

### 2. Add Registry Support (Hanzo Registry + Ollama-compatible)

```rust
// hanzo-bin/hanzo-node/src/model_pull/registry.rs
pub struct OllamaRegistry {
    base_url: String, // registry.ollama.ai/v2/
}

impl OllamaRegistry {
    pub async fn pull_manifest(&self, model: &str) -> Result<Manifest> {
        let url = format!("{}/library/{}/manifests/latest", self.base_url, model);
        // Fetch and parse manifest
    }
    
    pub async fn download_layers(&self, manifest: &Manifest) -> Result<()> {
        for layer in &manifest.layers {
            self.download_blob(&layer.digest).await?;
        }
    }
}
```

### 3. Integrate with hanzod CLI

```rust
// hanzo-bin/hanzo-node/src/main.rs
use clap::{Parser, Subcommand};

#[derive(Subcommand)]
enum Commands {
    /// Pull a model from a registry
    Pull {
        /// Model identifier (e.g., qwen3-embedding-8b)
        model: String,
        
        /// Source: huggingface, ollama, url
        #[arg(long, default_value = "huggingface")]
        source: String,
    },
    
    /// List available models
    List,
    
    /// Delete a model
    Delete { model: String },
}
```

### 4. Model Mapping for Qwen3

```rust
// Map user-friendly names to actual model IDs
pub fn resolve_model_id(name: &str) -> &str {
    match name {
        "qwen3-embedding-8b" => "Qwen/Qwen3-8B",
        "qwen3-embedding-4b" => "Qwen/Qwen3-4B", 
        "qwen3-embedding-0.6b" => "Qwen/Qwen3-0.6B",
        // Ollama models
        "dengcao/qwen3-embedding-8b" => "dengcao/Qwen3-Embedding-8B",
        _ => name,
    }
}
```

## Implementation Steps

### Phase 1: HuggingFace Integration (Quick Win)
1. Add `hf_hub` dependency to hanzod
2. Implement basic pull from HuggingFace
3. Add model resolution mapping
4. Test with Qwen3 models

### Phase 2: GGUF Support
1. Add GGUF download from direct URLs
2. Support quantized models (Q5_K_M, etc.)
3. Verify checksums

### Phase 3: Ollama Registry Compatibility
1. Implement manifest parsing
2. Layer-based downloading
3. Blob management with caching

### Phase 4: CLI Integration
1. Add pull/list/delete commands to hanzod
2. Progress reporting during download
3. Resume support for interrupted downloads

## Usage Examples

```bash
# Start hanzod embedding server on port 3690
hanzod serve --port 3690

# Pull from HuggingFace
hanzod pull qwen3-embedding-8b --source huggingface

# Pull GGUF model
hanzod pull https://huggingface.co/Qwen/Qwen3-8B-GGUF/blob/main/qwen3-8b-q5_k_m.gguf

# Pull MLX model for Apple Silicon
hanzod pull qwen3-embedding-8b --source mlx

# Pull from Ollama registry (for compatibility)
hanzod pull dengcao/qwen3-embedding-8b --source ollama

# List downloaded models
hanzod list

# Delete a model
hanzod delete qwen3-embedding-8b
```

## Benefits of hanzoai/engine Approach

1. **Native Integration**: Direct integration with our engine
2. **No External Dependencies**: Don't need Ollama installed
3. **Better Control**: Full control over download process
4. **Unified Experience**: Same tool for all operations
5. **Efficient**: Reuse existing hf_hub infrastructure

## Next Actions

1. ✅ Study Ollama implementation (DONE)
2. ✅ Check mistral.rs capabilities (DONE - has hf_hub)
3. 🔄 Implement HuggingFace pull in hanzod
4. 🔄 Add GGUF download support
5. 🔄 Test with Qwen3 models
6. 🔄 Create @hanzo/cli integration