#!/bin/bash

set -e
export CARGO_REGISTRY_TOKEN="cio2uOphUOkgWyPZZQaYFUvFKwpVUBGbyHY"

cd /Users/z/work/shinkai/hanzo-node

echo "================================================================="
echo "Publishing all fixed Hanzo crates"
echo "================================================================="

# Publish hanzo-http-api (fixed MCP fields)
echo ""
echo "1. Publishing hanzo-http-api..."
cd hanzo-libs/hanzo-http-api
cargo publish --allow-dirty 2>&1 | tail -20 || echo "Failed: hanzo-http-api"
cd ../..
sleep 10

# Publish hanzo-sheet (fixed imports)
echo ""
echo "2. Publishing hanzo-sheet..."
cd hanzo-libs/hanzo-sheet
cargo publish --allow-dirty 2>&1 | tail -20 || echo "Failed: hanzo-sheet"
cd ../..
sleep 10

# Publish hanzo-libp2p-relayer (fixed libp2p API)
echo ""
echo "3. Publishing hanzo-libp2p-relayer..."
cd hanzo-libs/hanzo-libp2p-relayer
cargo publish --allow-dirty 2>&1 | tail -20 || echo "Failed: hanzo-libp2p-relayer"
cd ../..
sleep 10

echo ""
echo "================================================================="
echo "✅ Publishing complete!"
echo "================================================================="

# Check what got published
echo ""
echo "Checking published crates..."
for crate in hanzo_http_api hanzo_sheet hanzo_libp2p_relayer; do
    if cargo search $crate | grep -q "1.1.10"; then
        echo "✅ $crate v1.1.10 published"
    else
        echo "❌ $crate v1.1.10 NOT found"
    fi
done

echo ""
echo "================================================================="
echo "Remaining crates that need more complex fixes:"
echo "- hanzo-db (109 errors - needs LanceDB API migration)"
echo "- hanzo-kbs (27 errors - needs attestation API updates)"
echo "- hanzo-model-discovery (6 errors - needs serde fixes)"
echo "- hanzo-hmm (16 errors - needs type inference fixes)"
echo "================================================================="
