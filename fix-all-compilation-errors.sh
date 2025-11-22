#!/bin/bash

echo "Fixing all compilation errors in Hanzo crates..."
echo "================================================================="

# 1. Fix hanzo-libp2p-relayer - update Cargo.toml version dependencies
echo "1. Updating hanzo-libp2p-relayer dependency versions..."
cd hanzo-libs/hanzo-libp2p-relayer
sed -i '' 's/hanzo-message-primitives = "1.1.10"/hanzo-message-primitives = { workspace = true }/' Cargo.toml
sed -i '' 's/hanzo-crypto-identities = "1.1.10"/hanzo-crypto-identities = { workspace = true }/' Cargo.toml
cd ../..

# 2. Fix hanzo-http-api - will need manual fixes
echo "2. hanzo-http-api needs manual MCP struct field fixes (skipping for now)"

# 3. Fix hanzo-sheet - will need manual fixes
echo "3. hanzo-sheet needs manual module import fixes (skipping for now)"

# 4. Fix hanzo-db - will need manual fixes
echo "4. hanzo-db needs extensive manual fixes (skipping for now)"

# 5. Fix hanzo-kbs - will need manual fixes
echo "5. hanzo-kbs needs manual attestation type fixes (skipping for now)"

# 6. Fix hanzo-hmm - will need manual type inference fixes
echo "6. hanzo-hmm needs manual type inference fixes (skipping for now)"

# 7. Fix hanzo-model-discovery - will need manual serde fixes
echo "7. hanzo-model-discovery needs manual serde fixes (skipping for now)"

echo "================================================================="
echo "Basic dependency fixes applied. Complex code errors require manual intervention."
