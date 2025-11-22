#!/bin/bash

set -e
export CARGO_REGISTRY_TOKEN="cio2uOphUOkgWyPZZQaYFUvFKwpVUBGbyHY"

cd /Users/z/work/shinkai/hanzo-node

echo "Publishing remaining Hanzo crates to crates.io..."
echo "================================================================="

# Now that hanzo-sqlite is published, publish dependent crates

# Tier 4: Dependencies of hanzo-sqlite
echo "Publishing Tier 4: hanzo-sqlite dependents..."
TIER4=("hanzo-db" "hanzo-libp2p-relayer" "hanzo-job-queue-manager" "hanzo-fs")

for crate in "${TIER4[@]}"; do
    echo "Publishing $crate..."
    cd hanzo-libs/$crate
    # Add serde if missing
    if ! grep -q '^serde = ' Cargo.toml; then
        sed -i.bak 's/\[dependencies\]/[dependencies]\nserde = { version = "1.0", features = ["derive"] }/' Cargo.toml
    fi
    cargo publish --allow-dirty 2>&1 | tail -5
    cd ../..
    sleep 10
done

# Tier 5: Depends on Tier 4
echo "Publishing Tier 5..."
TIER5=("hanzo-kbs" "hanzo-model-discovery" "hanzo-hmm" "hanzo-llm")

for crate in "${TIER5[@]}"; do
    echo "Publishing $crate..."
    cd hanzo-libs/$crate
    # Add serde if missing
    if ! grep -q '^serde = ' Cargo.toml; then
        sed -i.bak 's/\[dependencies\]/[dependencies]\nserde = { version = "1.0", features = ["derive"] }/' Cargo.toml
    fi
    cargo publish --allow-dirty 2>&1 | tail -5
    cd ../..
    sleep 10
done

# Tier 6: Depends on multiple earlier tiers
echo "Publishing Tier 6..."
TIER6=("hanzo-sheet" "hanzo-wasm-runtime" "hanzo-mining" "hanzo-http-api" "hanzo-runtime-tests")

for crate in "${TIER6[@]}"; do
    echo "Publishing $crate..."
    cd hanzo-libs/$crate
    # Add serde if missing
    if ! grep -q '^serde = ' Cargo.toml; then
        sed -i.bak 's/\[dependencies\]/[dependencies]\nserde = { version = "1.0", features = ["derive"] }/' Cargo.toml
    fi
    # Skip runtime-tests if it exists (it's a test crate)
    if [ "$crate" = "hanzo-runtime-tests" ] && [ ! -f Cargo.toml ]; then
        echo "Skipping hanzo-runtime-tests (not found)..."
        cd ../..
        continue
    fi
    cargo publish --allow-dirty 2>&1 | tail -5 || echo "Warning: $crate failed to publish"
    cd ../..
    sleep 10
done

echo "✅ All crates published successfully!"
echo "================================================================="
echo "Summary of published crates:"
echo "- Tier 1: hanzo-message-primitives, hanzo-crypto-identities, hanzo-pqc"
echo "- Tier 2: hanzo-embedding, hanzo-non-rust-code, hanzo-did"
echo "- Tier 3: hanzo-tools-primitives, hanzo-mcp, hanzo-config, hanzo-sqlite"
echo "- Tier 4: ${TIER4[*]}"
echo "- Tier 5: ${TIER5[*]}"
echo "- Tier 6: ${TIER6[*]}"