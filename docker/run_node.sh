#!/bin/bash
set -e

# ============================================================================
# Hanzo Node Startup Script
# Pure Environment Variable Configuration - No config files required
# ============================================================================

# Network Configuration
export NODE_API_IP=${NODE_API_IP:-0.0.0.0}
export NODE_IP=${NODE_IP:-0.0.0.0}
export NODE_API_PORT=${NODE_API_PORT:-9550}
export NODE_WS_PORT=${NODE_WS_PORT:-9551}
export NODE_PORT=${NODE_PORT:-9552}
export NODE_HTTPS_PORT=${NODE_HTTPS_PORT:-9553}

# Path Configuration
export INSTALL_FOLDER_PATH=${INSTALL_FOLDER_PATH:-/app/pre-install}
export NODE_STORAGE_PATH=${NODE_STORAGE_PATH:-hanzo-storage}
export HANZO_TOOLS_RUNNER_DENO_BINARY_PATH=${HANZO_TOOLS_RUNNER_DENO_BINARY_PATH:-/app/hanzo-tools-runner-resources/deno}
export HANZO_TOOLS_RUNNER_UV_BINARY_PATH=${HANZO_TOOLS_RUNNER_UV_BINARY_PATH:-/app/hanzo-tools-runner-resources/uv}
export PATH="/app/hanzo-tools-runner-resources:/root/.local/bin:$PATH"

# Node Identity and Security
export IDENTITY_SECRET_KEY=${IDENTITY_SECRET_KEY:-}
export ENCRYPTION_SECRET_KEY=${ENCRYPTION_SECRET_KEY:-}
export GLOBAL_IDENTITY_NAME=${GLOBAL_IDENTITY_NAME:-@@my_local_ai.sep-hanzo}

# Node Behavior
export PING_INTERVAL_SECS=${PING_INTERVAL_SECS:-0}
export STARTING_NUM_QR_PROFILES=${STARTING_NUM_QR_PROFILES:-1}
export STARTING_NUM_QR_DEVICES=${STARTING_NUM_QR_DEVICES:-1}
export FIRST_DEVICE_NEEDS_REGISTRATION_CODE=${FIRST_DEVICE_NEEDS_REGISTRATION_CODE:-false}
export SKIP_IMPORT_FROM_DIRECTORY=${SKIP_IMPORT_FROM_DIRECTORY:-false}
export NO_SECRET_FILE=${NO_SECRET_FILE:-true}

# Logging Configuration
export RUST_LOG=${RUST_LOG:-debug,error,info}
export LOG_SIMPLE=${LOG_SIMPLE:-true}
export LOG_ALL=${LOG_ALL:-1}

# AI Provider Configuration
export EMBEDDINGS_SERVER_URL=${EMBEDDINGS_SERVER_URL:-}
export PROXY_IDENTITY=${PROXY_IDENTITY:-@@relayer_pub_01.sep-hanzo}

# Multi-Provider AI Agent Configuration
# Format: comma-separated lists (must have same number of entries)
export INITIAL_AGENT_NAMES=${INITIAL_AGENT_NAMES:-do_qwen32b}
export INITIAL_AGENT_URLS=${INITIAL_AGENT_URLS:-https://inference.do-ai.run}
export INITIAL_AGENT_MODELS=${INITIAL_AGENT_MODELS:-openai:alibaba-qwen3-32b}
export INITIAL_AGENT_API_KEYS=${INITIAL_AGENT_API_KEYS:-}

# ============================================================================
# Startup Information
# ============================================================================
echo "========================================="
echo "Hanzo Node Starting"
echo "========================================="
echo "Version: ${HANZO_VERSION:-dev}"
echo "Build: ${BUILD_TYPE:-debug}"
echo ""
echo "Network Configuration:"
echo "  NODE_API_IP:        $NODE_API_IP"
echo "  NODE_API_PORT:      $NODE_API_PORT"
echo "  NODE_WS_PORT:       $NODE_WS_PORT"
echo "  NODE_PORT:          $NODE_PORT"
echo "  NODE_HTTPS_PORT:    $NODE_HTTPS_PORT"
echo ""
echo "Identity:"
echo "  GLOBAL_IDENTITY_NAME: $GLOBAL_IDENTITY_NAME"
echo "  PROXY_IDENTITY:       $PROXY_IDENTITY"
echo ""
echo "AI Providers:"
echo "  INITIAL_AGENT_NAMES:  $INITIAL_AGENT_NAMES"
echo "  INITIAL_AGENT_MODELS: $INITIAL_AGENT_MODELS"
echo ""
echo "Storage:"
echo "  NODE_STORAGE_PATH: $NODE_STORAGE_PATH"
echo "========================================="
echo ""

# Start the node
exec /app/hanzo_node