# Qwen3 Models Installation Guide for Hanzo Node

## 📊 Best Qwen3 Models (2025 Latest)

Based on latest benchmarks, Qwen3 embedding models are state-of-the-art, with **Qwen3-Embedding-8B ranking #1 on MTEB multilingual leaderboard** (score: 70.58).

## 🏆 Recommended Models

### **Primary: Qwen3-Embedding-8B** (Our "biggest" and best model)
```bash
# Install with Ollama (recommended)
ollama pull dengcao/Qwen3-Embedding-8B:Q5_K_M

# Configuration
export EMBEDDING_MODEL_TYPE="qwen3-embedding-8b"
export EMBEDDINGS_SERVER_URL="http://localhost:11434"
```
- **Performance**: #1 on MTEB multilingual leaderboard
- **Specifications**: 8B params, 4096 embedding dimensions, 32K context
- **Languages**: 100+ languages supported
- **Use case**: Maximum accuracy for multilingual embeddings

### **Alternative Options**

#### Qwen3-Embedding-4B (Balanced)
```bash
ollama pull dengcao/Qwen3-Embedding-4B:Q5_K_M
export EMBEDDING_MODEL_TYPE="qwen3-embedding-4b"
```
- 4B params, 2048 dimensions, 32K context
- Good balance of performance and speed

#### Qwen3-Embedding-0.6B (Lightweight)
```bash
ollama pull dengcao/Qwen3-Embedding-0.6B:Q5_K_M
export EMBEDDING_MODEL_TYPE="qwen3-embedding-0.6b"
```
- 0.6B params, 1024 dimensions, 32K context
- Fast inference for resource-constrained environments

## 🔄 Reranker Models

### Qwen3-Reranker-4B (High Quality)
```bash
ollama pull dengcao/Qwen3-Reranker-4B:F16
export RERANKER_MODEL_TYPE="qwen3-reranker-4b"
```
- 4B params for superior reranking
- 8K context window
- Improves retrieval quality significantly

### Qwen3-Reranker-0.6B (Lightweight)
```bash
ollama pull dengcao/Qwen3-Reranker-0.6B:F16
export RERANKER_MODEL_TYPE="qwen3-reranker-0.6b"
```
- Faster reranking option
- 8K context window

## 🚀 Quick Start

### 1. Install Ollama (if not already installed)
```bash
# macOS
brew install ollama

# Linux
curl -fsSL https://ollama.com/install.sh | sh

# Start Ollama service
ollama serve
```

### 2. Pull the Best Qwen3 Model
```bash
# Pull our recommended flagship model
ollama pull dengcao/Qwen3-Embedding-8B:Q5_K_M

# Verify installation
ollama list | grep qwen
```

### 3. Configure Hanzo Node
```bash
# Set environment variables
export EMBEDDING_MODEL_TYPE="qwen3-embedding-8b"
export EMBEDDINGS_SERVER_URL="http://localhost:11434"

# Run the node
sh scripts/run_node_localhost.sh
```

## 💡 LM Studio Alternative

If you prefer LM Studio (port 1234):

1. Download Qwen3 GGUF models from HuggingFace
2. Load in LM Studio
3. Configure:
```bash
export EMBEDDINGS_SERVER_URL="http://localhost:1234"
export EMBEDDING_MODEL_TYPE="qwen3-embedding-8b"
```

## 🔧 Advanced Configuration

### For Production Use
```bash
# Use the flagship model for best results
export EMBEDDING_MODEL_TYPE="qwen3-embedding-8b"
export EMBEDDINGS_SERVER_URL="http://localhost:11434"
export EMBEDDING_BATCH_SIZE="32"
export EMBEDDING_MAX_RETRIES="3"
```

### For Development/Testing
```bash
# Use lightweight model for faster iteration
export EMBEDDING_MODEL_TYPE="qwen3-embedding-0.6b"
export EMBEDDINGS_SERVER_URL="http://localhost:11434"
```

### With Reranking Pipeline
```bash
# Configure both embedding and reranking
export EMBEDDING_MODEL_TYPE="qwen3-embedding-8b"
export RERANKER_MODEL_TYPE="qwen3-reranker-4b"
export EMBEDDINGS_SERVER_URL="http://localhost:11434"
```

## 📈 Performance Comparison

| Model | Parameters | Dimensions | Context | MTEB Score | Speed |
|-------|------------|------------|---------|------------|-------|
| **Qwen3-Embedding-8B** | 8B | 4096 | 32K | **70.58** (#1) | Slow |
| Qwen3-Embedding-4B | 4B | 2048 | 32K | ~68 | Medium |
| Qwen3-Embedding-0.6B | 0.6B | 1024 | 32K | ~65 | Fast |

## 🌍 Language Support

All Qwen3 embedding models support **100+ languages** including:
- English, Chinese, Spanish, French, German
- Arabic, Japanese, Korean, Russian
- Hindi, Portuguese, Italian, Dutch
- And many more...

## 🔍 Verification

To verify your setup:
```bash
# Check if Ollama is running
curl http://localhost:11434/api/tags

# Check available models
ollama list

# Test embedding generation
curl http://localhost:11434/api/embeddings -d '{
  "model": "dengcao/Qwen3-Embedding-8B:Q5_K_M",
  "prompt": "Hello world"
}'
```

## 📝 Notes

- **Q5_K_M quantization** is recommended for optimal balance of performance and resource usage
- The 8B model requires ~8GB RAM for inference
- All models support the full 32K context window
- Models are hosted under the `dengcao` namespace on Ollama
- Use F16 precision for reranker models for best quality

## 🆚 Why Qwen3-Embedding-8B?

1. **#1 on MTEB leaderboard** - Best multilingual embedding model available
2. **4096 dimensions** - Rich semantic representation
3. **32K context** - Handle long documents
4. **100+ languages** - True multilingual support
5. **Active development** - Regular updates from Alibaba Cloud

Start with Qwen3-Embedding-8B for the best embedding quality in your Hanzo Node!