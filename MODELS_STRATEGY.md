# Hanzo AI Model Repository Strategy

## Overview
Hanzo AI maintains curated model repositories on HuggingFace to ensure availability, versioning, and customization of models for our AI infrastructure.

## HuggingFace Organizations

### 1. `hanzo-lm` - Language Models
Repository: https://huggingface.co/hanzo-lm

For text generation, chat, and instruction-following models:
- Llama models (Llama-3.3, Llama-3.2, etc.)
- Qwen models (Qwen2.5, QwQ, etc.)
- DeepSeek models (DeepSeek-V3, R1, etc.)
- Mistral models (Mistral-Large, Mixtral, etc.)
- Gemma models (Gemma-2, etc.)
- Custom fine-tuned Hanzo models

### 2. `hanzo-mlx` - Apple Silicon Optimized
Repository: https://huggingface.co/hanzo-mlx

MLX-optimized models for Apple Silicon (M1/M2/M3/M4):
- MLX quantized versions (4-bit, 8-bit)
- MLX LoRA adapters
- MLX-optimized embeddings
- Vision models for Apple Neural Engine

### 3. `hanzo-embeddings` - Embedding Models
Repository: https://huggingface.co/hanzo-embeddings

Dedicated to embedding and retrieval models:
- Qwen-Embedding models (8B, 7B)
- Snowflake Arctic Embed series
- Jina embeddings
- Custom Hanzo embeddings
- Reranker models

### 4. `hanzo-tools` - Tool-Use Models
Repository: https://huggingface.co/hanzo-tools

Models optimized for function calling and tool use:
- Tool-optimized Llama variants
- Function-calling Qwen models
- Custom tool routers

## Forking Process with HF CLI

### Prerequisites
```bash
# Install huggingface-cli if not already installed
pip install huggingface-hub

# Login to HuggingFace
huggingface-cli login
```

### Batch Fork Script

Create `fork_models.sh`:
```bash
#!/bin/bash

# Language Models to Fork
LM_MODELS=(
    "meta-llama/Llama-3.3-70B-Instruct"
    "Qwen/QwQ-32B-Preview"
    "deepseek-ai/DeepSeek-V3"
    "mistralai/Mistral-Large-Instruct-2411"
    "google/gemma-2-27b-it"
    "meta-llama/Llama-3.2-90B-Vision-Instruct"
    "Qwen/Qwen2.5-72B-Instruct"
    "NousResearch/Hermes-3-Llama-3.1-70B"
)

# MLX Models to Fork
MLX_MODELS=(
    "mlx-community/Llama-3.3-70B-Instruct-4bit"
    "mlx-community/Qwen2.5-72B-Instruct-4bit"
    "mlx-community/DeepSeek-Coder-V2-Instruct-4bit"
    "mlx-community/Mistral-Large-Instruct-2407-4bit"
)

# Embedding Models
EMBED_MODELS=(
    "Alibaba-NLP/gte-Qwen2-7B-instruct"
    "Snowflake/snowflake-arctic-embed-l-v2.0"
    "jinaai/jina-embeddings-v3"
    "BAAI/bge-en-icl"
    "BAAI/bge-reranker-v2.5-gemma2-lightweight"
)

# Function to fork a model to Hanzo org
fork_model() {
    local source=$1
    local target_org=$2
    local model_name=$(basename $source)

    echo "Forking $source to $target_org/$model_name"

    # Clone the model
    huggingface-cli download $source --local-dir /tmp/$model_name --local-dir-use-symlinks False

    # Create repo in target org
    huggingface-cli repo create $model_name --organization $target_org --type model -y

    # Upload to new repo
    huggingface-cli upload $target_org/$model_name /tmp/$model_name . --repo-type model

    # Clean up
    rm -rf /tmp/$model_name
}

# Fork Language Models
for model in "${LM_MODELS[@]}"; do
    fork_model "$model" "hanzo-lm"
done

# Fork MLX Models
for model in "${MLX_MODELS[@]}"; do
    fork_model "$model" "hanzo-mlx"
done

# Fork Embedding Models
for model in "${EMBED_MODELS[@]}"; do
    fork_model "$model" "hanzo-embeddings"
done
```

### Individual Model Forking

```bash
# Fork a single model
hf_fork() {
    local source=$1
    local org=$2
    local name=${3:-$(basename $source)}

    # Download
    huggingface-cli download $source --local-dir /tmp/$name

    # Create and upload
    huggingface-cli repo create $name --organization $org --type model
    huggingface-cli upload $org/$name /tmp/$name . --repo-type model

    # Cleanup
    rm -rf /tmp/$name
}

# Examples
hf_fork "meta-llama/Llama-3.3-70B-Instruct" "hanzo-lm"
hf_fork "mlx-community/Llama-3.3-70B-Instruct-4bit" "hanzo-mlx"
```

## Model Configuration in Hanzo Node

### Embedding Models (`hanzo-libs/hanzo-embedding/src/model_type.rs`)

```rust
pub enum HanzoEmbeddingModels {
    // From hanzo-embeddings org
    QwenEmbedding8B,      // hanzo-embeddings/gte-Qwen2-7B-instruct
    SnowflakeArcticL,     // hanzo-embeddings/snowflake-arctic-embed-l-v2.0
    JinaV3,               // hanzo-embeddings/jina-embeddings-v3
    BGEReranker,          // hanzo-embeddings/bge-reranker-v2.5-gemma2
}
```

### LLM Provider Configuration

Update environment variables:
```bash
# Use Hanzo-hosted models
export DEFAULT_LLM_MODEL="hanzo-lm/Llama-3.3-70B-Instruct"
export DEFAULT_EMBEDDING_MODEL="hanzo-embeddings/gte-Qwen2-7B-instruct"

# Model endpoints
export HANZO_MODEL_ENDPOINT="https://models.hanzo.ai/v1"
export HANZO_EMBEDDING_ENDPOINT="https://embeddings.hanzo.ai/v1"
```

## Model Selection Criteria

### Top Models by Category (Dec 2024)

#### Chat/Instruction Models
1. **Llama-3.3-70B-Instruct** - Best open-source general model
2. **QwQ-32B-Preview** - Strong reasoning, o1-preview competitor
3. **DeepSeek-V3** - 671B MoE, excellent coding
4. **Qwen2.5-72B-Instruct** - Multilingual, 128K context
5. **Mistral-Large-2411** - Commercial-grade, 128K context

#### Vision Models
1. **Llama-3.2-90B-Vision** - Best open vision-language
2. **Qwen2-VL-72B** - Strong multimodal understanding
3. **Pixtral-Large** - Mistral's vision model

#### Coding Models
1. **DeepSeek-Coder-V2** - Top coding performance
2. **Qwen2.5-Coder-32B** - Excellent code completion
3. **CodeLlama-70B** - Meta's specialized coding

#### Embedding Models
1. **gte-Qwen2-7B-instruct** - 4096 dims, best retrieval
2. **snowflake-arctic-embed-l-v2** - 1024 dims, balanced
3. **jina-embeddings-v3** - Task-specific embeddings
4. **bge-en-icl** - In-context learning embeddings

## Automated Sync Pipeline

### GitHub Action for Model Updates

`.github/workflows/sync-models.yml`:
```yaml
name: Sync HuggingFace Models

on:
  schedule:
    - cron: '0 0 * * 0'  # Weekly on Sunday
  workflow_dispatch:
    inputs:
      models:
        description: 'Comma-separated list of models to sync'
        required: false

jobs:
  sync:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install HF CLI
        run: pip install huggingface-hub

      - name: Login to HuggingFace
        run: huggingface-cli login --token ${{ secrets.HF_TOKEN }}

      - name: Sync Models
        run: |
          if [ -n "${{ inputs.models }}" ]; then
            IFS=',' read -ra MODELS <<< "${{ inputs.models }}"
            for model in "${MODELS[@]}"; do
              ./scripts/fork_model.sh "$model"
            done
          else
            ./scripts/sync_all_models.sh
          fi
```

## Model Versioning

### Tagging Strategy
```bash
# Tag a specific version
huggingface-cli tag hanzo-lm/Llama-3.3-70B-Instruct v1.0.0

# Tag as stable
huggingface-cli tag hanzo-lm/Llama-3.3-70B-Instruct stable

# Tag for production
huggingface-cli tag hanzo-lm/Llama-3.3-70B-Instruct production
```

## Benefits of Hanzo Model Repositories

1. **Availability Guarantee**: Models remain available even if original repos are removed
2. **Version Control**: Pin specific versions for production stability
3. **Customization**: Add Hanzo-specific configs, quantizations, and adapters
4. **Performance**: Serve from optimized infrastructure
5. **Compliance**: Ensure models meet Hanzo's standards
6. **Integration**: Direct integration with Hanzo Node and infrastructure

## Quick Start Commands

```bash
# Fork top 10 language models
./scripts/fork_top_lm_models.sh

# Fork MLX-optimized models for Apple Silicon
./scripts/fork_mlx_models.sh

# Fork embedding models
./scripts/fork_embedding_models.sh

# Sync all models from config
./scripts/sync_all_models.sh
```

## Model Serving

Models from Hanzo repos can be served via:
- Hanzo Engine (native Rust implementation)
- vLLM (high-throughput serving)
- Ollama (local deployment)
- TGI (Text Generation Inference)
- MLX (Apple Silicon)

## Contact

For model requests or issues:
- GitHub: https://github.com/hanzoai/models
- Discord: https://discord.gg/hanzoai