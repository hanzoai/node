#!/bin/bash

# Hanzo Crates Publishing Script - Correct Dependency Order
# v1.1.10
# This script publishes crates in the correct dependency order

set -e

export CARGO_REGISTRY_TOKEN="cio2uOphUOkgWyPZZQaYFUvFKwpVUBGbyHY"

cd /Users/z/work/shinkai/hanzo-node/hanzo-libs

echo "🚀 Publishing Hanzo Crates in Dependency Order"
echo "================================================================="

# Tier 0: No dependencies (already published)
echo ""
echo "✅ Tier 0: hanzo-message-primitives (already published)"

# Tier 1: Depends only on hanzo-message-primitives
echo ""
echo "📦 Tier 1: Core Infrastructure Crates"
echo "================================================================="

TIER1=(
  "hanzo-crypto-identities"
  "hanzo-pqc"
  "hanzo-sqlite"
  "hanzo-embedding"
)

for crate in "${TIER1[@]}"; do
  echo ""
  echo "Publishing $crate..."
  cd $crate
  cargo publish --allow-dirty 2>&1 | tail -10 || echo "⚠️  $crate may already be published or failed"
  cd ..
  sleep 2
done

# Tier 2: Depends on Tier 0 + Tier 1
echo ""
echo "📦 Tier 2: Database and DID Crates"
echo "================================================================="

TIER2=(
  "hanzo-non-rust-code"
  "hanzo-did"
  "hanzo-db"
)

for crate in "${TIER2[@]}"; do
  echo ""
  echo "Publishing $crate..."
  cd $crate
  cargo publish --allow-dirty 2>&1 | tail -10 || echo "⚠️  $crate may already be published or failed"
  cd ..
  sleep 2
done

# Tier 3: Depends on Tier 0-2
echo ""
echo "📦 Tier 3: Networking and Queueing Crates"
echo "================================================================="

TIER3=(
  "hanzo-libp2p-relayer"
  "hanzo-job-queue-manager"
  "hanzo-tools-primitives"
)

for crate in "${TIER3[@]}"; do
  echo ""
  echo "Publishing $crate..."
  cd $crate
  cargo publish --allow-dirty 2>&1 | tail -10 || echo "⚠️  $crate may already be published or failed"
  cd ..
  sleep 2
done

# Tier 4: Depends on Tier 0-3
echo ""
echo "📦 Tier 4: Service Crates"
echo "================================================================="

TIER4=(
  "hanzo-config"
  "hanzo-fs"
  "hanzo-mcp"
  "hanzo-kbs"
  "hanzo-model-discovery"
)

for crate in "${TIER4[@]}"; do
  echo ""
  echo "Publishing $crate..."
  cd $crate
  cargo publish --allow-dirty 2>&1 | tail -10 || echo "⚠️  $crate may already be published or failed"
  cd ..
  sleep 2
done

# Tier 5: Depends on Tier 0-4
echo ""
echo "📦 Tier 5: High-Level Crates"
echo "================================================================="

TIER5=(
  "hanzo-hmm"
  "hanzo-llm"
  "hanzo-sheet"
  "hanzo-wasm-runtime"
  "hanzo-mining"
)

for crate in "${TIER5[@]}"; do
  echo ""
  echo "Publishing $crate..."
  cd $crate
  cargo publish --allow-dirty 2>&1 | tail -10 || echo "⚠️  $crate may already be published or failed"
  cd ..
  sleep 2
done

# Tier 6: Depends on everything
echo ""
echo "📦 Tier 6: hanzo-http-api"
echo "================================================================="

cd hanzo-http-api
cargo publish --allow-dirty 2>&1 | tail -10 || echo "⚠️  hanzo-http-api may already be published or failed"
cd ..

echo ""
echo "================================================================="
echo "✅ Publishing Complete!"
echo "================================================================="
echo ""
echo "Verifying published crates on crates.io..."
sleep 10
cargo search hanzo_ --limit 30
