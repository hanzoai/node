# Native Embeddings with mistral.rs

The Hanzo node now supports native Rust-based embedding generation using mistral.rs, providing faster inference with automatic fallback to Ollama when needed.

## Features

- **Native Rust Inference**: Run embedding models directly in Rust using mistral.rs
- **GPU Acceleration**: Automatic CUDA (NVIDIA) and Metal (Apple Silicon) support
- **Automatic Fallback**: Seamlessly falls back to Ollama if native inference fails
- **Multiple Model Support**: Built-in support for popular embedding models
- **GGUF Format**: Support for quantized GGUF model files for efficient inference

## Configuration

Configure native embeddings through environment variables:

```bash
# Enable/disable native embeddings (default: true)
export USE_NATIVE_EMBEDDINGS="true"

# Enable GPU acceleration (default: true)
export USE_GPU="true"

# Path to local GGUF model file (optional)
export NATIVE_MODEL_PATH="/path/to/model.gguf"

# Fallback to Ollama (always configured)
export EMBEDDINGS_SERVER_URL="http://localhost:11434"

# Choose embedding model (RECOMMENDED: qwen3-next for best performance)
export DEFAULT_EMBEDDING_MODEL="qwen3-next"

# Optional: Enable reranking for better retrieval accuracy
export RERANKER_MODEL="qwen3-reranker-4b"
```

## Supported Models

### Native Embedding Models (mistral.rs)

#### Recommended: Qwen Models
- **`qwen3-next`** ⭐ - State-of-the-art Qwen3-Next embedding model
  - 1536 dimensions for rich semantic representation
  - 32,768 token context window (exceptional for long documents)
  - Best overall performance for most use cases
  - Optimized for both English and multilingual content

- `qwen2.5-embed` - Qwen2.5 embedding model (1536 dimensions, 32K context)

#### Other Embedding Models
- `mistral-embed` - Mistral's embedding model (1024 dimensions)
- `e5-mistral-embed` - E5 Mistral variant (1024 dimensions)
- `bge-m3` - BGE M3 multilingual model (1024 dimensions)
- `native:custom-model` - Custom models with "native:" prefix

### Native Reranker Models
Rerankers dramatically improve retrieval quality by scoring query-document pairs:

#### Recommended: Qwen3-Reranker
- **`qwen3-reranker-4b`** ⭐ - State-of-the-art Qwen3 reranker
  - 4B parameters for powerful relevance scoring
  - 8,192 token context window
  - Exceptional accuracy in distinguishing relevant documents
  - Works seamlessly with `qwen3-next` embeddings

#### Other Rerankers
- `bge-reranker-v2` - BGE Reranker v2 M3 (512 token context)
- `jina-reranker-v2` - Jina multilingual reranker (1024 token context)

### Ollama Models (Fallback)
- `snowflake-arctic-embed:xs` - Snowflake Arctic (384 dimensions)
- `all-minilm:l6-v2` - All-MiniLM L6 v2
- `jina/jina-embeddings-v2-base-es:latest` - Jina multilingual

## How It Works

### Embedding Generation
1. **Initialization**: On startup, the node attempts to load the native mistral.rs model
2. **Primary Path**: If successful, all embedding requests go through mistral.rs
3. **Fallback**: If native inference fails, requests automatically route to Ollama
4. **Transparent**: The switch between native and Ollama is transparent to clients

### Reranking Workflow
1. **Initial Retrieval**: Generate embeddings and find candidate documents
2. **Reranking**: Use reranker model to score query-document pairs
3. **Reordering**: Sort results by relevance scores
4. **Return**: Provide reranked results for better accuracy

### Recommended Configuration for Qwen3

For optimal performance with Qwen3 models:

```bash
# Use Qwen3-Next for embeddings (best quality)
export DEFAULT_EMBEDDING_MODEL="qwen3-next"

# Enable Qwen3 reranker for superior retrieval accuracy
export RERANKER_MODEL="qwen3-reranker-4b"

# Enable GPU acceleration for faster inference
export USE_GPU="true"

# Optional: Download and use local GGUF models
export NATIVE_MODEL_PATH="/path/to/qwen3-next.gguf"
export RERANKER_MODEL_PATH="/path/to/qwen3-reranker-4b.gguf"
```

This configuration provides:
- **Best embedding quality**: 1536-dimensional vectors with 32K context
- **Superior retrieval**: Reranking dramatically improves search relevance
- **Fast inference**: GPU acceleration for both embedding and reranking
- **Production ready**: Battle-tested models with excellent performance

## Performance Benefits

- **Lower Latency**: Native Rust inference eliminates HTTP overhead
- **Better Resource Usage**: Direct memory management without external process
- **GPU Acceleration**: Automatic use of CUDA or Metal when available
- **Reduced Dependencies**: Can run without Ollama for supported models

## Downloading Models

### GGUF Models
Download quantized GGUF models from Hugging Face:

```bash
# Example: Download a quantized embedding model
wget https://huggingface.co/user/model/resolve/main/model.gguf
export NATIVE_MODEL_PATH="./model.gguf"
```

### Model Recommendations

For best performance:
- Use Q4_K_M or Q5_K_M quantization for balanced speed/quality
- Q8_0 for highest quality with moderate speed
- Q2_K for maximum speed with acceptable quality loss

## Troubleshooting

### Native Model Fails to Load
- Check that the model file exists and is readable
- Verify GGUF format compatibility
- Check GPU drivers if using acceleration
- Review logs for specific error messages

### Falling Back to Ollama
The node automatically falls back to Ollama if:
- Native model fails to load
- GPU initialization fails
- Model file is missing or corrupted
- Inference errors occur

### GPU Not Detected
- For CUDA: Ensure NVIDIA drivers and CUDA toolkit are installed
- For Metal: Automatically detected on macOS with Apple Silicon
- Set `USE_GPU=false` to force CPU-only mode

## Development

### Building with GPU Support

```bash
# Build with CUDA support
cargo build --release --features cuda

# Build with Metal support (macOS)
cargo build --release --features metal

# Build with both
cargo build --release --features cuda,metal
```

### Testing

```bash
# Test with native embeddings
export USE_NATIVE_EMBEDDINGS=true
export NATIVE_MODEL_PATH="./test-model.gguf"
cargo test

# Test fallback behavior
export USE_NATIVE_EMBEDDINGS=true
# Don't set NATIVE_MODEL_PATH to test fallback
cargo test
```

## Future Improvements

- Support for more model architectures
- Dynamic model loading/unloading
- Model caching and preloading
- Batch inference optimization
- Support for sentence transformers
- Fine-tuning capabilities