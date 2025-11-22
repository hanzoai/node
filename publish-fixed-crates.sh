#!/bin/bash

set -e
export CARGO_REGISTRY_TOKEN="cio2uOphUOkgWyPZZQaYFUvFKwpVUBGbyHY"

cd /Users/z/work/shinkai/hanzo-node

echo "================================================================="
echo "Publishing Fixed Hanzo Crates"
echo "================================================================="

# Publish hanzo-sheet (we've fixed all struct issues)
echo ""
echo "1. Publishing hanzo-sheet..."
cd hanzo-libs/hanzo-sheet
cargo publish --allow-dirty 2>&1 | tail -30 || echo "Failed: hanzo-sheet"
cd ../..
sleep 15

echo ""
echo "================================================================="
echo "✅ Publishing complete!"
echo "================================================================="

# Check what got published
echo ""
echo "Checking published crates..."
for crate in hanzo_sheet; do
    if cargo search $crate | grep -q "1.1.10"; then
        echo "✅ $crate v1.1.10 published"
    else
        echo "❌ $crate v1.1.10 NOT found"
    fi
done
