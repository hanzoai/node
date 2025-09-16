#!/bin/bash

export NODE_IP="0.0.0.0"
export NODE_PORT="3691"
export NODE_API_IP="0.0.0.0"
export NODE_API_PORT="3690"
export NODE_WS_PORT="3692"  # WebSocket port (optional, defaults to API port + 1)
export PING_INTERVAL_SECS="0"
export GLOBAL_IDENTITY_NAME="@@localhost.sep-hanzo"
export RUST_LOG=debug,error,info
export STARTING_NUM_QR_PROFILES="1"
export STARTING_NUM_QR_DEVICES="1"
export FIRST_DEVICE_NEEDS_REGISTRATION_CODE="false"
export LOG_SIMPLE="true"
# Embedding server configuration (choose one):
# Option 1: Hanzo Public Embeddings (default)
# Leave EMBEDDINGS_SERVER_URL unset to default to Hanzo public: https://public.hanzo.ai/x-em
# export EMBEDDINGS_SERVER_URL="https://public.hanzo.ai/x-em"

# Option 2: LM Studio (default port 1234) - Great for testing!
# export EMBEDDINGS_SERVER_URL="http://localhost:1234"

# Option 3: Ollama fallback (default port 11434) - ACTIVE
export EMBEDDINGS_SERVER_URL="http://localhost:11434"

# Option 4: Hanzo public embeddings server
# export EMBEDDINGS_SERVER_URL="https://public.hanzo.ai/x-em"

# Option 5: Custom OpenAI-compatible endpoint
# export EMBEDDINGS_SERVER_URL="https://api.together.xyz"
# export EMBEDDINGS_SERVER_API_KEY="your-api-key-here"

# Native embedding configuration (RECOMMENDED: Qwen3 models for best performance)
export USE_NATIVE_EMBEDDINGS="true" # Prefer native embeddings / Hanzo API over Ollama
export USE_GPU="true" # Use GPU acceleration if available (CUDA or Metal)

# Qwen3 models by default (RECOMMENDED):
# export DEFAULT_EMBEDDING_MODEL="qwen3-embedding-8b" # 4096 dims, 32K context - Needs to be pulled first
# For now, use a simpler model that's available:
export DEFAULT_EMBEDDING_MODEL="qwen3-embedding-4b" # 2048 dims, balanced performance
export RERANKER_MODEL="qwen3-reranker-4b" # Optional: 4B params reranker

# Alternative embedding models:
# export DEFAULT_EMBEDDING_MODEL="mistral-embed" # 1024 dims
# export DEFAULT_EMBEDDING_MODEL="e5-mistral-embed" # 1024 dims
# export DEFAULT_EMBEDDING_MODEL="bge-m3" # 1024 dims, multilingual

# Path to local GGUF model file (optional - will download if not provided):
# export NATIVE_MODEL_PATH="/path/to/qwen3-embedding-8b.gguf"
# export RERANKER_MODEL_PATH="/path/to/qwen3-reranker-4b.gguf"

# Add these lines to enable all log options
export LOG_ALL=1

export USE_LOCAL_ENGINE="true" # Prefer local engine pool if available (port 36900)
# To run a local engine pool, expose it on http://localhost:36900
# Otherwise, the node will operate without a local pool;
# embedding defaults use Hanzo embeddings service unless overridden.

cargo run
