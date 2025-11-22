#!/bin/bash

set -e
export CARGO_REGISTRY_TOKEN="cio2uOphUOkgWyPZZQaYFUvFKwpVUBGbyHY"

cd /Users/z/work/shinkai/hanzo-node

echo "Publishing remaining Hanzo crates..."
echo "================================================================="

# hanzo-libp2p-relayer is already being published

# Fix and publish hanzo-kbs (already has serde)
echo "Publishing hanzo-kbs..."
cd hanzo-libs/hanzo-kbs
cargo publish --allow-dirty 2>&1 | tail -10 || echo "Failed: hanzo-kbs"
cd ../..
sleep 10

# Fix and publish hanzo-model-discovery
echo "Publishing hanzo-model-discovery..."
cd hanzo-libs/hanzo-model-discovery
# Check if serde is already there
if ! grep -q '^serde = ' Cargo.toml; then
    # Add serde after [dependencies] line
    sed -i.bak '/\[dependencies\]/a\
serde = { version = "1.0", features = ["derive"] }' Cargo.toml
fi
cargo publish --allow-dirty 2>&1 | tail -10 || echo "Failed: hanzo-model-discovery"
cd ../..
sleep 10

# Fix and publish hanzo-hmm
echo "Publishing hanzo-hmm..."
cd hanzo-libs/hanzo-hmm
if ! grep -q '^serde = ' Cargo.toml; then
    sed -i.bak '/\[dependencies\]/a\
serde = { version = "1.0", features = ["derive"] }' Cargo.toml
fi
cargo publish --allow-dirty 2>&1 | tail -10 || echo "Failed: hanzo-hmm"
cd ../..
sleep 10

# Fix and publish hanzo-http-api
echo "Publishing hanzo-http-api..."
cd hanzo-libs/hanzo-http-api
if ! grep -q '^serde = ' Cargo.toml; then
    sed -i.bak '/\[dependencies\]/a\
serde = { version = "1.0", features = ["derive"] }' Cargo.toml
fi
cargo publish --allow-dirty 2>&1 | tail -10 || echo "Failed: hanzo-http-api"
cd ../..
sleep 10

# Fix and publish hanzo-sheet
echo "Publishing hanzo-sheet..."
cd hanzo-libs/hanzo-sheet
# First check if serde is already there
if ! grep -q '^serde = ' Cargo.toml; then
    # Add serde with proper syntax
    sed -i.bak '/\[dependencies\]/a\
serde = { version = "1.0", features = ["derive"] }' Cargo.toml
fi
# Also remove any duplicate or malformed serde entries
sed -i.bak '/^version = "1.0"$/d' Cargo.toml
sed -i.bak 's/\[dependencies.serde\]/# Serde configured above/' Cargo.toml
cargo publish --allow-dirty 2>&1 | tail -10 || echo "Failed: hanzo-sheet"
cd ../..

echo "✅ Attempted to publish all remaining crates!"
echo "================================================================="

# List final status
echo "Checking which crates were successfully published..."
for crate in hanzo-kbs hanzo-model-discovery hanzo-hmm hanzo-http-api hanzo-sheet; do
    if cargo search $crate | grep -q "1.1.10"; then
        echo "✅ $crate v1.1.10 published"
    else
        echo "❌ $crate v1.1.10 NOT found"
    fi
done