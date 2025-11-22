#!/bin/bash

# Hanzo Complete Fix and Publish Script - v2
# Fixes ALL crates that depend on hanzo_sqlite

set -e

export CARGO_REGISTRY_TOKEN="cio2uOphUOkgWyPZZQaYFUvFKwpVUBGbyHY"

cd /Users/z/work/shinkai/hanzo-node

echo "🔧 Fixing All hanzo_sqlite Dependencies"
echo "================================================================="

# Fix ALL crates that depend on hanzo_sqlite to use path dependencies
CRATES_TO_FIX=("hanzo-fs" "hanzo-job-queue-manager" "hanzo-llm")

for crate in "${CRATES_TO_FIX[@]}"; do
  echo "Fixing $crate..."
  cd hanzo-libs/$crate
  sed -i.bak 's/hanzo_sqlite = "1.1.10"/hanzo_sqlite = { path = "..\/hanzo-sqlite", version = "1.1.10" }/' Cargo.toml
  cd ../..
done

# Fix hanzo-sheet Cargo.toml syntax error
echo "Fixing hanzo-sheet Cargo.toml..."
cd hanzo-libs/hanzo-sheet
sed -i.bak '/^features = \["derive"\]$/i\
version = "1.0"
' Cargo.toml
cd ../..

# Fix hanzo-wasm-runtime missing version
echo "Fixing hanzo-wasm-runtime..."
cd hanzo-libs/hanzo-wasm-runtime
sed -i.bak 's/hanzo_tools_primitives = { path = "..\/hanzo-tools-primitives" }/hanzo_tools_primitives = { path = "..\/hanzo-tools-primitives", version = "1.1.10" }/' Cargo.toml
cd ../..

echo ""
echo "✅ All Cargo.toml fixes applied!"
echo ""
echo "📦 Publishing hanzo-sqlite"
echo "================================================================="

cd hanzo-libs/hanzo-sqlite
cargo publish --allow-dirty 2>&1 | tail -10
cd ../..

echo ""
echo "⏳ Waiting 10 seconds for crates.io to index..."
sleep 10

echo ""
echo "🔧 Restoring crates.io dependencies..."
for crate in "${CRATES_TO_FIX[@]}"; do
  echo "Restoring $crate..."
  cd hanzo-libs/$crate
  sed -i.bak2 's/hanzo_sqlite = { path = "..\/hanzo-sqlite", version = "1.1.10" }/hanzo_sqlite = "1.1.10"/' Cargo.toml
  cd ../..
done

echo ""
echo "📦 Publishing remaining crates in order"
echo "================================================================="

cd hanzo-libs

# Tier 2
echo ""
echo "🔹 Tier 2: hanzo-db"
cd hanzo-db
cargo publish --allow-dirty 2>&1 | tail -5
cd ..
sleep 5

# Tier 3
echo ""
echo "🔹 Tier 3: hanzo-libp2p-relayer, hanzo-job-queue-manager"
for crate in hanzo-libp2p-relayer hanzo-job-queue-manager; do
  echo "Publishing $crate..."
  cd $crate
  cargo publish --allow-dirty 2>&1 | tail -5
  cd ..
  sleep 5
done

# Tier 4
echo ""
echo "🔹 Tier 4: hanzo-fs, hanzo-kbs, hanzo-model-discovery"
for crate in hanzo-fs hanzo-kbs hanzo-model-discovery; do
  echo "Publishing $crate..."
  cd $crate
  cargo publish --allow-dirty 2>&1 | tail -5
  cd ..
  sleep 5
done

# Tier 5
echo ""
echo "🔹 Tier 5: hanzo-hmm, hanzo-llm, hanzo-sheet, hanzo-wasm-runtime, hanzo-mining"
for crate in hanzo-hmm hanzo-llm hanzo-sheet hanzo-wasm-runtime hanzo-mining; do
  echo "Publishing $crate..."
  cd $crate
  cargo publish --allow-dirty 2>&1 | tail -5
  cd ..
  sleep 5
done

# Tier 6
echo ""
echo "🔹 Tier 6: hanzo-http-api"
cd hanzo-http-api
cargo publish --allow-dirty 2>&1 | tail -5
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
