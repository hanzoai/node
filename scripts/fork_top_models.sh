#!/bin/bash

# Fork top models from each category to Hanzo HuggingFace organizations

set -e

echo "🚀 Starting Hanzo Model Fork Pipeline"
echo "======================================"

# Check if logged in to HuggingFace
if ! huggingface-cli whoami &>/dev/null; then
    echo "❌ Not logged in to HuggingFace. Please run: huggingface-cli login"
    exit 1
fi

# Get the directory of this script
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"

# Language Models for hanzo-lm
echo ""
echo "📚 Forking Language Models to hanzo-lm..."
echo "----------------------------------------"

LM_MODELS=(
    # Top Open Models (Dec 2024)
    "meta-llama/Llama-3.3-70B-Instruct"
    "Qwen/QwQ-32B-Preview"
    "Qwen/Qwen2.5-72B-Instruct"
    "mistralai/Mistral-Large-Instruct-2411"
    "google/gemma-2-27b-it"
    "deepseek-ai/DeepSeek-V3"
    "NousResearch/Hermes-3-Llama-3.1-70B"
    "cognitivecomputations/dolphin-2.9.3-llama-3.1-70b"

    # Vision Models
    "meta-llama/Llama-3.2-90B-Vision-Instruct"
    "Qwen/Qwen2-VL-72B-Instruct"

    # Smaller but powerful
    "Qwen/Qwen2.5-32B-Instruct"
    "meta-llama/Llama-3.2-3B-Instruct"
    "google/gemma-2-9b-it"
)

for model in "${LM_MODELS[@]}"; do
    echo "Processing: $model"
    "$SCRIPT_DIR/fork_model.sh" "$model" "hanzo-lm" || echo "⚠️  Failed to fork $model, continuing..."
    sleep 2  # Be nice to HF servers
done

# MLX Models for hanzo-mlx
echo ""
echo "🍎 Forking MLX Models to hanzo-mlx..."
echo "------------------------------------"

MLX_MODELS=(
    # 4-bit Quantized for Apple Silicon
    "mlx-community/Llama-3.3-70B-Instruct-4bit"
    "mlx-community/Qwen2.5-72B-Instruct-4bit"
    "mlx-community/DeepSeek-Coder-V2-Instruct-4bit"
    "mlx-community/Mistral-Large-Instruct-2407-4bit"
    "mlx-community/gemma-2-27b-it-4bit"

    # 8-bit for better quality
    "mlx-community/Llama-3.3-70B-Instruct-8bit"
    "mlx-community/Qwen2.5-32B-Instruct-8bit"
)

for model in "${MLX_MODELS[@]}"; do
    echo "Processing: $model"
    "$SCRIPT_DIR/fork_model.sh" "$model" "hanzo-mlx" || echo "⚠️  Failed to fork $model, continuing..."
    sleep 2
done

# Embedding Models for hanzo-embeddings
echo ""
echo "🔍 Forking Embedding Models to hanzo-embeddings..."
echo "------------------------------------------------"

EMBED_MODELS=(
    # Best performers
    "Alibaba-NLP/gte-Qwen2-7B-instruct"
    "Snowflake/snowflake-arctic-embed-l-v2.0"
    "jinaai/jina-embeddings-v3"
    "BAAI/bge-en-icl"
    "BAAI/bge-m3"
    "nomic-ai/nomic-embed-text-v1.5"

    # Rerankers
    "BAAI/bge-reranker-v2.5-gemma2-lightweight"
    "jinaai/jina-reranker-v2-base-multilingual"
)

for model in "${EMBED_MODELS[@]}"; do
    echo "Processing: $model"
    "$SCRIPT_DIR/fork_model.sh" "$model" "hanzo-embeddings" || echo "⚠️  Failed to fork $model, continuing..."
    sleep 2
done

# Tool-use models for hanzo-tools
echo ""
echo "🔧 Forking Tool-Use Models to hanzo-tools..."
echo "------------------------------------------"

TOOL_MODELS=(
    # Models optimized for function calling
    "NousResearch/Hermes-3-Llama-3.1-70B"  # Excellent tool use
    "mistralai/Mistral-Large-Instruct-2411"  # Native function calling
    "gorilla-llm/gorilla-openfunctions-v2"  # Specialized for functions
)

for model in "${TOOL_MODELS[@]}"; do
    echo "Processing: $model"
    "$SCRIPT_DIR/fork_model.sh" "$model" "hanzo-tools" || echo "⚠️  Failed to fork $model, continuing..."
    sleep 2
done

echo ""
echo "✅ Model forking pipeline complete!"
echo ""
echo "View your models at:"
echo "  - https://huggingface.co/hanzo-lm"
echo "  - https://huggingface.co/hanzo-mlx"
echo "  - https://huggingface.co/hanzo-embeddings"
echo "  - https://huggingface.co/hanzo-tools"