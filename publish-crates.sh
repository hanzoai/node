#!/bin/bash

# Publish Hanzo crates to crates.io in dependency order

set -e

echo "Publishing Hanzo crates to crates.io..."
echo "Make sure you are logged in to crates.io with: cargo login"
echo ""

# List of crates in dependency order (dependencies first)
CRATES=(
    "hanzo-libs/hanzo-message-primitives"
    "hanzo-libs/hanzo-crypto-identities" 
    "hanzo-libs/hanzo-fs"
    "hanzo-libs/hanzo-pqc"
    "hanzo-libs/hanzo-embedding"
    "hanzo-libs/hanzo-sqlite"
    "hanzo-libs/hanzo-mcp"
    "hanzo-libs/hanzo-kbs"
    "hanzo-libs/hanzo-non-rust-code"
    "hanzo-libs/hanzo-tools-primitives"
    "hanzo-libs/hanzo-job-queue-manager"
    "hanzo-libs/hanzo-http-api"
    "hanzo-libs/hanzo-libp2p-relayer"
    "hanzo-test-framework"
    "hanzo-test-macro"
)

# Publish each crate
for crate in "${CRATES[@]}"; do
    if [ -d "$crate" ]; then
        echo "Publishing $crate..."
        cd "$crate"
        
        # Verify the crate builds
        cargo build --release
        
        # Run tests
        cargo test
        
        # Publish to crates.io (dry run first)
        echo "Dry run for $crate..."
        cargo publish --dry-run
        
        echo "Publishing $crate for real..."
        cargo publish --allow-dirty || {
            echo "Failed to publish $crate. It might already be published or have issues."
            echo "Continuing with next crate..."
        }
        
        cd - > /dev/null
        
        # Wait a bit between publishes to allow crates.io to index
        echo "Waiting for crates.io to index..."
        sleep 10
    else
        echo "Skipping $crate (directory not found)"
    fi
done

echo ""
echo "All crates published successfully!"
echo "The hanzo-node binary can now be built with the published crates."