#!/bin/bash

export NODE_IP="0.0.0.0"
export NODE_PORT="9452"
export NODE_API_IP="0.0.0.0"
export NODE_API_PORT="9450"
export PING_INTERVAL_SECS="0"
export GLOBAL_IDENTITY_NAME="@@localhost.sep-hanzo"
export RUST_LOG=debug,error,info
export STARTING_NUM_QR_PROFILES="1"
export STARTING_NUM_QR_DEVICES="1"
export FIRST_DEVICE_NEEDS_REGISTRATION_CODE="false"
export LOG_SIMPLE="true"
# Embedding server configuration (choose one):
# Option 1: Ollama (default port 11434)
export EMBEDDINGS_SERVER_URL="http://localhost:11434"

# Option 2: LM Studio (default port 1234) - Great for testing!
# export EMBEDDINGS_SERVER_URL="http://localhost:1234"

# Option 3: Hanzo public embeddings server
# export EMBEDDINGS_SERVER_URL="https://public.hanzo.ai/x-em"

# Option 4: Custom OpenAI-compatible endpoint
# export EMBEDDINGS_SERVER_URL="https://api.together.xyz"
# export EMBEDDINGS_SERVER_API_KEY="your-api-key-here"

# Native embedding configuration (RECOMMENDED: Qwen3 models for best performance)
export USE_NATIVE_EMBEDDINGS="true" # Enable native mistral.rs embeddings (with Ollama fallback)
export USE_GPU="true" # Use GPU acceleration if available (CUDA or Metal)

# Uncomment to use Qwen3 models (RECOMMENDED):
# export DEFAULT_EMBEDDING_MODEL="qwen3-next" # Best: 1536 dims, 32K context
# export RERANKER_MODEL="qwen3-reranker-4b" # Best: 4B params for superior reranking

# Alternative embedding models:
# export DEFAULT_EMBEDDING_MODEL="mistral-embed" # 1024 dims
# export DEFAULT_EMBEDDING_MODEL="e5-mistral-embed" # 1024 dims
# export DEFAULT_EMBEDDING_MODEL="bge-m3" # 1024 dims, multilingual

# Path to local GGUF model file (optional - will download if not provided):
# export NATIVE_MODEL_PATH="/path/to/qwen3-next.gguf"
# export RERANKER_MODEL_PATH="/path/to/qwen3-reranker-4b.gguf"

# Add these lines to enable all log options
export LOG_ALL=1

cargo run
