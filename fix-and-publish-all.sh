#!/bin/bash

# Hanzo Complete Fix and Publish Script
# Fixes circular dependencies and publishes all crates

set -e

export CARGO_REGISTRY_TOKEN="cio2uOphUOkgWyPZZQaYFUvFKwpVUBGbyHY"

cd /Users/z/work/shinkai/hanzo-node

echo "🔧 Fixing Circular Dependencies and Cargo.toml Issues"
echo "================================================================="

# 1. Fix hanzo-job-queue-manager to use path dependency temporarily
echo "1. Fixing hanzo-job-queue-manager..."
cd hanzo-libs/hanzo-job-queue-manager
sed -i.bak 's/hanzo_sqlite = "1.1.10"/hanzo_sqlite = { path = "..\/hanzo-sqlite", version = "1.1.10" }/' Cargo.toml
cd ../..

# 2. Fix hanzo-sheet Cargo.toml syntax error (line 20)
echo "2. Fixing hanzo-sheet Cargo.toml..."
cd hanzo-libs/hanzo-sheet
# Find the line with just features = ["derive"] and add version before it
sed -i.bak '/^features = \["derive"\]$/i\
version = "1.0"
' Cargo.toml
cd ../..

# 3. Fix hanzo-wasm-runtime missing version
echo "3. Fixing hanzo-wasm-runtime..."
cd hanzo-libs/hanzo-wasm-runtime
sed -i.bak 's/hanzo_tools_primitives = { path = "..\/hanzo-tools-primitives" }/hanzo_tools_primitives = { path = "..\/hanzo-tools-primitives", version = "1.1.10" }/' Cargo.toml
cd ../..

echo ""
echo "✅ All Cargo.toml fixes applied!"
echo ""
echo "📦 Publishing in Correct Order"
echo "================================================================="

cd hanzo-libs

# Now publish in order - hanzo-sqlite will work because job-queue-manager uses path dep
echo ""
echo "🔹 Publishing hanzo-sqlite (was blocked)..."
cd hanzo-sqlite
cargo publish --allow-dirty 2>&1 | tail -5 || echo "⚠️  May already be published"
cd ..
sleep 3

# Restore hanzo-job-queue-manager to use crates.io dependency
echo ""
echo "🔧 Restoring hanzo-job-queue-manager to use crates.io dependency..."
cd hanzo-job-queue-manager
sed -i.bak2 's/hanzo_sqlite = { path = "..\/hanzo-sqlite", version = "1.1.10" }/hanzo_sqlite = "1.1.10"/' Cargo.toml
cd ..

# Now publish remaining crates in dependency order
REMAINING_TIER2=(
  "hanzo-db"
)

for crate in "${REMAINING_TIER2[@]}"; do
  echo ""
  echo "🔹 Publishing $crate..."
  cd $crate
  cargo publish --allow-dirty 2>&1 | tail -5 || echo "⚠️  $crate may already be published or failed"
  cd ..
  sleep 3
done

# Tier 3
REMAINING_TIER3=(
  "hanzo-libp2p-relayer"
  "hanzo-job-queue-manager"
)

for crate in "${REMAINING_TIER3[@]}"; do
  echo ""
  echo "🔹 Publishing $crate..."
  cd $crate
  cargo publish --allow-dirty 2>&1 | tail -5 || echo "⚠️  $crate may already be published or failed"
  cd ..
  sleep 3
done

# Tier 4
REMAINING_TIER4=(
  "hanzo-fs"
  "hanzo-kbs"
  "hanzo-model-discovery"
)

for crate in "${REMAINING_TIER4[@]}"; do
  echo ""
  echo "🔹 Publishing $crate..."
  cd $crate
  cargo publish --allow-dirty 2>&1 | tail -5 || echo "⚠️  $crate may already be published or failed"
  cd ..
  sleep 3
done

# Tier 5
REMAINING_TIER5=(
  "hanzo-hmm"
  "hanzo-llm"
  "hanzo-sheet"
  "hanzo-wasm-runtime"
  "hanzo-mining"
)

for crate in "${REMAINING_TIER5[@]}"; do
  echo ""
  echo "🔹 Publishing $crate..."
  cd $crate
  cargo publish --allow-dirty 2>&1 | tail -5 || echo "⚠️  $crate may already be published or failed"
  cd ..
  sleep 3
done

# Tier 6
echo ""
echo "🔹 Publishing hanzo-http-api..."
cd hanzo-http-api
cargo publish --allow-dirty 2>&1 | tail -5 || echo "⚠️  hanzo-http-api may already be published or failed"
cd ..

echo ""
echo "================================================================="
echo "✅ Publishing Complete!"
echo "================================================================="
echo ""
echo "Verifying published crates..."
sleep 10
cargo search hanzo_ --limit 30

echo ""
echo "🎉 All done! Check output above for any failures."
