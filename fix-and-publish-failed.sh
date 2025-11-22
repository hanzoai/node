#!/bin/bash

set -e
export CARGO_REGISTRY_TOKEN="cio2uOphUOkgWyPZZQaYFUvFKwpVUBGbyHY"

cd /Users/z/work/shinkai/hanzo-node

echo "Fixing and publishing failed crates..."
echo "================================================================="

# Function to add serde if missing
add_serde() {
    local crate=$1
    cd hanzo-libs/$crate
    if ! grep -q '^serde = ' Cargo.toml; then
        echo "Adding serde to $crate..."
        sed -i.bak '/\[dependencies\]/a\
serde = { version = "1.0", features = ["derive"] }' Cargo.toml
    fi
    cd ../..
}

# Fix hanzo-db
echo "Fixing hanzo-db..."
add_serde "hanzo-db"
cd hanzo-libs/hanzo-db
cargo publish --allow-dirty 2>&1 | tail -5 || echo "Failed: hanzo-db"
cd ../..
sleep 10

# Fix hanzo-libp2p-relayer
echo "Fixing hanzo-libp2p-relayer..."
add_serde "hanzo-libp2p-relayer"
cd hanzo-libs/hanzo-libp2p-relayer
cargo publish --allow-dirty 2>&1 | tail -5 || echo "Failed: hanzo-libp2p-relayer"
cd ../..
sleep 10

# Fix hanzo-kbs
echo "Fixing hanzo-kbs..."
add_serde "hanzo-kbs"
cd hanzo-libs/hanzo-kbs
cargo publish --allow-dirty 2>&1 | tail -5 || echo "Failed: hanzo-kbs"
cd ../..
sleep 10

# Fix hanzo-model-discovery
echo "Fixing hanzo-model-discovery..."
add_serde "hanzo-model-discovery"
cd hanzo-libs/hanzo-model-discovery
cargo publish --allow-dirty 2>&1 | tail -5 || echo "Failed: hanzo-model-discovery"
cd ../..
sleep 10

# Fix hanzo-hmm
echo "Fixing hanzo-hmm..."
add_serde "hanzo-hmm"
cd hanzo-libs/hanzo-hmm
cargo publish --allow-dirty 2>&1 | tail -5 || echo "Failed: hanzo-hmm"
cd ../..
sleep 10

# Publish hanzo-wasm-runtime (metadata already added)
echo "Publishing hanzo-wasm-runtime..."
cd hanzo-libs/hanzo-wasm-runtime
cargo publish --allow-dirty 2>&1 | tail -5 || echo "Failed: hanzo-wasm-runtime"
cd ../..
sleep 10

# Publish hanzo-mining (metadata already added)
echo "Publishing hanzo-mining..."
cd hanzo-libs/hanzo-mining
cargo publish --allow-dirty 2>&1 | tail -5 || echo "Failed: hanzo-mining"
cd ../..
sleep 10

# Fix hanzo-http-api
echo "Fixing hanzo-http-api..."
add_serde "hanzo-http-api"
cd hanzo-libs/hanzo-http-api
cargo publish --allow-dirty 2>&1 | tail -5 || echo "Failed: hanzo-http-api"
cd ../..
sleep 10

# Fix hanzo-sheet
echo "Fixing hanzo-sheet..."
cd hanzo-libs/hanzo-sheet
# Fix the Cargo.toml syntax issue
sed -i.bak '/^version = "1.0"$/d' Cargo.toml
sed -i.bak 's/\[dependencies.serde\]/serde = { version = "1.0", features = ["derive"] }/' Cargo.toml
cargo publish --allow-dirty 2>&1 | tail -5 || echo "Failed: hanzo-sheet"
cd ../..

echo "✅ Attempted to fix and publish all failed crates!"