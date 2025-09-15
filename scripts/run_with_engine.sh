#!/bin/bash
# Start Hanzo Engine and Node together

echo "🚀 Starting Hanzo Engine on port 36900..."
cd ~/work/hanzo/engine
cargo run --bin hanzo-engine -- serve --port 36900 &
ENGINE_PID=$!

echo "   Waiting for engine to start..."
sleep 5

echo "🚀 Starting Hanzo Node on port 3690..."
cd ~/work/hanzo/node

# Node configuration
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

# Connect to Hanzo Engine
export EMBEDDINGS_SERVER_URL="http://localhost:36900"
export USE_NATIVE_EMBEDDINGS="true"
export USE_GPU="true"
export DEFAULT_EMBEDDING_MODEL="qwen3-embedding-8b"
export RERANKER_MODEL="qwen3-reranker-4b"

# Enable all logging
export LOG_ALL=1

echo "✓ Configuration:"
echo "   Engine: http://localhost:36900"
echo "   Node API: http://localhost:9450"
echo "   Embedding Model: qwen3-embedding-8b"
echo "   Reranker Model: qwen3-reranker-4b"
echo ""

cargo run --bin hanzod &
NODE_PID=$!

echo "✓ Services started:"
echo "   Engine PID: $ENGINE_PID"
echo "   Node PID: $NODE_PID"
echo ""
echo "Press Ctrl+C to stop both services"

# Wait for interrupt
trap "echo 'Stopping services...'; kill $ENGINE_PID $NODE_PID; exit" INT
wait