#!/bin/bash

echo "Checking which Hanzo crates have been published at v1.1.11..."
echo "================================================================="

CRATES=(
  "hanzo-baml"
  "hanzo-config"
  "hanzo-crypto-identities"
  "hanzo-db"
  "hanzo-did"
  "hanzo-embedding"
  "hanzo-fs"
  "hanzo-hmm"
  "hanzo-http-api"
  "hanzo-job-queue-manager"
  "hanzo-kbs"
  "hanzo-libp2p-relayer"
  "hanzo-llm"
  "hanzo-mcp"
  "hanzo-message-primitives"
  "hanzo-mining"
  "hanzo-model-discovery"
  "hanzo-non-rust-code"
  "hanzo-pqc"
  "hanzo-sheet"
  "hanzo-sqlite"
  "hanzo-tools-primitives"
  "hanzo-tools-runner"
  "hanzo-wasm-runtime"
)

SUCCESS_COUNT=0
FAIL_COUNT=0

for crate in "${CRATES[@]}"; do
  if curl -s "https://crates.io/api/v1/crates/$crate" | grep -q '"max_stable_version":"1.1.11"'; then
    echo "✅ $crate v1.1.11 published"
    SUCCESS_COUNT=$((SUCCESS_COUNT + 1))
  else
    echo "❌ $crate v1.1.11 NOT found"
    FAIL_COUNT=$((FAIL_COUNT + 1))
  fi
  sleep 0.5
done

echo "================================================================="
echo "Summary: $SUCCESS_COUNT published, $FAIL_COUNT not found"
