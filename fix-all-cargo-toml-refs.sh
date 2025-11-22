#!/bin/bash

# Fix all Cargo.toml references to use new idiomatic names
# Compatible with macOS bash 3.2

set -e

echo "========================================================================"
echo "Fixing all Cargo.toml references to use new idiomatic names"
echo "========================================================================"
echo ""

# Define all old:new pairs that need updating
RENAMES="
hanzo-embedding:hanzo-embed
hanzo-http-api:hanzo-api
hanzo-message-primitives:hanzo-messages
hanzo-crypto-identities:hanzo-identity
hanzo-libp2p-relayer:hanzo-libp2p
hanzo-job-queue-manager:hanzo-jobs
hanzo-tools-primitives:hanzo-tools
hanzo-tools-runner:hanzo-runner
hanzo-sqlite:hanzo-db-sqlite
hanzo-non-rust-code:hanzo-runtime
hanzo-model-discovery:hanzo-models
hanzo-wasm-runtime:hanzo-wasm
hanzo-runtime-tests:hanzo-tests
"

# Note: hanzo-db should NOT be replaced (it becomes hanzo-database via directory rename)

echo "Replacing old names in all Cargo.toml files..."

echo "$RENAMES" | while IFS=: read -r old_name new_name; do
    [ -z "$old_name" ] && continue

    echo "Replacing: $old_name → $new_name"

    # Find all Cargo.toml files (excluding target directory) and replace
    find . -name "Cargo.toml" -not -path "./target/*" -exec sed -i '' "s/$old_name/$new_name/g" {} \;
done

echo ""
echo "========================================================================"
echo "All Cargo.toml references updated!"
echo "========================================================================"
echo ""
echo "Next: cargo check --workspace"
echo ""
