#!/bin/bash

set -e
export CARGO_REGISTRY_TOKEN="cio2uOphUOkgWyPZZQaYFUvFKwpVUBGbyHY"

cd /Users/z/work/shinkai/hanzo-node

echo "================================================================="
echo "Publishing all remaining Hanzo crates with latest dependencies"
echo "================================================================="

# Fix and publish hanzo-libp2p-relayer
echo ""
echo "1. Publishing hanzo-libp2p-relayer..."
cd hanzo-libs/hanzo-libp2p-relayer

# Add missing libp2p features
if ! grep -q '"json"' Cargo.toml; then
    sed -i.bak 's/libp2p = { version = "0.55.0", features = \[/libp2p = { version = "0.55.0", features = ["json", /' Cargo.toml
fi
if ! grep -q '"tokio"' Cargo.toml; then
    sed -i.bak 's/features = \["tcp",/features = ["tcp", "tokio", /' Cargo.toml
fi

cargo publish --allow-dirty 2>&1 | tail -20 || echo "Failed: hanzo-libp2p-relayer"
cd ../..
sleep 10

# Publish hanzo-db (already updated to LanceDB 0.22.3)
echo ""
echo "2. Publishing hanzo-db..."
cd hanzo-libs/hanzo-db
cargo publish --allow-dirty 2>&1 | tail -20 || echo "Failed: hanzo-db"
cd ../..
sleep 10

# Fix and publish hanzo-kbs
echo ""
echo "3. Publishing hanzo-kbs..."
cd hanzo-libs/hanzo-kbs
cargo publish --allow-dirty 2>&1 | tail -20 || echo "Failed: hanzo-kbs (needs code fixes)"
cd ../..
sleep 10

# Fix and publish hanzo-model-discovery
echo ""
echo "4. Publishing hanzo-model-discovery..."
cd hanzo-libs/hanzo-model-discovery
cargo publish --allow-dirty 2>&1 | tail -20 || echo "Failed: hanzo-model-discovery (needs serde fixes)"
cd ../..
sleep 10

# Fix and publish hanzo-hmm
echo ""
echo "5. Publishing hanzo-hmm..."
cd hanzo-libs/hanzo-hmm
cargo publish --allow-dirty 2>&1 | tail -20 || echo "Failed: hanzo-hmm (needs type inference fixes)"
cd ../..
sleep 10

# Fix and publish hanzo-http-api
echo ""
echo "6. Publishing hanzo-http-api..."
cd hanzo-libs/hanzo-http-api
cargo publish --allow-dirty 2>&1 | tail -20 || echo "Failed: hanzo-http-api (needs MCP fixes)"
cd ../..
sleep 10

# Fix and publish hanzo-sheet
echo ""
echo "7. Publishing hanzo-sheet..."
cd hanzo-libs/hanzo-sheet
cargo publish --allow-dirty 2>&1 | tail -20 || echo "Failed: hanzo-sheet (needs import fixes)"
cd ../..

echo ""
echo "================================================================="
echo "✅ Publishing attempt complete!"
echo "================================================================="

# Check what got published
echo ""
echo "Checking published crates..."
for crate in hanzo_libp2p_relayer hanzo_db hanzo-kbs hanzo-model-discovery hanzo_hmm hanzo_http_api hanzo_sheet; do
    if cargo search $crate | grep -q "1.1.10"; then
        echo "✅ $crate v1.1.10 published"
    else
        echo "❌ $crate v1.1.10 NOT found"
    fi
done
