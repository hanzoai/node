#!/bin/bash
# Hanzo Node configured for our local Hanzo Engine + zen models.
#
# Supports 3 engine topologies — hanzod itself is topology-agnostic, it just
# talks to whichever endpoints are reachable. Set CHAT_ENGINE_URL / EMBED_ENGINE_URL
# to point at the right master rank.
#
#   1. local       — single-host engine via topology YAML (default; see start-stack.sh)
#   2. ring        — Ring-distributed engine; point CHAT_ENGINE_URL at the master rank
#   3. nccl        — NCCL-distributed engine (CUDA only); point CHAT_ENGINE_URL at master
#
# Optional ZAP fast-path:
#   export HANZO_ENGINE_ZAP_URL=127.0.0.1:3692   # binary protocol → lower overhead than HTTP

set -e

export NODE_IP="0.0.0.0"
export NODE_PORT="9452"
export NODE_API_IP="0.0.0.0"
export NODE_API_PORT="9450"
export NODE_WS_PORT="9451"
export NODE_ZAP_PORT="${NODE_ZAP_PORT:-3693}"
export PING_INTERVAL_SECS="0"
export GLOBAL_IDENTITY_NAME="@@localhost.sep-hanzo"
export RUST_LOG="error,info"
export STARTING_NUM_QR_PROFILES="1"
export STARTING_NUM_QR_DEVICES="1"
export FIRST_DEVICE_NEEDS_REGISTRATION_CODE="false"
export LOG_SIMPLE="true"
export NODE_STORAGE_PATH="/tmp/hanzo-runtime/storage"

# Engine endpoints (override via env for ring/nccl masters or remote engines)
CHAT_ENGINE_URL="${CHAT_ENGINE_URL:-http://localhost:3690}"
EMBED_ENGINE_URL="${EMBED_ENGINE_URL:-http://localhost:3680}"

export EMBEDDINGS_SERVER_URL="$EMBED_ENGINE_URL"

# Register chat-capable engines. Comma-separated; all 4 INITIAL_* lists must match length.
# Add more engines (e.g. a second Ring cluster's master) by appending to each list.
export INITIAL_AGENT_NAMES="${INITIAL_AGENT_NAMES:-zen_nano}"
export INITIAL_AGENT_URLS="${INITIAL_AGENT_URLS:-$CHAT_ENGINE_URL}"
export INITIAL_AGENT_MODELS="${INITIAL_AGENT_MODELS:-openai:default}"
export INITIAL_AGENT_API_KEYS="${INITIAL_AGENT_API_KEYS:-local-no-key}"

mkdir -p "$NODE_STORAGE_PATH"

# Note: hanzo-engine should be started separately (see start-stack.sh or start-stack-ring.sh).
# Metal acceleration on Apple Silicon needs a topology YAML in single-host mode because
# 24-layer all-metal triggers a kv_cache panic. Workaround: layers 0-21 on metal[0], 22-23 on cpu.

exec /Users/a/work/hanzo/hanzoai/node/target/release/hanzod "$@"
